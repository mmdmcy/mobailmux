from __future__ import annotations

import json
import os
import queue
import re
import shlex
import signal
import subprocess
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path

import requests

from .commands import cd_target_arg, parse_command_message, queue_request_arg, unknown_command_text


ENV_FILE = Path(os.environ.get("MOBAILMUX_ENV", ".env"))


def load_env_file(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    data: dict[str, str] = {}
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]
        data[key.strip()] = value
    return data


ENV = load_env_file(ENV_FILE)


def cfg(key: str, default: str | None = None) -> str | None:
    value = os.environ.get(key)
    if value not in (None, ""):
        return value
    value = ENV.get(key)
    if value not in (None, ""):
        return value
    return default


def required_cfg(key: str) -> str:
    value = cfg(key)
    if not value:
        raise SystemExit(f"Missing required config: {key}")
    return value


def expand_path(value: str) -> str:
    return str(Path(os.path.expandvars(os.path.expanduser(value))).resolve())


def env_key_fragment(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "_", value).strip("_").upper()


@dataclass(frozen=True)
class SlotConfig:
    name: str
    channel: str
    default_workdir: str


BASE_URL = required_cfg("MOBAILMUX_MATTERMOST_URL").rstrip("/")
TOKEN = required_cfg("MOBAILMUX_BOT_TOKEN")
TEAM_NAME = required_cfg("MOBAILMUX_TEAM_NAME")
OWNER_USERNAME = cfg("MOBAILMUX_OWNER_USERNAME")
OWNER_USER_ID = cfg("MOBAILMUX_OWNER_USER_ID")
BOT_USERNAME = cfg("MOBAILMUX_BOT_USERNAME", "mobailmux")
SLOTS_CHANNEL = cfg("MOBAILMUX_SLOTS_CHANNEL", "slots") or ""
POLL_SECONDS = float(cfg("MOBAILMUX_POLL_SECONDS", "2"))
STATUS_SECONDS = int(cfg("MOBAILMUX_STATUS_SECONDS", "60"))
MAX_PROGRESS_POSTS = int(cfg("MOBAILMUX_MAX_PROGRESS_POSTS", "0"))
OUTPUT_SNIPPET_CHARS = int(cfg("MOBAILMUX_OUTPUT_SNIPPET_CHARS", "1200"))
MAX_QUEUED_PER_SLOT = int(cfg("MOBAILMUX_MAX_QUEUED_PER_SLOT", "5"))
MAX_LS_ENTRIES = int(cfg("MOBAILMUX_MAX_LS_ENTRIES", "120"))
STATE_DIR = Path(expand_path(cfg("MOBAILMUX_STATE_DIR", "~/.local/state/mobailmux") or "~/.local/state/mobailmux"))
STATE_FILE = STATE_DIR / "state.json"
DEFAULT_WORKDIR = expand_path(cfg("MOBAILMUX_DEFAULT_WORKDIR", "~") or "~")

CODEX_BIN = cfg("MOBAILMUX_CODEX_BIN", "codex") or "codex"
IS_WINDOWS = os.name == "nt"
CODEX_ARGS = shlex.split(cfg("MOBAILMUX_CODEX_ARGS", "") or "", posix=not IS_WINDOWS)
AGENT_HOME = cfg("MOBAILMUX_AGENT_HOME")
PATH_EXTRA = cfg("MOBAILMUX_PATH_EXTRA", "") or ""


def parse_slots() -> dict[str, SlotConfig]:
    raw = cfg("MOBAILMUX_SLOTS", "one:agent-one,two:agent-two,three:agent-three") or ""
    slots: dict[str, SlotConfig] = {}
    for item in raw.split(","):
        item = item.strip()
        if not item:
            continue
        parts = [part.strip() for part in item.split(":", 2)]
        if len(parts) < 2 or not parts[0] or not parts[1]:
            raise SystemExit(f"Invalid slot spec {item!r}; expected name:channel[:workdir]")
        name, channel = parts[0], parts[1]
        workdir = parts[2] if len(parts) == 3 and parts[2] else DEFAULT_WORKDIR
        env_workdir = cfg(f"MOBAILMUX_SLOT_{env_key_fragment(name)}_WORKDIR")
        if env_workdir:
            workdir = env_workdir
        slots[name] = SlotConfig(name=name, channel=channel, default_workdir=expand_path(workdir))
    if not slots:
        raise SystemExit("MOBAILMUX_SLOTS did not define any slots")
    return slots


SLOTS = parse_slots()

http = requests.Session()
http.headers.update({"Authorization": f"Bearer {TOKEN}"})

state_lock = threading.Lock()
workers: dict[str, dict] = {}
history_lock = threading.Lock()
history: dict[str, list[str]] = {}
queued_lock = threading.Lock()
queued_requests: dict[str, list[str]] = {}
stop_event = threading.Event()
job_queue: queue.Queue[tuple[str, dict]] = queue.Queue()


def api(method: str, path: str, **kwargs):
    response = http.request(method, f"{BASE_URL}{path}", timeout=30, **kwargs)
    response.raise_for_status()
    if response.content:
        return response.json()
    return None


def load_state() -> dict:
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    if STATE_FILE.exists():
        return json.loads(STATE_FILE.read_text())
    return {
        "initialized_at": int(time.time() * 1000),
        "last_seen": {},
        "sessions": {},
        "workdirs": {slot: config.default_workdir for slot, config in SLOTS.items()},
    }


def save_state(state: dict) -> None:
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    tmp = STATE_FILE.with_suffix(".tmp")
    tmp.write_text(json.dumps(state, indent=2, sort_keys=True))
    tmp.replace(STATE_FILE)


state = load_state()


def truncate(value: str, max_chars: int) -> str:
    value = value.strip()
    if len(value) <= max_chars:
        return value
    return value[: max_chars - 20] + "\n...[truncated]"


def fenced(value: str, max_chars: int = OUTPUT_SNIPPET_CHARS) -> str:
    value = truncate(value, max_chars)
    return f"```text\n{value}\n```"


def post(channel_id: str, message: str) -> None:
    max_len = 3500
    chunks = [message[i : i + max_len] for i in range(0, len(message), max_len)] or [""]
    for chunk in chunks:
        api("POST", "/api/v4/posts", json={"channel_id": channel_id, "message": chunk})


def record_history(slot: str, message: str) -> None:
    line = " ".join(message.strip().split())
    if not line:
        return
    stamp = time.strftime("%H:%M:%S")
    with history_lock:
        entries = history.setdefault(slot, [])
        entries.append(f"[{stamp}] {truncate(line, 1800)}")
        del entries[:-60]


def clear_history(slot: str) -> None:
    with history_lock:
        history[slot] = []


def post_slot(slot: str, channel_id: str, message: str) -> None:
    record_history(slot, message)
    post(channel_id, message)


def queue_request(slot: str, message: str) -> tuple[bool, int]:
    with queued_lock:
        requests_for_slot = queued_requests.setdefault(slot, [])
        if len(requests_for_slot) >= MAX_QUEUED_PER_SLOT:
            return False, len(requests_for_slot)
        requests_for_slot.append(message)
        return True, len(requests_for_slot)


def pop_queued_request(slot: str) -> str | None:
    with queued_lock:
        requests_for_slot = queued_requests.setdefault(slot, [])
        if not requests_for_slot:
            return None
        return requests_for_slot.pop(0)


def clear_queued_requests(slot: str) -> int:
    with queued_lock:
        count = len(queued_requests.setdefault(slot, []))
        queued_requests[slot] = []
        return count


def queue_length(slot: str) -> int:
    with queued_lock:
        return len(queued_requests.setdefault(slot, []))


def queued_text(slot: str) -> str:
    with queued_lock:
        items = list(queued_requests.setdefault(slot, []))
    if not items:
        return f"{slot} queue is empty."
    lines = [f"{idx}. {truncate(item, 220)}" for idx, item in enumerate(items, start=1)]
    return f"{slot} queued requests:\n" + "\n".join(lines)


def resolve(path: str, base: str) -> str:
    expanded = os.path.expandvars(os.path.expanduser(path.strip()))
    if not os.path.isabs(expanded):
        expanded = os.path.join(base, expanded)
    return str(Path(expanded).resolve())


def parse_ls_args(raw: str) -> tuple[bool, str | None, str | None]:
    try:
        args = shlex.split(raw, posix=not IS_WINDOWS)
    except ValueError as exc:
        return False, None, f"Could not parse `ls` arguments: {exc}"

    show_hidden = False
    targets = []
    for arg in args:
        if arg == "--all":
            show_hidden = True
            continue
        if arg.startswith("-") and arg != "-":
            flags = arg[1:]
            if flags and set(flags) <= {"a", "l", "h"}:
                show_hidden = show_hidden or "a" in flags
                continue
            return False, None, f"Unsupported `ls` option: `{arg}`. Supported: `-a`, `-l`, `-la`, `--all`."
        targets.append(arg)

    if len(targets) > 1:
        return False, None, "Usage: `ls [path]`"
    return show_hidden, targets[0] if targets else None, None


def list_path_text(slot: str, raw_args: str) -> str:
    show_hidden, target_arg, error = parse_ls_args(raw_args)
    if error:
        return error

    target = Path(resolve(target_arg or ".", current_workdir(slot)))
    if not target.exists():
        return f"Path does not exist: `{target}`"
    if target.is_file():
        return f"{slot} file:\n```text\n{target.name}\n```"
    if not target.is_dir():
        return f"Path is not a directory: `{target}`"

    rows = []
    try:
        for child in target.iterdir():
            if not show_hidden and child.name.startswith("."):
                continue
            try:
                is_dir = child.is_dir()
                is_link = child.is_symlink()
            except OSError:
                is_dir = False
                is_link = False
            suffix = "/" if is_dir else "@" if is_link else ""
            rows.append((0 if is_dir else 1, child.name.lower(), f"{child.name}{suffix}"))
    except PermissionError:
        return f"Permission denied: `{target}`"
    except OSError as exc:
        return f"Could not list `{target}`: {exc}"

    rows.sort()
    names = [row[2] for row in rows]
    omitted = max(0, len(names) - MAX_LS_ENTRIES)
    names = names[:MAX_LS_ENTRIES]
    if omitted:
        names.append(f"... {omitted} more")
    if not names:
        names = ["(empty)"]
    return f"{slot} listing `{target}`:\n```text\n" + "\n".join(names) + "\n```"


def owner_user_id() -> str:
    if OWNER_USER_ID:
        return OWNER_USER_ID
    if not OWNER_USERNAME:
        raise SystemExit("Set MOBAILMUX_OWNER_USERNAME or MOBAILMUX_OWNER_USER_ID")
    user = api("GET", f"/api/v4/users/username/{OWNER_USERNAME}")
    return user["id"]


def bot_user_id() -> str:
    user = api("GET", "/api/v4/users/me")
    return user["id"]


def team_id() -> str:
    team = api("GET", f"/api/v4/teams/name/{TEAM_NAME}")
    return team["id"]


def channel_id(team: str, channel_name: str) -> str:
    channel = api("GET", f"/api/v4/teams/{team}/channels/name/{channel_name}")
    return channel["id"]


def optional_channel_id(team: str, channel_name: str) -> str | None:
    try:
        return channel_id(team, channel_name)
    except requests.HTTPError as exc:
        status = exc.response.status_code if exc.response is not None else None
        if status == 404:
            return None
        raise


def current_workdir(slot: str) -> str:
    with state_lock:
        return state.setdefault("workdirs", {}).get(slot) or SLOTS[slot].default_workdir


def set_workdir(slot: str, workdir: str) -> None:
    with state_lock:
        state.setdefault("workdirs", {})[slot] = workdir
        save_state(state)


def current_session(slot: str) -> dict:
    with state_lock:
        return dict(state.setdefault("sessions", {}).get(slot, {}))


def set_session(slot: str, thread_id: str, workdir: str) -> None:
    with state_lock:
        state.setdefault("sessions", {})[slot] = {"thread_id": thread_id, "workdir": workdir}
        save_state(state)


def clear_session(slot: str) -> None:
    with state_lock:
        state.setdefault("sessions", {}).pop(slot, None)
        save_state(state)


def update_last_seen(slot: str, timestamp_ms: int) -> None:
    with state_lock:
        last_seen = state.setdefault("last_seen", {})
        last_seen[slot] = max(timestamp_ms, int(last_seen.get(slot, 0)))
        save_state(state)


def get_last_seen(slot: str) -> int:
    with state_lock:
        return int(state.setdefault("last_seen", {}).get(slot, 0))


def worker_running(slot: str) -> bool:
    info = workers.get(slot)
    proc = info.get("proc") if info else None
    return bool(proc and proc.poll() is None)


def kill_worker(slot: str) -> bool:
    info = workers.get(slot)
    proc = info.get("proc") if info else None
    if not proc or proc.poll() is not None:
        return False
    info["stop_requested"] = True
    terminate_process(proc, force=False)
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        terminate_process(proc, force=True)
    return True


def terminate_process(proc: subprocess.Popen, *, force: bool) -> None:
    if IS_WINDOWS:
        if force:
            proc.kill()
            return
        try:
            proc.send_signal(signal.CTRL_BREAK_EVENT)
        except Exception:
            proc.terminate()
        return

    try:
        os.killpg(proc.pid, signal.SIGKILL if force else signal.SIGTERM)
    except ProcessLookupError:
        return
    except Exception:
        if force:
            proc.kill()
        else:
            proc.terminate()


def process_start_options() -> dict:
    if IS_WINDOWS:
        return {"creationflags": getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)}
    return {"start_new_session": True}


def slot_overview_text() -> str:
    lines = ["Available slots:"]
    for name, slot_cfg in SLOTS.items():
        if slot_cfg.channel == name:
            lines.append(f"- `{name}`")
        else:
            lines.append(f"- `{name}` in Mattermost channel `{slot_cfg.channel}`")
    return "\n".join(lines)


def help_text(slot: str) -> str:
    return (
        f"Mobailmux help for `{slot}`\n\n"
        f"{slot_overview_text()}\n\n"
        "Type command shortcuts with `!`, not Mattermost slash commands. Plain messages go to the agent.\n"
        "- `!help` or `!commands` shows this help\n"
        "- `!slots` shows all slot states\n"
        "- `!pwd` shows the current folder\n"
        "- `!ls [path]` lists files without starting an agent job\n"
        "- `!cd [path]` sets the folder for future jobs; no path goes to your home folder\n"
        "- `!fresh` resets this channel's agent chat and Mobailmux logs\n"
        "- `!status` shows whether this slot is busy\n"
        "- `!stop` cancels the active job in this slot\n"
        "- `!logs` shows recent Mobailmux events for this slot\n"
        "- `!model` shows the Codex command/model settings\n"
        "- `!next <request>` queues one follow-up request for this slot\n"
        "- `!queue` shows queued requests\n"
        "- `!clearqueue` clears queued requests\n"
        "- any other message continues this channel's agent chat in the current folder\n\n"
        "Progress posts show command/tool starts and exits automatically."
    )


def codex_config_path() -> Path:
    codex_home = cfg("CODEX_HOME") or os.environ.get("CODEX_HOME")
    if codex_home:
        return Path(expand_path(codex_home)) / "config.toml"
    return Path.home() / ".codex" / "config.toml"


def agent_settings_text() -> str:
    config = codex_config_path()
    details = [
        "Agent settings:",
        f"- driver: `codex`",
        f"- command: `{CODEX_BIN}`",
        f"- extra args: `{shlex.join(CODEX_ARGS) if CODEX_ARGS else '(none)'}`",
        f"- config: `{config}`",
    ]
    if config.exists():
        text = config.read_text()

        def setting(name: str, default: str = "unset") -> str:
            match = re.search(rf"^{re.escape(name)}\s*=\s*(.+?)\s*(?:#.*)?$", text, flags=re.MULTILINE)
            if not match:
                return default
            raw = match.group(1).strip()
            if len(raw) >= 2 and raw[0] == raw[-1] and raw[0] in ("'", '"'):
                return raw[1:-1]
            return raw

        details.extend(
            [
                f"- model: `{setting('model')}`",
                f"- reasoning: `{setting('model_reasoning_effort')}`",
            ]
        )
    return "\n".join(details)


def slot_status_line(slot: str) -> str:
    running = worker_running(slot)
    queued = queue_length(slot)
    workdir = current_workdir(slot)
    session_info = current_session(slot)
    chat = "chat saved" if session_info.get("thread_id") else "new chat"
    current = (workers.get(slot) or {}).get("current_command")
    state_text = "running" if running else "idle"
    line = f"{slot}: {state_text} | {chat} | queued {queued} | {workdir}"
    if current:
        line += f" | {truncate(current, 180)}"
    return line


def all_slots_status() -> str:
    return "Slots:\n```text\n" + "\n".join(slot_status_line(slot) for slot in SLOTS) + "\n```"


def slots_channel_help() -> str:
    return (
        "This is the Mobailmux `slots` channel.\n\n"
        "Type `!slots` to show all slot states.\n"
        "Use the work channels to run jobs:\n"
        + "\n".join(f"- `{slot_cfg.channel}`" for slot_cfg in SLOTS.values())
    )


def history_text(slot: str) -> str:
    with history_lock:
        entries = list(history.setdefault(slot, []))[-20:]
    if not entries:
        return f"{slot} has no recorded job events since the service last started."
    return f"{slot} recent events:\n```text\n" + "\n".join(entries) + "\n```"


def handle_control_message(slot: str, channel: str, message: str) -> bool:
    command = parse_command_message(message)
    if not command.explicit:
        return False
    text = command.text
    lower = text.lower()
    if lower in {"help", "commands"}:
        post(channel, help_text(slot))
        return True
    if lower in {"slots", "list", "overview"}:
        post(channel, all_slots_status())
        return True
    if lower == "pwd":
        post(channel, f"{slot} folder: `{current_workdir(slot)}`")
        return True
    match = re.fullmatch(r"ls(?:\s+(.*))?", text, flags=re.IGNORECASE)
    if match:
        post(channel, list_path_text(slot, match.group(1) or ""))
        return True
    if lower in {"model", "settings"}:
        post(channel, agent_settings_text())
        return True
    if lower in {"mode", "session"}:
        post(channel, f"No mode needed. Just keep talking in {slot}; send `!fresh` when you want a new agent chat.")
        return True
    if lower in {"fresh", "new"}:
        stopped = kill_worker(slot)
        cleared = clear_queued_requests(slot)
        clear_session(slot)
        clear_history(slot)
        extra = []
        if stopped:
            extra.append("stopped the current job")
        if cleared:
            extra.append(f"cleared {cleared} queued request(s)")
        suffix = f" ({', '.join(extra)})." if extra else "."
        post_slot(slot, channel, f"{slot} chat reset{suffix} Previous Mobailmux logs for this slot were cleared. Your next message starts a new agent chat.")
        return True
    if lower == "status":
        session_info = current_session(slot)
        session_text = "chat saved" if session_info.get("thread_id") else "new chat"
        queue_text = f", queued `{queue_length(slot)}`"
        if worker_running(slot):
            info = workers.get(slot) or {}
            current = info.get("current_command") or "working"
            post(channel, f"{slot} is running in `{current_workdir(slot)}` ({session_text}{queue_text}). Current: `{truncate(current, 700)}`")
        else:
            post(channel, f"{slot} is idle in `{current_workdir(slot)}` ({session_text}{queue_text}).")
        return True
    if lower in {"log", "logs", "tail"}:
        post(channel, history_text(slot))
        return True
    if lower in {"queue", "queued"}:
        post(channel, queued_text(slot))
        return True
    if lower in {"clearqueue", "clear queue"}:
        count = clear_queued_requests(slot)
        post(channel, f"Cleared {count} queued request(s) for {slot}.")
        return True
    if lower == "stop":
        if kill_worker(slot):
            post(channel, f"Stop requested for {slot}.")
        else:
            post(channel, f"{slot} is not running.")
        return True
    target_arg = cd_target_arg(text)
    if target_arg is not None:
        target = resolve(target_arg, current_workdir(slot))
        if not Path(target).is_dir():
            post(channel, f"Folder does not exist: `{target}`")
            return True
        set_workdir(slot, target)
        session_info = current_session(slot)
        if session_info.get("workdir") and session_info.get("workdir") != target:
            clear_session(slot)
            post(channel, f"{slot} folder set to `{target}`. Chat reset because the folder changed.")
            return True
        post(channel, f"{slot} folder set to `{target}`")
        return True
    return False


def progress_post(slot: str, channel: str, counter: dict, message: str) -> None:
    with counter["lock"]:
        if MAX_PROGRESS_POSTS > 0 and counter["count"] >= MAX_PROGRESS_POSTS:
            if not counter.get("suppressed"):
                post_slot(slot, channel, "Progress post limit reached; suppressing further command updates until completion.")
                counter["suppressed"] = True
            return
        counter["count"] += 1
    post_slot(slot, channel, message)


def command_progress(slot: str, channel: str, event: dict, counter: dict) -> None:
    item = event.get("item") or {}
    if item.get("type") != "command_execution":
        return

    command = item.get("command") or "(unknown command)"
    if event.get("type") == "item.started":
        workers.setdefault(slot, {})["current_command"] = command
        progress_post(slot, channel, counter, f"{slot} running: `{truncate(command, 1200)}`")
        return

    if event.get("type") != "item.completed":
        return

    exit_code = item.get("exit_code")
    output = item.get("aggregated_output") or ""
    workers.setdefault(slot, {})["current_command"] = None
    reply = f"{slot} command exit {exit_code}: `{truncate(command, 1000)}`"
    if exit_code not in (0, None) and output:
        reply += f"\n{fenced(output)}"
    elif output and len(output.strip()) <= 300:
        reply += f"\n{fenced(output, 500)}"
    progress_post(slot, channel, counter, reply)


def make_progress_helper(temp_dir: Path, progress_file: Path) -> None:
    if IS_WINDOWS:
        helper = temp_dir / "aiprogress.cmd"
        helper.write_text(f"@echo off\r\n>>\"{progress_file}\" echo %*\r\n")
    else:
        helper = temp_dir / "aiprogress"
        helper.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            f"printf '%s\\n' \"$*\" >> {shlex.quote(str(progress_file))}\n"
        )
    helper.chmod(0o700)
    progress_file.touch()


def progress_file_watcher(slot: str, channel: str, progress_file: Path, counter: dict, done: threading.Event) -> None:
    offset = 0
    while not done.is_set():
        try:
            with progress_file.open("r") as handle:
                handle.seek(offset)
                lines = handle.readlines()
                offset = handle.tell()
        except FileNotFoundError:
            lines = []
        for line in lines:
            text = line.strip()
            if text:
                progress_post(slot, channel, counter, f"{slot} note: {truncate(text, 1200)}")
        done.wait(1)

    try:
        with progress_file.open("r") as handle:
            handle.seek(offset)
            lines = handle.readlines()
    except FileNotFoundError:
        lines = []
    for line in lines:
        text = line.strip()
        if text:
            progress_post(slot, channel, counter, f"{slot} note: {truncate(text, 1200)}")


def status_watcher(slot: str, channel: str, started: float, done: threading.Event) -> None:
    while not done.wait(STATUS_SECONDS):
        info = workers.get(slot) or {}
        proc = info.get("proc")
        if not proc or proc.poll() is not None:
            return
        mins = int((time.time() - started) // 60)
        current = info.get("current_command")
        if current:
            post_slot(slot, channel, f"{slot} is still running ({mins} min). Current: `{truncate(current, 900)}`")
        else:
            post_slot(slot, channel, f"{slot} is still running ({mins} min).")


def run_codex(slot: str, channel: str, message: str) -> None:
    workdir = current_workdir(slot)
    session_info = current_session(slot)
    out_file = tempfile.NamedTemporaryFile(prefix=f"{slot}-mobailmux-", suffix=".txt", delete=False)
    out_path = out_file.name
    out_file.close()

    env = os.environ.copy()
    if AGENT_HOME:
        env["HOME"] = expand_path(AGENT_HOME)
    base_path = env.get("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
    started = time.time()
    log_tail: list[str] = []
    progress_counter = {"count": 0, "suppressed": False, "lock": threading.Lock()}
    usage = {}
    observed_thread_id = None
    proc = None
    done = threading.Event()
    stop_requested = False

    with tempfile.TemporaryDirectory(prefix=f"{slot}-mobailmux-progress-") as temp_dir_raw:
        temp_dir = Path(temp_dir_raw)
        progress_file = temp_dir / "progress.log"
        make_progress_helper(temp_dir, progress_file)
        path_parts = [str(temp_dir)]
        if PATH_EXTRA:
            path_parts.append(PATH_EXTRA)
        path_parts.append(base_path)
        env["PATH"] = os.pathsep.join(path_parts)

        prompt = (
            f"You are running from Mattermost slot {slot}.\n"
            f"Current working folder: {workdir}\n"
            "Mattermost already receives automatic command start/exit progress.\n"
            "Use aiprogress for human milestone notes, not for every command.\n"
            "For non-trivial requests, send an early note with the goal and current investigation path. "
            "For longer work, send another note when exploration finishes, when an important finding changes direction, "
            "before risky edits, before verification, or after a couple of minutes without a human-readable update. "
            "Keep notes short and factual.\n"
            "Keep the final reply concise and include what changed plus any verification run.\n\n"
            f"User request:\n{message}"
        )

        session_thread_id = session_info.get("thread_id")
        session_workdir = session_info.get("workdir")
        use_resume = bool(session_thread_id and session_workdir == workdir)

        if use_resume:
            cmd = [
                CODEX_BIN,
                "exec",
                "resume",
                "--json",
                *CODEX_ARGS,
                "--output-last-message",
                out_path,
                session_thread_id,
                prompt,
            ]
        else:
            cmd = [
                CODEX_BIN,
                "exec",
                "--json",
                *CODEX_ARGS,
                "--cd",
                workdir,
                "--output-last-message",
                out_path,
                prompt,
            ]

        start_detail = "continuing chat" if use_resume else "new chat"
        post_slot(slot, channel, f"{slot} started in `{workdir}` ({start_detail}).")
        proc = subprocess.Popen(
            cmd,
            cwd=workdir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            **process_start_options(),
        )
        workers[slot] = {"proc": proc, "started": started, "channel": channel, "current_command": None}
        threading.Thread(target=status_watcher, args=(slot, channel, started, done), daemon=True).start()
        threading.Thread(target=progress_file_watcher, args=(slot, channel, progress_file, progress_counter, done), daemon=True).start()
        try:
            assert proc.stdout is not None
            while proc.poll() is None:
                line = proc.stdout.readline()
                if line:
                    raw_line = line.rstrip()
                    log_tail.append(raw_line)
                    log_tail = log_tail[-80:]
                    try:
                        event = json.loads(raw_line)
                    except json.JSONDecodeError:
                        continue
                    event_type = event.get("type")
                    if event_type == "thread.started" and event.get("thread_id"):
                        observed_thread_id = event["thread_id"]
                        set_session(slot, observed_thread_id, workdir)
                    elif event_type in {"item.started", "item.completed"}:
                        command_progress(slot, channel, event, progress_counter)
                    elif event_type == "turn.completed":
                        usage = event.get("usage") or {}

            for line in proc.stdout.readlines():
                raw_line = line.rstrip()
                log_tail.append(raw_line)
                log_tail = log_tail[-80:]
                try:
                    event = json.loads(raw_line)
                except json.JSONDecodeError:
                    continue
                event_type = event.get("type")
                if event_type == "thread.started" and event.get("thread_id"):
                    observed_thread_id = event["thread_id"]
                    set_session(slot, observed_thread_id, workdir)
                elif event_type in {"item.started", "item.completed"}:
                    command_progress(slot, channel, event, progress_counter)
                elif event_type == "turn.completed":
                    usage = event.get("usage") or {}
        finally:
            done.set()
            time.sleep(0.2)
            stop_requested = bool((workers.get(slot) or {}).get("stop_requested"))
            workers.pop(slot, None)

    try:
        final = Path(out_path).read_text().strip()
    except Exception:
        final = ""
    try:
        Path(out_path).unlink(missing_ok=True)
    except Exception:
        pass

    elapsed = int(time.time() - started)
    if observed_thread_id and not stop_requested:
        set_session(slot, observed_thread_id, workdir)

    usage_text = ""
    if usage:
        input_tokens = usage.get("input_tokens")
        cached_input_tokens = usage.get("cached_input_tokens")
        output_tokens = usage.get("output_tokens")
        if input_tokens is not None or output_tokens is not None:
            usage_text = f"\n\nUsage total across tool calls: input `{input_tokens}`, cached `{cached_input_tokens}`, output `{output_tokens}`"

    returncode = proc.returncode if proc is not None else 1
    if stop_requested:
        post_slot(slot, channel, f"{slot} stopped after {elapsed}s.")
    elif returncode == 0:
        post_slot(slot, channel, f"{slot} done in {elapsed}s.{usage_text}\n\n{final or '(Agent completed without a final message.)'}")
    else:
        tail = "\n".join(log_tail[-30:]).strip()
        post_slot(slot, channel, f"{slot} failed with exit code {returncode} after {elapsed}s.\n\n```text\n{tail[-3000:]}\n```")

    if not stop_requested:
        start_next_job(slot, channel)


def start_worker(slot: str, channel: str, message: str) -> None:
    thread = threading.Thread(target=run_codex, args=(slot, channel, message), daemon=True)
    thread.start()


def start_next_job(slot: str, channel: str) -> None:
    next_message = pop_queued_request(slot)
    if not next_message:
        return
    post_slot(slot, channel, f"{slot} starting queued request. Remaining queued: `{queue_length(slot)}`.")
    start_worker(slot, channel, next_message)


def dispatcher() -> None:
    while not stop_event.is_set():
        try:
            slot, post_obj = job_queue.get(timeout=0.5)
        except queue.Empty:
            continue
        channel = post_obj["channel_id"]
        message = post_obj.get("message", "").strip()
        if not message:
            continue
        if handle_control_message(slot, channel, message):
            continue
        command = parse_command_message(message)
        queue_arg = queue_request_arg(command.text) if command.explicit else None
        if worker_running(slot):
            if queue_arg is not None:
                queued, count = queue_request(slot, queue_arg)
                if queued:
                    post(channel, f"Queued request for {slot}. Queue length: `{count}`.")
                else:
                    post(channel, f"{slot} queue is full (`{count}`). Use `!queue` or `!clearqueue`.")
                continue
            if command.explicit:
                post(channel, unknown_command_text(command.text))
                continue
            post(channel, f"{slot} is already running. Use another slot, send `!next <request>` to queue one, or send `!stop` here first.")
            continue
        if queue_arg is not None:
            message = queue_arg
        elif command.explicit:
            post(channel, unknown_command_text(command.text))
            continue
        start_worker(slot, channel, message)


def poll_loop(slot: str, channel: str, owner: str, bot: str) -> None:
    if get_last_seen(slot) == 0:
        update_last_seen(slot, int(time.time() * 1000))
    while not stop_event.is_set():
        since = get_last_seen(slot)
        try:
            data = api("GET", f"/api/v4/channels/{channel}/posts", params={"since": since})
            order = data.get("order", [])
            posts = data.get("posts", {})
            for post_id in reversed(order):
                item = posts.get(post_id)
                if not item:
                    continue
                create_at = int(item.get("create_at") or 0)
                update_last_seen(slot, create_at)
                if item.get("user_id") == bot:
                    continue
                if item.get("user_id") != owner:
                    continue
                if item.get("type"):
                    continue
                job_queue.put((slot, item))
        except Exception as exc:
            print(f"[{slot}] poll error: {exc}", flush=True)
        time.sleep(POLL_SECONDS)


def poll_slots_channel(channel: str, owner: str, bot: str) -> None:
    state_key = f"channel:{SLOTS_CHANNEL}"
    if get_last_seen(state_key) == 0:
        update_last_seen(state_key, int(time.time() * 1000))
    while not stop_event.is_set():
        since = get_last_seen(state_key)
        try:
            data = api("GET", f"/api/v4/channels/{channel}/posts", params={"since": since})
            order = data.get("order", [])
            posts = data.get("posts", {})
            for post_id in reversed(order):
                item = posts.get(post_id)
                if not item:
                    continue
                create_at = int(item.get("create_at") or 0)
                update_last_seen(state_key, create_at)
                if item.get("user_id") == bot:
                    continue
                if item.get("user_id") != owner:
                    continue
                if item.get("type"):
                    continue
                message = item.get("message", "").strip()
                command = parse_command_message(message)
                if not command.explicit:
                    if message:
                        post(channel, "This channel only accepts Mobailmux shortcuts. Type `!slots` or `!help`.")
                    continue
                text = command.text.lower()
                if text in {"slots", "status", "list", "overview"}:
                    post(channel, all_slots_status())
                elif text in {"help", "commands"}:
                    post(channel, slots_channel_help())
                elif text:
                    post(channel, unknown_command_text(command.text))
        except Exception as exc:
            print(f"[{SLOTS_CHANNEL}] poll error: {exc}", flush=True)
        time.sleep(POLL_SECONDS)


def main() -> None:
    def on_signal(_signum, _frame):
        stop_event.set()
        for slot in list(workers):
            kill_worker(slot)

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT, on_signal)

    owner = owner_user_id()
    bot = bot_user_id()
    team = team_id()
    channels = {slot: channel_id(team, slot_cfg.channel) for slot, slot_cfg in SLOTS.items()}
    work_channel_names = {slot_cfg.channel for slot_cfg in SLOTS.values()}
    slots_channel = None
    if SLOTS_CHANNEL and SLOTS_CHANNEL not in work_channel_names:
        slots_channel = optional_channel_id(team, SLOTS_CHANNEL)
        if not slots_channel:
            print(f"mobailmux slots channel `{SLOTS_CHANNEL}` not found; skipping status channel", flush=True)

    threading.Thread(target=dispatcher, daemon=True).start()
    for slot, channel in channels.items():
        threading.Thread(target=poll_loop, args=(slot, channel, owner, bot), daemon=True).start()
    if slots_channel:
        threading.Thread(target=poll_slots_channel, args=(slots_channel, owner, bot), daemon=True).start()

    status_channel_text = f"; status channel {SLOTS_CHANNEL}" if slots_channel else ""
    print(f"mobailmux running for {', '.join(channels)}{status_channel_text}", flush=True)
    while not stop_event.is_set():
        time.sleep(1)

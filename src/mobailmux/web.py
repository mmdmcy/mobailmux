from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import queue
import re
import secrets
import shlex
import signal
import sqlite3
import subprocess
import tempfile
import threading
import time
from dataclasses import dataclass
from http import cookies
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse


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


def expand_path(value: str) -> str:
    return str(Path(os.path.expandvars(os.path.expanduser(value))).resolve())


def env_key_fragment(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "_", value).strip("_").upper()


def truncate(value: str, max_chars: int) -> str:
    value = value.strip()
    if len(value) <= max_chars:
        return value
    return value[: max_chars - 20] + "\n...[truncated]"


def fenced(value: str, max_chars: int) -> str:
    value = truncate(value, max_chars)
    return f"```text\n{value}\n```"


@dataclass(frozen=True)
class SlotConfig:
    name: str
    label: str
    default_workdir: str


class MobailmuxWeb:
    def __init__(self) -> None:
        self.is_windows = os.name == "nt"
        self.default_workdir = expand_path(cfg("MOBAILMUX_DEFAULT_WORKDIR", "~") or "~")
        self.state_dir = Path(expand_path(cfg("MOBAILMUX_STATE_DIR", "~/.local/state/mobailmux") or "~/.local/state/mobailmux"))
        self.state_file = self.state_dir / "state.json"
        self.db_file = Path(expand_path(cfg("MOBAILMUX_WEB_DB", str(self.state_dir / "web.sqlite3")) or str(self.state_dir / "web.sqlite3")))
        self.slots = self.parse_slots()
        self.codex_bin = cfg("MOBAILMUX_CODEX_BIN", "codex") or "codex"
        self.codex_args = shlex.split(cfg("MOBAILMUX_CODEX_ARGS", "") or "", posix=not self.is_windows)
        self.agent_home = cfg("MOBAILMUX_AGENT_HOME")
        self.path_extra = cfg("MOBAILMUX_PATH_EXTRA", "") or ""
        self.status_seconds = int(cfg("MOBAILMUX_STATUS_SECONDS", "60"))
        self.max_progress_posts = int(cfg("MOBAILMUX_MAX_PROGRESS_POSTS", "0"))
        self.output_snippet_chars = int(cfg("MOBAILMUX_OUTPUT_SNIPPET_CHARS", "1200"))
        self.max_queued_per_slot = int(cfg("MOBAILMUX_MAX_QUEUED_PER_SLOT", "5"))
        self.max_ls_entries = int(cfg("MOBAILMUX_MAX_LS_ENTRIES", "120"))
        self.state_lock = threading.Lock()
        self.db_lock = threading.Lock()
        self.event_cond = threading.Condition()
        self.workers: dict[str, dict] = {}
        self.queued_lock = threading.Lock()
        self.queued_requests: dict[str, list[str]] = {}
        self.stop_event = threading.Event()
        self.state = self.load_state()
        self.init_db()

    def parse_slots(self) -> dict[str, SlotConfig]:
        raw = cfg("MOBAILMUX_SLOTS", "one:agent-one,two:agent-two,three:agent-three") or ""
        slots: dict[str, SlotConfig] = {}
        for item in raw.split(","):
            item = item.strip()
            if not item:
                continue
            parts = [part.strip() for part in item.split(":", 2)]
            if len(parts) < 2 or not parts[0] or not parts[1]:
                raise SystemExit(f"Invalid slot spec {item!r}; expected name:label[:workdir]")
            name, label = parts[0], parts[1]
            workdir = parts[2] if len(parts) == 3 and parts[2] else self.default_workdir
            env_workdir = cfg(f"MOBAILMUX_SLOT_{env_key_fragment(name)}_WORKDIR")
            if env_workdir:
                workdir = env_workdir
            slots[name] = SlotConfig(name=name, label=label, default_workdir=expand_path(workdir))
        if not slots:
            raise SystemExit("MOBAILMUX_SLOTS did not define any slots")
        return slots

    def init_db(self) -> None:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        self.db_file.parent.mkdir(parents=True, exist_ok=True)
        with self.connect_db() as db:
            db.execute(
                """
                create table if not exists messages (
                    seq integer primary key autoincrement,
                    created_at integer not null,
                    slot text not null,
                    role text not null,
                    message text not null
                )
                """
            )
            db.execute("create index if not exists messages_slot_seq on messages(slot, seq)")

    def connect_db(self) -> sqlite3.Connection:
        db = sqlite3.connect(self.db_file, timeout=30)
        db.row_factory = sqlite3.Row
        return db

    def load_state(self) -> dict:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        if self.state_file.exists():
            try:
                return json.loads(self.state_file.read_text())
            except json.JSONDecodeError:
                pass
        return {
            "initialized_at": int(time.time() * 1000),
            "last_seen": {},
            "sessions": {},
            "workdirs": {slot: config.default_workdir for slot, config in self.slots.items()},
        }

    def save_state(self) -> None:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        tmp = self.state_file.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.state, indent=2, sort_keys=True))
        tmp.replace(self.state_file)

    def append_message(self, slot: str, role: str, message: str) -> dict:
        now = int(time.time() * 1000)
        with self.db_lock:
            with self.connect_db() as db:
                cur = db.execute(
                    "insert into messages(created_at, slot, role, message) values (?, ?, ?, ?)",
                    (now, slot, role, message),
                )
                seq = int(cur.lastrowid)
        payload = {"seq": seq, "created_at": now, "slot": slot, "role": role, "message": message}
        with self.event_cond:
            self.event_cond.notify_all()
        return payload

    def messages_since(self, after: int = 0, limit: int = 250) -> list[dict]:
        with self.db_lock:
            with self.connect_db() as db:
                rows = db.execute(
                    """
                    select seq, created_at, slot, role, message
                    from messages
                    where seq > ?
                    order by seq asc
                    limit ?
                    """,
                    (after, limit),
                ).fetchall()
        return [dict(row) for row in rows]

    def recent_messages(self, limit: int = 250) -> list[dict]:
        with self.db_lock:
            with self.connect_db() as db:
                rows = db.execute(
                    """
                    select seq, created_at, slot, role, message
                    from messages
                    order by seq desc
                    limit ?
                    """,
                    (limit,),
                ).fetchall()
        return [dict(row) for row in reversed(rows)]

    def resolve(self, path: str, base: str) -> str:
        expanded = os.path.expandvars(os.path.expanduser(path.strip()))
        if not os.path.isabs(expanded):
            expanded = os.path.join(base, expanded)
        return str(Path(expanded).resolve())

    def current_workdir(self, slot: str) -> str:
        with self.state_lock:
            return self.state.setdefault("workdirs", {}).get(slot) or self.slots[slot].default_workdir

    def set_workdir(self, slot: str, workdir: str) -> None:
        with self.state_lock:
            self.state.setdefault("workdirs", {})[slot] = workdir
            self.save_state()

    def current_session(self, slot: str) -> dict:
        with self.state_lock:
            return dict(self.state.setdefault("sessions", {}).get(slot, {}))

    def set_session(self, slot: str, thread_id: str, workdir: str) -> None:
        with self.state_lock:
            self.state.setdefault("sessions", {})[slot] = {"thread_id": thread_id, "workdir": workdir}
            self.save_state()

    def clear_session(self, slot: str) -> None:
        with self.state_lock:
            self.state.setdefault("sessions", {}).pop(slot, None)
            self.save_state()

    def worker_running(self, slot: str) -> bool:
        info = self.workers.get(slot)
        proc = info.get("proc") if info else None
        return bool(proc and proc.poll() is None)

    def terminate_process(self, proc: subprocess.Popen, *, force: bool) -> None:
        if self.is_windows:
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

    def process_start_options(self) -> dict:
        if self.is_windows:
            return {"creationflags": getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)}
        return {"start_new_session": True}

    def kill_worker(self, slot: str) -> bool:
        info = self.workers.get(slot)
        proc = info.get("proc") if info else None
        if not proc or proc.poll() is not None:
            return False
        info["stop_requested"] = True
        self.terminate_process(proc, force=False)
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.terminate_process(proc, force=True)
        return True

    def queue_request(self, slot: str, message: str) -> tuple[bool, int]:
        with self.queued_lock:
            requests_for_slot = self.queued_requests.setdefault(slot, [])
            if len(requests_for_slot) >= self.max_queued_per_slot:
                return False, len(requests_for_slot)
            requests_for_slot.append(message)
            return True, len(requests_for_slot)

    def pop_queued_request(self, slot: str) -> str | None:
        with self.queued_lock:
            requests_for_slot = self.queued_requests.setdefault(slot, [])
            if not requests_for_slot:
                return None
            return requests_for_slot.pop(0)

    def clear_queued_requests(self, slot: str) -> int:
        with self.queued_lock:
            count = len(self.queued_requests.setdefault(slot, []))
            self.queued_requests[slot] = []
            return count

    def queue_length(self, slot: str) -> int:
        with self.queued_lock:
            return len(self.queued_requests.setdefault(slot, []))

    def queued_text(self, slot: str) -> str:
        with self.queued_lock:
            items = list(self.queued_requests.setdefault(slot, []))
        if not items:
            return f"{slot} queue is empty."
        lines = [f"{idx}. {truncate(item, 220)}" for idx, item in enumerate(items, start=1)]
        return f"{slot} queued requests:\n" + "\n".join(lines)

    def parse_ls_args(self, raw: str) -> tuple[bool, str | None, str | None]:
        try:
            args = shlex.split(raw, posix=not self.is_windows)
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

    def list_path_text(self, slot: str, raw_args: str) -> str:
        show_hidden, target_arg, error = self.parse_ls_args(raw_args)
        if error:
            return error
        target = Path(self.resolve(target_arg or ".", self.current_workdir(slot)))
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
        omitted = max(0, len(names) - self.max_ls_entries)
        names = names[: self.max_ls_entries]
        if omitted:
            names.append(f"... {omitted} more")
        if not names:
            names = ["(empty)"]
        return f"{slot} listing `{target}`:\n```text\n" + "\n".join(names) + "\n```"

    def help_text(self, slot: str) -> str:
        return (
            f"{slot} commands:\n"
            "- `slots` shows all slot states\n"
            "- `pwd` shows the current folder\n"
            "- `ls [path]` lists files without starting an agent job\n"
            "- `cd /path/to/project` sets the folder for future jobs in this slot\n"
            "- `fresh` resets this slot's agent chat\n"
            "- `status` shows whether this slot is busy\n"
            "- `stop` cancels the active job in this slot\n"
            "- any other message continues this slot's agent chat in the current folder\n"
            "\nAdvanced commands: `logs`, `model`, `next <request>`, `queue`, `clearqueue`."
        )

    def codex_config_path(self) -> Path:
        codex_home = cfg("CODEX_HOME") or os.environ.get("CODEX_HOME")
        if codex_home:
            return Path(expand_path(codex_home)) / "config.toml"
        return Path.home() / ".codex" / "config.toml"

    def agent_settings_text(self) -> str:
        config = self.codex_config_path()
        details = [
            "Agent settings:",
            f"- driver: `codex`",
            f"- command: `{self.codex_bin}`",
            f"- extra args: `{shlex.join(self.codex_args) if self.codex_args else '(none)'}`",
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

            details.extend([f"- model: `{setting('model')}`", f"- reasoning: `{setting('model_reasoning_effort')}`"])
        return "\n".join(details)

    def slot_status(self, slot: str) -> dict:
        running = self.worker_running(slot)
        session_info = self.current_session(slot)
        current = (self.workers.get(slot) or {}).get("current_command")
        return {
            "name": slot,
            "label": self.slots[slot].label,
            "running": running,
            "state": "running" if running else "idle",
            "chat": "chat saved" if session_info.get("thread_id") else "new chat",
            "queued": self.queue_length(slot),
            "workdir": self.current_workdir(slot),
            "current_command": current,
        }

    def all_slots_status_text(self) -> str:
        lines = []
        for slot in self.slots:
            item = self.slot_status(slot)
            line = f"{slot}: {item['state']} | {item['chat']} | queued {item['queued']} | {item['workdir']}"
            if item["current_command"]:
                line += f" | {truncate(item['current_command'], 180)}"
            lines.append(line)
        return "Slots:\n```text\n" + "\n".join(lines) + "\n```"

    def history_text(self, slot: str) -> str:
        with self.db_lock:
            with self.connect_db() as db:
                rows = db.execute(
                    """
                    select created_at, role, message
                    from messages
                    where slot = ?
                    order by seq desc
                    limit 20
                    """,
                    (slot,),
                ).fetchall()
        if not rows:
            return f"{slot} has no recorded events."
        lines = []
        for row in reversed(rows):
            stamp = time.strftime("%H:%M:%S", time.localtime(int(row["created_at"]) / 1000))
            first_line = " ".join(str(row["message"]).strip().split())[:180]
            lines.append(f"[{stamp}] {row['role']}: {first_line}")
        return f"{slot} recent events:\n```text\n" + "\n".join(lines) + "\n```"

    def handle_control_message(self, slot: str, message: str) -> bool:
        text = message.strip()
        lower = text.lower()
        if lower in {"help", "commands"}:
            self.append_message(slot, "assistant", self.help_text(slot))
            return True
        if lower in {"slots", "list", "overview"}:
            self.append_message(slot, "assistant", self.all_slots_status_text())
            return True
        if lower == "pwd":
            self.append_message(slot, "assistant", f"{slot} folder: `{self.current_workdir(slot)}`")
            return True
        match = re.fullmatch(r"ls(?:\s+(.*))?", text, flags=re.IGNORECASE)
        if match:
            self.append_message(slot, "assistant", self.list_path_text(slot, match.group(1) or ""))
            return True
        if lower in {"model", "settings"}:
            self.append_message(slot, "assistant", self.agent_settings_text())
            return True
        if lower in {"fresh", "new"}:
            stopped = self.kill_worker(slot)
            cleared = self.clear_queued_requests(slot)
            self.clear_session(slot)
            extra = []
            if stopped:
                extra.append("stopped the current job")
            if cleared:
                extra.append(f"cleared {cleared} queued request(s)")
            suffix = f" ({', '.join(extra)})." if extra else "."
            self.append_message(slot, "assistant", f"{slot} chat reset{suffix} Your next message starts a new agent chat.")
            return True
        if lower == "status":
            item = self.slot_status(slot)
            if item["running"]:
                current = item["current_command"] or "working"
                self.append_message(slot, "assistant", f"{slot} is running in `{item['workdir']}` ({item['chat']}, queued `{item['queued']}`). Current: `{truncate(current, 700)}`")
            else:
                self.append_message(slot, "assistant", f"{slot} is idle in `{item['workdir']}` ({item['chat']}, queued `{item['queued']}`).")
            return True
        if lower in {"log", "logs", "tail"}:
            self.append_message(slot, "assistant", self.history_text(slot))
            return True
        if lower in {"queue", "queued"}:
            self.append_message(slot, "assistant", self.queued_text(slot))
            return True
        if lower in {"clearqueue", "clear queue"}:
            count = self.clear_queued_requests(slot)
            self.append_message(slot, "assistant", f"Cleared {count} queued request(s) for {slot}.")
            return True
        if lower == "stop":
            if self.kill_worker(slot):
                self.append_message(slot, "assistant", f"Stop requested for {slot}.")
            else:
                self.append_message(slot, "assistant", f"{slot} is not running.")
            return True
        match = re.fullmatch(r"(?:cd|folder|workdir)\s+(.+)", text, flags=re.IGNORECASE)
        if match:
            target = self.resolve(match.group(1), self.current_workdir(slot))
            if not Path(target).is_dir():
                self.append_message(slot, "assistant", f"Folder does not exist: `{target}`")
                return True
            old_session = self.current_session(slot)
            self.set_workdir(slot, target)
            if old_session.get("workdir") and old_session.get("workdir") != target:
                self.clear_session(slot)
                self.append_message(slot, "assistant", f"{slot} folder set to `{target}`. Chat reset because the folder changed.")
            else:
                self.append_message(slot, "assistant", f"{slot} folder set to `{target}`")
            return True
        return False

    def submit_message(self, slot: str, message: str) -> None:
        if slot not in self.slots:
            raise KeyError(slot)
        message = message.strip()
        if not message:
            return
        self.append_message(slot, "user", message)
        if self.handle_control_message(slot, message):
            return
        queue_match = re.fullmatch(r"(?:next|queue)\s+(.+)", message, flags=re.IGNORECASE | re.DOTALL)
        if self.worker_running(slot):
            if queue_match:
                queued, count = self.queue_request(slot, queue_match.group(1).strip())
                if queued:
                    self.append_message(slot, "assistant", f"Queued request for {slot}. Queue length: `{count}`.")
                else:
                    self.append_message(slot, "assistant", f"{slot} queue is full (`{count}`). Use `queue` or `clearqueue`.")
                return
            self.append_message(slot, "assistant", f"{slot} is already running. Use another slot, send `next <request>` to queue one, or send `stop` here first.")
            return
        if queue_match:
            message = queue_match.group(1).strip()
        self.start_worker(slot, message)

    def progress_post(self, slot: str, counter: dict, message: str) -> None:
        with counter["lock"]:
            if self.max_progress_posts > 0 and counter["count"] >= self.max_progress_posts:
                if not counter.get("suppressed"):
                    self.append_message(slot, "assistant", "Progress post limit reached; suppressing further command updates until completion.")
                    counter["suppressed"] = True
                return
            counter["count"] += 1
        self.append_message(slot, "assistant", message)

    def command_progress(self, slot: str, event: dict, counter: dict) -> None:
        item = event.get("item") or {}
        if item.get("type") != "command_execution":
            return
        command = item.get("command") or "(unknown command)"
        if event.get("type") == "item.started":
            self.workers.setdefault(slot, {})["current_command"] = command
            self.progress_post(slot, counter, f"{slot} running: `{truncate(command, 1200)}`")
            return
        if event.get("type") != "item.completed":
            return
        exit_code = item.get("exit_code")
        output = item.get("aggregated_output") or ""
        self.workers.setdefault(slot, {})["current_command"] = None
        reply = f"{slot} command exit {exit_code}: `{truncate(command, 1000)}`"
        if exit_code not in (0, None) and output:
            reply += f"\n{fenced(output, self.output_snippet_chars)}"
        elif output and len(output.strip()) <= 300:
            reply += f"\n{fenced(output, 500)}"
        self.progress_post(slot, counter, reply)

    def make_progress_helper(self, temp_dir: Path, progress_file: Path) -> None:
        if self.is_windows:
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

    def progress_file_watcher(self, slot: str, progress_file: Path, counter: dict, done: threading.Event) -> None:
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
                    self.progress_post(slot, counter, f"{slot} note: {truncate(text, 1200)}")
            done.wait(1)

    def status_watcher(self, slot: str, started: float, done: threading.Event) -> None:
        while not done.wait(self.status_seconds):
            info = self.workers.get(slot) or {}
            proc = info.get("proc")
            if not proc or proc.poll() is not None:
                return
            mins = int((time.time() - started) // 60)
            current = info.get("current_command")
            if current:
                self.append_message(slot, "assistant", f"{slot} is still running ({mins} min). Current: `{truncate(current, 900)}`")
            else:
                self.append_message(slot, "assistant", f"{slot} is still running ({mins} min).")

    def run_codex(self, slot: str, message: str) -> None:
        workdir = self.current_workdir(slot)
        session_info = self.current_session(slot)
        out_file = tempfile.NamedTemporaryFile(prefix=f"{slot}-mobailmux-", suffix=".txt", delete=False)
        out_path = out_file.name
        out_file.close()
        env = os.environ.copy()
        if self.agent_home:
            env["HOME"] = expand_path(self.agent_home)
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
            self.make_progress_helper(temp_dir, progress_file)
            path_parts = [str(temp_dir)]
            if self.path_extra:
                path_parts.append(self.path_extra)
            path_parts.append(base_path)
            env["PATH"] = os.pathsep.join(path_parts)
            prompt = (
                f"You are running from Mobailmux web slot {slot}.\n"
                f"Current working folder: {workdir}\n"
                "The web UI already receives automatic command start/exit progress.\n"
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
                    self.codex_bin,
                    "exec",
                    "resume",
                    "--json",
                    *self.codex_args,
                    "--output-last-message",
                    out_path,
                    session_thread_id,
                    prompt,
                ]
            else:
                cmd = [
                    self.codex_bin,
                    "exec",
                    "--json",
                    *self.codex_args,
                    "--cd",
                    workdir,
                    "--output-last-message",
                    out_path,
                    prompt,
                ]
            start_detail = "continuing chat" if use_resume else "new chat"
            self.append_message(slot, "assistant", f"{slot} started in `{workdir}` ({start_detail}).")
            try:
                proc = subprocess.Popen(
                    cmd,
                    cwd=workdir,
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    stdin=subprocess.DEVNULL,
                    text=True,
                    bufsize=1,
                    **self.process_start_options(),
                )
            except FileNotFoundError:
                self.append_message(slot, "assistant", f"Codex command not found: `{self.codex_bin}`")
                return
            self.workers[slot] = {"proc": proc, "started": started, "current_command": None}
            threading.Thread(target=self.status_watcher, args=(slot, started, done), daemon=True).start()
            threading.Thread(target=self.progress_file_watcher, args=(slot, progress_file, progress_counter, done), daemon=True).start()
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
                            self.set_session(slot, observed_thread_id, workdir)
                        elif event_type in {"item.started", "item.completed"}:
                            self.command_progress(slot, event, progress_counter)
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
                        self.set_session(slot, observed_thread_id, workdir)
                    elif event_type in {"item.started", "item.completed"}:
                        self.command_progress(slot, event, progress_counter)
                    elif event_type == "turn.completed":
                        usage = event.get("usage") or {}
            finally:
                done.set()
                time.sleep(0.2)
                stop_requested = bool((self.workers.get(slot) or {}).get("stop_requested"))
                self.workers.pop(slot, None)

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
            self.set_session(slot, observed_thread_id, workdir)
        usage_text = ""
        if usage:
            input_tokens = usage.get("input_tokens")
            cached_input_tokens = usage.get("cached_input_tokens")
            output_tokens = usage.get("output_tokens")
            if input_tokens is not None or output_tokens is not None:
                usage_text = f"\n\nUsage total across tool calls: input `{input_tokens}`, cached `{cached_input_tokens}`, output `{output_tokens}`"
        returncode = proc.returncode if proc is not None else 1
        if stop_requested:
            self.append_message(slot, "assistant", f"{slot} stopped after {elapsed}s.")
        elif returncode == 0:
            self.append_message(slot, "assistant", f"{slot} done in {elapsed}s.{usage_text}\n\n{final or '(Agent completed without a final message.)'}")
        else:
            tail = "\n".join(log_tail[-30:]).strip()
            self.append_message(slot, "assistant", f"{slot} failed with exit code {returncode} after {elapsed}s.\n\n```text\n{tail[-3000:]}\n```")
        if not stop_requested:
            self.start_next_job(slot)

    def start_worker(self, slot: str, message: str) -> None:
        thread = threading.Thread(target=self.run_codex, args=(slot, message), daemon=True)
        thread.start()

    def start_next_job(self, slot: str) -> None:
        next_message = self.pop_queued_request(slot)
        if not next_message:
            return
        self.append_message(slot, "assistant", f"{slot} starting queued request. Remaining queued: `{self.queue_length(slot)}`.")
        self.start_worker(slot, next_message)

    def public_state(self) -> dict:
        return {
            "slots": [self.slot_status(slot) for slot in self.slots],
            "messages": self.recent_messages(),
        }


class Auth:
    def __init__(self, runtime: MobailmuxWeb, password: str | None, enabled: bool) -> None:
        self.runtime = runtime
        self.password = password
        self.enabled = enabled
        self.cookie_name = "mobailmux_session"
        self.max_age = int(cfg("MOBAILMUX_WEB_SESSION_SECONDS", str(7 * 24 * 60 * 60)) or str(7 * 24 * 60 * 60))
        self.secret = self.load_secret()

    def load_secret(self) -> bytes:
        secret_path = self.runtime.state_dir / "web-cookie-secret"
        if secret_path.exists():
            return secret_path.read_bytes()
        secret = secrets.token_bytes(32)
        self.runtime.state_dir.mkdir(parents=True, exist_ok=True)
        secret_path.write_bytes(secret)
        try:
            secret_path.chmod(0o600)
        except OSError:
            pass
        return secret

    def sign(self, value: str) -> str:
        digest = hmac.new(self.secret, value.encode(), hashlib.sha256).digest()
        return base64.urlsafe_b64encode(digest).decode().rstrip("=")

    def make_cookie(self) -> str:
        exp = str(int(time.time()) + self.max_age)
        value = f"{exp}.{self.sign(exp)}"
        return f"{self.cookie_name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={self.max_age}"

    def clear_cookie(self) -> str:
        return f"{self.cookie_name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"

    def verify_cookie(self, header: str | None) -> bool:
        if not self.enabled:
            return True
        if not header:
            return False
        jar = cookies.SimpleCookie()
        try:
            jar.load(header)
        except cookies.CookieError:
            return False
        morsel = jar.get(self.cookie_name)
        if not morsel:
            return False
        parts = morsel.value.split(".", 1)
        if len(parts) != 2:
            return False
        exp, sig = parts
        if not exp.isdigit() or int(exp) < int(time.time()):
            return False
        return hmac.compare_digest(sig, self.sign(exp))

    def verify_password(self, password: str) -> bool:
        return bool(self.password and hmac.compare_digest(password, self.password))


INDEX_HTML = r"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Mobailmux</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #11100f;
      --panel: #1c1b19;
      --panel-alt: #171615;
      --line: #3a3630;
      --text: #f2eee8;
      --muted: #aaa39a;
      --accent: #2dd4bf;
      --accent-dark: #14b8a6;
      --danger: #f87171;
      --input: #121110;
      --user-bg: #132d2a;
      --user-line: #24776d;
      --active-bg: #123b36;
      --overlay: rgba(17, 16, 15, .92);
    }
    * { box-sizing: border-box; }
    html, body {
      height: 100%;
      overflow: hidden;
    }
    body {
      margin: 0;
      background: var(--bg);
      color: var(--text);
      font: 15px/1.45 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }
    button, input, textarea { font: inherit; }
    button {
      min-height: 38px;
      border: 1px solid var(--line);
      border-radius: 6px;
      background: var(--panel);
      color: var(--text);
      padding: 7px 11px;
      cursor: pointer;
    }
    button:hover { border-color: #716a60; }
    button.primary {
      background: var(--accent);
      border-color: var(--accent);
      color: #081311;
    }
    button.primary:hover { background: var(--accent-dark); }
    button.active {
      border-color: var(--accent);
      color: var(--text);
      background: var(--active-bg);
    }
    .app {
      height: 100vh;
      height: 100dvh;
      min-height: 0;
      display: grid;
      grid-template-rows: auto auto minmax(0, 1fr) auto;
      overflow: hidden;
    }
    header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      padding: 12px 14px;
      border-bottom: 1px solid var(--line);
      background: var(--panel);
    }
    h1 {
      margin: 0;
      font-size: 18px;
      font-weight: 650;
    }
    .status {
      min-width: 78px;
      text-align: right;
      color: var(--muted);
      font-size: 13px;
    }
    .slots {
      display: flex;
      gap: 8px;
      overflow-x: auto;
      padding: 10px 12px;
      border-bottom: 1px solid var(--line);
      background: var(--panel-alt);
    }
    .slot {
      flex: 0 0 auto;
      min-width: 104px;
      display: grid;
      gap: 2px;
      text-align: left;
    }
    .slot small {
      color: var(--muted);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .messages {
      min-height: 0;
      overflow-y: auto;
      overscroll-behavior: contain;
      -webkit-overflow-scrolling: touch;
      padding: 14px;
      display: flex;
      flex-direction: column;
      gap: 10px;
    }
    .message {
      max-width: min(860px, 100%);
      border: 1px solid var(--line);
      border-radius: 8px;
      background: var(--panel);
      padding: 10px 12px;
      white-space: pre-wrap;
      overflow-wrap: anywhere;
    }
    .message.user {
      align-self: flex-end;
      border-color: var(--user-line);
      background: var(--user-bg);
    }
    .message.assistant {
      align-self: flex-start;
    }
    .meta {
      margin-bottom: 6px;
      color: var(--muted);
      font-size: 12px;
      display: flex;
      justify-content: space-between;
      gap: 10px;
    }
    pre, code {
      font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      font-size: 13px;
    }
    .composer {
      border-top: 1px solid var(--line);
      background: var(--panel);
      padding: 10px;
      display: grid;
      gap: 8px;
    }
    .quick {
      display: flex;
      gap: 7px;
      overflow-x: auto;
    }
    .compose-row {
      display: grid;
      grid-template-columns: 1fr auto;
      gap: 8px;
      align-items: end;
    }
    textarea {
      min-height: 46px;
      max-height: 180px;
      resize: vertical;
      border: 1px solid var(--line);
      border-radius: 7px;
      padding: 10px;
      background: var(--input);
      color: var(--text);
    }
    .login {
      position: fixed;
      inset: 0;
      display: none;
      place-items: center;
      background: var(--overlay);
      padding: 16px;
    }
    .login.visible { display: grid; }
    .login form {
      width: min(360px, 100%);
      display: grid;
      gap: 10px;
      padding: 18px;
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
    }
    .login input {
      border: 1px solid var(--line);
      border-radius: 7px;
      padding: 10px;
    }
    .error { color: var(--danger); min-height: 20px; }
    @media (min-width: 900px) {
      .app {
        grid-template-columns: 238px 1fr;
        grid-template-rows: auto minmax(0, 1fr) auto;
      }
      header { grid-column: 1 / 3; }
      .slots {
        grid-row: 2 / 4;
        flex-direction: column;
        border-right: 1px solid var(--line);
        border-bottom: 0;
        align-items: stretch;
      }
      .slot { width: 100%; }
      .messages { grid-column: 2; }
      .composer { grid-column: 2; }
    }
  </style>
</head>
<body>
  <main class="app">
    <header>
      <h1>Mobailmux</h1>
      <div class="status" id="connection">offline</div>
    </header>
    <nav class="slots" id="slots"></nav>
    <section class="messages" id="messages"></section>
    <section class="composer">
      <div class="quick">
        <button data-cmd="status">status</button>
        <button data-cmd="pwd">pwd</button>
        <button data-cmd="ls">ls</button>
        <button data-cmd="slots">slots</button>
        <button data-cmd="fresh">fresh</button>
        <button data-cmd="stop">stop</button>
      </div>
      <form class="compose-row" id="composer">
        <textarea id="input" autocomplete="off" placeholder="Message current slot"></textarea>
        <button class="primary" type="submit">Send</button>
      </form>
    </section>
  </main>
  <div class="login" id="login">
    <form id="loginForm">
      <h1>Mobailmux</h1>
      <input id="password" name="password" type="password" autocomplete="current-password" placeholder="Password">
      <button class="primary" type="submit">Sign in</button>
      <div class="error" id="loginError"></div>
    </form>
  </div>
  <script>
    const state = { slots: [], messages: [], activeSlot: null, seen: new Set(), lastSeq: 0, events: null };
    const $ = (id) => document.getElementById(id);
    const escapeHtml = (value) => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
    const timeText = (ms) => new Date(ms).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});

    async function request(path, options = {}) {
      const response = await fetch(path, {
        credentials: 'same-origin',
        headers: {'Content-Type': 'application/json', ...(options.headers || {})},
        ...options
      });
      if (response.status === 401) {
        showLogin();
        throw new Error('unauthorized');
      }
      if (!response.ok) throw new Error(await response.text());
      return response.json();
    }

    function showLogin() { $('login').classList.add('visible'); }
    function hideLogin() { $('login').classList.remove('visible'); }

    function renderSlots() {
      const root = $('slots');
      root.innerHTML = '';
      state.slots.forEach(slot => {
        const button = document.createElement('button');
        button.className = 'slot' + (slot.name === state.activeSlot ? ' active' : '');
        button.innerHTML = `<strong>${escapeHtml(slot.name)}</strong><small>${escapeHtml(slot.state)} | queued ${slot.queued}</small>`;
        button.onclick = () => { state.activeSlot = slot.name; render(); };
        root.appendChild(button);
      });
    }

    function renderMessages() {
      const root = $('messages');
      const nearBottom = root.scrollTop + root.clientHeight >= root.scrollHeight - 80;
      root.innerHTML = '';
      const filtered = state.messages.filter(m => m.slot === state.activeSlot);
      filtered.forEach(message => {
        const node = document.createElement('article');
        node.className = 'message ' + message.role;
        node.innerHTML = `<div class="meta"><span>${escapeHtml(message.role)}</span><span>${timeText(message.created_at)}</span></div>${escapeHtml(message.message)}`;
        root.appendChild(node);
      });
      if (!filtered.length) {
        const empty = document.createElement('article');
        empty.className = 'message assistant';
        empty.textContent = 'No messages in this slot yet.';
        root.appendChild(empty);
      }
      if (nearBottom) root.scrollTop = root.scrollHeight;
    }

    function render() {
      if (!state.activeSlot && state.slots.length) state.activeSlot = state.slots[0].name;
      renderSlots();
      renderMessages();
    }

    function mergeMessages(messages) {
      messages.forEach(message => {
        if (state.seen.has(message.seq)) return;
        state.seen.add(message.seq);
        state.messages.push(message);
        state.lastSeq = Math.max(state.lastSeq, message.seq);
      });
      state.messages.sort((a, b) => a.seq - b.seq);
    }

    async function loadState() {
      const data = await request('/api/state');
      state.slots = data.slots;
      mergeMessages(data.messages);
      hideLogin();
      render();
      connectEvents();
    }

    function connectEvents() {
      if (state.events) state.events.close();
      const source = new EventSource(`/api/events?after=${state.lastSeq}`);
      state.events = source;
      source.onopen = () => { $('connection').textContent = 'online'; };
      source.onerror = () => { $('connection').textContent = 'reconnecting'; };
      source.addEventListener('update', (event) => {
        const data = JSON.parse(event.data);
        if (data.slots) state.slots = data.slots;
        if (data.messages) mergeMessages(data.messages);
        render();
      });
    }

    async function sendMessage(text) {
      const slot = state.activeSlot;
      if (!slot || !text.trim()) return;
      await request(`/api/slots/${encodeURIComponent(slot)}/messages`, {
        method: 'POST',
        body: JSON.stringify({message: text})
      });
    }

    $('composer').addEventListener('submit', async (event) => {
      event.preventDefault();
      const input = $('input');
      const text = input.value;
      input.value = '';
      input.focus();
      try { await sendMessage(text); } catch (error) { input.value = text; }
    });

    document.querySelectorAll('[data-cmd]').forEach(button => {
      button.addEventListener('click', () => sendMessage(button.dataset.cmd));
    });

    $('loginForm').addEventListener('submit', async (event) => {
      event.preventDefault();
      $('loginError').textContent = '';
      try {
        await request('/api/login', {method: 'POST', body: JSON.stringify({password: $('password').value})});
        $('password').value = '';
        await loadState();
      } catch (error) {
        $('loginError').textContent = 'Sign in failed.';
      }
    });

    loadState().catch(() => showLogin());
  </script>
</body>
</html>
"""


class Handler(BaseHTTPRequestHandler):
    runtime: MobailmuxWeb
    auth: Auth

    def log_message(self, fmt: str, *args) -> None:
        print(f"{self.address_string()} - {fmt % args}", flush=True)

    def authenticated(self) -> bool:
        return self.auth.verify_cookie(self.headers.get("Cookie"))

    def read_json(self) -> dict:
        size = int(self.headers.get("Content-Length") or "0")
        if size > 200_000:
            raise ValueError("request body too large")
        raw = self.rfile.read(size) if size else b"{}"
        return json.loads(raw.decode() or "{}")

    def send_json(self, status: int, payload: dict, headers: dict[str, str] | None = None) -> None:
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        if headers:
            for key, value in headers.items():
                self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def send_text(self, status: int, text: str, content_type: str = "text/plain; charset=utf-8") -> None:
        body = text.encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def require_auth(self) -> bool:
        if self.authenticated():
            return True
        self.send_json(401, {"error": "unauthorized"})
        return False

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/":
            self.send_text(200, INDEX_HTML, "text/html; charset=utf-8")
            return
        if parsed.path == "/api/state":
            if not self.require_auth():
                return
            self.send_json(200, self.runtime.public_state())
            return
        if parsed.path == "/api/events":
            if not self.require_auth():
                return
            self.stream_events(parsed.query)
            return
        if parsed.path == "/api/me":
            self.send_json(200, {"authenticated": self.authenticated(), "auth_enabled": self.auth.enabled})
            return
        if parsed.path == "/favicon.ico":
            self.send_response(204)
            self.end_headers()
            return
        self.send_json(404, {"error": "not found"})

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/api/login":
            try:
                body = self.read_json()
            except Exception:
                self.send_json(400, {"error": "invalid json"})
                return
            if not self.auth.enabled or self.auth.verify_password(str(body.get("password") or "")):
                self.send_json(200, {"ok": True}, {"Set-Cookie": self.auth.make_cookie()})
            else:
                self.send_json(401, {"error": "invalid password"})
            return
        if parsed.path == "/api/logout":
            self.send_json(200, {"ok": True}, {"Set-Cookie": self.auth.clear_cookie()})
            return
        if not self.require_auth():
            return
        match = re.fullmatch(r"/api/slots/([^/]+)/messages", parsed.path)
        if match:
            slot = match.group(1)
            try:
                body = self.read_json()
                self.runtime.submit_message(slot, str(body.get("message") or ""))
            except KeyError:
                self.send_json(404, {"error": "unknown slot"})
                return
            except Exception as exc:
                self.send_json(400, {"error": str(exc)})
                return
            self.send_json(200, {"ok": True})
            return
        self.send_json(404, {"error": "not found"})

    def stream_events(self, query: str) -> None:
        params = parse_qs(query)
        after = int((params.get("after") or ["0"])[0] or "0")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()
        try:
            while not self.runtime.stop_event.is_set():
                messages = self.runtime.messages_since(after)
                if messages:
                    after = max(int(item["seq"]) for item in messages)
                payload = {"messages": messages, "slots": [self.runtime.slot_status(slot) for slot in self.runtime.slots]}
                data = json.dumps(payload)
                self.wfile.write(f"event: update\ndata: {data}\n\n".encode())
                self.wfile.flush()
                with self.runtime.event_cond:
                    self.runtime.event_cond.wait(timeout=15)
        except (BrokenPipeError, ConnectionResetError):
            return


class Server(ThreadingHTTPServer):
    daemon_threads = True


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run the built-in Mobailmux web UI")
    parser.add_argument("--host", default=cfg("MOBAILMUX_WEB_HOST", cfg("MOBAILMUX_WEB_BIND", "127.0.0.1")))
    parser.add_argument("--port", type=int, default=int(cfg("MOBAILMUX_WEB_PORT", "8765") or "8765"))
    parser.add_argument("--no-auth", action="store_true", help="disable password auth; intended only for trusted loopback testing")
    return parser


def main() -> None:
    args = build_arg_parser().parse_args()
    runtime = MobailmuxWeb()
    password = cfg("MOBAILMUX_WEB_PASSWORD")
    auth_enabled = not args.no_auth and (cfg("MOBAILMUX_WEB_AUTH", "password") or "password").lower() != "none"
    if auth_enabled and not password:
        raise SystemExit("Set MOBAILMUX_WEB_PASSWORD in the environment or .env before running the web UI.")
    Handler.runtime = runtime
    Handler.auth = Auth(runtime, password=password, enabled=auth_enabled)
    server = Server((args.host, args.port), Handler)

    def on_signal(_signum, _frame):
        runtime.stop_event.set()
        for slot in list(runtime.workers):
            runtime.kill_worker(slot)
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT, on_signal)
    print(f"mobailmux web running at http://{args.host}:{args.port}", flush=True)
    try:
        server.serve_forever()
    finally:
        runtime.stop_event.set()
        server.server_close()


if __name__ == "__main__":
    main()

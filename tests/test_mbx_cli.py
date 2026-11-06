from __future__ import annotations

import json
import os
import shutil
import signal
import stat
import subprocess
import tempfile
import time
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
MBX = REPO_ROOT / "commands" / "bin" / "mbx"
INSTALL = REPO_ROOT / "commands" / "install.sh"


FAKE_TMUX = r"""#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "$MBX_TMUX_LOG"

argument_after() {
  local wanted="$1"
  shift
  while (( $# > 1 )); do
    if [[ "$1" == "$wanted" ]]; then
      printf '%s\n' "$2"
      return 0
    fi
    shift
  done
  return 1
}

session_dir() {
  local target="${1%%:*}"
  printf '%s/%s\n' "$MBX_TMUX_STATE" "${target#=}"
}

read_value() {
  local directory="$1" field="$2"
  [[ -f "$directory/$field" ]] && head -n 1 "$directory/$field"
}

case "${1:-}" in
  has-session)
    directory="$(session_dir "${3:-}")"
    [[ -d "$directory" ]]
    ;;
  new-session)
    session="$(argument_after -s "$@")"
    workdir="$(argument_after -c "$@")"
    directory="$(session_dir "$session")"
    mkdir -p "$directory"
    printf '0\n' > "$directory/attached"
    printf 'bash\n' > "$directory/command"
    printf '%s\n' "$workdir" > "$directory/path"
    printf '0\n' > "$directory/dead"
    ;;
  display-message)
    target="$(argument_after -t "$@")"
    directory="$(session_dir "$target")"
    format="${*: -1}"
    case "$format" in
      '#{session_attached}') read_value "$directory" attached ;;
      '#{pane_current_command}') read_value "$directory" command ;;
      '#{pane_current_path}') read_value "$directory" path ;;
      '#{pane_dead}') read_value "$directory" dead ;;
      '#{window_activity}') read_value "$directory" activity ;;
    esac
    ;;
  kill-session)
    target="$(argument_after -t "$@")"
    directory="$(session_dir "$target")"
    rm -rf -- "$directory"
    ;;
  set-option|set-hook|attach-session|switch-client|show-options|run-shell)
    ;;
  *)
    printf 'unexpected tmux command: %s\n' "$*" >&2
    exit 2
    ;;
esac
"""


class MbxCliTest(unittest.TestCase):
    def run_mbx(
        self,
        *args: str,
        sessions: dict[str, dict[str, str]] | None = None,
        env_overrides: dict[str, str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            state = root / "state"
            state.mkdir()
            start_dir = root / "terminal-home"
            start_dir.mkdir()
            log = root / "tmux.log"
            log.touch()

            for name, values in (sessions or {}).items():
                directory = state / name
                directory.mkdir()
                defaults = {
                    "attached": "0",
                    "command": "bash",
                    "path": str(start_dir),
                    "dead": "0",
                    "activity": str(int(time.time())),
                }
                defaults.update(values)
                for field, value in defaults.items():
                    (directory / field).write_text(f"{value}\n", encoding="utf-8")

            fake_tmux = root / "tmux"
            fake_tmux.write_text(FAKE_TMUX, encoding="utf-8")
            fake_tmux.chmod(fake_tmux.stat().st_mode | stat.S_IXUSR)

            env = os.environ.copy()
            env.pop("TMUX", None)
            env.update(
                {
                    "PATH": f"{root}{os.pathsep}{env['PATH']}",
                    "MBX_TMUX_STATE": str(state),
                    "MBX_TMUX_LOG": str(log),
                    # `mbx stop` edits the snapshot; never the real one.
                    "XDG_STATE_HOME": str(root / "xdg-state"),
                }
            )
            if env_overrides:
                env.update(env_overrides)

            result = subprocess.run(
                [str(MBX), *args],
                check=False,
                capture_output=True,
                text=True,
                env=env,
                cwd=REPO_ROOT,
            )
            return result, log.read_text(encoding="utf-8")

    def test_status_reports_only_tmux_level_state(self) -> None:
        result, _ = self.run_mbx(
            "status",
            sessions={
                "mbx-a": {"command": "bash", "attached": "1", "path": "/terminal-home"},
                "mbx-b": {"command": "pi", "path": "/projects/site"},
                "mbx-c": {"command": "bash", "dead": "1"},
            },
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout, r"(?m)^a\s+IDLE\s+yes\s+bash\s+/terminal-home$")
        self.assertRegex(result.stdout, r"(?m)^b\s+ACTIVE\s+no\s+pi\s+/projects/site$")
        self.assertRegex(result.stdout, r"(?m)^c\s+EXITED\s+no\s+bash\s+")
        self.assertRegex(result.stdout, r"(?m)^j\s+EMPTY\s+-\s+-\s+-$")

    def test_status_can_target_one_slot(self) -> None:
        result, _ = self.run_mbx("status", "b")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout, r"(?m)^b\s+EMPTY")
        self.assertNotRegex(result.stdout, r"(?m)^a\s+")

    def test_check_distinguishes_working_quiet_done_and_empty(self) -> None:
        now = int(time.time())
        result, _ = self.run_mbx(
            "check",
            sessions={
                "mbx-a": {"command": "bash", "activity": str(now - 5)},
                "mbx-b": {"command": "pi", "activity": str(now - 5)},
                "mbx-c": {"command": "codex", "activity": str(now - 120)},
                "mbx-d": {"command": "codex", "dead": "1", "activity": str(now - 5)},
            },
            env_overrides={"MBX_QUIET_SECONDS": "60"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout, r"(?m)^a\s+DONE\s+-\s+bash\s+")
        self.assertRegex(result.stdout, r"(?m)^b\s+WORKING\s+[0-9]+s\s+pi\s+")
        self.assertRegex(result.stdout, r"(?m)^c\s+QUIET\s+1[12][0-9]s\s+codex\s+")
        self.assertRegex(result.stdout, r"(?m)^d\s+EXITED\s+-\s+codex\s+")
        self.assertRegex(result.stdout, r"(?m)^j\s+EMPTY\s+-\s+-\s+-$")

    def test_check_rejects_invalid_quiet_threshold(self) -> None:
        result, _ = self.run_mbx(
            "check", "a", env_overrides={"MBX_QUIET_SECONDS": "soon"}
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a positive integer", result.stderr)

    def test_resume_creates_an_untouched_home_shell(self) -> None:
        result, log = self.run_mbx("r", "a")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Created terminal a.", result.stdout)
        self.assertIn(
            f"new-session -d -s mbx-a -n a -c {os.environ['HOME']}", log
        )
        self.assertIn("set-option -t mbx-a mouse on", log)
        self.assertIn("attach-session -t mbx-a", log)
        self.assertNotIn("send-keys", log)
        self.assertNotIn("clear", log)

    def test_resume_existing_terminal_only_attaches(self) -> None:
        result, log = self.run_mbx("r", "a", sessions={"mbx-a": {"command": "pi"}})

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Returning to terminal a.", result.stdout)
        self.assertNotIn("new-session", log)
        self.assertIn("set-option -t mbx-a mouse on", log)
        self.assertIn("attach-session -t mbx-a", log)

    def test_copies_reach_the_system_clipboard(self) -> None:
        result, log = self.run_mbx("r", "a", sessions={"mbx-a": {}})

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("set-option -s set-clipboard on", log)
        if shutil.which("pbcopy"):
            self.assertIn("set-option -s copy-command pbcopy", log)
            self.assertIn("set-hook -g pane-set-clipboard", log)

    def test_resume_inside_tmux_switches_client(self) -> None:
        result, log = self.run_mbx(
            "r",
            "a",
            sessions={"mbx-a": {}},
            env_overrides={"TMUX": "/tmp/tmux,1,0"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("switch-client -t mbx-a", log)
        self.assertNotIn("attach-session", log)

    def test_resume_rejects_directory_or_harness_arguments(self) -> None:
        result, _ = self.run_mbx("r", "a", "/some/project")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly one slot", result.stderr)

    def test_stop_all_does_not_touch_unrelated_tmux_sessions(self) -> None:
        result, log = self.run_mbx(
            "stop", "all", sessions={"mbx-a": {}, "mbx-j": {}, "other": {}}
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("kill-session -t mbx-a", log)
        self.assertIn("kill-session -t mbx-j", log)
        self.assertNotIn("kill-session -t other", log)

    def test_invalid_slot_is_rejected(self) -> None:
        result, _ = self.run_mbx("r", "k")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected a through j", result.stderr)

    def test_help_describes_plain_terminals(self) -> None:
        result = subprocess.run(
            [str(MBX), "help"], check=False, capture_output=True, text=True
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ten plain tmux terminals", result.stdout)
        self.assertIn("never runs, clears, or restarts", result.stdout)
        self.assertIn("Mouse-wheel scrolling is enabled", result.stdout)
        self.assertIn("mbx check", result.stdout)
        self.assertIn("mbx ui", result.stdout)
        self.assertNotIn("Pi", result.stdout)
        self.assertNotIn("harness", result.stdout)

    def test_bare_command_shows_help(self) -> None:
        result = subprocess.run(
            [str(MBX)], check=False, capture_output=True, text=True
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("mbx r <a-j>", result.stdout)

    def test_ui_rejects_arguments_and_bad_width(self) -> None:
        result, _ = self.run_mbx("ui", "a")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ui does not accept arguments", result.stderr)

        result, log = self.run_mbx("ui", env_overrides={"MBX_UI_WIDTH": "3"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MBX_UI_WIDTH", result.stderr)
        self.assertEqual(log, "")

        result, log = self.run_mbx("ui", env_overrides={"MBX_UI_LAYOUT": "left"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MBX_UI_LAYOUT must be auto, side or top", result.stderr)
        self.assertEqual(log, "")

        # Refused up front, not by a sidebar that keeps failing to draw.
        result, log = self.run_mbx("ui", env_overrides={"MBX_QUIET_SECONDS": "0"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MBX_QUIET_SECONDS", result.stderr)
        self.assertEqual(log, "")

    def test_stop_forgets_slots_in_the_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as state:
            snapshot = Path(state) / "mobailmux" / "slots.tsv"
            snapshot.parent.mkdir()
            snapshot.write_text("a\t/tmp\t\nb\t/tmp\tclaude --continue\n")
            env = {"XDG_STATE_HOME": state}

            result, _ = self.run_mbx(
                "stop", "a", sessions={"mbx-a": {}}, env_overrides=env
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(snapshot.read_text(), "b\t/tmp\tclaude --continue\n")

            result, _ = self.run_mbx("stop", "all", env_overrides=env)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(snapshot.exists())

    def test_copy_install_contains_only_the_command(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            result = subprocess.run(
                [str(INSTALL), "--prefix", temp_dir, "--copy"],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((Path(temp_dir) / "bin" / "mbx").is_file())
            self.assertFalse((Path(temp_dir) / "libexec").exists())


@unittest.skipUnless(shutil.which("tmux"), "tmux is not installed")
class MbxUiTmuxTest(unittest.TestCase):
    """Drives `mbx ui` inside a real, isolated tmux server."""

    def setUp(self) -> None:
        # Short paths: tmux socket paths are limited to about 100 bytes.
        self.outer = tempfile.mkdtemp(prefix="mbxo", dir="/tmp")
        self.inner = tempfile.mkdtemp(prefix="mbxi", dir="/tmp")
        self.addCleanup(self.cleanup)

    def cleanup(self) -> None:
        for directory in (self.outer, self.inner):
            self.tmux(directory, "kill-server", check=False)
            shutil.rmtree(directory, ignore_errors=True)

    def tmux(
        self, directory: str, *args: str, check: bool = True
    ) -> subprocess.CompletedProcess[str]:
        env = {k: v for k, v in os.environ.items() if k != "TMUX"}
        env["TMUX_TMPDIR"] = directory
        # The UI's saver must never write the real ~/.local/state snapshot.
        env["XDG_STATE_HOME"] = self.outer
        return subprocess.run(
            ["tmux", *args], check=check, capture_output=True, text=True, env=env
        )

    def wait_for(self, predicate, timeout: float = 5.0) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if predicate():
                return
            time.sleep(0.1)
        self.fail("timed out waiting for the UI")

    def screen(self) -> str:
        return self.tmux(self.outer, "capture-pane", "-p", "-t", "harness").stdout

    def viewed_sessions(self) -> list[str]:
        clients = self.tmux(
            self.inner, "list-clients", "-F", "#{client_session}", check=False
        ).stdout.split()
        return [name for name in clients if not name.startswith("mbx-ui-")]

    def tap(self, row: int, col: int = 3) -> None:
        self.tmux(
            self.outer, "send-keys", "-t", "harness", "-l",
            f"\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m",
        )

    def ui_option(self, name: str) -> str:
        sessions = self.tmux(self.inner, "ls", "-F", "#S", check=False).stdout.split()
        ui = [s for s in sessions if s.startswith("mbx-ui-")]
        if not ui:
            return ""
        return self.tmux(
            self.inner, "show-options", "-qv", "-t", f"={ui[0]}:", name, check=False
        ).stdout.strip()

    def test_sidebar_lists_slots_and_taps_switch_the_view(self) -> None:
        home = os.environ["HOME"]
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-a", "-c", home)
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-c", "sleep 600")
        command = f"env -u TMUX TMUX_TMPDIR={self.inner} {MBX} ui; sleep 600"
        self.tmux(
            self.outer, "new-session", "-d", "-s", "harness",
            "-x", "100", "-y", "20", command,
        )

        # Not "+ new" alone: the view's "Tap + new." can show before the
        # sidebar draws. The sidebar draws all its rows in one go.
        self.wait_for(lambda: " c " in self.screen())
        self.assertIn(" a ", self.screen())
        self.assertIn("+ new", self.screen())
        self.wait_for(lambda: self.viewed_sessions() == ["mbx-a"])

        self.tap(5)  # rows 1-2 are the header; slot c is on rows 5-6
        self.wait_for(lambda: self.viewed_sessions() == ["mbx-c"])

        self.tap(8)  # "+ new" creates the first empty slot, b
        self.wait_for(lambda: self.viewed_sessions() == ["mbx-b"])

        self.tmux(self.outer, "send-keys", "-t", "harness", "C-b", "d")
        # Detaching closes only the UI; every slot keeps running.
        self.wait_for(
            lambda: sorted(self.tmux(self.inner, "ls", "-F", "#S").stdout.split())
            == ["mbx-a", "mbx-b", "mbx-c"]
        )

    def test_sidebar_comes_back_after_it_exits(self) -> None:
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-a", "sleep 600")
        command = f"env -u TMUX TMUX_TMPDIR={self.inner} {MBX} ui; sleep 600"
        self.tmux(
            self.outer, "new-session", "-d", "-s", "harness",
            "-x", "100", "-y", "20", command,
        )
        # The UI client attaches last, once every option and hook is set.
        self.wait_for(
            lambda: any(
                name.startswith("mbx-ui-")
                for name in self.tmux(
                    self.inner, "list-clients", "-F", "#{client_session}"
                ).stdout.split()
            )
        )

        def sidebar_pid() -> str:
            panes = self.tmux(
                self.inner, "list-panes", "-a", "-F",
                "#{@mbx_role} #{pane_dead} #{@mbx_pid}",
            ).stdout.splitlines()
            alive = [p.split()[2] for p in panes if p.startswith("sidebar 0 ")]
            return alive[0] if alive else ""

        self.wait_for(lambda: sidebar_pid() != "")
        first = sidebar_pid()
        os.kill(int(first), signal.SIGKILL)
        self.wait_for(lambda: sidebar_pid() not in ("", first))
        self.wait_for(lambda: "+ new" in self.screen())

    def test_narrow_window_shows_tabs_on_top(self) -> None:
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-a", "-c", self.inner)
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-c", "sleep 600")
        command = f"env -u TMUX TMUX_TMPDIR={self.inner} {MBX} ui; sleep 600"
        # A phone in portrait: narrower than 80 columns.
        self.tmux(
            self.outer, "new-session", "-d", "-s", "harness",
            "-x", "50", "-y", "20", command,
        )

        self.wait_for(lambda: " c" in self.screen().splitlines()[0])
        first, border = self.screen().splitlines()[:2]
        self.assertTrue(first.startswith(" a○ "), first)
        self.assertTrue(first.rstrip().endswith("+"), first)
        self.assertNotIn("MOBAILMUX", self.screen())
        self.assertEqual(set(border), {"─"})
        self.wait_for(lambda: self.viewed_sessions() == ["mbx-a"])

        # Taps pick a tab by column: c's tab, then "+" for the first empty slot.
        cols = self.ui_option("@mbx_cols")
        self.tap(1, cols.index("c") + 1)
        self.wait_for(lambda: self.viewed_sessions() == ["mbx-c"])
        self.tap(1, cols.index("+") + 1)
        self.wait_for(lambda: self.viewed_sessions() == ["mbx-b"])

        # Turned sideways, it becomes the sidebar; and back again.
        self.tmux(self.outer, "resize-window", "-t", "harness", "-x", "120")
        self.wait_for(lambda: "MOBAILMUX" in self.screen())
        self.tmux(self.outer, "resize-window", "-t", "harness", "-x", "50")
        self.wait_for(lambda: self.screen().startswith(" a"))
        self.assertNotIn("MOBAILMUX", self.screen())

    def test_layout_can_be_fixed(self) -> None:
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-a", "sleep 600")
        command = (
            f"env -u TMUX TMUX_TMPDIR={self.inner} MBX_UI_LAYOUT=side {MBX} ui;"
            " sleep 600"
        )
        self.tmux(
            self.outer, "new-session", "-d", "-s", "harness",
            "-x", "50", "-y", "20", command,
        )
        self.wait_for(lambda: "+ new" in self.screen())
        self.assertEqual(self.ui_option("@mbx_layout"), "side")



@unittest.skipUnless(shutil.which("tmux"), "tmux is not installed")
class MbxSnapshotTmuxTest(unittest.TestCase):
    """Saves and restores slots against a real, isolated tmux server."""

    def setUp(self) -> None:
        self.server = tempfile.mkdtemp(prefix="mbxs", dir="/tmp")
        self.home = Path(tempfile.mkdtemp(prefix="mbxh"))
        self.state = self.home / "state"
        self.addCleanup(self.cleanup)

    def cleanup(self) -> None:
        self.run_cmd(["tmux", "kill-server"], check=False)
        shutil.rmtree(self.server, ignore_errors=True)
        shutil.rmtree(self.home, ignore_errors=True)

    def run_cmd(
        self, argv: list[str], check: bool = True
    ) -> subprocess.CompletedProcess[str]:
        env = {k: v for k, v in os.environ.items() if k != "TMUX"}
        env.update(
            TMUX_TMPDIR=self.server, HOME=str(self.home),
            XDG_STATE_HOME=str(self.state), MBX_SAVE_SECONDS="1",
        )
        return subprocess.run(
            argv, check=check, capture_output=True, text=True, env=env
        )

    def wait_for(self, predicate, timeout: float = 5.0) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if predicate():
                return
            time.sleep(0.1)
        self.fail("timed out")

    def test_save_records_claude_session_and_restore_types_it(self) -> None:
        project = (self.home / "project").resolve()
        project.mkdir()
        sessions = self.home / ".claude" / "sessions"
        sessions.mkdir(parents=True)
        session_id = "11111111-2222-3333-4444-555555555555"
        # A stand-in for Claude Code: registers its pid like the real CLI does.
        fake_claude = self.home / "fake-claude"
        fake_claude.write_text(
            f"cd {project}\n"
            f"printf '{{\"pid\":%s,\"sessionId\":\"{session_id}\",\"cwd\":\"{project}\"}}'"
            f" $$ > {sessions}/$$.json\n"
            "exec -a claude tail -f /dev/null\n"
        )
        self.run_cmd(["tmux", "new-session", "-d", "-s", "mbx-a", "-c", str(self.home)])
        self.run_cmd(["tmux", "new-session", "-d", "-s", "mbx-c", "-c", str(project)])
        self.run_cmd(["tmux", "send-keys", "-t", "mbx-a", f"bash {fake_claude}", "Enter"])
        self.wait_for(lambda: any(sessions.glob("*.json")))
        time.sleep(0.3)

        result = self.run_cmd([str(MBX), "save"])
        self.assertEqual(result.returncode, 0, result.stderr)
        snapshot = (self.state / "mobailmux" / "slots.tsv").read_text()
        self.assertEqual(
            snapshot,
            f"a\t{project}\tclaude --resume {session_id}\n"
            f"c\t{project}\t\n",
        )

        # Simulate a reboot, with a harmless command in place of Claude.
        self.run_cmd(["tmux", "kill-server"])
        (self.state / "mobailmux" / "slots.tsv").write_text(
            f"a\t{project}\techo restored-a\nc\t/missing/dir\t\n"
        )
        result = self.run_cmd([str(MBX), "restore"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Restored terminal a", result.stdout)

        def pane(target: str, fmt: str) -> str:
            return self.run_cmd(
                ["tmux", "display-message", "-p", "-t", target, fmt]
            ).stdout.strip()

        self.assertEqual(Path(pane("mbx-a", "#{pane_current_path}")).resolve(), project)
        self.assertEqual(Path(pane("mbx-c", "#{pane_current_path}")).resolve(), self.home.resolve())
        self.wait_for(
            lambda: "restored-a\n" in self.run_cmd(
                ["tmux", "capture-pane", "-p", "-t", "mbx-a"]
            ).stdout
        )

        # The background saver follows the slot to a new folder.
        elsewhere = (self.home / "elsewhere").resolve()
        elsewhere.mkdir()
        self.run_cmd(["tmux", "send-keys", "-t", "mbx-a", f"cd {elsewhere}", "Enter"])
        snapshot_file = self.state / "mobailmux" / "slots.tsv"
        self.wait_for(
            lambda: snapshot_file.read_text().startswith(f"a\t{elsewhere}\t"),
            timeout=8,
        )

        # Restoring again leaves running slots alone.
        result = self.run_cmd([str(MBX), "restore"])
        self.assertIn("already running", result.stdout)


@unittest.skipUnless(
    shutil.which("tmux") and shutil.which("jq"), "tmux or jq is not installed"
)
class MbxAgentsTmuxTest(unittest.TestCase):
    """mbx who, log and peek against a real, isolated tmux server."""

    claude_id = "11111111-2222-3333-4444-555555555555"
    codex_id = "01a0aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee"

    def setUp(self) -> None:
        self.server = tempfile.mkdtemp(prefix="mbxw", dir="/tmp")
        self.home = Path(tempfile.mkdtemp(prefix="mbxh")).resolve()
        self.addCleanup(self.cleanup)
        self.project = self.home / "project"
        self.project.mkdir()

    def cleanup(self) -> None:
        self.run_cmd(["tmux", "kill-server"], check=False)
        shutil.rmtree(self.server, ignore_errors=True)
        shutil.rmtree(self.home, ignore_errors=True)

    def run_cmd(
        self, argv: list[str], check: bool = True
    ) -> subprocess.CompletedProcess[str]:
        drop = {"TMUX", "TMUX_PANE", "CLAUDE_CONFIG_DIR", "CODEX_HOME"}
        env = {k: v for k, v in os.environ.items() if k not in drop}
        env.update(
            TMUX_TMPDIR=self.server, HOME=str(self.home),
            XDG_STATE_HOME=str(self.home / "state"), TZ="UTC",
        )
        return subprocess.run(
            argv, check=check, capture_output=True, text=True, env=env
        )

    def mbx(self, *args: str) -> subprocess.CompletedProcess[str]:
        return self.run_cmd([str(MBX), *args], check=False)

    def wait_for(self, predicate, timeout: float = 5.0) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if predicate():
                return
            time.sleep(0.1)
        self.fail("timed out")

    def write_jsonl(self, path: Path, rows: list[dict]) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        # Compact, like the real CLIs write their transcripts.
        path.write_text(
            "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows)
        )

    def start_agents(self) -> None:
        sessions = self.home / ".claude" / "sessions"
        sessions.mkdir(parents=True)
        t = "2026-11-03T09:00:00.000Z"
        self.write_jsonl(
            self.home / ".claude" / "projects" / "-project" / f"{self.claude_id}.jsonl",
            [
                {"type": "user", "timestamp": t, "origin": {"kind": "human"},
                 "message": {"role": "user", "content": "fix the login page"}},
                {"type": "user", "timestamp": t, "isMeta": True,
                 "message": {"role": "user", "content": [
                     {"type": "text", "text": "injected skill text"}]}},
                {"type": "user", "timestamp": t, "isMeta": True,
                 "origin": {"kind": "peer", "name": "other-session",
                            "body": "please check slot b"},
                 "message": {"role": "user", "content": "wrapped peer text"}},
                {"type": "assistant", "timestamp": t, "message": {
                    "role": "assistant", "content": [
                        {"type": "thinking", "thinking": "hidden"},
                        {"type": "text", "text": "Looking at it.\nSecond line."}]}},
                {"type": "assistant", "timestamp": t, "message": {
                    "role": "assistant", "content": [
                        {"type": "tool_use", "name": "Bash",
                         "input": {"command": "git status\ngit diff"}}]}},
                {"type": "user", "timestamp": t, "message": {
                    "role": "user", "content": [
                        {"type": "tool_result", "content": "clean"}]}},
                {"type": "ai-title", "aiTitle": "Login page fix"},
                {"type": "last-prompt", "lastPrompt": "fix the login page"},
            ],
        )
        # Stand-ins for Claude Code and Codex: they register like the real CLIs.
        fake_claude = self.home / "fake-claude"
        fake_claude.write_text(
            f"cd {self.project}\n"
            f"printf '{{\"pid\":%s,\"sessionId\":\"{self.claude_id}\","
            f"\"cwd\":\"{self.project}\",\"name\":\"peer-a\",\"status\":\"idle\"}}'"
            f" $$ > {sessions}/$$.json\n"
            "exec -a claude tail -f /dev/null\n"
        )
        rollout = (
            self.home / ".codex" / "sessions" / "2026" / "11" / "03"
            / f"rollout-2026-11-03T09-00-00-{self.codex_id}.jsonl"
        )
        self.write_jsonl(
            rollout,
            [
                {"type": "session_meta", "payload": {"cwd": str(self.project)}},
                {"type": "event_msg", "timestamp": t, "payload": {
                    "type": "item_completed", "item": {
                        "type": "UserMessage",
                        "content": [{"type": "text", "text": "update the docs"}]}}},
                {"type": "event_msg", "timestamp": t, "payload": {
                    "type": "item_completed", "item": {
                        "type": "CommandExecution",
                        "command": ["/bin/bash", "-lc", "ls docs"]}}},
                {"type": "event_msg", "timestamp": t, "payload": {
                    "type": "item_completed", "item": {
                        "type": "AgentMessage",
                        "content": [{"type": "Text", "text": "Docs updated."}]}}},
            ],
        )
        self.write_jsonl(
            self.home / ".codex" / "session_index.jsonl",
            [{"id": self.codex_id, "thread_name": "Docs refresh"}],
        )
        self.run_cmd(["tmux", "new-session", "-d", "-s", "mbx-a", "-c", str(self.home)])
        self.run_cmd(["tmux", "new-session", "-d", "-s", "mbx-b", "-c", str(self.home)])
        self.run_cmd(["tmux", "new-session", "-d", "-s", "mbx-c", "-c", str(self.home)])
        self.run_cmd(["tmux", "send-keys", "-t", "mbx-a", f"bash {fake_claude}", "Enter"])
        # A child of the slot's shell, as a real Codex is.
        self.run_cmd([
            "tmux", "send-keys", "-t", "mbx-b",
            f"bash -c \"exec -a codex bash -c 'sleep 600; :' x resume {self.codex_id}\"",
            "Enter",
        ])
        self.run_cmd(["tmux", "send-keys", "-t", "mbx-c", "echo hello-from-c", "Enter"])
        self.wait_for(lambda: any(sessions.glob("*.json")))
        self.wait_for(lambda: f"resume {self.codex_id}" in self.run_cmd(
            ["ps", "-A", "-o", "command="]
        ).stdout)
        time.sleep(0.3)

    def test_who_names_each_agent_and_what_it_works_on(self) -> None:
        self.start_agents()
        result = self.mbx("who", "--format", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        slots = {row["slot"]: row for row in json.loads(result.stdout)}
        self.assertEqual(sorted(slots), ["a", "b", "c"])

        a = slots["a"]
        self.assertEqual(a["agent"], "claude")
        self.assertEqual(a["name"], "peer-a")
        self.assertEqual(a["state"], "idle")
        self.assertEqual(a["session_id"], self.claude_id)
        self.assertEqual(a["title"], "Login page fix")
        self.assertEqual(a["last_prompt"], "fix the login page")
        self.assertEqual(a["folder"], str(self.project))
        self.assertFalse(a["you"])

        b = slots["b"]
        self.assertEqual(b["agent"], "codex")
        self.assertIsNone(b["name"])
        self.assertEqual(b["session_id"], self.codex_id)
        self.assertEqual(b["title"], "Docs refresh")
        self.assertEqual(b["last_prompt"], "update the docs")
        self.assertEqual(b["folder"], str(self.project))

        c = slots["c"]
        self.assertIsNone(c["agent"])
        self.assertEqual(c["state"], "done")

        text = self.mbx("who")
        self.assertEqual(text.returncode, 0, text.stderr)
        self.assertRegex(text.stdout, r"(?m)^SLOT\s+STATE\s+PROGRAM\s+NAME\s+FOLDER\s+DOING$")
        self.assertRegex(text.stdout, r"(?m)^a\s+idle\s+claude\s+peer-a\s+~/project\s+Login page fix$")

        empty = self.mbx("who", "h", "--format=json")
        self.assertEqual(json.loads(empty.stdout), [{"slot": "h", "you": False, "state": "empty"}])

    def test_log_reads_the_agents_own_transcript(self) -> None:
        self.start_agents()
        result = self.mbx("log", "a", "--format", "json")
        self.assertEqual(result.returncode, 0, result.stderr)
        log = json.loads(result.stdout)
        self.assertEqual(log["agent"], "claude")
        self.assertEqual(
            [(e["from"], e["kind"]) for e in log["entries"]],
            [("user", "prompt"), ("other-session", "message"),
             ("claude", "text"), ("claude", "tool")],
        )
        self.assertEqual(log["entries"][1]["text"], "please check slot b")
        self.assertNotIn("injected skill text", result.stdout)
        self.assertNotIn("hidden", result.stdout)

        text = self.mbx("log", "a", "-n", "2")
        self.assertEqual(text.returncode, 0, text.stderr)
        self.assertIn("11-03 09:00  claude: Looking at it.\n    Second line.", text.stdout)
        self.assertIn("claude: → Bash: git status …", text.stdout)
        self.assertNotIn("fix the login page", text.stdout)

        codex_result = self.mbx("log", "b", "--format", "json")
        self.assertEqual(codex_result.returncode, 0, codex_result.stderr)
        codex = json.loads(codex_result.stdout)
        self.assertEqual(
            [(e["from"], e["kind"], e["text"]) for e in codex["entries"]],
            [("user", "prompt", "update the docs"), ("codex", "tool", "ls docs"),
             ("codex", "text", "Docs updated.")],
        )

        shell = self.mbx("log", "c")
        self.assertNotEqual(shell.returncode, 0)
        self.assertIn("mbx peek c", shell.stderr)

    def test_peek_prints_the_screen_as_text(self) -> None:
        self.start_agents()
        self.wait_for(lambda: "hello-from-c\n" in self.mbx("peek", "c").stdout)
        last = self.mbx("peek", "c", "-n", "1")
        self.assertEqual(last.returncode, 0, last.stderr)
        self.assertEqual(len(last.stdout.splitlines()), 1)
        screen = json.loads(self.mbx("peek", "c", "--format", "json").stdout)
        self.assertEqual(screen["slot"], "c")
        self.assertIn("hello-from-c", screen["screen"])

        missing = self.mbx("peek", "h")
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("terminal h is empty", missing.stderr)

    def test_views_refuse_bad_arguments(self) -> None:
        for args, message in [
            (("log",), "log needs a slot"),
            (("peek",), "peek needs a slot"),
            (("who", "-n", "3"), "who does not take -n"),
            (("who", "--format", "yaml"), "--format must be json or text"),
            (("log", "a", "-n", "0"), "-n needs a positive number"),
            (("log", "k"), "expected a through j"),
            (("who", "a", "b"), "who accepts one slot"),
            (("peek", "a", "--follow"), "unknown option: --follow"),
        ]:
            result = self.mbx(*args)
            self.assertNotEqual(result.returncode, 0, args)
            self.assertIn(message, result.stderr, args)


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import os
import shutil
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
  set-option|attach-session|switch-client|show-options|run-shell)
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

    def tap(self, row: int) -> None:
        self.tmux(
            self.outer, "send-keys", "-t", "harness", "-l",
            f"\x1b[<0;3;{row}M\x1b[<0;3;{row}m",
        )

    def test_sidebar_lists_slots_and_taps_switch_the_view(self) -> None:
        home = os.environ["HOME"]
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-a", "-c", home)
        self.tmux(self.inner, "new-session", "-d", "-s", "mbx-c", "sleep 600")
        command = f"env -u TMUX TMUX_TMPDIR={self.inner} {MBX} ui; sleep 600"
        self.tmux(
            self.outer, "new-session", "-d", "-s", "harness",
            "-x", "60", "-y", "20", command,
        )

        self.wait_for(lambda: "+ new" in self.screen())
        self.assertIn(" a ", self.screen())
        self.assertIn(" c ", self.screen())
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


if __name__ == "__main__":
    unittest.main()

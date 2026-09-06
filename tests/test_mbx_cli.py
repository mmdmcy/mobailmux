from __future__ import annotations

import os
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
  set-option|attach-session|switch-client)
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
        self.assertNotIn("Pi", result.stdout)
        self.assertNotIn("harness", result.stdout)

    def test_bare_command_shows_help(self) -> None:
        result = subprocess.run(
            [str(MBX)], check=False, capture_output=True, text=True
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("mbx r <a-j>", result.stdout)

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


if __name__ == "__main__":
    unittest.main()

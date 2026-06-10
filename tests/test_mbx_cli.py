from __future__ import annotations

import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
MBX = REPO_ROOT / "commands" / "bin" / "mbx"


class MbxCliTest(unittest.TestCase):
    def run_mbx_with_fake_tmux(self, *args: str) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            fake_tmux = Path(temp_dir) / "tmux"
            fake_tmux.write_text(
                """#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  list-sessions)
    echo 'codex-1|1'
    echo 'other|0'
    ;;
  has-session)
    case "$*" in
      *codex-1*) exit 0 ;;
      *) exit 1 ;;
    esac
    ;;
  display-message)
    case "$*" in
      *session_attached*) echo '1' ;;
      *pane_current_command*) echo 'node' ;;
      *pane_current_path*) echo '/workspace' ;;
      *) echo '' ;;
    esac
    ;;
  *)
    echo "unexpected tmux command: $*" >&2
    exit 2
    ;;
esac
""",
                encoding="utf-8",
            )
            fake_tmux.chmod(fake_tmux.stat().st_mode | stat.S_IXUSR)

            env = os.environ.copy()
            env["PATH"] = f"{temp_dir}{os.pathsep}{env['PATH']}"
            env["MBX_NO_UPDATE_CHECK"] = "1"
            return subprocess.run(
                [str(MBX), *args],
                check=False,
                env=env,
                text=True,
                capture_output=True,
            )

    def test_list_shows_running_sessions(self) -> None:
        listed = self.run_mbx_with_fake_tmux("list")

        self.assertEqual(listed.returncode, 0, listed.stderr)
        self.assertIn("name", listed.stdout)
        self.assertIn("a", listed.stdout)
        self.assertNotIn("b", listed.stdout)

    def test_status_aliases_show_available_sessions(self) -> None:
        status = self.run_mbx_with_fake_tmux("status")
        slots = self.run_mbx_with_fake_tmux("slots")
        sessions = self.run_mbx_with_fake_tmux("sessions")

        self.assertEqual(status.returncode, 0, status.stderr)
        self.assertEqual(slots.returncode, 0, slots.stderr)
        self.assertEqual(sessions.returncode, 0, sessions.stderr)
        self.assertEqual(slots.stdout, status.stdout)
        self.assertEqual(sessions.stdout, status.stdout)
        self.assertIn("a", status.stdout)
        self.assertIn("attached", status.stdout)
        self.assertIn("b", status.stdout)
        self.assertIn("available", status.stdout)

    def test_status_can_target_one_session(self) -> None:
        result = self.run_mbx_with_fake_tmux("status", "a")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("a", result.stdout)
        self.assertNotIn("b", result.stdout)

    def test_legacy_slot_names_still_work(self) -> None:
        result = self.run_mbx_with_fake_tmux("status", "one")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("a", result.stdout)
        self.assertNotIn("b", result.stdout)

    def test_short_resume_alias_accepts_letter_slot(self) -> None:
        result = self.run_mbx_with_fake_tmux("r", "a", "--dry-run")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("slot:    a", result.stdout)
        self.assertIn("session: codex-1", result.stdout)
        self.assertIn("resume --last", result.stdout)

    def test_commands_mentions_slots_alias(self) -> None:
        result = subprocess.run(
            [str(MBX), "commands"],
            check=False,
            text=True,
            capture_output=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("mbx list", result.stdout)
        self.assertIn("mbx slots", result.stdout)
        self.assertIn("mbx status", result.stdout)

    def test_singular_command_alias(self) -> None:
        result = subprocess.run(
            [str(MBX), "command"],
            check=False,
            text=True,
            capture_output=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("mbx status", result.stdout)

    def test_unknown_command_points_to_help(self) -> None:
        result = subprocess.run(
            [str(MBX), "wat"],
            check=False,
            text=True,
            capture_output=True,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown command: wat", result.stderr)
        self.assertIn("mbx help", result.stderr)

    def test_list_rejects_arguments(self) -> None:
        result = self.run_mbx_with_fake_tmux("list", "a")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not accept arguments", result.stderr)

    def test_start_help_does_not_parse_help_as_slot(self) -> None:
        result = subprocess.run(
            [str(MBX), "start", "--help"],
            check=False,
            text=True,
            capture_output=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Usage:", result.stdout)
        self.assertNotIn("unknown slot", result.stderr)


if __name__ == "__main__":
    unittest.main()

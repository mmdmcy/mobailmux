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
    def run_mbx_with_fake_tmux(
        self,
        *args: str,
        env_overrides: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            fake_tmux = Path(temp_dir) / "tmux"
            fake_tmux.write_text(
                """#!/usr/bin/env bash
set -euo pipefail

log() {
  if [[ -n "${MBX_TMUX_LOG:-}" ]]; then
    printf '%s\\n' "$*" >> "$MBX_TMUX_LOG"
  fi
}

log "$*"

case "${1:-}" in
  list-sessions)
    echo 'codex-1|1'
    echo 'other|0'
    ;;
  ls)
    echo 'codex-1: 1 windows (created Wed Jan 01 00:00:00 2025)'
    ;;
  has-session)
    if [[ "${MBX_FAKE_TMUX_NO_SESSION:-0}" == "1" ]]; then
      exit 1
    fi
    case "${3:-}" in
      codex-1|plugdeck-a) exit 0 ;;
      *) exit 1 ;;
    esac
    ;;
  new-session|set-option|send-keys|attach-session|switch-client|kill-session)
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
            if env_overrides:
                env.update(env_overrides)
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
        self.assertIn("j", status.stdout)
        self.assertIn("agent-10", status.stdout)

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

    def test_resume_attaches_to_the_requested_tmux_slot(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "r",
                "a",
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Attaching to tmux slot a (codex-1)", result.stdout)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("attach-session -t codex-1", log)
            self.assertNotIn("send-keys", log)
            self.assertNotIn("Codex", result.stdout)

    def test_resume_creates_a_generic_shell_slot_when_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "r",
                "j",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={
                    "MBX_FAKE_TMUX_NO_SESSION": "1",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Started tmux slot j (agent-10)", result.stdout)
            self.assertNotIn("Codex", result.stdout)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn(f"new-session -d -s agent-10 -n agent-10 -c {workdir}", log)
            self.assertIn("set-option -g mouse on", log)
            self.assertNotIn("send-keys", log)

    def test_resume_dry_run_describes_the_existing_slot(self) -> None:
        result = self.run_mbx_with_fake_tmux("r", "a", "--dry-run")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("slot:    a", result.stdout)
        self.assertIn("session: codex-1", result.stdout)
        self.assertIn("state:   existing", result.stdout)
        self.assertIn("tmux attach-session -t codex-1", result.stdout)

    def test_resume_dry_run_describes_a_new_slot(self) -> None:
        result = self.run_mbx_with_fake_tmux(
            "r",
            "10",
            "--dry-run",
            env_overrides={"MBX_FAKE_TMUX_NO_SESSION": "1"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("slot:    j", result.stdout)
        self.assertIn("session: agent-10", result.stdout)
        self.assertIn("state:   new shell", result.stdout)
        self.assertIn("tmux new-session -d -s agent-10", result.stdout)

    def test_short_quit_alias_kills_the_requested_slot(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "q",
                "a",
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Stopped a (codex-1)", result.stdout)
            self.assertIn("kill-session -t codex-1", tmux_log.read_text(encoding="utf-8"))

    def test_short_start_alias_launches_pi_in_a_new_slot(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "s",
                "a",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={
                    "MBX_FAKE_TMUX_NO_SESSION": "1",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Started pi in a (agent-1)", result.stdout)
            self.assertIn("send-keys -t agent-1", tmux_log.read_text(encoding="utf-8"))

    def test_tmux_mouse_mode_is_enabled(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "list",
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("set-option -g mouse on", tmux_log.read_text(encoding="utf-8"))

    def test_start_uses_pi_by_default(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={
                    "MBX_FAKE_TMUX_NO_SESSION": "1",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Started pi in a (agent-1)", result.stdout)
            self.assertIn("send-keys -t agent-1", tmux_log.read_text(encoding="utf-8"))

    def test_start_can_select_opencode(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            result = self.run_mbx_with_fake_tmux(
                "start",
                "b",
                "--harness",
                "opencode",
                "--dry-run",
                "-C",
                str(workdir),
                env_overrides={"MBX_FAKE_TMUX_NO_SESSION": "1"},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("session: agent-2", result.stdout)
            self.assertIn(f"opencode --auto {workdir}", result.stdout)

    def test_resume_rejects_harness_conversation_options(self) -> None:
        result = self.run_mbx_with_fake_tmux("r", "a", "--session-id", "anything")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown option: --session-id", result.stderr)

    def test_commands_mentions_generic_slots(self) -> None:
        result = subprocess.run(
            [str(MBX), "commands"],
            check=False,
            text=True,
            capture_output=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("mbx list", result.stdout)
        self.assertIn("mbx slots", result.stdout)
        self.assertIn("mbx s <slot>", result.stdout)
        self.assertIn("mbx q <slot>", result.stdout)
        self.assertIn("Attach to that exact tmux slot", result.stdout)

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

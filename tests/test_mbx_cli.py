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


class MbxCliTest(unittest.TestCase):
    def run_mbx_with_fake_tmux(
        self,
        *args: str,
        env_overrides: dict[str, str] | None = None,
        slot_session_id: str | None = None,
        slot: str = "a",
    ) -> subprocess.CompletedProcess[str]:
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
    if [[ "${MBX_FAKE_TMUX_NO_SESSION:-0}" == "1" ]]; then
      exit 1
    fi
    case "${3:-}" in
      codex-1) exit 0 ;;
      *) exit 1 ;;
    esac
    ;;
  new-session)
    ;;
  set-option)
    if [[ -n "${MBX_TMUX_LOG:-}" ]]; then
      echo "$*" >> "$MBX_TMUX_LOG"
    fi
    ;;
  send-keys)
    if [[ -n "${MBX_FAKE_SESSION_FILE:-}" ]]; then
      printf '%s\n' "${MBX_FAKE_SESSION_METADATA:-}" > "$MBX_FAKE_SESSION_FILE"
    fi
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
            if slot_session_id is not None:
                state_dir = Path(temp_dir) / "slot-state"
                state_dir.mkdir()
                (state_dir / slot).write_text(f"{slot_session_id}\n", encoding="utf-8")
                env["MBX_SLOT_STATE_DIR"] = str(state_dir)
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
        self.assertIn("codex-10", status.stdout)

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
        result = self.run_mbx_with_fake_tmux(
            "r",
            "a",
            "--dry-run",
            slot_session_id="slot-a-session",
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("slot:    a", result.stdout)
        self.assertIn("session: codex-1", result.stdout)
        self.assertIn("resume slot-a-session", result.stdout)
        self.assertNotIn("--last", result.stdout)

    def test_resume_refuses_to_guess_without_a_remembered_conversation(self) -> None:
        result = self.run_mbx_with_fake_tmux("r", "a", "--dry-run")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no remembered Codex conversation", result.stderr)

    def test_resume_can_switch_to_another_saved_conversation(self) -> None:
        result = self.run_mbx_with_fake_tmux(
            "r",
            "a",
            "--session-id",
            "other-session",
            "--dry-run",
            slot_session_id="slot-a-session",
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("resume other-session", result.stdout)
        self.assertNotIn("resume slot-a-session", result.stdout)

    def test_slot_j_uses_codex_ten(self) -> None:
        result = self.run_mbx_with_fake_tmux(
            "r",
            "j",
            "--dry-run",
            slot_session_id="slot-j-session",
            slot="j",
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("slot:    j", result.stdout)
        self.assertIn("session: codex-10", result.stdout)
        self.assertIn("resume slot-j-session", result.stdout)

    def test_tmux_mouse_mode_is_enabled(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "list",
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("set-option -g mouse on", tmux_log.read_text(encoding="utf-8"))

    def test_session_tracker_records_new_codex_id_for_slot(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            codex_home = root / "codex-home"
            sessions = codex_home / "sessions"
            sessions.mkdir(parents=True)
            workdir = root / "workspace"
            workdir.mkdir()
            old_session = sessions / "old.jsonl"
            old_session.write_text(
                '{"type":"session_meta","payload":{"id":"old","cwd":"/elsewhere"}}\n',
                encoding="utf-8",
            )
            new_session = sessions / "new.jsonl"
            new_session.write_text(
                f'{{"type":"session_meta","payload":{{"id":"new-slot-id","cwd":"{workdir}"}}}}\n',
                encoding="utf-8",
            )
            snapshot = root / "snapshot"
            snapshot.write_text(f"{old_session}\n", encoding="utf-8")
            state_dir = root / "slot-state"
            env = os.environ.copy()
            env["CODEX_HOME"] = str(codex_home)
            env["MBX_SLOT_STATE_DIR"] = str(state_dir)
            env["MBX_SESSION_DISCOVERY_ATTEMPTS"] = "1"

            result = subprocess.run(
                [str(MBX), "__track-session", "1", str(workdir), str(snapshot)],
                check=False,
                env=env,
                text=True,
                capture_output=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((state_dir / "a").read_text(encoding="utf-8"), "new-slot-id\n")

    def test_start_records_the_session_created_in_its_slot(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            codex_home = root / "codex-home"
            (codex_home / "sessions").mkdir(parents=True)
            workdir = root / "workspace"
            workdir.mkdir()
            state_dir = root / "slot-state"
            created_session = codex_home / "sessions" / "started.jsonl"
            metadata = f'{{"type":"session_meta","payload":{{"id":"started-slot-id","cwd":"{workdir}"}}}}'
            env = {
                "CODEX_HOME": str(codex_home),
                "MBX_SLOT_STATE_DIR": str(state_dir),
                "MBX_FAKE_TMUX_NO_SESSION": "1",
                "MBX_FAKE_SESSION_FILE": str(created_session),
                "MBX_FAKE_SESSION_METADATA": metadata,
                "MBX_SESSION_DISCOVERY_ATTEMPTS": "8",
            }

            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides=env,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            state_file = state_dir / "a"
            for _ in range(20):
                if state_file.exists():
                    break
                time.sleep(0.05)
            self.assertEqual(state_file.read_text(encoding="utf-8"), "started-slot-id\n")

    @unittest.skipUnless(shutil.which("flock"), "flock is required for session tracking")
    def test_concurrent_starts_in_one_workdir_keep_distinct_session_ids(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            fake_tmux = fake_bin / "tmux"
            fake_tmux.write_text(
                """#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  has-session)
    exit 1
    ;;
  new-session|set-option)
    ;;
  send-keys)
    : > "${MBX_FAKE_SEND_STARTED:?}"
    sleep "${MBX_FAKE_SEND_DELAY:-0.2}"
    printf '%s\n' "${MBX_FAKE_SESSION_METADATA:?}" > "${MBX_FAKE_SESSION_FILE:?}"
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

            codex_home = root / "codex-home"
            sessions = codex_home / "sessions"
            sessions.mkdir(parents=True)
            state_dir = root / "slot-state"
            workdir = root / "workspace"
            workdir.mkdir()
            common_env = os.environ.copy()
            common_env.update(
                {
                    "PATH": f"{fake_bin}{os.pathsep}{common_env['PATH']}",
                    "CODEX_HOME": str(codex_home),
                    "MBX_SLOT_STATE_DIR": str(state_dir),
                    "MBX_NO_UPDATE_CHECK": "1",
                    "MBX_SESSION_DISCOVERY_ATTEMPTS": "20",
                }
            )

            def start_env(slot: str) -> dict[str, str]:
                env = common_env.copy()
                session_file = sessions / f"{slot}.jsonl"
                marker = root / f"{slot}.started"
                env.update(
                    {
                        "MBX_FAKE_SEND_STARTED": str(marker),
                        "MBX_FAKE_SESSION_FILE": str(session_file),
                        "MBX_FAKE_SESSION_METADATA": (
                            f'{{"type":"session_meta","payload":'
                            f'{{"id":"slot-{slot}-id","cwd":"{workdir}"}}}}'
                        ),
                    }
                )
                return env

            first = subprocess.Popen(
                [str(MBX), "start", "a", "--no-attach", "-C", str(workdir)],
                env=start_env("a"),
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            first_marker = root / "a.started"
            for _ in range(100):
                if first_marker.exists():
                    break
                time.sleep(0.01)
            self.assertTrue(first_marker.exists(), "first start never reached tmux")

            second = subprocess.Popen(
                [str(MBX), "start", "b", "--no-attach", "-C", str(workdir)],
                env=start_env("b"),
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            first_stdout, first_stderr = first.communicate(timeout=10)
            second_stdout, second_stderr = second.communicate(timeout=10)
            self.assertEqual(first.returncode, 0, first_stderr or first_stdout)
            self.assertEqual(second.returncode, 0, second_stderr or second_stdout)

            for _ in range(100):
                if (state_dir / "a").exists() and (state_dir / "b").exists():
                    break
                time.sleep(0.05)
            self.assertEqual((state_dir / "a").read_text(encoding="utf-8"), "slot-a-id\n")
            self.assertEqual((state_dir / "b").read_text(encoding="utf-8"), "slot-b-id\n")

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

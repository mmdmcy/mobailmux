from __future__ import annotations

import json
import os
import shlex
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
MBX = REPO_ROOT / "commands" / "bin" / "mbx"
INSTALL = REPO_ROOT / "commands" / "install.sh"


class MbxCliTest(unittest.TestCase):
    def run_mbx_with_fake_tmux(
        self,
        *args: str,
        env_overrides: dict[str, str] | None = None,
        input_text: str | None = None,
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            state_file = root / "sessions"
            links_file = root / "links"
            if not env_overrides or env_overrides.get("MBX_FAKE_TMUX_NO_SESSION") != "1":
                state_file.write_text("codex-1\n", encoding="utf-8")
            else:
                state_file.write_text("", encoding="utf-8")
            if env_overrides and env_overrides.get("MBX_FAKE_TMUX_EXTRA_SESSIONS"):
                with state_file.open("a", encoding="utf-8") as state:
                    state.write(env_overrides["MBX_FAKE_TMUX_EXTRA_SESSIONS"] + "\n")
            links_file.write_text(
                (env_overrides or {}).get("MBX_FAKE_TMUX_INITIAL_LINKS", ""),
                encoding="utf-8",
            )
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

has_fake_session() {
  local wanted="$1" item
  while IFS= read -r item; do
    [[ "$item" == "$wanted" ]] && return 0
  done < "$MBX_FAKE_TMUX_STATE"
  return 1
}

add_fake_session() {
  has_fake_session "$1" || printf '%s\\n' "$1" >> "$MBX_FAKE_TMUX_STATE"
}

remove_fake_session() {
  local wanted="$1" item temporary="${MBX_FAKE_TMUX_STATE}.tmp"
  : > "$temporary"
  while IFS= read -r item; do
    [[ "$item" == "$wanted" ]] || printf '%s\\n' "$item" >> "$temporary"
  done < "$MBX_FAKE_TMUX_STATE"
  mv "$temporary" "$MBX_FAKE_TMUX_STATE"
}

argument_after() {
  local wanted="$1"
  shift
  while [[ $# -gt 1 ]]; do
    if [[ "$1" == "$wanted" ]]; then
      printf '%s\\n' "$2"
      return 0
    fi
    shift
  done
  return 1
}

linked_value() {
  local target="$1" field="$2" line link_target source slot
  while IFS='|' read -r link_target source slot; do
    if [[ "$link_target" == "$target" ]]; then
      case "$field" in
        source) printf '%s\\n' "$source" ;;
        slot) printf '%s\\n' "$slot" ;;
      esac
      return 0
    fi
  done < "$MBX_FAKE_TMUX_LINKS"
  return 1
}

slot_for_index() {
  case "$1" in
    11) echo a ;; 12) echo b ;; 13) echo c ;; 14) echo d ;; 15) echo e ;;
    16) echo f ;; 17) echo g ;; 18) echo h ;; 19) echo i ;; 20) echo j ;;
  esac
}

case "${1:-}" in
  list-sessions)
    while IFS= read -r session; do
      [[ -n "$session" ]] && printf '%s|%s\\n' "$session" "${MBX_FAKE_ATTACHED:-1}"
    done < "$MBX_FAKE_TMUX_STATE"
    echo 'other|0'
    ;;
  list-windows)
    target="$(argument_after -t "$@" 2>/dev/null || true)"
    if [[ "$target" == "mobailmux" ]] && has_fake_session mobailmux; then
      case "$*" in
        *@mbx_home*) printf '0|%s\n' "${MBX_FAKE_HOME:-1}" ;;
        *window_id*) printf '0|@home\n' ;;
        *@mbx_slot*) printf '0|\n' ;;
      esac
    fi
    while IFS='|' read -r link_target source slot; do
      index="${link_target##*:}"
      case "$*" in
        *window_id*) printf '%s|%s\\n' "$index" "$source" ;;
        *@mbx_slot*) printf '%s|%s\\n' "$index" "$slot" ;;
      esac
    done < "$MBX_FAKE_TMUX_LINKS"
    ;;
  ls)
    while IFS= read -r session; do
      [[ -n "$session" ]] && printf '%s: 1 windows\\n' "$session"
    done < "$MBX_FAKE_TMUX_STATE"
    ;;
  has-session)
    has_fake_session "${3:-}"
    ;;
  new-session)
    session="$(argument_after -s "$@")"
    add_fake_session "$session"
    ;;
  link-window)
    source="$(argument_after -s "$@")"
    target="$(argument_after -t "$@")"
    index="${target##*:}"
    printf '%s|%s|%s\\n' "$target" "$source" "$(slot_for_index "$index")" >> "$MBX_FAKE_TMUX_LINKS"
    ;;
  kill-window)
    target="$(argument_after -t "$@")"
    source="$(linked_value "$target" source 2>/dev/null || true)"
    if [[ -n "$source" ]]; then
      remove_fake_session "${source#@}"
    else
      remove_fake_session "${target%%:*}"
    fi
    ;;
  kill-session)
    remove_fake_session "$(argument_after -t "$@")"
    ;;
  show-options)
    if [[ "$*" == *'@mbx_hub'* ]]; then
      printf '%s\n' "${MBX_FAKE_HUB_MARKER:-1}"
    fi
    ;;
  set-option|send-keys|attach-session|switch-client|move-window|new-window|rename-window|select-window|bind-key|unbind-key|display-menu|refresh-client)
    ;;
  display-message)
    target="$(argument_after -t "$@" 2>/dev/null || true)"
    case "$*" in
      *'#{version}'*) echo "${MBX_FAKE_TMUX_VERSION:-3.7b}" ;;
      *client_height*) echo "${MBX_FAKE_CLIENT_HEIGHT:-18}" ;;
      *session_attached*) echo "${MBX_FAKE_ATTACHED:-1}" ;;
      *pane_current_command*) echo "${MBX_FAKE_CURRENT_COMMAND:-node}" ;;
      *pane_current_path*) echo "${MBX_FAKE_CURRENT_PATH:-/workspace}" ;;
      *window_id*)
        if [[ "$target" == mobailmux:* ]]; then
          linked_value "$target" source 2>/dev/null || true
        elif [[ -n "$target" ]]; then
          printf '@%s\\n' "${target%%:*}"
        fi
        ;;
      *@mbx_state_at*) echo "${MBX_FAKE_AGENT_STATE_AT:-}" ;;
      *@mbx_state*) echo "${MBX_FAKE_AGENT_STATE:-}" ;;
      *@mbx_slot*)
        if [[ -z "$target" ]]; then
          echo "${MBX_FAKE_CURRENT_SLOT:-}"
        elif [[ "$target" == mobailmux:* ]]; then
          linked_value "$target" slot 2>/dev/null || true
        fi
        ;;
      *@mbx_workdir*) echo "${MBX_FAKE_WORKDIR:-}" ;;
      *'#W'*) echo "${MBX_FAKE_WINDOW_NAME:-a:workspace}" ;;
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
            env["MOBAILMUX_DEFAULT_HARNESS"] = "pi"
            env["MBX_FAKE_TMUX_STATE"] = str(state_file)
            env["MBX_FAKE_TMUX_LINKS"] = str(links_file)
            if env_overrides:
                env.update(env_overrides)
            return subprocess.run(
                [str(MBX), *args],
                check=False,
                env=env,
                text=True,
                capture_output=True,
                input=input_text,
            )

    def test_list_shows_running_sessions(self) -> None:
        listed = self.run_mbx_with_fake_tmux("list")

        self.assertEqual(listed.returncode, 0, listed.stderr)
        self.assertIn("slot", listed.stdout)
        self.assertIn("tab", listed.stdout)
        self.assertRegex(listed.stdout, r"(?m)^a\s+UNKNOWN\s+attached")
        self.assertNotRegex(listed.stdout, r"(?m)^b\s+")

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
        self.assertIn("AVAILABLE", status.stdout)
        self.assertIn("j", status.stdout)
        self.assertIn("agent-10", status.stdout)

    def test_status_can_target_one_session(self) -> None:
        result = self.run_mbx_with_fake_tmux("status", "a")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout, r"(?m)^a\s+UNKNOWN\s+attached")
        self.assertNotRegex(result.stdout, r"(?m)^b\s+")

    def test_legacy_slot_names_still_work(self) -> None:
        result = self.run_mbx_with_fake_tmux("status", "one")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout, r"(?m)^a\s+UNKNOWN\s+attached")
        self.assertNotRegex(result.stdout, r"(?m)^b\s+")

    def test_resume_attaches_to_the_requested_tmux_slot(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "r",
                "a",
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Opening project tab a", result.stdout)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("link-window -s @codex-1 -t mobailmux:11", log)
            self.assertIn("select-window -t mobailmux:11", log)
            self.assertIn("attach-session -t mobailmux", log)
            self.assertNotIn("send-keys -t codex-1", log)
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
            self.assertIn("Started shell tab j:workspace", result.stdout)
            self.assertNotIn("Codex", result.stdout)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn(
                f"new-session -d -s agent-10 -n agent-10 -c {workdir.resolve()}",
                log,
            )
            self.assertIn("rename-window -t agent-10 j:workspace", log)
            self.assertIn("link-window -s @agent-10 -t mobailmux:20", log)
            self.assertIn("set-option -t mobailmux mouse on", log)
            self.assertEqual(log.count("#{qh:window_name}"), 6)
            self.assertNotRegex(log, r"window-status-format[^\n]*#W")
            self.assertNotIn("send-keys -t agent-10", log)

    def test_resume_dry_run_describes_the_existing_slot(self) -> None:
        result = self.run_mbx_with_fake_tmux("r", "a", "--dry-run")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("slot:    a", result.stdout)
        self.assertIn("tab:     a:mobailmux", result.stdout)
        self.assertIn("state:   existing", result.stdout)
        self.assertIn("open Mobailmux tab a", result.stdout)

    def test_resume_dry_run_describes_a_new_slot(self) -> None:
        result = self.run_mbx_with_fake_tmux(
            "r",
            "10",
            "--dry-run",
            env_overrides={"MBX_FAKE_TMUX_NO_SESSION": "1"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("slot:    j", result.stdout)
        self.assertIn("tab:     j:mobailmux", result.stdout)
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
            self.assertIn("Stopped tab a", result.stdout)
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
            self.assertIn("Started pi in tab a:workspace", result.stdout)
            self.assertIn("send-keys -t agent-1", tmux_log.read_text(encoding="utf-8"))

    def test_tmux_mouse_mode_is_enabled(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "open",
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("set-option -t mobailmux mouse on", log)
            self.assertNotIn("set-option -g mouse on", log)

    def test_tmux_37_adds_clickable_agent_controls(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "open",
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("range=control|0] AGENTS", log)
            self.assertIn("range=control|1] +NEW", log)
            self.assertIn("range=control|2] STOP", log)
            self.assertIn("set-option -t mobailmux key-table mbx-mobailmux", log)
            self.assertIn("bind-key -T mbx-mobailmux MouseDown1Control0", log)
            self.assertIn("bind-key -T mbx-mobailmux MouseDown1Control1", log)
            self.assertIn("bind-key -T mbx-mobailmux MouseDown1Control2", log)
            self.assertNotIn("bind-key -n", log)

    def test_older_tmux_uses_one_clickable_agents_menu(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "open",
                env_overrides={
                    "MBX_FAKE_TMUX_VERSION": "3.6",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("range=left] AGENTS", log)
            self.assertIn("bind-key -T mbx-mobailmux MouseDown1StatusLeft", log)

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
            self.assertIn("Started pi in tab a:workspace", result.stdout)
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
            self.assertIn("tab:     b:workspace", result.stdout)
            self.assertIn(f"opencode --auto {workdir.resolve()}", result.stdout)
            self.assertIn("opencode-state.js", result.stdout)

    def test_shell_and_harness_slots_initialize_agent_state(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            shell_log = root / "shell.log"
            shell_result = self.run_mbx_with_fake_tmux(
                "resume",
                "j",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={
                    "MBX_FAKE_TMUX_NO_SESSION": "1",
                    "MBX_TMUX_LOG": str(shell_log),
                },
            )
            harness_log = root / "harness.log"
            harness_result = self.run_mbx_with_fake_tmux(
                "start",
                "j",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={
                    "MBX_FAKE_TMUX_NO_SESSION": "1",
                    "MBX_TMUX_LOG": str(harness_log),
                },
            )

            self.assertEqual(shell_result.returncode, 0, shell_result.stderr)
            self.assertEqual(harness_result.returncode, 0, harness_result.stderr)
            self.assertIn(
                "set-option -w -t agent-10 @mbx_state READY",
                shell_log.read_text(encoding="utf-8"),
            )
            harness_commands = harness_log.read_text(encoding="utf-8")
            self.assertIn("set-option -w -t agent-10 @mbx_state RUNNING", harness_commands)
            self.assertIn("pi-state.ts", harness_commands)
            self.assertIn("_state-exit", harness_commands)

    def test_status_and_overview_show_exact_agent_state(self) -> None:
        status = self.run_mbx_with_fake_tmux(
            "status",
            "a",
            env_overrides={"MBX_FAKE_AGENT_STATE": "DONE"},
        )
        overview = self.run_mbx_with_fake_tmux(
            "overview",
            "a",
            env_overrides={"MBX_FAKE_AGENT_STATE": "DONE"},
        )

        self.assertEqual(status.returncode, 0, status.stderr)
        self.assertEqual(overview.returncode, 0, overview.stderr)
        self.assertEqual(overview.stdout, status.stdout)
        self.assertRegex(status.stdout, r"(?m)^a\s+DONE\s+attached")

    def test_agent_tab_format_contains_state_badges(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "resume",
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
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("@mbx_state},RUNNING", log)
            self.assertIn("[DONE]", log)
            self.assertIn("[ERR]", log)

    def test_clickable_overview_lists_agents_and_actions(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "menu",
                env_overrides={
                    "MBX_FAKE_AGENT_STATE": "DONE",
                    "MBX_TMUX_LOG": str(tmux_log),
                    "TMUX": "/tmp/fake,1,0",
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("display-menu -T AI AGENTS", log)
            self.assertIn("a  #[fg=green,bold]DONE#[default]  workspace", log)
            self.assertIn("+ START PROJECT", log)
            self.assertIn("STOP CURRENT", log)

    def test_clickable_project_and_stop_menus_have_mouse_actions(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            projects = root / "projects"
            (projects / "alpha").mkdir(parents=True)
            project_log = root / "project.log"
            project_result = self.run_mbx_with_fake_tmux(
                "projects-menu",
                env_overrides={
                    "MBX_PROJECTS_DIR": str(projects),
                    "MBX_TMUX_LOG": str(project_log),
                    "TMUX": "/tmp/fake,1,0",
                },
            )
            stop_log = root / "stop.log"
            stop_result = self.run_mbx_with_fake_tmux(
                "stop-menu",
                env_overrides={
                    "MBX_FAKE_AGENT_STATE": "RUNNING",
                    "MBX_FAKE_CURRENT_SLOT": "a",
                    "MBX_TMUX_LOG": str(stop_log),
                    "TMUX": "/tmp/fake,1,0",
                },
            )

            self.assertEqual(project_result.returncode, 0, project_result.stderr)
            self.assertEqual(stop_result.returncode, 0, stop_result.stderr)
            self.assertIn("display-menu -T START WITH PI - PAGE 1", project_log.read_text(encoding="utf-8"))
            self.assertIn("+  alpha", project_log.read_text(encoding="utf-8"))
            stop_commands = stop_log.read_text(encoding="utf-8")
            self.assertIn("display-menu -T CONFIRM STOP", stop_commands)
            self.assertIn("STOP THIS AGENT", stop_commands)
            self.assertIn("_menu-stop 1", stop_commands)

    def test_internal_state_exit_marks_failures_and_preserves_errors(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            failed = self.run_mbx_with_fake_tmux(
                "_state-exit",
                "7",
                env_overrides={
                    "MBX_TMUX_LOG": str(tmux_log),
                    "TMUX": "/tmp/fake,1,0",
                    "TMUX_PANE": "%4",
                },
            )

            self.assertEqual(failed.returncode, 0, failed.stderr)
            self.assertIn(
                "set-option -w -t %4 @mbx_state ERROR",
                tmux_log.read_text(encoding="utf-8"),
            )

    def test_no_arguments_opens_the_clickable_hub(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("new-session -d -s mobailmux -n home", log)
            self.assertIn("link-window -s @codex-1 -t mobailmux:11", log)
            self.assertIn("attach-session -t mobailmux", log)

    def test_start_without_slot_uses_first_available_tab(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "start",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={"MBX_TMUX_LOG": str(tmux_log)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Started pi in tab b:workspace", result.stdout)
            self.assertIn(
                "new-session -d -s agent-2",
                tmux_log.read_text(encoding="utf-8"),
            )

    def test_project_name_is_a_directory_shorthand(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            projects = Path(temp_dir) / "projects"
            selected = projects / "example"
            selected.mkdir(parents=True)
            result = self.run_mbx_with_fake_tmux(
                "start",
                "example",
                "--dry-run",
                env_overrides={
                    "MBX_FAKE_TMUX_NO_SESSION": "1",
                    "MBX_PROJECTS_DIR": str(projects),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("slot:    a", result.stdout)
            self.assertIn(f"workdir: {selected.resolve()}", result.stdout)
            self.assertIn("tab:     a:example", result.stdout)

    def test_idle_slot_changes_to_requested_project_before_pi(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "another-project"
            workdir.mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={
                    "MBX_FAKE_CURRENT_COMMAND": "zsh",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("send-keys -t codex-1", log)
            self.assertIn("/bin/bash -lc", log)
            self.assertIn(str(workdir.resolve()), log)
            self.assertIn("pi\\ --approve", log)
            self.assertIn("pi-state.ts", log)

    def test_two_attached_clients_are_still_reported_attached(self) -> None:
        result = self.run_mbx_with_fake_tmux(
            "status",
            "a",
            env_overrides={"MBX_FAKE_ATTACHED": "2"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout, r"(?m)^a\s+UNKNOWN\s+attached")

    def test_quit_without_slot_stops_current_project_tab(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "q",
                env_overrides={
                    "MBX_FAKE_CURRENT_SLOT": "a",
                    "MBX_TMUX_LOG": str(tmux_log),
                    "TMUX": "/tmp/fake,1,0",
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Stopped tab a", result.stdout)
            self.assertIn("kill-session -t codex-1", tmux_log.read_text(encoding="utf-8"))

    def test_project_picker_starts_selected_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            projects = root / "projects"
            (projects / "alpha").mkdir(parents=True)
            selected = projects / "beta"
            selected.mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "p",
                "--no-attach",
                env_overrides={
                    "MBX_FAKE_TMUX_NO_SESSION": "1",
                    "MBX_PROJECTS_DIR": str(projects),
                    "MBX_TMUX_LOG": str(tmux_log),
                },
                input_text="2\n",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("1.         alpha", result.stdout)
            self.assertIn("2.         beta", result.stdout)
            self.assertIn("Started pi in tab a:beta", result.stdout)
            self.assertIn(
                f"new-session -d -s agent-1 -n agent-1 -c {selected.resolve()}",
                tmux_log.read_text(encoding="utf-8"),
            )

    def test_existing_unmarked_hub_session_is_not_modified(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "open",
                env_overrides={
                    "MBX_FAKE_TMUX_EXTRA_SESSIONS": "mobailmux",
                    "MBX_FAKE_HUB_MARKER": "0",
                    "MBX_FAKE_HOME": "0",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("is not a Mobailmux hub", result.stderr)
            self.assertNotIn("set-option -t mobailmux mouse on", tmux_log.read_text(encoding="utf-8"))

    def test_missing_home_window_is_recreated(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "open",
                env_overrides={
                    "MBX_FAKE_TMUX_EXTRA_SESSIONS": "mobailmux",
                    "MBX_FAKE_HUB_MARKER": "1",
                    "MBX_FAKE_HOME": "0",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("move-window -s mobailmux:0 -t mobailmux:", log)
            self.assertIn("new-window -d -t mobailmux:0 -n home", log)
            self.assertIn("set-option -w -t mobailmux:0 @mbx_home 1", log)

    def test_managed_launch_uses_bash_even_from_fish(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            workdir = Path(temp_dir) / "workspace"
            workdir.mkdir()
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--no-attach",
                "-C",
                str(workdir),
                env_overrides={
                    "MBX_FAKE_CURRENT_COMMAND": "fish",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("send-keys -t codex-1 /bin/bash -lc", tmux_log.read_text(encoding="utf-8"))

    def test_opencode_config_conflict_fails_instead_of_losing_tracking(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            workdir = Path(temp_dir) / "workspace"
            workdir.mkdir()
            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--harness",
                "opencode",
                "--dry-run",
                "-C",
                str(workdir),
                env_overrides={
                    "OPENCODE_CONFIG": "/tmp/opencode.json",
                    "OPENCODE_CONFIG_CONTENT": "{}",
                },
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("both OPENCODE_CONFIG and OPENCODE_CONFIG_CONTENT", result.stderr)

    def test_opencode_tracking_layers_with_either_existing_config_source(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            workdir = Path(temp_dir) / "workspace"
            workdir.mkdir()
            with_file = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--harness",
                "opencode",
                "--dry-run",
                "-C",
                str(workdir),
                env_overrides={"OPENCODE_CONFIG": "/tmp/custom-opencode.json"},
            )
            with_content = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--harness",
                "opencode",
                "--dry-run",
                "-C",
                str(workdir),
                env_overrides={"OPENCODE_CONFIG_CONTENT": "{\"theme\":\"system\"}"},
            )

            self.assertEqual(with_file.returncode, 0, with_file.stderr)
            self.assertEqual(with_content.returncode, 0, with_content.stderr)
            self.assertIn("OPENCODE_CONFIG_CONTENT=", with_file.stdout)
            self.assertIn("opencode-state.js", with_file.stdout)
            self.assertIn("OPENCODE_CONFIG=", with_content.stdout)
            self.assertIn("opencode.json", with_content.stdout)

    def test_configured_pi_binary_participates_in_default_detection(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            workdir = Path(temp_dir) / "workspace"
            workdir.mkdir()
            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--dry-run",
                "-C",
                str(workdir),
                env_overrides={
                    "MOBAILMUX_DEFAULT_HARNESS": "",
                    "MOBAILMUX_PI_BIN": "/usr/bin/true",
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("/usr/bin/true --approve", result.stdout)

    def test_resource_directory_is_canonicalized_before_launch(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            resources = REPO_ROOT / "commands" / "libexec" / "mobailmux"
            resource_link = root / "resources"
            resource_link.symlink_to(resources, target_is_directory=True)
            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--harness",
                "opencode",
                "--dry-run",
                "-C",
                str(workdir),
                env_overrides={"MBX_RESOURCE_DIR": str(resource_link)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(str(resources.resolve() / "opencode-state.js"), result.stdout)
            self.assertNotIn(str(resource_link / "opencode-state.js"), result.stdout)

    def test_project_menu_is_paginated_and_escapes_tmux_formats(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            projects = root / "projects"
            projects.mkdir()
            for name in ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot#[red]"]:
                (projects / name).mkdir()
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "projects-menu",
                "3",
                env_overrides={
                    "MBX_PROJECTS_DIR": str(projects),
                    "MBX_MENU_PAGE_SIZE": "2",
                    "MBX_TMUX_LOG": str(tmux_log),
                    "TMUX": "/tmp/fake,1,0",
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("START WITH PI - PAGE 3", log)
            self.assertIn("+  echo", log)
            self.assertIn("+  foxtrot##[red]", log)
            self.assertIn("< PREVIOUS PAGE", log)
            self.assertNotIn("NEXT PAGE >", log)
            self.assertNotIn("+  alpha", log)

    def test_symlinked_projects_are_listed_once_by_canonical_path(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            projects = root / "projects"
            project = projects / "alpha"
            project.mkdir(parents=True)
            (projects / "alpha-alias").symlink_to(project, target_is_directory=True)
            tmux_log = root / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "projects-menu",
                env_overrides={
                    "MBX_PROJECTS_DIR": str(projects),
                    "MBX_TMUX_LOG": str(tmux_log),
                    "TMUX": "/tmp/fake,1,0",
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            menu_command = next(
                line
                for line in tmux_log.read_text(encoding="utf-8").splitlines()
                if line.startswith("display-menu ")
            )
            self.assertEqual(menu_command.count("+  alpha"), 1)
            self.assertNotIn("alpha-alias", menu_command)

    def test_stop_removes_source_session_and_linked_hub_window(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            tmux_log = Path(temp_dir) / "tmux.log"
            result = self.run_mbx_with_fake_tmux(
                "q",
                "a",
                env_overrides={
                    "MBX_FAKE_TMUX_EXTRA_SESSIONS": "mobailmux",
                    "MBX_FAKE_TMUX_INITIAL_LINKS": "mobailmux:11|@codex-1|a\n",
                    "MBX_TMUX_LOG": str(tmux_log),
                },
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("kill-session -t codex-1", log)
            self.assertIn("kill-window -t mobailmux:11", log)

    def test_opencode_state_is_derived_across_sessions_without_duplicate_writes(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            tmux_log = root / "tmux.log"
            fake_tmux = root / "tmux"
            fake_tmux.write_text(
                "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"$MBX_TMUX_LOG\"\n",
                encoding="utf-8",
            )
            fake_tmux.chmod(fake_tmux.stat().st_mode | stat.S_IXUSR)
            plugin = REPO_ROOT / "commands" / "libexec" / "mobailmux" / "opencode-state.js"
            script = f"""
import {{ MobailmuxState }} from {plugin.as_uri()!r};
const plugin = await MobailmuxState();
const send = (type, properties) => plugin.event({{ event: {{ type, properties }} }});
await send("session.status", {{ sessionID: "one", status: "busy" }});
await send("session.status", {{ sessionID: "two", status: "busy" }});
await send("session.error", {{ sessionID: "one" }});
await send("session.idle", {{ sessionID: "two" }});
await send("session.status", {{ sessionID: "one", status: "busy" }});
await send("permission.asked", {{ sessionID: "one", id: "p1" }});
await send("permission.asked", {{ sessionID: "one", id: "p2" }});
await send("permission.replied", {{ sessionID: "one", id: "p1" }});
await send("permission.replied", {{ sessionID: "one", id: "p2" }});
await send("session.idle", {{ sessionID: "one" }});
"""
            env = os.environ.copy()
            env["PATH"] = f"{root}{os.pathsep}{env['PATH']}"
            env["TMUX_PANE"] = "%9"
            env["MBX_TMUX_LOG"] = str(tmux_log)
            result = subprocess.run(
                ["node", "--input-type=module", "-e", script],
                check=False,
                env=env,
                text=True,
                capture_output=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            states = [
                line.rsplit(" ", 1)[-1]
                for line in tmux_log.read_text(encoding="utf-8").splitlines()
                if "set-option" in line and " @mbx_state " in line
            ]
            self.assertEqual(states, ["READY", "RUNNING", "ERROR", "RUNNING", "WAITING", "RUNNING", "DONE"])

    def test_opencode_state_preserves_boot_state_even_without_activity(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            tmux_log = root / "tmux.log"
            fake_tmux = root / "tmux"
            fake_tmux.write_text(
                "#!/usr/bin/env bash\n"
                "if [[ \"$*\" == *'#{@mbx_state}'* ]]; then printf 'RUNNING\\n'; fi\n"
                "printf '%s\\n' \"$*\" >> \"$MBX_TMUX_LOG\"\n",
                encoding="utf-8",
            )
            fake_tmux.chmod(fake_tmux.stat().st_mode | stat.S_IXUSR)
            plugin = REPO_ROOT / "commands" / "libexec" / "mobailmux" / "opencode-state.js"
            script = f"""
import {{ MobailmuxState }} from {plugin.as_uri()!r};
await MobailmuxState();
"""
            env = os.environ.copy()
            env["PATH"] = f"{root}{os.pathsep}{env['PATH']}"
            env["TMUX_PANE"] = "%9"
            env["MBX_TMUX_LOG"] = str(tmux_log)
            result = subprocess.run(
                ["node", "--input-type=module", "-e", script],
                check=False,
                env=env,
                text=True,
                capture_output=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            log = tmux_log.read_text(encoding="utf-8")
            self.assertIn("display-message", log)
            self.assertNotIn("set-option", log)

    def test_opencode_plugin_json_is_escaped_for_quoted_resource_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            workdir = root / "workspace"
            workdir.mkdir()
            resources = REPO_ROOT / "commands" / "libexec" / "mobailmux"
            resource_link = root / 'res"dir'
            resource_link.symlink_to(resources, target_is_directory=True)
            result = self.run_mbx_with_fake_tmux(
                "start",
                "a",
                "--harness",
                "opencode",
                "--dry-run",
                "-C",
                str(workdir),
                env_overrides={"MBX_RESOURCE_DIR": str(resource_link)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            launch = [
                line for line in result.stdout.splitlines()
                if line.startswith("command: ") and "OPENCODE_CONFIG_CONTENT=" in line
            ]
            self.assertEqual(len(launch), 1)
            words = shlex.split(launch[0].removeprefix("command: "))
            payload = next(
                word.split("=", 1)[1] for word in words if word.startswith("OPENCODE_CONFIG_CONTENT=")
            )
            config = json.loads(payload)
            self.assertEqual(
                config,
                {"plugin": [f"file://{resource_link.resolve()}/opencode-state.js"]},
            )

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
        self.assertIn("mbx s [slot]", result.stdout)
        self.assertIn("mbx q [slot]", result.stdout)
        self.assertIn("folder-named tabs", result.stdout)

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

    def test_copy_install_includes_agent_state_adapters(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            prefix = Path(temp_dir) / "prefix"
            result = subprocess.run(
                [str(INSTALL), "--copy", "--prefix", str(prefix)],
                check=False,
                text=True,
                capture_output=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((prefix / "bin" / "mbx").is_file())
            resources = prefix / "libexec" / "mobailmux"
            self.assertTrue((resources / "opencode-state.js").is_file())
            self.assertTrue((resources / "opencode.json").is_file())
            self.assertTrue((resources / "pi-state.ts").is_file())

    def test_copy_install_replaces_an_existing_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            prefix = Path(temp_dir) / "prefix"
            bin_dir = prefix / "bin"
            bin_dir.mkdir(parents=True)
            target = bin_dir / "mbx"
            target.symlink_to(MBX)
            result = subprocess.run(
                [str(INSTALL), "--copy", "--prefix", str(prefix)],
                check=False,
                text=True,
                capture_output=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(target.is_file())
            self.assertFalse(target.is_symlink())


if __name__ == "__main__":
    unittest.main()

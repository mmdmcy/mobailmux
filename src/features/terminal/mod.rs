use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use axum::{
    Form, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::{
    AppState, default_home_dir, get_agent_slot, html_escape, raw_guard, set_agent_workdir,
    truncate_text,
};

const MAX_COMMAND_CHARS: usize = 4096;
const MAX_OUTPUT_CHARS: usize = 64 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const RUN_ROUTE: &str = "/agents/terminal/run";

#[derive(Deserialize)]
pub(crate) struct RunForm {
    slot_id: i64,
    command: String,
}

#[derive(Serialize)]
struct RunResult {
    ok: bool,
    status: String,
    output: String,
    cwd: String,
}

pub(crate) fn panel_html(slot_id: i64, cwd: &str) -> String {
    format!(
        r#"<dialog class="terminal-panel" id="terminalPanel">
  <header><div><strong>Terminal</strong><br><span data-terminal-cwd>{}</span></div><button type="button" class="icon" data-terminal-close aria-label="Close">x</button></header>
  <main><pre class="terminal-output" data-terminal-output>$ ready
</pre><form class="terminal-form" data-terminal-form><input name="slot_id" type="hidden" value="{slot_id}"><input name="command" autocomplete="off" autocapitalize="off" spellcheck="false" placeholder="Command"><button type="submit">Run</button></form></main>
</dialog>"#,
        html_escape(cwd)
    )
}

pub(crate) async fn run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<RunForm>,
) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    let command = form.command.trim();
    let slot = {
        let db = state.db.lock().unwrap();
        get_agent_slot(&db, form.slot_id).unwrap_or(None)
    };
    let Some(slot) = slot else {
        return error(StatusCode::NOT_FOUND, "missing slot", "", "");
    };
    if command.is_empty() {
        return Json(result(false, "empty", "", &slot.workdir)).into_response();
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "too long", "", &slot.workdir);
    }
    let workdir = PathBuf::from(&slot.workdir);
    if !workdir.is_dir() {
        return error(
            StatusCode::CONFLICT,
            "bad cwd",
            "Execution directory is unavailable.",
            &slot.workdir,
        );
    }

    let marker = format!("__MOBAILMUX_{}__", Uuid::new_v4().simple());
    let script = format!(
        "set +e\n{command}\n__mobailmux_status=$?\nprintf '\\n{marker}STATUS:%s\\n{marker}PWD:%s\\n' \"$__mobailmux_status\" \"$PWD\"\nexit \"$__mobailmux_status\""
    );
    let mut child = Command::new("/bin/bash");
    child
        .arg("-lc")
        .arg(script)
        .current_dir(&workdir)
        .env("HOME", default_home_dir())
        .env(
            "PATH",
            env::var("PATH").unwrap_or_else(|_| {
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into()
            }),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let Ok(child) = child.spawn() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "spawn failed",
            "Could not start /bin/bash.",
            &slot.workdir,
        );
    };
    let output = match tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "wait failed",
                &err.to_string(),
                &slot.workdir,
            );
        }
        Err(_) => {
            return Json(result(
                false,
                "timeout",
                "Command timed out after 30s.",
                &slot.workdir,
            ))
            .into_response();
        }
    };

    let (stdout, exit_code, final_cwd) = visible_stdout(
        &String::from_utf8_lossy(&output.stdout),
        &marker,
        &slot.workdir,
    );
    if final_cwd != slot.workdir && Path::new(&final_cwd).is_dir() {
        let db = state.db.lock().unwrap();
        let _ = set_agent_workdir(&db, slot.id, &final_cwd);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = stdout;
    if !stderr.trim().is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    let status_code = exit_code.or_else(|| output.status.code()).unwrap_or(-1);
    Json(result(
        output.status.success() || status_code == 0,
        &format!("exit {status_code}"),
        &truncate_text(&combined, MAX_OUTPUT_CHARS),
        &final_cwd,
    ))
    .into_response()
}

fn result(ok: bool, status: &str, output: &str, cwd: &str) -> RunResult {
    RunResult {
        ok,
        status: status.into(),
        output: output.into(),
        cwd: cwd.into(),
    }
}

fn error(code: StatusCode, status: &str, output: &str, cwd: &str) -> Response {
    (code, Json(result(false, status, output, cwd))).into_response()
}

fn visible_stdout(stdout: &str, marker: &str, fallback_cwd: &str) -> (String, Option<i32>, String) {
    let status_prefix = format!("{marker}STATUS:");
    let pwd_prefix = format!("{marker}PWD:");
    let mut visible = String::new();
    let mut exit_code = None;
    let mut cwd = fallback_cwd.to_string();
    for chunk in stdout.split_inclusive('\n') {
        let line = chunk.trim_end_matches(['\r', '\n']);
        if let Some(rest) = line.strip_prefix(&status_prefix) {
            exit_code = rest.trim().parse().ok();
            continue;
        }
        if let Some(rest) = line.strip_prefix(&pwd_prefix) {
            cwd = rest.trim().to_string();
            continue;
        }
        visible.push_str(chunk);
    }
    (visible, exit_code, cwd)
}

#[cfg(test)]
mod tests {
    use super::{RUN_ROUTE, panel_html, visible_stdout};

    #[test]
    fn strips_private_protocol_markers_from_output() {
        let marker = "__TEST__";
        let parsed = visible_stdout("hello\n__TEST__STATUS:0\n__TEST__PWD:/tmp\n", marker, "/");
        assert_eq!(parsed, ("hello\n".into(), Some(0), "/tmp".into()));
    }

    #[test]
    fn panel_exposes_terminal_without_file_or_folder_controls() {
        let panel = panel_html(7, "/runtime/project");
        assert!(panel.contains("data-terminal-form"));
        assert!(panel.contains("name=\"command\""));
        assert!(!panel.contains("type=\"file\""));
        assert!(!panel.contains("Create folder"));
        assert_eq!(RUN_ROUTE, "/agents/terminal/run");
    }
}

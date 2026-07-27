use crate::AgentHarness;
use crate::AgentProgress;
use crate::AgentRun;
use crate::AgentRunSettings;
use crate::AgentSlotRow;
use crate::AgentStdoutSummary;
use crate::AppState;
use crate::Arc;
use crate::BufReader;
use crate::MAX_AGENT_MESSAGE_CHARS;
use crate::Mutex;
use crate::Path;
use crate::PathBuf;
use crate::SeekFrom;
use crate::StdDuration;
use crate::Stdio;
use crate::TokioCommand;
use crate::Utc;
use crate::Uuid;
use crate::agent_run_settings_label;
use crate::agent_session;
use crate::append_agent_assistant;
use crate::apply_agent_run_settings;
#[cfg(test)]
use crate::codex_content_text;
use crate::env;
use crate::fs;
use crate::get_agent_slot;
use crate::io;
use crate::oneshot;
use crate::set_agent_session;
use crate::sleep;
use crate::truncate_text;
use std::io::Read;
use std::io::Seek;
use std::os::unix::fs::PermissionsExt;
use tokio::io::AsyncBufReadExt;

pub(crate) fn prepare_agent_progress(slot_id: i64) -> io::Result<AgentProgress> {
    let dir = env::temp_dir().join(format!(
        "mobailmux-agent-progress-{}-{}",
        slot_id,
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&dir)?;
    let file = dir.join("progress.log");
    fs::File::create(&file)?;
    let helper = dir.join("aiprogress");
    let progress_file = shell_single_quote(&file.to_string_lossy());
    fs::write(
        &helper,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" >> {progress_file}\n"
        ),
    )?;
    let mut permissions = fs::metadata(&helper)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&helper, permissions)?;
    Ok(AgentProgress { dir, file })
}

pub(crate) fn progress_path_env(progress_dir: &Path) -> Option<std::ffi::OsString> {
    let mut paths = vec![progress_dir.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).ok()
}

pub(crate) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn watch_agent_progress_file(
    state: Arc<AppState>,
    slot_id: i64,
    path: PathBuf,
    mut done_rx: oneshot::Receiver<()>,
) {
    let mut offset = 0u64;
    loop {
        tokio::select! {
            _ = &mut done_rx => {
                let _ = drain_agent_progress_file(&state, slot_id, &path, &mut offset);
                break;
            }
            _ = sleep(StdDuration::from_secs(1)) => {
                let _ = drain_agent_progress_file(&state, slot_id, &path, &mut offset);
            }
        }
    }
}

pub(crate) fn drain_agent_progress_file(
    state: &AppState,
    slot_id: i64,
    path: &Path,
    offset: &mut u64,
) -> io::Result<()> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(*offset))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    *offset = file.stream_position()?;
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            append_agent_assistant(
                state,
                slot_id,
                &format!("note: {}", truncate_text(line, 1200)),
            );
        }
    }
    Ok(())
}

pub(crate) fn start_agent_job(
    state: Arc<AppState>,
    slot_id: i64,
    request_body: String,
    settings: AgentRunSettings,
) {
    state.agent_jobs.lock().unwrap().insert(
        slot_id,
        AgentRun {
            status: "starting".into(),
            current: "starting".into(),
            started_at: Utc::now().to_rfc3339(),
        },
    );
    tokio::spawn(run_agent_job(state, slot_id, request_body, settings));
}

async fn run_agent_job(
    state: Arc<AppState>,
    slot_id: i64,
    request_body: String,
    settings: AgentRunSettings,
) {
    let slot = {
        let db = state.db.lock().unwrap();
        get_agent_slot(&db, slot_id).unwrap_or(None)
    };
    let Some(slot) = slot else {
        state.agent_jobs.lock().unwrap().remove(&slot_id);
        return;
    };
    if !slot.harness.is_runnable() {
        append_agent_assistant(
            &state,
            slot_id,
            "This is a retained legacy Codex lane. Its messages are preserved, but Mobailmux will not resume or launch Codex. Start a new Pi or OpenCode project lane to continue.",
        );
        state.agent_jobs.lock().unwrap().remove(&slot_id);
        return;
    }
    append_agent_assistant(
        &state,
        slot_id,
        &format!(
            "{} started {} in `{}`{}.",
            slot.name,
            slot.harness.display_name(),
            slot.workdir,
            agent_run_settings_label(&settings)
        ),
    );
    let progress = if state.config.agent_progress_notes {
        prepare_agent_progress(slot_id).ok()
    } else {
        None
    };
    let prompt = build_agent_prompt(&slot, &request_body, progress.is_some());
    let session = {
        let db = state.db.lock().unwrap();
        agent_session(&db, slot_id).unwrap_or(None)
    };
    let use_resume = session
        .as_ref()
        .is_some_and(|(_, workdir)| workdir == &slot.workdir);
    let binary = match slot.harness {
        AgentHarness::Pi => &state.config.pi_bin,
        AgentHarness::OpenCode => &state.config.opencode_bin,
        AgentHarness::LegacyCodex => unreachable!(),
    };
    let mut command = TokioCommand::new(binary);
    match slot.harness {
        AgentHarness::Pi => {
            let session_id = session
                .filter(|(_, workdir)| workdir == &slot.workdir)
                .map(|(session_id, _)| session_id)
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            if !use_resume {
                let db = state.db.lock().unwrap();
                let _ = set_agent_session(&db, slot_id, &session_id, &slot.workdir);
            }
            command
                .args(&state.config.pi_args)
                .arg("--mode")
                .arg("json")
                .arg("--session-id")
                .arg(session_id)
                .arg("--print");
            apply_agent_run_settings(&mut command, &settings, slot.harness);
            command.arg(prompt);
        }
        AgentHarness::OpenCode => {
            command
                .arg("run")
                .arg("--format")
                .arg("json")
                .arg("--dir")
                .arg(&slot.workdir)
                .args(&state.config.opencode_args);
            if use_resume && let Some((session_id, _)) = session {
                command.arg("--session").arg(session_id);
            }
            apply_agent_run_settings(&mut command, &settings, slot.harness);
            command.arg(prompt);
        }
        AgentHarness::LegacyCodex => unreachable!(),
    }
    command
        .current_dir(&slot.workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(progress) = &progress
        && let Some(path) = progress_path_env(&progress.dir)
    {
        command.env("PATH", path);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            append_agent_assistant(
                &state,
                slot_id,
                &format!("Could not start `{binary}`: {err}"),
            );
            state.agent_jobs.lock().unwrap().remove(&slot_id);
            if let Some(progress) = progress {
                let _ = tokio::fs::remove_dir_all(progress.dir).await;
            }
            return;
        }
    };
    let (cancel_tx, mut cancel_rx) = oneshot::channel();
    state
        .agent_cancels
        .lock()
        .unwrap()
        .insert(slot_id, cancel_tx);
    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(read_agent_stdout(
            state.clone(),
            slot_id,
            slot.workdir.clone(),
            slot.harness,
            stdout,
        ))
    });
    let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_agent_stderr(stderr_tail.clone(), stderr)));
    let (progress_done_tx, progress_task) = if let Some(progress) = &progress {
        let (done_tx, done_rx) = oneshot::channel();
        (
            Some(done_tx),
            Some(tokio::spawn(watch_agent_progress_file(
                state.clone(),
                slot_id,
                progress.file.clone(),
                done_rx,
            ))),
        )
    } else {
        (None, None)
    };
    let started = Utc::now();
    let stopped;
    let status = tokio::select! {
        result = child.wait() => {
            stopped = false;
            result
        }
        _ = &mut cancel_rx => {
            stopped = true;
            let _ = child.start_kill();
            child.wait().await
        }
    };
    let mut stdout_summary = AgentStdoutSummary::default();
    if let Some(task) = stdout_task {
        stdout_summary = task.await.unwrap_or_default();
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
    if let Some(done_tx) = progress_done_tx {
        let _ = done_tx.send(());
    }
    if let Some(task) = progress_task {
        let _ = task.await;
    }
    if let Some(progress) = progress {
        let _ = tokio::fs::remove_dir_all(progress.dir).await;
    }
    state.agent_cancels.lock().unwrap().remove(&slot_id);
    state.agent_jobs.lock().unwrap().remove(&slot_id);
    let elapsed = (Utc::now() - started).num_seconds().max(0);
    if stopped {
        append_agent_assistant(
            &state,
            slot_id,
            &format!("{} stopped after {elapsed}s.", slot.name),
        );
        return;
    }
    match status {
        Ok(status) if status.success() => {
            let final_text = stdout_summary
                .final_text
                .as_deref()
                .or(stdout_summary.last_assistant_text.as_deref())
                .unwrap_or("")
                .trim()
                .to_string();
            if final_text.is_empty() {
                append_agent_assistant(
                    &state,
                    slot_id,
                    &format!(
                        "({} completed without a final message.)",
                        slot.harness.display_name()
                    ),
                );
            } else {
                append_agent_assistant(&state, slot_id, &final_text);
            }
        }
        Ok(status) => {
            let tail = stderr_tail.lock().unwrap().join("\n");
            append_agent_assistant(
                &state,
                slot_id,
                &format!(
                    "{} failed with exit code {} after {elapsed}s.\n\n```text\n{}\n```",
                    slot.name,
                    status.code().unwrap_or(-1),
                    truncate_text(&tail, 2400)
                ),
            );
        }
        Err(err) => {
            append_agent_assistant(
                &state,
                slot_id,
                &format!("{} wait failed after {elapsed}s: {err}", slot.name),
            );
        }
    }
}

async fn read_agent_stdout(
    state: Arc<AppState>,
    slot_id: i64,
    workdir: String,
    harness: AgentHarness,
    stdout: tokio::process::ChildStdout,
) -> AgentStdoutSummary {
    let mut lines = BufReader::new(stdout).lines();
    let mut summary = AgentStdoutSummary::default();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(thread_id) = harness_session_id(harness, &event) {
            let db = state.db.lock().unwrap();
            let _ = set_agent_session(&db, slot_id, thread_id, &workdir);
        }
        if let Some((message, final_answer)) = harness_stdout_agent_message(harness, &event) {
            let message = truncate_text(&message, MAX_AGENT_MESSAGE_CHARS);
            if !message.trim().is_empty()
                && summary.last_assistant_text.as_deref() != Some(message.as_str())
            {
                state
                    .agent_jobs
                    .lock()
                    .unwrap()
                    .entry(slot_id)
                    .and_modify(|run| {
                        run.status = if final_answer {
                            "finishing".into()
                        } else {
                            "responding".into()
                        };
                        run.current.clear();
                    });
                summary.last_assistant_text = Some(message.clone());
                if final_answer {
                    summary.final_text = Some(message);
                }
            }
            continue;
        }
    }
    summary
}

pub(crate) fn harness_session_id(harness: AgentHarness, value: &serde_json::Value) -> Option<&str> {
    match harness {
        AgentHarness::Pi => (value.get("type").and_then(|item| item.as_str()) == Some("session"))
            .then(|| value.get("id").and_then(|item| item.as_str()))
            .flatten(),
        AgentHarness::OpenCode => value.get("sessionID").and_then(|item| item.as_str()),
        AgentHarness::LegacyCodex => None,
    }
}

pub(crate) fn harness_stdout_agent_message(
    harness: AgentHarness,
    value: &serde_json::Value,
) -> Option<(String, bool)> {
    match harness {
        AgentHarness::Pi => {
            if value.get("type").and_then(|item| item.as_str()) != Some("message_end") {
                return None;
            }
            let message = value.get("message")?;
            if message.get("role").and_then(|item| item.as_str()) != Some("assistant") {
                return None;
            }
            let text = json_content_text(message.get("content")?);
            Some((text, true))
        }
        AgentHarness::OpenCode => {
            if value.get("type").and_then(|item| item.as_str()) != Some("text") {
                return None;
            }
            let part = value.get("part")?;
            let text = part.get("text")?.as_str()?.to_string();
            let final_answer = part
                .pointer("/metadata/openai/phase")
                .and_then(|item| item.as_str())
                .is_some_and(is_final_agent_phase);
            Some((text, final_answer))
        }
        AgentHarness::LegacyCodex => None,
    }
}

fn json_content_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let item_type = item.get("type").and_then(|value| value.as_str());
                matches!(item_type, Some("text") | Some("output_text"))
                    .then(|| item.get("text").and_then(|value| value.as_str()))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
pub(crate) fn codex_stdout_agent_message(value: &serde_json::Value) -> Option<(String, bool)> {
    let event_type = value.get("type").and_then(|value| value.as_str())?;
    if event_type == "response_item" {
        let payload = value.get("payload")?;
        if payload.get("type").and_then(|value| value.as_str()) != Some("message") {
            return None;
        }
        if payload.get("role").and_then(|value| value.as_str()) != Some("assistant") {
            return None;
        }
        let text = codex_content_text(payload.get("content")?);
        let final_answer = payload
            .get("phase")
            .and_then(|value| value.as_str())
            .is_some_and(is_final_agent_phase);
        return Some((text, final_answer));
    }

    let payload = if event_type == "event_msg" {
        value.get("payload")?
    } else {
        value
    };
    let payload_type = payload
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or(event_type);
    if payload_type != "agent_message" {
        return None;
    }
    let text = payload
        .get("message")
        .or_else(|| payload.get("text"))
        .and_then(|value| value.as_str())?
        .to_string();
    let final_answer = payload
        .get("phase")
        .and_then(|value| value.as_str())
        .is_some_and(is_final_agent_phase);
    Some((text, final_answer))
}

pub(crate) fn is_final_agent_phase(value: &str) -> bool {
    matches!(value, "final" | "final_answer")
}

async fn read_agent_stderr(tail: Arc<Mutex<Vec<String>>>, stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut tail = tail.lock().unwrap();
        tail.push(line.to_string());
        if tail.len() > 80 {
            let extra = tail.len() - 80;
            tail.drain(0..extra);
        }
    }
}

pub(crate) fn build_agent_prompt(
    slot: &AgentSlotRow,
    request_body: &str,
    progress_notes_enabled: bool,
) -> String {
    let goal = slot.goal.trim();
    let mut sections = Vec::new();
    if !goal.is_empty() {
        sections.push(format!("Current slot goal:\n{goal}"));
    }
    if progress_notes_enabled {
        sections.push(
            "Mobailmux has an optional `aiprogress 'message'` command for short human progress notes. Use it only when useful."
                .to_string(),
        );
    }
    sections.push(request_body.to_string());
    sections.join("\n\n")
}

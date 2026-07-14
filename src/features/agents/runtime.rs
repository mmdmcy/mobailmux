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
use crate::agent_codex_args_for_command;
use crate::agent_run_settings_label;
use crate::agent_session;
use crate::append_agent_assistant;
use crate::apply_agent_run_settings;
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
    append_agent_assistant(
        &state,
        slot_id,
        &format!(
            "{} started in `{}`{}.",
            slot.name,
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
    let out_path = env::temp_dir().join(format!(
        "mobailmux-agent-{}-{}.txt",
        slot_id,
        Uuid::new_v4().simple()
    ));
    let mut command = TokioCommand::new(&state.config.agent_codex_bin);
    command.arg("exec");
    if use_resume {
        command.arg("resume").arg("--json");
    } else {
        command.arg("--json");
    }
    command.args(agent_codex_args_for_command(&state.config));
    apply_agent_run_settings(&mut command, &settings);
    if use_resume {
        if let Some((thread_id, _)) = session {
            command
                .arg("--output-last-message")
                .arg(&out_path)
                .arg(thread_id)
                .arg(prompt);
        }
    } else {
        command
            .arg("--cd")
            .arg(&slot.workdir)
            .arg("--output-last-message")
            .arg(&out_path)
            .arg(prompt);
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
                &format!("Could not start `{}`: {err}", state.config.agent_codex_bin),
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
        let _ = tokio::fs::remove_file(&out_path).await;
        return;
    }
    match status {
        Ok(status) if status.success() => {
            let final_text = tokio::fs::read_to_string(&out_path)
                .await
                .unwrap_or_default()
                .trim()
                .to_string();
            let streamed_final = stdout_summary
                .final_text
                .as_deref()
                .or(stdout_summary.last_assistant_text.as_deref())
                .is_some_and(|streamed| streamed.trim() == final_text.trim());
            if final_text.is_empty() {
                if stdout_summary.last_assistant_text.is_none() {
                    append_agent_assistant(
                        &state,
                        slot_id,
                        "(Codex completed without a final message.)",
                    );
                }
            } else if !streamed_final {
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
    let _ = tokio::fs::remove_file(&out_path).await;
}

async fn read_agent_stdout(
    state: Arc<AppState>,
    slot_id: i64,
    workdir: String,
    stdout: tokio::process::ChildStdout,
) -> AgentStdoutSummary {
    let mut lines = BufReader::new(stdout).lines();
    let mut summary = AgentStdoutSummary::default();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let event_type = event
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if event_type == "thread.started"
            && let Some(thread_id) = event.get("thread_id").and_then(|value| value.as_str())
        {
            let db = state.db.lock().unwrap();
            let _ = set_agent_session(&db, slot_id, thread_id, &workdir);
            continue;
        }
        if let Some((message, final_answer)) = codex_stdout_agent_message(&event) {
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
                append_agent_assistant(&state, slot_id, &message);
                summary.last_assistant_text = Some(message.clone());
                if final_answer {
                    summary.final_text = Some(message);
                }
            }
            continue;
        }
        if !matches!(event_type, "item.started" | "item.completed") {
            continue;
        }
        let item = event.get("item").unwrap_or(&serde_json::Value::Null);
        if item.get("type").and_then(|value| value.as_str()) != Some("command_execution") {
            continue;
        }
        let command = item
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or("(unknown command)");
        if event_type == "item.started" {
            state
                .agent_jobs
                .lock()
                .unwrap()
                .entry(slot_id)
                .and_modify(|run| {
                    run.status = "running".into();
                    run.current = truncate_text(command, 160);
                });
            append_agent_assistant(
                &state,
                slot_id,
                &format!("running: `{}`", truncate_text(command, 700)),
            );
        } else {
            state
                .agent_jobs
                .lock()
                .unwrap()
                .entry(slot_id)
                .and_modify(|run| {
                    run.current.clear();
                });
            let exit_code = item.get("exit_code").and_then(|value| value.as_i64());
            if exit_code.is_some_and(|code| code != 0) {
                let output = item
                    .get("aggregated_output")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                append_agent_assistant(
                    &state,
                    slot_id,
                    &format!(
                        "command exit {}: `{}`\n```text\n{}\n```",
                        exit_code.unwrap_or(-1),
                        truncate_text(command, 500),
                        truncate_text(output, 1200)
                    ),
                );
            }
        }
    }
    summary
}

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

use crate::AgentRunSettings;
use crate::AgentSlotRow;
use crate::AppState;
use crate::Arc;
use crate::CodexModel;
use crate::MAX_AGENT_GOAL_CHARS;
use crate::Path;
use crate::TokioCommand;
use crate::agent_command_label;
use crate::agent_control_text;
use crate::agent_slot_runtime;
use crate::append_agent_assistant;
use crate::codex_model_catalog_snapshot;
use crate::codex_usage_text;
use crate::command_arg;
use crate::list_agent_slots;
use crate::normalize_agent_command_text;
use crate::refresh_codex_index_blocking;
use crate::reset_agent_slot_chat;
use crate::set_agent_goal;
use crate::truncate_text;

pub(crate) fn handle_agent_control(state: &Arc<AppState>, slot: &AgentSlotRow, body: &str) -> bool {
    let trimmed = body.trim();
    let Some((prefix, raw_text)) = agent_control_text(trimmed) else {
        return false;
    };
    let text = normalize_agent_command_text(raw_text);
    let lower = text.to_ascii_lowercase();
    if matches!(lower.as_str(), "help" | "commands") {
        append_agent_assistant(state, slot.id, &agent_help_text(state));
        return true;
    }
    if let Some(arg) = command_arg(&text, "goal") {
        let goal = arg.trim();
        if goal.is_empty() {
            let current = slot.goal.trim();
            let response = if current.is_empty() {
                "No goal is set for this slot. Use `/goal <objective>` to set one.".to_string()
            } else {
                format!(
                    "Current goal:\n```text\n{}\n```",
                    truncate_text(current, 2000)
                )
            };
            append_agent_assistant(state, slot.id, &response);
            return true;
        }
        if matches!(
            goal.to_ascii_lowercase().as_str(),
            "clear" | "none" | "unset" | "off"
        ) {
            let db = state.db.lock().unwrap();
            let _ = set_agent_goal(&db, slot.id, "");
            drop(db);
            append_agent_assistant(state, slot.id, "Goal cleared for this slot.");
            return true;
        }
        let goal = truncate_text(goal, MAX_AGENT_GOAL_CHARS);
        let db = state.db.lock().unwrap();
        let _ = set_agent_goal(&db, slot.id, &goal);
        drop(db);
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "Goal set for this slot. Future Codex messages will include:\n```text\n{goal}\n```"
            ),
        );
        return true;
    }
    if lower == "clear-goal" {
        let db = state.db.lock().unwrap();
        let _ = set_agent_goal(&db, slot.id, "");
        drop(db);
        append_agent_assistant(state, slot.id, "Goal cleared for this slot.");
        return true;
    }
    if matches!(lower.as_str(), "slots" | "list" | "overview") {
        append_agent_assistant(state, slot.id, &agent_slots_status_text(state));
        return true;
    }
    if lower == "status" {
        let runtime = agent_slot_runtime(state, slot);
        let usage = refresh_codex_index_blocking(state).usage;
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{} is {}.\n{}",
                slot.name,
                runtime.label,
                codex_usage_text(usage.as_ref())
            ),
        );
        return true;
    }
    if lower == "usage" || lower == "limits" {
        let usage = refresh_codex_index_blocking(state).usage;
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{}\n\nOpen the 📊 Usage panel to refresh again or use a reset credit manually.",
                codex_usage_text(usage.as_ref())
            ),
        );
        return true;
    }
    if lower == "model" || lower == "settings" {
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "Choose a model and thinking difficulty in the composer. Agent command: `{}`",
                agent_command_label(&state.config)
            ),
        );
        return true;
    }
    if matches!(lower.as_str(), "fresh" | "new") {
        let workdir = slot.workdir.clone();
        let stopped = reset_agent_slot_chat(state, slot.id, Path::new(&workdir));
        let stop_text = if stopped {
            " Stopped the current job."
        } else {
            ""
        };
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{} chat reset.{stop_text} Your next message starts a new agent chat.",
                slot.name
            ),
        );
        return true;
    }
    if lower == "stop" {
        let stopped = stop_agent_job(state, slot.id);
        let message = if stopped {
            "Stop requested."
        } else {
            "This slot is not running."
        };
        append_agent_assistant(state, slot.id, message);
        return true;
    }
    append_agent_assistant(
        state,
        slot.id,
        &format!("Unknown command: `{prefix}{raw_text}`. Type `/help` for commands."),
    );
    true
}

pub(crate) fn stop_agent_job(state: &AppState, slot_id: i64) -> bool {
    let cancel = state.agent_cancels.lock().unwrap().remove(&slot_id);
    if let Some(cancel) = cancel {
        let _ = cancel.send(());
        true
    } else {
        false
    }
}

pub(crate) fn requested_agent_run_settings(
    state: &Arc<AppState>,
    requested_model: &str,
    requested_reasoning_effort: &str,
) -> AgentRunSettings {
    let models = codex_model_catalog_snapshot(state);
    validate_agent_run_settings(&models, requested_model, requested_reasoning_effort)
}

pub(crate) fn validate_agent_run_settings(
    models: &[CodexModel],
    requested_model: &str,
    requested_reasoning_effort: &str,
) -> AgentRunSettings {
    let requested_model = requested_model.trim();
    let Some(model) = models.iter().find(|model| model.model == requested_model) else {
        return AgentRunSettings::default();
    };
    let requested_reasoning_effort = requested_reasoning_effort.trim();
    let reasoning_effort = model
        .supported_reasoning_efforts
        .iter()
        .any(|option| option.effort == requested_reasoning_effort)
        .then(|| requested_reasoning_effort.to_string());
    AgentRunSettings {
        model: Some(model.model.clone()),
        reasoning_effort,
    }
}

pub(crate) fn agent_run_settings_label(settings: &AgentRunSettings) -> String {
    let mut parts = Vec::new();
    if let Some(model) = &settings.model {
        parts.push(format!("model `{model}`"));
    }
    if let Some(effort) = &settings.reasoning_effort {
        parts.push(format!("thinking `{effort}`"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

pub(crate) fn apply_agent_run_settings(command: &mut TokioCommand, settings: &AgentRunSettings) {
    if let Some(model) = &settings.model {
        command.arg("--model").arg(model);
    }
    if let Some(effort) = &settings.reasoning_effort {
        let value = serde_json::to_string(effort).unwrap_or_else(|_| "\"medium\"".into());
        command
            .arg("--config")
            .arg(format!("model_reasoning_effort={value}"));
    }
}

pub(crate) fn agent_help_text(state: &AppState) -> String {
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let mut lines = vec![
        "Agent commands:".to_string(),
        "Use `/` in the web chatbox. The old `!` prefix still works.".to_string(),
    ];
    lines.extend(
        [
            "- `/goal <objective>` sets a goal that is included in future Codex prompts",
            "- `/goal` shows the current goal",
            "- `/goal clear` or `/clear-goal` clears it",
            "- `/slots`",
            "- `/fresh`",
            "- `/status`",
            "- `/usage`",
            "- `/stop`",
            "- `/model`",
            "- `/help` or `/commands`",
        ]
        .into_iter()
        .map(str::to_string),
    );
    lines.push(String::new());
    lines.push(format!("{} slots configured.", slots.len()));
    lines.join("\n")
}

pub(crate) fn agent_slots_status_text(state: &AppState) -> String {
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let jobs = state.agent_jobs.lock().unwrap();
    let lines = slots
        .iter()
        .map(|slot| {
            let status = if jobs.contains_key(&slot.id) {
                "running"
            } else {
                "idle"
            };
            let goal = slot.goal.trim();
            if goal.is_empty() {
                format!("{}: {status}", slot.name)
            } else {
                format!(
                    "{}: {status} | goal: {}",
                    slot.name,
                    truncate_text(goal, 120)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Slots:\n```text\n{lines}\n```")
}

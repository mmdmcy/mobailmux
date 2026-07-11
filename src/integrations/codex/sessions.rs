use crate::AgentMessageRow;
use crate::CODEX_SESSION_SCAN_LIMIT;
use crate::CodexConversation;
use crate::CodexIndex;
use crate::CodexUsageSnapshot;
use crate::CodexVisibleMessage;
use crate::Config;
use crate::DateTime;
use crate::HashMap;
use crate::MAX_CODEX_CONVERSATIONS;
use crate::MAX_CODEX_TRANSCRIPT_MESSAGES;
use crate::Path;
use crate::PathBuf;
use crate::Reverse;
use crate::codex_usage_from_payload;
use crate::fetch_codex_app_server_dashboard;
use crate::file_modified;
use crate::fs;
use crate::io;
use crate::is_final_agent_phase;
use crate::merge_codex_rate_limit_status;
use crate::system_time_to_rfc3339;
use crate::truncate_text;
use std::io::BufRead;

pub(crate) fn load_codex_index(config: &Config) -> CodexIndex {
    let thread_names = load_codex_thread_names(&config.codex_home);
    let mut files = collect_codex_session_files(&config.codex_home);
    files.sort_by_key(|path| Reverse(file_modified(path)));
    files.truncate(CODEX_SESSION_SCAN_LIMIT);

    let mut conversations = files
        .iter()
        .filter_map(|path| codex_conversation_from_file(path, &thread_names))
        .collect::<Vec<_>>();
    conversations.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    conversations.truncate(MAX_CODEX_CONVERSATIONS);

    let dashboard = fetch_codex_app_server_dashboard(config);
    let usage =
        merge_codex_rate_limit_status(latest_codex_usage(&files), dashboard.rate_limits.as_ref());

    CodexIndex {
        usage,
        conversations,
    }
}

pub(crate) fn load_codex_thread_names(codex_home: &Path) -> HashMap<String, (String, String)> {
    let path = codex_home.join("session_index.jsonl");
    let Ok(file) = fs::File::open(path) else {
        return HashMap::new();
    };
    let reader = io::BufReader::new(file);
    let mut names = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        let title = value
            .get("thread_name")
            .and_then(|value| value.as_str())
            .unwrap_or("Untitled")
            .to_string();
        let updated_at = value
            .get("updated_at")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        names.insert(id.to_string(), (title, updated_at));
    }
    names
}

pub(crate) fn collect_codex_session_files(codex_home: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_files(&codex_home.join("sessions"), 5, &mut files);
    let index = codex_home.join("history.jsonl");
    if index.exists() {
        files.push(index);
    }
    files
}

pub(crate) fn collect_jsonl_files(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth == 0 || dir.to_string_lossy().contains("/.tmp/") {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, depth - 1, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

pub(crate) fn codex_conversation_from_file(
    path: &Path,
    thread_names: &HashMap<String, (String, String)>,
) -> Option<CodexConversation> {
    if path.file_name().and_then(|name| name.to_str()) == Some("history.jsonl") {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let reader = io::BufReader::new(file);
    let mut id = None::<String>;
    let mut started_at = None::<String>;
    let mut updated_at = None::<String>;
    let mut first_user = None::<String>;

    let mut visible_messages = Vec::new();
    for (order, line) in reader.lines().map_while(Result::ok).enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(timestamp) = value.get("timestamp").and_then(|value| value.as_str()) {
            updated_at = Some(timestamp.to_string());
        }
        if value.get("type").and_then(|value| value.as_str()) == Some("session_meta") {
            let payload = value.get("payload").unwrap_or(&serde_json::Value::Null);
            id = payload
                .get("id")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            started_at = payload
                .get("timestamp")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            continue;
        }
        if let Some(message) = codex_visible_message_event(&value, order)
            && !message.text.trim().is_empty()
        {
            visible_messages.push(message);
        }
    }

    for message in dedupe_codex_visible_messages(visible_messages) {
        if message.role == "user" && first_user.is_none() {
            first_user = Some(message.text.clone());
        }
    }

    let id = id?;
    let (indexed_title, indexed_updated) = thread_names
        .get(&id)
        .cloned()
        .unwrap_or_else(|| (String::new(), String::new()));
    let indexed_title = indexed_title.trim();
    let title = if indexed_title.is_empty() || is_codex_synthetic_user_text(indexed_title) {
        first_user
            .as_deref()
            .map(|value| truncate_text(value, 56))
            .unwrap_or_else(|| "Untitled Codex conversation".into())
    } else {
        indexed_title.to_string()
    };
    let updated_at = if !indexed_updated.trim().is_empty() {
        indexed_updated
    } else {
        updated_at
            .or_else(|| started_at.clone())
            .unwrap_or_else(|| system_time_to_rfc3339(file_modified(path)))
    };
    Some(CodexConversation {
        id,
        title,
        updated_at,
        path: path.to_path_buf(),
    })
}

pub(crate) fn codex_transcript_messages(path: &Path) -> io::Result<Vec<AgentMessageRow>> {
    let file = fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut messages = Vec::new();
    for (order, line) in reader.lines().map_while(Result::ok).enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(message) = codex_visible_message_event(&value, order)
            && !message.text.trim().is_empty()
        {
            messages.push(message);
        }
    }
    let mut visible_messages = dedupe_codex_visible_messages(messages);
    if codex_transcript_interrupted(&visible_messages) {
        let timestamp = visible_messages
            .last()
            .map(|message| message.timestamp.clone())
            .unwrap_or_default();
        visible_messages.push(CodexVisibleMessage {
            role: "assistant".into(),
            text: "This saved Codex transcript ended before Codex returned a final answer. The Mobailmux service or Codex process was likely interrupted; send a new message in the agent slot to continue.".into(),
            timestamp,
            order: usize::MAX,
            fallback: false,
            final_answer: false,
            assistant_progress: false,
        });
    }
    let mut rows = visible_messages
        .into_iter()
        .map(|message| AgentMessageRow {
            id: 0,
            role: message.role,
            body: message.text,
            created_at: message.timestamp,
        })
        .collect::<Vec<_>>();
    if rows.len() > MAX_CODEX_TRANSCRIPT_MESSAGES {
        let start = rows.len() - MAX_CODEX_TRANSCRIPT_MESSAGES;
        rows = rows.split_off(start);
    }
    rows.reverse();
    Ok(rows)
}

pub(crate) fn codex_transcript_interrupted(messages: &[CodexVisibleMessage]) -> bool {
    messages.iter().any(|message| message.assistant_progress)
        && !messages.iter().any(|message| message.final_answer)
}

pub(crate) fn codex_visible_message_event(
    value: &serde_json::Value,
    order: usize,
) -> Option<CodexVisibleMessage> {
    let timestamp = value
        .get("timestamp")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    if let Some((role, text, final_answer, assistant_progress)) = codex_visible_message(value) {
        return Some(CodexVisibleMessage {
            role,
            text,
            timestamp,
            order,
            fallback: false,
            final_answer,
            assistant_progress,
        });
    }
    let (role, text, final_answer, assistant_progress) = codex_event_visible_message(value)?;
    Some(CodexVisibleMessage {
        role,
        text,
        timestamp,
        order,
        fallback: true,
        final_answer,
        assistant_progress,
    })
}

pub(crate) fn codex_visible_message(
    value: &serde_json::Value,
) -> Option<(String, String, bool, bool)> {
    if value.get("type").and_then(|value| value.as_str()) != Some("response_item") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(|value| value.as_str()) != Some("message") {
        return None;
    }
    let role = payload.get("role").and_then(|value| value.as_str())?;
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let text = codex_content_text(payload.get("content")?);
    if role == "user" && is_codex_synthetic_user_text(&text) {
        return None;
    }
    let phase = payload.get("phase").and_then(|value| value.as_str());
    let final_answer = phase.is_some_and(is_final_agent_phase);
    let assistant_progress =
        role == "assistant" && phase.is_some_and(|value| !is_final_agent_phase(value));
    Some((role.to_string(), text, final_answer, assistant_progress))
}

pub(crate) fn codex_event_visible_message(
    value: &serde_json::Value,
) -> Option<(String, String, bool, bool)> {
    let event_type = value.get("type").and_then(|value| value.as_str())?;
    let payload = if event_type == "event_msg" {
        value.get("payload")?
    } else {
        value
    };
    let payload_type = payload.get("type").and_then(|value| value.as_str())?;
    let role = match payload_type {
        "user_message" => "user",
        "agent_message" => "assistant",
        _ => return None,
    };
    let text = payload
        .get("message")
        .or_else(|| payload.get("text"))
        .and_then(|value| value.as_str())?
        .to_string();
    if role == "user" && is_codex_synthetic_user_text(&text) {
        return None;
    }
    let phase = payload.get("phase").and_then(|value| value.as_str());
    let final_answer = phase.is_some_and(is_final_agent_phase);
    let assistant_progress =
        role == "assistant" && phase.is_some_and(|value| !is_final_agent_phase(value));
    Some((role.to_string(), text, final_answer, assistant_progress))
}

pub(crate) fn dedupe_codex_visible_messages(
    messages: Vec<CodexVisibleMessage>,
) -> Vec<CodexVisibleMessage> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            if message.fallback
                && messages.iter().enumerate().any(|(other_index, other)| {
                    other_index != index
                        && !other.fallback
                        && same_codex_visible_message_near(message, other)
                })
            {
                None
            } else {
                Some(message.clone())
            }
        })
        .collect()
}

pub(crate) fn same_codex_visible_message_near(
    left: &CodexVisibleMessage,
    right: &CodexVisibleMessage,
) -> bool {
    if left.role != right.role || left.text.trim() != right.text.trim() {
        return false;
    }
    match (
        codex_timestamp_seconds(&left.timestamp),
        codex_timestamp_seconds(&right.timestamp),
    ) {
        (Some(left), Some(right)) => (left - right).abs() <= 2,
        _ => left.order.abs_diff(right.order) <= 2,
    }
}

pub(crate) fn codex_timestamp_seconds(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp())
}

pub(crate) fn is_codex_synthetic_user_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("# AGENTS.md instructions for")
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.starts_with("<permissions instructions>")
        || trimmed.starts_with("<collaboration_mode>")
        || trimmed.starts_with("<apps_instructions>")
        || trimmed.starts_with("<skills_instructions>")
        || trimmed.starts_with("<plugins_instructions>")
        || (trimmed.contains("<environment_context>") && trimmed.contains("<cwd>"))
}

pub(crate) fn codex_content_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let Some(items) = content.as_array() else {
        return String::new();
    };
    items
        .iter()
        .filter_map(|item| {
            item.get("text")
                .or_else(|| item.get("content"))
                .and_then(|value| value.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn latest_codex_usage(files: &[PathBuf]) -> Option<CodexUsageSnapshot> {
    let mut latest = None::<(String, CodexUsageSnapshot)>;
    for path in files.iter().take(CODEX_SESSION_SCAN_LIMIT) {
        let Ok(file) = fs::File::open(path) else {
            continue;
        };
        let reader = io::BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value.get("type").and_then(|value| value.as_str()) != Some("event_msg") {
                continue;
            }
            let payload = value.get("payload").unwrap_or(&serde_json::Value::Null);
            if payload.get("type").and_then(|value| value.as_str()) != Some("token_count") {
                continue;
            }
            let observed_at = value
                .get("timestamp")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let snapshot = codex_usage_from_payload(&observed_at, payload);
            if latest
                .as_ref()
                .is_none_or(|(timestamp, _)| observed_at.as_str() > timestamp.as_str())
            {
                latest = Some((observed_at, snapshot));
            }
        }
    }
    latest.map(|(_, snapshot)| snapshot)
}

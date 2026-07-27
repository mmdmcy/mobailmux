use crate::AgentMessageRow;
use crate::AgentRun;
use crate::AgentSlotRow;
use crate::AgentSlotSummary;
use crate::AppState;
use crate::CodeBlockKind;
#[cfg(test)]
use crate::CodexIndex;
use crate::CowStr;
use crate::Event;
use crate::Options;
use crate::Parser;
use crate::Sha256;
use crate::SlotRuntime;
use crate::Tag;
use crate::TagEnd;
#[cfg(test)]
use crate::codex_conversation_by_id;
#[cfg(test)]
use crate::codex_transcript_messages;
use crate::compact_local_time;
use crate::html;
use crate::html_attr_escape;
use crate::html_escape;
#[cfg(test)]
use crate::io;
use crate::truncate_text;
use sha2::Digest;

pub(crate) fn agent_run_for(state: &AppState, slot_id: i64) -> Option<AgentRun> {
    state.agent_jobs.lock().unwrap().get(&slot_id).cloned()
}

pub(crate) fn agent_slot_summary(state: &AppState, slot: &AgentSlotRow) -> AgentSlotSummary {
    if let Some(run) = agent_run_for(state, slot.id) {
        let label = if run.current.trim().is_empty() {
            run.status.clone()
        } else {
            run.current.clone()
        };
        return AgentSlotSummary {
            id: slot.id,
            name: slot.name.clone(),
            running: true,
            current: label.clone(),
            status: label,
            harness: slot.harness,
        };
    }
    AgentSlotSummary {
        id: slot.id,
        name: slot.name.clone(),
        running: false,
        current: String::new(),
        status: "idle".into(),
        harness: slot.harness,
    }
}

pub(crate) fn agent_slot_runtime(state: &AppState, slot: &AgentSlotRow) -> SlotRuntime {
    if let Some(run) = agent_run_for(state, slot.id) {
        let label = if run.current.trim().is_empty() {
            run.status
        } else {
            run.current
        };
        return SlotRuntime { label };
    }
    SlotRuntime {
        label: "idle".into(),
    }
}

pub(crate) fn agent_slot_rail_html(
    state: &AppState,
    slots: &[AgentSlotRow],
    active_id: i64,
) -> String {
    let rows = slots.iter().map(|slot| {
        let summary = agent_slot_summary(state, slot);
        let active_class = if slot.id == active_id { " active" } else { "" };
        let running_class = if summary.running { " running" } else { "" };
        format!(r#"<div class="channel-row{active_class}{running_class}" data-slot-row data-slot-id="{}" data-slot-running="{}"><a class="channel-link" href="/agents?slot={}" aria-label="Open {}"><strong>{}</strong><span data-slot-status>{} · {}</span><span class="slot-badge" data-slot-badge hidden></span></a></div>"#, summary.id, summary.running, summary.id, html_escape(&summary.name), html_escape(&summary.name), html_escape(summary.harness.as_str()), html_escape(&summary.status))
    }).collect::<Vec<_>>().join("");
    format!(
        r#"<aside class="channel-rail" aria-label="Agent projects"><div class="rail-title">Projects</div><div class="channel-list">{rows}</div></aside>"#
    )
}

pub(crate) fn agent_messages_html(messages: &[AgentMessageRow]) -> String {
    if messages.is_empty() {
        return r#"<p class="empty">No messages in this slot yet.</p>"#.into();
    }
    let ordered = messages.iter().rev().collect::<Vec<_>>();
    let mut rendered = String::new();
    let mut index = 0usize;
    while index < ordered.len() {
        if agent_activity_kind(ordered[index]).is_some() {
            let start = index;
            while index < ordered.len() && agent_activity_kind(ordered[index]).is_some() {
                index += 1;
            }
            rendered.push_str(&agent_activity_stack_html(&ordered[start..index]));
        } else {
            rendered.push_str(&agent_message_html(ordered[index]));
            index += 1;
        }
    }
    rendered
}

#[derive(Copy, Clone)]
pub(crate) enum AgentActivityKind {
    Start,
    Run,
    Exit,
}

pub(crate) fn agent_activity_kind(message: &AgentMessageRow) -> Option<AgentActivityKind> {
    if message.role != "assistant" {
        return None;
    }
    let body = message.body.trim();
    if body.starts_with("running: `") {
        return Some(AgentActivityKind::Run);
    }
    if body.starts_with("command exit ") {
        return Some(AgentActivityKind::Exit);
    }
    if body.contains(" started in `") && body.ends_with("`.") {
        return Some(AgentActivityKind::Start);
    }
    None
}

pub(crate) fn agent_message_html(message: &AgentMessageRow) -> String {
    let role_class = agent_role_class(&message.role);
    let avatar = if role_class == "user" { "U" } else { "A" };
    let actions = if role_class == "user" && message.id > 0 {
        format!(
            r#"<button type="button" class="message-edit" data-edit-message="{}" data-edit-body="{}">Edit</button>"#,
            message.id,
            html_attr_escape(&message.body)
        )
    } else {
        String::new()
    };
    let body = message_body_html(&message.body);
    format!(
        r#"<article class="message message-{role_class}" data-message-entry>
  <div class="message-avatar">{avatar}</div>
  <div class="message-body">
    <div class="message-meta"><strong>{}</strong><span class="message-log">{}</span>{actions}</div>
    {}
  </div>
</article>"#,
        html_escape(agent_role_label(&message.role)),
        html_escape(&compact_local_time(&message.created_at)),
        body
    )
}

pub(crate) fn message_body_html(body: &str) -> String {
    if body.trim().is_empty() {
        return String::new();
    }
    let normalized = normalize_markdown_fences(body);
    let parser = Parser::new_ext(&normalized, markdown_options()).map(markdown_event);
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    format!(r#"<div class="message-content">{rendered}</div>"#)
}

pub(crate) fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options
}

pub(crate) fn markdown_event(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(Tag::CodeBlock(kind)) => Event::Html(CowStr::from(code_block_open_html(kind))),
        Event::End(TagEnd::CodeBlock) => Event::Html(CowStr::Borrowed("</code></pre></div>")),
        Event::Html(value) | Event::InlineHtml(value) => Event::Text(value),
        _ => event,
    }
}

pub(crate) fn code_block_open_html(kind: CodeBlockKind<'_>) -> String {
    let language = match kind {
        CodeBlockKind::Fenced(info) => info
            .split_whitespace()
            .next()
            .map(|value| {
                value
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+'))
                    .collect::<String>()
            })
            .unwrap_or_default(),
        CodeBlockKind::Indented => String::new(),
    };
    let label = if language.is_empty() {
        "code"
    } else {
        language.as_str()
    };
    let class_attr = if language.is_empty() {
        String::new()
    } else {
        format!(r#" class="language-{}""#, html_attr_escape(&language))
    };
    format!(
        r#"<div class="message-code"><div class="message-code-head"><span>{}</span><button type="button" class="message-copy" data-copy-code>Copy</button></div><pre><code{}>"#,
        html_escape(label),
        class_attr
    )
}

pub(crate) fn normalize_markdown_fences(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut changed = false;
    for line in value.split_inclusive('\n') {
        let without_spaces = line.trim_start_matches(' ');
        let space_count = line.len().saturating_sub(without_spaces.len());
        let quote_count = without_spaces.chars().take_while(|ch| *ch == '\'').count();
        if space_count <= 3 && quote_count >= 3 {
            normalized.extend(std::iter::repeat_n(' ', space_count));
            normalized.push_str("```");
            normalized.push_str(&without_spaces[quote_count..]);
            changed = true;
        } else {
            normalized.push_str(line);
        }
    }
    if changed {
        normalized
    } else {
        value.to_string()
    }
}

pub(crate) fn agent_role_class(role: &str) -> &'static str {
    if role == "user" { "user" } else { "assistant" }
}

pub(crate) fn agent_role_label(role: &str) -> &str {
    if role == "user" { "You" } else { "Agent" }
}

pub(crate) fn agent_activity_stack_html(messages: &[&AgentMessageRow]) -> String {
    let rows = messages
        .iter()
        .enumerate()
        .map(|(index, message)| agent_activity_row_html(index + 1, message))
        .collect::<Vec<_>>()
        .join("");
    let event_count = messages.len();
    let event_label = if event_count == 1 {
        "1 event".to_string()
    } else {
        format!("{event_count} events")
    };
    let preview = messages
        .iter()
        .find_map(|message| agent_activity_preview(message))
        .unwrap_or_else(|| "Agent command activity".into());
    let started_at = messages
        .first()
        .map(|message| html_escape(&compact_local_time(&message.created_at)))
        .unwrap_or_default();
    let fold_key = html_attr_escape(&agent_activity_fold_key(messages));
    format!(
        r#"<article class="message message-activity" data-message-entry>
  <div class="message-avatar">$</div>
  <div class="message-body">
    <div class="message-meta"><strong>Activity</strong><span class="message-log">{started_at}</span></div>
    <details class="tool-fold" data-fold-key="{fold_key}">
      <summary><span>{}</span><code>{}</code></summary>
      <ol class="tool-stack" aria-label="Agent command activity">{rows}</ol>
    </details>
  </div>
 </article>"#,
        html_escape(&event_label),
        html_escape(&truncate_text(&preview, 140))
    )
}

pub(crate) fn agent_activity_fold_key(messages: &[&AgentMessageRow]) -> String {
    messages
        .first()
        .map(|message| message_fold_key("activity", message))
        .unwrap_or_else(|| "activity-empty".into())
}

pub(crate) fn message_fold_key(prefix: &str, message: &AgentMessageRow) -> String {
    if message.id > 0 {
        return format!("{prefix}-{}", message.id);
    }
    let mut hasher = Sha256::new();
    hasher.update(message.created_at.as_bytes());
    hasher.update([0]);
    hasher.update(message.role.as_bytes());
    hasher.update([0]);
    hasher.update(message.body.as_bytes());
    let digest = hasher.finalize();
    format!("{prefix}-{}", hex::encode(&digest[..8]))
}

pub(crate) fn agent_activity_preview(message: &AgentMessageRow) -> Option<String> {
    let body = message.body.trim();
    match agent_activity_kind(message)? {
        AgentActivityKind::Start => Some(body.to_string()),
        AgentActivityKind::Run => first_backtick_text(body)
            .or_else(|| body.strip_prefix("running:"))
            .map(|value| value.trim().to_string()),
        AgentActivityKind::Exit => {
            let exit_code = body
                .strip_prefix("command exit ")
                .and_then(|rest| rest.split_once(':').map(|(code, _)| code.trim()))
                .unwrap_or("?");
            let command = first_backtick_text(body).unwrap_or("(unknown command)");
            Some(format!("exit {exit_code}: {command}"))
        }
    }
}

pub(crate) fn agent_activity_row_html(index: usize, message: &AgentMessageRow) -> String {
    let Some(kind) = agent_activity_kind(message) else {
        return String::new();
    };
    let body = message.body.trim();
    let number = format!("{index:02}");
    match kind {
        AgentActivityKind::Start => format!(
            r#"<li class="tool-row tool-row-start"><span class="tool-index">{number}</span><span class="tool-label">start</span><code>{}</code></li>"#,
            html_escape(body)
        ),
        AgentActivityKind::Run => {
            let command = first_backtick_text(body)
                .or_else(|| body.strip_prefix("running:"))
                .unwrap_or(body)
                .trim();
            format!(
                r#"<li class="tool-row tool-row-run"><span class="tool-index">{number}</span><span class="tool-label">run</span><code>{}</code></li>"#,
                html_escape(command)
            )
        }
        AgentActivityKind::Exit => {
            let exit_code = body
                .strip_prefix("command exit ")
                .and_then(|rest| rest.split_once(':').map(|(code, _)| code.trim()))
                .unwrap_or("?");
            let command = first_backtick_text(body).unwrap_or("(unknown command)");
            let output = fenced_text(body);
            let output_html = if output.trim().is_empty() {
                String::new()
            } else {
                let fold_key = html_attr_escape(&message_fold_key("output", message));
                format!(
                    r#"<details class="tool-output" data-fold-key="{fold_key}"><summary>output</summary><pre>{}</pre></details>"#,
                    html_escape(output.trim())
                )
            };
            format!(
                r#"<li class="tool-row tool-row-exit"><span class="tool-index">{number}</span><span class="tool-label">exit {}</span><code>{}</code>{output_html}</li>"#,
                html_escape(exit_code),
                html_escape(command)
            )
        }
    }
}

pub(crate) fn first_backtick_text(text: &str) -> Option<&str> {
    let start = text.find('`')? + 1;
    let end = text[start..].find('`')?;
    Some(&text[start..start + end])
}

pub(crate) fn fenced_text(text: &str) -> &str {
    text.split_once("```text\n")
        .map(|(_, output)| output.strip_suffix("\n```").unwrap_or(output))
        .unwrap_or("")
}

#[cfg(test)]
pub(crate) fn codex_transcript_html(index: &CodexIndex, thread_id: &str) -> io::Result<String> {
    let Some(conversation) = codex_conversation_by_id(index, thread_id) else {
        return Ok(r#"<p class="empty">Conversation not found.</p>"#.into());
    };
    let messages = codex_transcript_messages(&conversation.path)?;
    if messages.is_empty() {
        return Ok(r#"<p class="empty">This Codex conversation has no visible user or assistant messages yet.</p>"#.into());
    }
    Ok(agent_messages_html(&messages))
}

#[cfg(test)]
pub(crate) fn codex_transcript_count(html: &str) -> Option<usize> {
    let count = html.matches("data-message-entry").count();
    (count > 0).then_some(count)
}

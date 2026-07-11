use crate::AppState;
use crate::Arc;
use crate::CodexIndex;
use crate::HeaderMap;
use crate::MAX_AGENT_MESSAGE_CHARS;
use crate::Query;
use crate::Response;
use crate::State;
use crate::StatusCode;
use crate::agent_composer_suggestions_json;
use crate::agent_execution_mode_html;
use crate::agent_messages_html;
use crate::agent_run_for;
use crate::agent_slot_rail_html;
use crate::agent_slot_runtime;
use crate::codex_conversation_by_id;
use crate::codex_index_snapshot;
use crate::codex_model_catalog_snapshot;
use crate::codex_transcript_count;
use crate::codex_transcript_html;
use crate::codex_usage_dialog;
use crate::html_attr_escape;
use crate::html_escape;
use crate::json_for_inline_script;
use crate::list_agent_messages;
use crate::list_agent_slots;
use crate::page;
use crate::page_guard;
use crate::refresh_codex_index_blocking;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct AgentsQuery {
    slot: Option<i64>,
    thread: Option<String>,
    refresh: Option<String>,
    usage: Option<String>,
}

pub(crate) async fn agents_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AgentsQuery>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = page_guard(&state, &headers) {
        return response;
    }
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let active_slot = slots
        .iter()
        .find(|slot| Some(slot.id) == query.slot)
        .or_else(|| slots.first());
    let Some(active_slot) = active_slot else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "missing agent slot").into_response();
    };
    let mut codex_snapshot = if query.refresh.is_some() {
        Some(refresh_codex_index_blocking(&state))
    } else {
        codex_index_snapshot(&state)
    };
    let selected_thread = query.thread.clone();
    if selected_thread.as_ref().is_some_and(|thread_id| {
        codex_snapshot
            .as_ref()
            .is_none_or(|index| codex_conversation_by_id(index, thread_id).is_none())
    }) {
        codex_snapshot = Some(refresh_codex_index_blocking(&state));
    }
    let codex_loaded = codex_snapshot.is_some();
    let codex = codex_snapshot.unwrap_or_else(CodexIndex::empty);
    let messages = {
        let db = state.db.lock().unwrap();
        list_agent_messages(&db, active_slot.id).unwrap_or_default()
    };
    let runtime = agent_slot_runtime(&state, active_slot);
    let messages_html = selected_thread
        .as_deref()
        .and_then(|thread_id| codex_transcript_html(&codex, thread_id).ok())
        .unwrap_or_else(|| agent_messages_html(&messages));
    let active_title = selected_thread
        .as_deref()
        .and_then(|thread_id| codex_conversation_by_id(&codex, thread_id))
        .map(|conversation| conversation.title.clone())
        .unwrap_or_else(|| "Codex".into());
    let active_title = html_escape(&active_title);
    let message_count = if selected_thread.is_some() {
        codex_transcript_count(&messages_html).unwrap_or(messages.len())
    } else {
        messages.len()
    };
    let slot_rail = agent_slot_rail_html(&state, &slots, active_slot.id);
    let usage_dialog = codex_usage_dialog(
        &state.config,
        codex.usage.as_ref(),
        codex_loaded,
        active_slot.id,
    );
    let terminal_panel =
        crate::features::terminal::panel_html(active_slot.id, &active_slot.workdir);
    let reopen_usage = query.usage.is_some();
    let viewing_transcript = selected_thread.is_some();
    let refresh_thread_input = selected_thread
        .as_deref()
        .map(|thread| {
            format!(
                r#"<input type="hidden" name="thread" value="{}">"#,
                html_attr_escape(thread)
            )
        })
        .unwrap_or_default();
    let composer_suggestions_json = agent_composer_suggestions_json(&state.config);
    let model_catalog_json = json_for_inline_script(&codex_model_catalog_snapshot(&state));
    let execution_mode = agent_execution_mode_html(&state.config);
    let active_running = agent_run_for(&state, active_slot.id).is_some();
    let cancel_disabled = if active_running { "" } else { " disabled" };
    let agent_script = include_str!("agents.js")
        .replace("{composer_suggestions_json}", &composer_suggestions_json)
        .replace("{model_catalog_json}", &model_catalog_json)
        .replace("{viewing_transcript}", &viewing_transcript.to_string())
        .replace("{reopen_usage}", &reopen_usage.to_string());
    page(
        "Agents",
        &format!(
            r##"
<nav><a href="/">Mobailmux</a><div class="nav-actions"><button type="button" class="ghost nav-icon" data-codex-open aria-label="Usage" title="Usage">📊</button><button type="button" class="ghost nav-icon" data-terminal-open aria-label="Terminal" title="Terminal">⌨</button><form action="/agents" method="get" data-refresh-form><input type="hidden" name="slot" value="{}"><input type="hidden" name="refresh" value="1">{refresh_thread_input}<button type="submit" class="ghost nav-icon" aria-label="Refresh" title="Refresh" data-refresh-button>↻</button></form><strong>Agents</strong></div></nav>
<main class="chat-shell agent-shell">
  {slot_rail}
  <section class="chat-pane agent-pane">
    <header class="chat-head">
      <div class="chat-title"><strong>{active_title}</strong></div>
      <div class="chat-stats"><span data-agent-count>{message_count} messages</span><span class="agent-status" data-agent-status>{}</span></div>
    </header>
    <div class="message-list" data-agent-messages>{messages_html}</div>
    <section class="agent-compose-wrap">
      <form action="/agents" method="post" class="agent-composer">
        <input name="slot_id" type="hidden" value="{}">
        <input name="edit_message_id" id="editMessageId" type="hidden" value="">
        <textarea id="agentBody" name="body" maxlength="{MAX_AGENT_MESSAGE_CHARS}" placeholder="Message Codex"></textarea>
        <div class="command-suggestions" id="commandSuggestions" role="listbox" hidden></div>
        <div class="edit-strip" id="editStrip" hidden><span>Editing message</span><button type="button" class="ghost" data-edit-clear>Discard</button></div>
        <div class="agent-settings" data-agent-settings>
          <label class="agent-setting"><span>Model</span><select name="model" data-agent-model aria-label="Codex model" disabled><option>Loading models…</option></select></label>
          <label class="agent-setting"><span>Thinking</span><select name="reasoning_effort" data-agent-reasoning aria-label="Thinking difficulty" disabled><option>Loading…</option></select></label>
          {execution_mode}
        </div>
        <button type="submit" name="control" value="stop" class="ghost cancel-button" data-cancel-button{cancel_disabled}>Cancel</button>
        <button type="submit" data-send-button>Send</button>
      </form>
    </section>
  </section>
</main>
{usage_dialog}
{terminal_panel}
<script>
{agent_script}</script>
"##,
            active_slot.id,
            html_escape(&runtime.label),
            active_slot.id
        ),
    )
}

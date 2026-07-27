use crate::AppState;
use crate::Arc;
use crate::HeaderMap;
use crate::MAX_AGENT_MESSAGE_CHARS;
use crate::MAX_AGENT_SLOT_CHARS;
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
use crate::html_escape;
use crate::list_agent_messages;
use crate::list_agent_slots;
use crate::page;
use crate::page_guard;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct AgentsQuery {
    slot: Option<i64>,
    new_project: Option<String>,
    project_error: Option<String>,
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
    let messages = {
        let db = state.db.lock().unwrap();
        list_agent_messages(&db, active_slot.id).unwrap_or_default()
    };
    let runtime = agent_slot_runtime(&state, active_slot);
    let messages_html = agent_messages_html(&messages);
    let active_title = active_slot.name.clone();
    let active_title = html_escape(&active_title);
    let active_workdir = html_escape(&active_slot.workdir);
    let message_count = messages.len();
    let slot_rail = agent_slot_rail_html(&state, &slots, active_slot.id);
    let terminal_panel =
        crate::features::terminal::panel_html(active_slot.id, &active_slot.workdir);
    let reopen_project = query.new_project.is_some();
    let project_error_html = query
        .project_error
        .as_deref()
        .filter(|error| !error.trim().is_empty())
        .map(|error| format!(r#"<p class="error">{}</p>"#, html_escape(error)))
        .unwrap_or_default();
    let viewing_transcript = false;
    let composer_suggestions_json = agent_composer_suggestions_json(&state.config);
    let model_catalog_json = "[]";
    let execution_mode = agent_execution_mode_html(&state.config, active_slot.harness);
    let pi_selected = if state.config.default_harness.as_str() == "pi" {
        " selected"
    } else {
        ""
    };
    let opencode_selected = if state.config.default_harness.as_str() == "opencode" {
        " selected"
    } else {
        ""
    };
    let active_running = agent_run_for(&state, active_slot.id).is_some();
    let cancel_disabled = if active_running { "" } else { " disabled" };
    let agent_script = include_str!("agents.js")
        .replace("{composer_suggestions_json}", &composer_suggestions_json)
        .replace("{model_catalog_json}", &model_catalog_json)
        .replace("{active_slot_id}", &active_slot.id.to_string())
        .replace("{viewing_transcript}", &viewing_transcript.to_string())
        .replace("{reopen_project}", &reopen_project.to_string());
    page(
        "Agents",
        &format!(
            r##"
<nav><a href="/">Mobailmux</a><div class="nav-actions"><button type="button" class="ghost nav-icon" data-project-open aria-label="Start another project" title="Start another project">＋</button><button type="button" class="ghost nav-icon" data-terminal-open aria-label="Terminal" title="Terminal">⌨</button><form action="/agents" method="get" data-refresh-form><input type="hidden" name="slot" value="{}"><input type="hidden" name="refresh" value="1"><button type="submit" class="ghost nav-icon" aria-label="Refresh" title="Refresh" data-refresh-button>↻</button></form><strong>Agents</strong></div></nav>
<main class="chat-shell agent-shell">
  {slot_rail}
  <section class="chat-pane agent-pane">
    <header class="chat-head">
      <div class="chat-title"><strong>{active_title}</strong><span data-active-cwd>{active_workdir}</span></div>
      <div class="chat-stats"><span data-agent-count>{message_count} messages</span><span class="agent-status" data-agent-status>{}</span></div>
    </header>
    <div class="message-list" data-agent-messages>{messages_html}</div>
    <section class="agent-compose-wrap">
      <form action="/agents" method="post" class="agent-composer">
        <input name="slot_id" type="hidden" value="{}">
        <input name="edit_message_id" id="editMessageId" type="hidden" value="">
        <textarea id="agentBody" name="body" maxlength="{MAX_AGENT_MESSAGE_CHARS}" placeholder="Message {}"></textarea>
        <div class="command-suggestions" id="commandSuggestions" role="listbox" hidden></div>
        <div class="edit-strip" id="editStrip" hidden><span>Editing message</span><button type="button" class="ghost" data-edit-clear>Discard</button></div>
        <div class="agent-settings" data-agent-settings>
          {execution_mode}
        </div>
        <button type="submit" name="control" value="stop" class="ghost cancel-button" data-cancel-button{cancel_disabled}>Cancel</button>
        <button type="submit" data-send-button>Send</button>
      </form>
    </section>
  </section>
</main>
{terminal_panel}
<dialog class="project-panel" id="projectPanel">
  <header><strong>Start another project</strong><button type="button" class="icon" data-project-close aria-label="Close">x</button></header>
  <main>
    <p class="muted">Each project gets its own lane, transcript, and harness session, so it can run independently.</p>
    {project_error_html}
    <form action="/agents/projects" method="post" class="project-form" data-project-form>
      <input name="slot_id" type="hidden" value="{}">
      <label><span>Harness</span><select name="harness"><option value="pi"{pi_selected}>Pi</option><option value="opencode"{opencode_selected}>OpenCode</option></select></label>
      <label><span>Project folder</span><input name="workdir" required autocomplete="off" autocapitalize="off" spellcheck="false" placeholder="/path/to/project"></label>
      <label><span>Lane name <em>(optional)</em></span><input name="name" maxlength="{MAX_AGENT_SLOT_CHARS}" autocomplete="off" placeholder="defaults to the folder name"></label>
      <label><span>First task <em>(optional)</em></span><textarea name="body" maxlength="{MAX_AGENT_MESSAGE_CHARS}" placeholder="Tell the harness what to do now, or open an empty lane."></textarea></label>
      <div class="project-form-actions"><button type="submit">Open project</button></div>
    </form>
  </main>
</dialog>
<script>
{agent_script}</script>
"##,
            active_slot.id,
            html_escape(&runtime.label),
            active_slot.id,
            active_slot.harness.display_name(),
            active_slot.id
        ),
    )
}

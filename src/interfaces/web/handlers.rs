use crate::AgentSlotSummary;
use crate::AppState;
use crate::Arc;
use crate::AxumPath;
use crate::CodexModel;
use crate::Form;
use crate::HeaderMap;
use crate::Json;
use crate::MAX_AGENT_MESSAGE_CHARS;
use crate::QueuedAgentRequest;
use crate::Redirect;
use crate::Response;
use crate::State;
use crate::agent_location;
use crate::agent_messages_html;
use crate::agent_run_for;
use crate::agent_slot_runtime;
use crate::agent_slot_summary;
use crate::agent_user_message_exists;
use crate::append_agent_assistant;
use crate::append_agent_message;
use crate::clear_agent_queue;
use crate::codex_model_catalog_snapshot;
use crate::delete_agent_messages_after;
use crate::delete_agent_session;
use crate::get_agent_slot;
use crate::handle_agent_control;
use crate::list_agent_messages;
use crate::list_agent_slots;
use crate::page_guard;
use crate::queue_agent_request;
use crate::raw_guard;
use crate::requested_agent_run_settings;
use crate::start_agent_job;
use crate::stop_agent_job;
use crate::update_agent_user_message;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize)]
struct AgentSlotPoll {
    running: bool,
    current: String,
    message_count: usize,
    messages_html: String,
    active_status: String,
}

#[derive(Serialize)]
struct AgentSlotsPoll {
    slots: Vec<AgentSlotSummary>,
}

#[derive(Serialize)]
struct AgentModelCatalogPoll {
    models: Vec<CodexModel>,
}

#[derive(Deserialize)]
pub(crate) struct AgentMessageForm {
    slot_id: Option<i64>,
    edit_message_id: Option<i64>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    control: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    reasoning_effort: String,
}

pub(crate) async fn agent_message_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<AgentMessageForm>,
) -> Response {
    if let Some(response) = page_guard(&state, &headers) {
        return response;
    }
    let slot_id = form.slot_id.unwrap_or(1);
    let slot = {
        let db = state.db.lock().unwrap();
        get_agent_slot(&db, slot_id).unwrap_or(None)
    };
    let Some(slot) = slot else {
        return Redirect::to("/agents").into_response();
    };
    let settings = requested_agent_run_settings(&state, &form.model, &form.reasoning_effort);
    if form.control.trim().eq_ignore_ascii_case("stop") {
        let stopped = stop_agent_job(&state, slot.id);
        let cleared = clear_agent_queue(&state, slot.id);
        let queue_text = if cleared == 0 {
            String::new()
        } else if cleared == 1 {
            " Cleared 1 queued follow-up.".into()
        } else {
            format!(" Cleared {cleared} queued follow-ups.")
        };
        append_agent_assistant(
            &state,
            slot.id,
            &if stopped {
                format!("Stop requested.{queue_text}")
            } else {
                format!("This slot is not running.{queue_text}")
            },
        );
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    let body = form.body.trim().to_string();
    if body.len() > MAX_AGENT_MESSAGE_CHARS || body.is_empty() {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    if let Some(message_id) = form.edit_message_id.filter(|id| *id > 0) {
        if state.agent_jobs.lock().unwrap().contains_key(&slot.id) {
            append_agent_assistant(&state, slot.id, "Cancel the running job before editing.");
            return Redirect::to(&agent_location(Some(slot.id))).into_response();
        }
        let message_exists = {
            let db = state.db.lock().unwrap();
            agent_user_message_exists(&db, slot.id, message_id).unwrap_or(false)
        };
        if !message_exists {
            return Redirect::to(&agent_location(Some(slot.id))).into_response();
        }
        {
            let db = state.db.lock().unwrap();
            let _ = update_agent_user_message(&db, slot.id, message_id, &body);
            let _ = delete_agent_messages_after(&db, slot.id, message_id);
            let _ = delete_agent_session(&db, slot.id);
        }
        let _ = clear_agent_queue(&state, slot.id);
        if handle_agent_control(&state, &slot, &body) {
            return Redirect::to(&agent_location(Some(slot.id))).into_response();
        }
        start_agent_job(state.clone(), slot.id, body, settings);
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    {
        let db = state.db.lock().unwrap();
        let _ = append_agent_message(&db, slot.id, "user", &body);
    }
    if handle_agent_control(&state, &slot, &body) {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    if state.agent_jobs.lock().unwrap().contains_key(&slot.id) {
        let queued_count =
            queue_agent_request(&state, slot.id, QueuedAgentRequest { body, settings });
        let queued_text = if queued_count == 1 {
            "1 queued follow-up".to_string()
        } else {
            format!("{queued_count} queued follow-ups")
        };
        append_agent_assistant(
            &state,
            slot.id,
            &format!("Queued behind the current Codex turn. {queued_text}."),
        );
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    start_agent_job(state.clone(), slot.id, body, settings);
    Redirect::to(&agent_location(Some(slot.id))).into_response()
}

pub(crate) async fn agent_slot_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    let messages = {
        let db = state.db.lock().unwrap();
        list_agent_messages(&db, id).unwrap_or_default()
    };
    let run = agent_run_for(&state, id);
    let active_status = {
        let db = state.db.lock().unwrap();
        get_agent_slot(&db, id)
            .unwrap_or(None)
            .map(|slot| agent_slot_runtime(&state, &slot).label)
            .unwrap_or_else(|| "idle".into())
    };
    Json(AgentSlotPoll {
        running: run.is_some(),
        current: run.map(|run| run.current).unwrap_or_default(),
        message_count: messages.len(),
        messages_html: agent_messages_html(&messages),
        active_status,
    })
    .into_response()
}

pub(crate) async fn agent_slots_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let summaries = slots
        .iter()
        .map(|slot| agent_slot_summary(&state, slot))
        .collect();
    Json(AgentSlotsPoll { slots: summaries }).into_response()
}

pub(crate) async fn agent_model_catalog(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    Json(AgentModelCatalogPoll {
        models: codex_model_catalog_snapshot(&state),
    })
    .into_response()
}

use crate::AgentSlotSummary;
use crate::AppState;
use crate::Arc;
use crate::AxumPath;
use crate::CodexModel;
use crate::Form;
use crate::HeaderMap;
use crate::Json;
use crate::MAX_AGENT_MESSAGE_CHARS;
use crate::Redirect;
use crate::Response;
use crate::State;
use crate::StatusCode;
use crate::agent_control_text;
use crate::agent_location;
use crate::agent_messages_html;
use crate::agent_run_for;
use crate::agent_slot_runtime;
use crate::agent_slot_summary;
use crate::agent_user_message_exists;
use crate::append_agent_assistant;
use crate::append_agent_message;
use crate::codex_model_catalog_snapshot;
use crate::create_agent_slot;
use crate::create_parallel_agent_slot;
use crate::delete_agent_messages_after;
use crate::delete_agent_session;
use crate::expand_local_path;
use crate::get_agent_slot;
use crate::handle_agent_control;
use crate::list_agent_messages;
use crate::list_agent_slots;
use crate::page_guard;
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

#[derive(Deserialize)]
pub(crate) struct AgentProjectForm {
    slot_id: Option<i64>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    workdir: String,
    #[serde(default)]
    body: String,
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
    let Some(slot_id) = form.slot_id.filter(|id| *id > 0) else {
        return (StatusCode::BAD_REQUEST, "agent slot is required").into_response();
    };
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
        let message = if stopped {
            "Stop requested."
        } else {
            "This slot is not running."
        };
        append_agent_assistant(&state, slot.id, message);
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
        if handle_agent_control(&state, &slot, &body) {
            return Redirect::to(&agent_location(Some(slot.id))).into_response();
        }
        start_agent_job(state.clone(), slot.id, body, settings);
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    if agent_control_text(&body).is_some() {
        {
            let db = state.db.lock().unwrap();
            let _ = append_agent_message(&db, slot.id, "user", &body);
        }
        let _ = handle_agent_control(&state, &slot, &body);
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    let target_slot = if state.agent_jobs.lock().unwrap().contains_key(&slot.id) {
        let created = {
            let db = state.db.lock().unwrap();
            create_parallel_agent_slot(&db, &slot)
        };
        match created {
            Ok(created) => {
                append_agent_assistant(
                    &state,
                    slot.id,
                    &format!(
                        "{} is still running. Started this request in `{}` as a separate lane.",
                        slot.name, created.name
                    ),
                );
                created
            }
            Err(err) => {
                append_agent_assistant(
                    &state,
                    slot.id,
                    &format!(
                        "Could not start a separate lane for this request: {err}. The running lane was left unchanged."
                    ),
                );
                return Redirect::to(&agent_location(Some(slot.id))).into_response();
            }
        }
    } else {
        slot.clone()
    };
    {
        let db = state.db.lock().unwrap();
        let _ = append_agent_message(&db, target_slot.id, "user", &body);
    }
    start_agent_job(state.clone(), target_slot.id, body, settings);
    Redirect::to(&agent_location(Some(target_slot.id))).into_response()
}

pub(crate) async fn agent_project_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<AgentProjectForm>,
) -> Response {
    if let Some(response) = page_guard(&state, &headers) {
        return response;
    }
    let return_slot = form.slot_id.filter(|id| *id > 0);
    let raw_workdir = form.workdir.trim();
    if raw_workdir.is_empty() {
        return Redirect::to(&project_form_location(
            return_slot,
            "Choose the folder for the project lane.",
        ))
        .into_response();
    }
    let requested_workdir = expand_local_path(raw_workdir);
    let workdir = match requested_workdir.canonicalize() {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            return Redirect::to(&project_form_location(
                return_slot,
                "The project path must be a directory.",
            ))
            .into_response();
        }
        Err(_) => {
            return Redirect::to(&project_form_location(
                return_slot,
                &format!(
                    "Project folder does not exist: {}",
                    requested_workdir.display()
                ),
            ))
            .into_response();
        }
    };
    let body = form.body.trim().to_string();
    if body.len() > MAX_AGENT_MESSAGE_CHARS {
        return Redirect::to(&project_form_location(
            return_slot,
            "The first message is too long.",
        ))
        .into_response();
    }
    let slot = {
        let db = state.db.lock().unwrap();
        create_agent_slot(&db, &form.name, &workdir)
    };
    let slot = match slot {
        Ok(slot) => slot,
        Err(err) => {
            return Redirect::to(&project_form_location(
                return_slot,
                &format!("Could not create the project lane: {err}"),
            ))
            .into_response();
        }
    };
    if body.is_empty() {
        append_agent_assistant(
            &state,
            slot.id,
            &format!("`{}` is ready in `{}`.", slot.name, slot.workdir),
        );
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    {
        let db = state.db.lock().unwrap();
        let _ = append_agent_message(&db, slot.id, "user", &body);
    }
    let settings = requested_agent_run_settings(&state, &form.model, &form.reasoning_effort);
    start_agent_job(state.clone(), slot.id, body, settings);
    Redirect::to(&agent_location(Some(slot.id))).into_response()
}

fn project_form_location(slot_id: Option<i64>, error: &str) -> String {
    let mut location = agent_location(slot_id);
    location.push(if location.contains('?') { '&' } else { '?' });
    location.push_str("new_project=1&project_error=");
    location.extend(url::form_urlencoded::byte_serialize(error.as_bytes()));
    location
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

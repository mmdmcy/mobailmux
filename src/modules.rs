use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};

use crate::{AppState, MAX_AGENT_MESSAGE_CHARS, MAX_AGENT_UPLOAD_BYTES};

pub(crate) fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(crate::agents_page))
        .route("/login", get(crate::login_page).post(crate::login_post))
        .route("/logout", post(crate::logout_post))
        .route(
            "/agents",
            get(crate::agents_page).post(crate::agent_message_create),
        )
        .route(
            "/agents/slots/{id}/conversation",
            post(crate::agent_conversation_load),
        )
        .route("/agents/slots/{id}/state", get(crate::agent_slot_state))
        .route("/agents/attachments/{id}", get(crate::agent_attachment))
        .route("/agents/codex/reset", post(crate::codex_reset_post))
        .layer(DefaultBodyLimit::max(
            MAX_AGENT_UPLOAD_BYTES + MAX_AGENT_MESSAGE_CHARS + 1024 * 1024,
        ))
        .with_state(state)
}

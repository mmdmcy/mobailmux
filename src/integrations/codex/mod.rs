//! Owns Codex app-server, saved-session, model, usage, and cache protocols.

mod api;
mod sessions;
mod state;
mod types;

pub(crate) use api::{
    codex_conversation_by_id, codex_usage_from_payload, consume_codex_rate_limit_reset_credit,
    fetch_codex_app_server_dashboard, fetch_codex_model_catalog, merge_codex_rate_limit_status,
};
#[cfg(test)]
pub(crate) use api::{codex_models_from_payload, codex_rate_window, codex_reset_credits_summary};
#[cfg(test)]
pub(crate) use sessions::codex_conversation_from_file;
pub(crate) use sessions::{codex_content_text, codex_transcript_messages, load_codex_index};
pub(crate) use state::{
    codex_index_snapshot, codex_model_catalog_snapshot, open_db, refresh_codex_index,
    refresh_codex_index_blocking, refresh_codex_model_catalog,
};
pub(crate) use types::{
    CodexAppServerDashboard, CodexConversation, CodexIndex, CodexIndexCache, CodexModel,
    CodexModelCatalogCache, CodexRateWindow, CodexReasoningEffort, CodexResetCredit,
    CodexResetCreditsSummary, CodexUsageSnapshot, CodexVisibleMessage,
};

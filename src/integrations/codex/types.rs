use crate::Instant;
use crate::PathBuf;
use crate::persistence;
use serde::Serialize;

#[derive(Clone, Debug)]
pub(crate) struct CodexConversation {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) updated_at: String,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexVisibleMessage {
    pub(crate) role: String,
    pub(crate) text: String,
    pub(crate) timestamp: String,
    pub(crate) order: usize,
    pub(crate) fallback: bool,
    pub(crate) final_answer: bool,
    pub(crate) assistant_progress: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CodexReasoningEffort {
    pub(crate) effort: String,
    pub(crate) description: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CodexModel {
    pub(crate) model: String,
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) default_reasoning_effort: String,
    pub(crate) supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    pub(crate) is_default: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexIndex {
    pub(crate) conversations: Vec<CodexConversation>,
    pub(crate) usage: Option<CodexUsageSnapshot>,
}

impl CodexIndex {
    pub(crate) fn empty() -> Self {
        Self {
            conversations: Vec::new(),
            usage: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CodexUsageSnapshot {
    pub(crate) observed_at: String,
    pub(crate) plan_type: String,
    pub(crate) total_units: i64,
    pub(crate) last_units: i64,
    pub(crate) cached_input_units: i64,
    pub(crate) context_window: i64,
    pub(crate) primary: Option<CodexRateWindow>,
    pub(crate) secondary: Option<CodexRateWindow>,
    pub(crate) credits: Option<String>,
    pub(crate) reset_credits: Option<CodexResetCreditsSummary>,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexRateWindow {
    pub(crate) label: String,
    pub(crate) used_percent: f64,
    pub(crate) remaining_percent: f64,
    pub(crate) window_minutes: i64,
    pub(crate) resets_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexResetCreditsSummary {
    pub(crate) available_count: i64,
    pub(crate) credits: Vec<CodexResetCredit>,
    pub(crate) estimate: Option<persistence::reset_ledger::ResetCreditEstimate>,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexResetCredit {
    pub(crate) title: String,
    pub(crate) expires_at: Option<i64>,
}

#[derive(Default)]
pub(crate) struct CodexAppServerDashboard {
    pub(crate) rate_limits: Option<serde_json::Value>,
}

#[derive(Default)]
pub(crate) struct CodexIndexCache {
    pub(crate) snapshot: Option<CodexIndex>,
    pub(crate) refreshed_at: Option<Instant>,
    pub(crate) refreshing: bool,
}

#[derive(Default)]
pub(crate) struct CodexModelCatalogCache {
    pub(crate) models: Vec<CodexModel>,
    pub(crate) refreshed_at: Option<Instant>,
    pub(crate) refreshing: bool,
}

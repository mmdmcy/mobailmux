use crate::PathBuf;
use serde::Serialize;

#[derive(Copy, Clone, Serialize)]
pub(crate) struct AgentCommandSpec {
    pub(crate) name: &'static str,
    pub(crate) usage: &'static str,
    pub(crate) description: &'static str,
    pub(crate) takes_arg: bool,
}

#[derive(Clone, Serialize)]
pub(crate) struct ComposerSuggestion {
    pub(crate) kind: &'static str,
    pub(crate) name: String,
    pub(crate) insert: String,
    pub(crate) description: String,
    pub(crate) takes_arg: bool,
}

#[derive(Clone, Serialize)]
pub(crate) struct AgentRun {
    pub(crate) status: String,
    pub(crate) current: String,
    pub(crate) started_at: String,
}

#[derive(Default)]
pub(crate) struct AgentStdoutSummary {
    pub(crate) last_assistant_text: Option<String>,
    pub(crate) final_text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentRunSettings {
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SlotRuntime {
    pub(crate) label: String,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentSlotRow {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) workdir: String,
    pub(crate) goal: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentMessageRow {
    pub(crate) id: i64,
    pub(crate) role: String,
    pub(crate) body: String,
    pub(crate) created_at: String,
}

pub(crate) struct AgentProgress {
    pub(crate) dir: PathBuf,
    pub(crate) file: PathBuf,
}

#[derive(Serialize)]
pub(crate) struct AgentSlotSummary {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) running: bool,
    pub(crate) current: String,
    pub(crate) status: String,
}

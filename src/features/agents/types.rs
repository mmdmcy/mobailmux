use crate::PathBuf;
use serde::Serialize;
use std::fmt;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AgentHarness {
    #[default]
    Pi,
    OpenCode,
    LegacyCodex,
}

impl AgentHarness {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pi" => Some(Self::Pi),
            "opencode" => Some(Self::OpenCode),
            "legacy-codex" | "codex" => Some(Self::LegacyCodex),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::LegacyCodex => "legacy-codex",
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Pi => "Pi",
            Self::OpenCode => "OpenCode",
            Self::LegacyCodex => "Legacy Codex",
        }
    }

    pub(crate) fn is_runnable(self) -> bool {
        !matches!(self, Self::LegacyCodex)
    }
}

impl fmt::Display for AgentHarness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

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
    pub(crate) harness: AgentHarness,
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
    pub(crate) harness: AgentHarness,
}

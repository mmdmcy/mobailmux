//! Owns agent lanes, messages, execution, commands, and rendering.

mod composer;
mod control;
mod runtime;
mod types;
mod views;

pub(crate) use composer::{
    agent_composer_suggestions_json, agent_control_text, agent_location, command_arg,
    json_for_inline_script, normalize_agent_command_text,
};
#[cfg(test)]
pub(crate) use composer::{
    discover_codex_plugin_suggestions, discover_codex_skill_suggestions,
    looks_like_agent_control_request,
};
#[cfg(test)]
pub(crate) use control::validate_agent_run_settings;
pub(crate) use control::{
    agent_run_settings_label, apply_agent_run_settings, handle_agent_control,
    requested_agent_run_settings, stop_agent_job,
};
#[cfg(test)]
pub(crate) use runtime::{build_agent_prompt, codex_stdout_agent_message};
pub(crate) use runtime::{is_final_agent_phase, shell_single_quote, start_agent_job};
pub(crate) use types::{
    AgentCommandSpec, AgentMessageRow, AgentProgress, AgentRun, AgentRunSettings, AgentSlotRow,
    AgentSlotSummary, AgentStdoutSummary, ComposerSuggestion, SlotRuntime,
};
#[cfg(test)]
pub(crate) use views::message_body_html;
pub(crate) use views::{
    agent_activity_kind, agent_messages_html, agent_run_for, agent_slot_rail_html,
    agent_slot_runtime, agent_slot_summary, codex_transcript_count, codex_transcript_html,
};

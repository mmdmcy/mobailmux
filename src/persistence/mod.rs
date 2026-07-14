//! SQLite schema and reset-credit persistence.

mod agents;
pub(crate) mod migrations;
pub(crate) mod reset_ledger;

#[cfg(test)]
pub(crate) use agents::ensure_agent_slot;
pub(crate) use agents::{
    agent_session, agent_user_message_exists, append_agent_assistant, append_agent_message,
    create_agent_slot, create_parallel_agent_slot, delete_agent_messages_after,
    delete_agent_session, ensure_agent_slot_seeds, get_agent_slot, list_agent_messages,
    list_agent_slots, mark_interrupted_agent_runs, reset_agent_slot_chat, set_agent_goal,
    set_agent_session, set_agent_workdir, update_agent_user_message,
};

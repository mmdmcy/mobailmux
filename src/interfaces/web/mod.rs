//! Owns HTTP routing and server-rendered agent pages.

mod agents;
mod handlers;
pub(crate) mod router;

pub(crate) use agents::agents_page;
#[cfg(test)]
pub(crate) use handlers::agent_model_catalog;
pub(crate) use handlers::{
    agent_message_create, agent_project_create, agent_slot_state, agent_slots_state,
};

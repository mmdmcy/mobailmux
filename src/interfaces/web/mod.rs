//! Owns HTTP routing and server-rendered agent pages.

mod agents;
mod handlers;
pub(crate) mod router;

pub(crate) use agents::agents_page;
pub(crate) use handlers::{
    agent_message_create, agent_model_catalog, agent_project_create, agent_slot_state,
    agent_slots_state,
};

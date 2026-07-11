//! Application startup, configuration, and process-wide state.

mod config;
mod startup;
mod state;

pub(crate) use config::{AgentSlotSeed, Config};
pub(crate) use startup::serve;
pub(crate) use state::AppState;

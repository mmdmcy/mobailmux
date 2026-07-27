use crate::AgentRun;
#[cfg(test)]
use crate::CodexIndexCache;
#[cfg(test)]
use crate::CodexModelCatalogCache;
use crate::Config;
use crate::Connection;
use crate::HashMap;
use crate::Mutex;
use crate::oneshot;

pub(crate) struct AppState {
    pub(crate) db: Mutex<Connection>,
    pub(crate) config: Config,
    pub(crate) agent_jobs: Mutex<HashMap<i64, AgentRun>>,
    pub(crate) agent_cancels: Mutex<HashMap<i64, oneshot::Sender<()>>>,
    #[cfg(test)]
    pub(crate) codex_index: Mutex<CodexIndexCache>,
    #[cfg(test)]
    pub(crate) codex_models: Mutex<CodexModelCatalogCache>,
}

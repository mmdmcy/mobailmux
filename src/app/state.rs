use crate::AgentRun;
use crate::CodexIndexCache;
use crate::CodexModelCatalogCache;
use crate::Config;
use crate::Connection;
use crate::HashMap;
use crate::Mutex;
use crate::QueuedAgentRequest;
use crate::VecDeque;
use crate::oneshot;

pub(crate) struct AppState {
    pub(crate) db: Mutex<Connection>,
    pub(crate) config: Config,
    pub(crate) agent_jobs: Mutex<HashMap<i64, AgentRun>>,
    pub(crate) agent_cancels: Mutex<HashMap<i64, oneshot::Sender<()>>>,
    pub(crate) agent_queues: Mutex<HashMap<i64, VecDeque<QueuedAgentRequest>>>,
    pub(crate) codex_index: Mutex<CodexIndexCache>,
    pub(crate) codex_models: Mutex<CodexModelCatalogCache>,
}

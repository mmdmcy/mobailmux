use crate::AppState;
use crate::Arc;
use crate::CodexIndexCache;
use crate::CodexModelCatalogCache;
use crate::Config;
use crate::HashMap;
use crate::Mutex;
use crate::SocketAddr;
use crate::ensure_agent_slot_seeds;
use crate::interfaces;
use crate::io;
use crate::io_other;
use crate::mark_interrupted_agent_runs;
use crate::open_db;
use crate::refresh_codex_index;
use crate::refresh_codex_model_catalog;

pub(crate) async fn serve() -> io::Result<()> {
    let config = Config::from_env()?;
    let conn = open_db(&config.db_path)?;
    ensure_agent_slot_seeds(&conn, &config.agent_slots, &config.agent_default_workdir)
        .map_err(io_other)?;

    let state = Arc::new(AppState {
        db: Mutex::new(conn),
        config,
        agent_jobs: Mutex::new(HashMap::new()),
        agent_cancels: Mutex::new(HashMap::new()),
        codex_index: Mutex::new(CodexIndexCache::default()),
        codex_models: Mutex::new(CodexModelCatalogCache::default()),
    });

    mark_interrupted_agent_runs(&state);
    refresh_codex_index(state.clone());
    refresh_codex_model_catalog(state.clone());

    let app = interfaces::web::router::build_router(state.clone());
    let bind = state
        .config
        .bind
        .parse::<SocketAddr>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("Mobailmux listening on http://{bind}");
    axum::serve(listener, app).await
}

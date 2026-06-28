use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use axum::{
    Json,
    extract::{Form, Multipart, Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Reverse,
    collections::HashMap,
    env, fs,
    io::{self, BufRead, Read, Write},
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::{Arc, Mutex},
    time::{Duration as StdDuration, Instant, SystemTime},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command as TokioCommand,
    sync::oneshot,
};
use uuid::Uuid;

mod codex_reset_ledger;
mod db_migrations;
mod modules;

const DEFAULT_AGENT_SLOTS: &str = "codex";
const MAX_AGENT_MESSAGE_CHARS: usize = 128 * 1024;
const MAX_AGENT_UPLOAD_BYTES: usize = 25 * 1024 * 1024;
const MAX_AGENT_SLOT_CHARS: usize = 32;
const MAX_CODEX_CONVERSATIONS: usize = 120;
const MAX_CODEX_TRANSCRIPT_MESSAGES: usize = 80;
const CODEX_SESSION_SCAN_LIMIT: usize = 180;
const CODEX_PROJECT_DRAWER_LIMIT: usize = 48;
const CODEX_PROJECT_VISIBLE_CONVERSATIONS: usize = 3;
const CODEX_PROJECT_CONVERSATION_LIMIT: usize = 24;
const CODEX_INDEX_REFRESH_AFTER: StdDuration = StdDuration::from_secs(30);
const CODEX_APP_SERVER_TIMEOUT: StdDuration = StdDuration::from_secs(12);
const CODEX_APP_SERVER_WRITE_SETTLE: StdDuration = StdDuration::from_secs(5);
const SESSION_COOKIE: &str = "mobailmux_session";
const SESSION_DAYS: i64 = 30;
const PAGE_CSS: &str = include_str!("page.css");

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str).unwrap_or("serve") {
        "serve" => serve().await,
        "hash-password" => {
            hash_password_cmd(&args[1..])?;
            Ok(())
        }
        "audit-public" => {
            if audit_public_cmd(&args[1..])? != 0 {
                std::process::exit(1);
            }
            Ok(())
        }
        _ => {
            eprintln!("usage: mobailmux [serve|hash-password --stdin|audit-public]");
            Ok(())
        }
    }
}

async fn serve() -> io::Result<()> {
    let config = Config::from_env()?;
    fs::create_dir_all(&config.agent_upload_dir)?;
    let conn = open_db(&config.db_path)?;
    ensure_agent_slot_seeds(&conn, &config.agent_slots, &config.agent_default_workdir)
        .map_err(io_other)?;

    let state = Arc::new(AppState {
        db: Mutex::new(conn),
        config,
        agent_jobs: Mutex::new(HashMap::new()),
        agent_cancels: Mutex::new(HashMap::new()),
        codex_index: Mutex::new(CodexIndexCache::default()),
    });

    refresh_codex_index(state.clone());

    let app = modules::build_router(state.clone());

    let bind = state
        .config
        .bind
        .parse::<SocketAddr>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("Mobailmux listening on http://{bind}");
    axum::serve(listener, app).await
}

#[derive(Clone)]
struct Config {
    bind: String,
    db_path: PathBuf,
    agent_default_workdir: PathBuf,
    agent_upload_dir: PathBuf,
    agent_codex_bin: String,
    agent_codex_args: Vec<String>,
    codex_home: PathBuf,
    codex_reset_command: Option<Vec<String>>,
    agent_slots: Vec<AgentSlotSeed>,
    user: String,
    password_hash: Option<String>,
    cookie_secret: Vec<u8>,
    auth_disabled: bool,
}

#[derive(Clone)]
struct AgentSlotSeed {
    name: String,
    workdir: PathBuf,
}

#[derive(Clone, Serialize)]
struct AgentRun {
    status: String,
    current: String,
    started_at: String,
}

#[derive(Clone, Debug)]
struct SlotRuntime {
    label: String,
}

#[derive(Clone, Debug)]
struct CodexConversation {
    id: String,
    title: String,
    cwd: String,
    updated_at: String,
    path: PathBuf,
    preview: String,
    message_count: usize,
}

#[derive(Clone, Debug)]
struct CodexProject {
    path: String,
    name: String,
    trusted: bool,
    conversation_count: usize,
}

#[derive(Clone, Debug)]
struct CodexIndex {
    conversations: Vec<CodexConversation>,
    projects: Vec<CodexProject>,
    usage: Option<CodexUsageSnapshot>,
}

impl CodexIndex {
    fn empty() -> Self {
        Self {
            conversations: Vec::new(),
            projects: Vec::new(),
            usage: None,
        }
    }
}

#[derive(Clone, Debug)]
struct CodexUsageSnapshot {
    observed_at: String,
    plan_type: String,
    total_units: i64,
    last_units: i64,
    cached_input_units: i64,
    context_window: i64,
    primary: Option<CodexRateWindow>,
    secondary: Option<CodexRateWindow>,
    credits: Option<String>,
    reset_credits: Option<CodexResetCreditsSummary>,
}

#[derive(Clone, Debug)]
struct CodexRateWindow {
    label: String,
    used_percent: f64,
    remaining_percent: f64,
    window_minutes: i64,
    resets_at: Option<i64>,
}

#[derive(Clone, Debug)]
struct CodexResetCreditsSummary {
    available_count: i64,
    estimate: Option<codex_reset_ledger::ResetCreditEstimate>,
}

#[derive(Default)]
struct CodexIndexCache {
    snapshot: Option<CodexIndex>,
    refreshed_at: Option<Instant>,
    refreshing: bool,
}

#[derive(Debug)]
struct AuditFinding {
    path: String,
    line: Option<usize>,
    message: String,
}

impl Config {
    fn from_env() -> io::Result<Self> {
        let bind = env::var("MOBAILMUX_BIND").unwrap_or_else(|_| "127.0.0.1:8765".into());
        let db_path = env::var("MOBAILMUX_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/mobailmux.sqlite"));
        let agent_default_workdir = env::var("MOBAILMUX_AGENT_DEFAULT_WORKDIR")
            .map(|value| expand_local_path(&value))
            .unwrap_or_else(|_| default_home_dir());
        let agent_upload_dir = env::var("MOBAILMUX_AGENT_UPLOAD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/agent-uploads"));
        let agent_codex_bin =
            env::var("MOBAILMUX_AGENT_CODEX_BIN").unwrap_or_else(|_| default_codex_bin());
        let agent_codex_args = env::var("MOBAILMUX_AGENT_CODEX_ARGS")
            .ok()
            .map(|value| split_env_args(&value))
            .unwrap_or_default();
        let codex_home = env::var("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_home_dir().join(".codex"));
        let codex_reset_command = env::var("MOBAILMUX_CODEX_RESET_COMMAND")
            .ok()
            .map(|value| split_env_args(&value))
            .filter(|parts| !parts.is_empty());
        let agent_slots = parse_agent_slot_seeds(
            env::var("MOBAILMUX_AGENT_SLOTS").ok(),
            &agent_default_workdir,
        );
        let user = env::var("MOBAILMUX_USER").unwrap_or_else(|_| "mobailmux".into());
        let password_hash = env::var("MOBAILMUX_PASSWORD_HASH")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let auth_disabled = env_flag("MOBAILMUX_AUTH_DISABLED", false);
        if password_hash.is_none() && !auth_disabled {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MOBAILMUX_PASSWORD_HASH is required unless MOBAILMUX_AUTH_DISABLED=1",
            ));
        }
        let cookie_secret = env::var("MOBAILMUX_COOKIE_SECRET")
            .ok()
            .and_then(|value| hex::decode(value.trim()).ok())
            .filter(|bytes| bytes.len() >= 32)
            .unwrap_or_else(random_secret);

        Ok(Self {
            bind,
            db_path,
            agent_default_workdir,
            agent_upload_dir,
            agent_codex_bin,
            agent_codex_args,
            codex_home,
            codex_reset_command,
            agent_slots,
            user,
            password_hash,
            cookie_secret,
            auth_disabled,
        })
    }
}

struct AppState {
    db: Mutex<Connection>,
    config: Config,
    agent_jobs: Mutex<HashMap<i64, AgentRun>>,
    agent_cancels: Mutex<HashMap<i64, oneshot::Sender<()>>>,
    codex_index: Mutex<CodexIndexCache>,
}

fn codex_index_snapshot(state: &Arc<AppState>) -> Option<CodexIndex> {
    let (snapshot, should_refresh) = {
        let mut cache = state.codex_index.lock().unwrap();
        let stale = cache
            .refreshed_at
            .is_none_or(|refreshed_at| refreshed_at.elapsed() >= CODEX_INDEX_REFRESH_AFTER);
        let should_refresh = stale && !cache.refreshing;
        if should_refresh {
            cache.refreshing = true;
        }
        (cache.snapshot.clone(), should_refresh)
    };
    if should_refresh {
        refresh_codex_index(state.clone());
    }
    snapshot
}

fn refresh_codex_index(state: Arc<AppState>) {
    {
        let mut cache = state.codex_index.lock().unwrap();
        cache.refreshing = true;
    }
    tokio::task::spawn_blocking(move || {
        let index = load_codex_index_for_state(&state);
        let mut cache = state.codex_index.lock().unwrap();
        cache.snapshot = Some(index);
        cache.refreshed_at = Some(Instant::now());
        cache.refreshing = false;
    });
}

fn refresh_codex_index_blocking(state: &Arc<AppState>) -> CodexIndex {
    let index = load_codex_index_for_state(state);
    let mut cache = state.codex_index.lock().unwrap();
    cache.snapshot = Some(index.clone());
    cache.refreshed_at = Some(Instant::now());
    cache.refreshing = false;
    index
}

fn load_codex_index_for_state(state: &Arc<AppState>) -> CodexIndex {
    let mut index = load_codex_index(&state.config);
    attach_codex_reset_credit_estimate(state, &mut index);
    index
}

fn attach_codex_reset_credit_estimate(state: &Arc<AppState>, index: &mut CodexIndex) {
    let Some(summary) = index
        .usage
        .as_mut()
        .and_then(|usage| usage.reset_credits.as_mut())
    else {
        return;
    };
    let db = state.db.lock().unwrap();
    if let Ok(estimate) = codex_reset_ledger::reconcile(&db, summary.available_count, Utc::now()) {
        summary.estimate = Some(estimate);
    }
}

fn open_db(path: &Path) -> io::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path).map_err(io_other)?;
    db_migrations::migrate(&conn).map_err(io_other)?;
    Ok(conn)
}

async fn login_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if authorized(&state.config, &headers) {
        return Redirect::to("/").into_response();
    }
    page(
        "Mobailmux Login",
        r#"
<main class="login">
  <h1>Mobailmux</h1>
  <form action="/login" method="post">
    <label>Password</label>
    <input name="password" type="password" autocomplete="current-password" autofocus required>
    <button type="submit">Log in</button>
  </form>
</main>
"#,
    )
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

async fn login_post(State(state): State<Arc<AppState>>, Form(form): Form<LoginForm>) -> Response {
    if !verify_password(&state.config, &form.password) {
        return page(
            "Mobailmux Login",
            r#"
<main class="login">
  <h1>Mobailmux</h1>
  <p class="error">Wrong password.</p>
  <form action="/login" method="post">
    <label>Password</label>
    <input name="password" type="password" autocomplete="current-password" autofocus required>
    <button type="submit">Log in</button>
  </form>
</main>
"#,
        );
    }
    let cookie = make_session_cookie(&state.config);
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, HeaderValue::from_static("/")),
            (header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap()),
        ],
    )
        .into_response()
}

async fn logout_post() -> Response {
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, HeaderValue::from_static("/login")),
            (
                header::SET_COOKIE,
                HeaderValue::from_static(
                    "mobailmux_session=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax",
                ),
            ),
        ],
    )
        .into_response()
}

#[derive(Deserialize)]
struct AgentsQuery {
    slot: Option<i64>,
    thread: Option<String>,
}

#[derive(Deserialize)]
struct AgentConversationForm {
    thread_id: String,
    workdir: String,
}

#[derive(Deserialize)]
struct CodexResetForm {
    confirm: String,
}

struct PendingUpload {
    filename: String,
    content_type: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct AgentSlotRow {
    id: i64,
    name: String,
    workdir: String,
}

#[derive(Debug, Clone, Serialize)]
struct AgentAttachmentSummary {
    id: i64,
    name: String,
    content_type: String,
    size_bytes: i64,
    is_image: bool,
}

#[derive(Debug, Clone, Serialize)]
struct AgentMessageRow {
    role: String,
    body: String,
    created_at: String,
    attachment: Option<AgentAttachmentSummary>,
}

#[derive(Serialize)]
struct AgentSlotPoll {
    running: bool,
    current: String,
    message_count: usize,
    messages_html: String,
    active_status: String,
}

async fn agents_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AgentsQuery>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = page_guard(&state, &headers) {
        return response;
    }
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let active_slot = slots
        .iter()
        .find(|slot| Some(slot.id) == query.slot)
        .or_else(|| slots.first());
    let Some(active_slot) = active_slot else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "missing agent slot").into_response();
    };
    let mut codex_snapshot = codex_index_snapshot(&state);
    let selected_thread = query.thread.clone();
    if selected_thread.as_ref().is_some_and(|thread_id| {
        codex_snapshot
            .as_ref()
            .is_none_or(|index| codex_conversation_by_id(index, thread_id).is_none())
    }) {
        codex_snapshot = Some(refresh_codex_index_blocking(&state));
    }
    let codex_loaded = codex_snapshot.is_some();
    let codex = codex_snapshot.unwrap_or_else(CodexIndex::empty);
    let messages = {
        let db = state.db.lock().unwrap();
        list_agent_messages(&db, active_slot.id).unwrap_or_default()
    };
    let runtime = agent_slot_runtime(&state, active_slot);
    let browser_drawer = codex_browser_drawer_html(
        &codex.projects,
        &codex.conversations,
        active_slot.id,
        selected_thread.as_deref(),
        codex_loaded,
    );
    let messages_html = selected_thread
        .as_deref()
        .and_then(|thread_id| codex_transcript_html(&codex, thread_id).ok())
        .unwrap_or_else(|| agent_messages_html(&messages));
    let active_title = selected_thread
        .as_deref()
        .and_then(|thread_id| codex_conversation_by_id(&codex, thread_id))
        .map(|conversation| conversation.title.clone())
        .unwrap_or_else(|| "Codex".into());
    let active_title = html_escape(&active_title);
    let active_slot_workdir = html_escape(&active_slot.workdir);
    let message_count = if selected_thread.is_some() {
        codex_transcript_count(&messages_html).unwrap_or(messages.len())
    } else {
        messages.len()
    };
    let usage_dialog = codex_usage_dialog(&state.config, codex.usage.as_ref(), codex_loaded);
    let viewing_transcript = selected_thread.is_some();
    page(
        "Agents",
        &format!(
            r##"
<nav><a href="/">Mobailmux</a><div class="nav-actions"><button type="button" class="ghost" data-browser-open>Browse</button><button type="button" class="ghost" data-codex-open>Usage</button><strong>Agents</strong></div></nav>
<main class="chat-shell agent-shell agent-single">
  <section class="chat-pane agent-pane">
    <header class="chat-head">
      <div class="chat-title"><strong>{active_title}</strong><span>{active_slot_workdir}</span></div>
      <div class="chat-stats"><span data-agent-count>{message_count} messages</span><span class="agent-status" data-agent-status>{}</span></div>
    </header>
    <div class="message-list" data-agent-messages>{messages_html}</div>
    <section class="agent-compose-wrap">
      <form action="/agents" method="post" enctype="multipart/form-data" class="agent-composer">
        <input name="slot_id" type="hidden" value="{}">
        <textarea id="agentBody" name="body" maxlength="{MAX_AGENT_MESSAGE_CHARS}" placeholder="Message Codex"></textarea>
        <label class="file-pill"><input name="attachment" type="file" accept="image/*,.pdf,.txt,.md,.csv,.json,.doc,.docx,.xls,.xlsx,.ppt,.pptx,.zip,application/pdf,text/*"><span>Attach</span></label>
        <button type="submit">Send</button>
      </form>
    </section>
  </section>
</main>
{browser_drawer}
{usage_dialog}
<script>
(() => {{
  const list = document.querySelector("[data-agent-messages]");
  const status = document.querySelector("[data-agent-status]");
  const count = document.querySelector("[data-agent-count]");
  const input = document.getElementById("agentBody");
  const viewingTranscript = {viewing_transcript};
  const codexPanel = document.getElementById("codexPanel");
  document.querySelector("[data-codex-open]")?.addEventListener("click", () => {{
    if (codexPanel && typeof codexPanel.showModal === "function") codexPanel.showModal();
  }});
  document.querySelector("[data-codex-close]")?.addEventListener("click", () => codexPanel?.close());
  const browserPanel = document.getElementById("conversationDrawer");
  const closestElement = (target, selector) => target instanceof Element ? target.closest(selector) : null;
  const lockPageScroll = () => {{
    if (document.body.classList.contains("modal-scroll-locked")) return;
    const scrollY = window.scrollY || document.documentElement.scrollTop || 0;
    document.body.dataset.scrollLockY = String(scrollY);
    document.body.style.top = "-" + scrollY + "px";
    document.body.classList.add("modal-scroll-locked");
  }};
  const unlockPageScroll = () => {{
    if (!document.body.classList.contains("modal-scroll-locked")) return;
    const scrollY = Number(document.body.dataset.scrollLockY || "0");
    document.body.classList.remove("modal-scroll-locked");
    document.body.style.top = "";
    delete document.body.dataset.scrollLockY;
    window.scrollTo(0, scrollY);
  }};
  const showLockedDialog = (panel) => {{
    if (panel && typeof panel.showModal === "function") {{
      panel.showModal();
      lockPageScroll();
    }}
  }};
  document.querySelector("[data-browser-open]")?.addEventListener("click", () => {{
    showLockedDialog(browserPanel);
  }});
  browserPanel?.addEventListener("close", unlockPageScroll);
  document.querySelector("[data-browser-close]")?.addEventListener("click", () => browserPanel?.close());
  let browserTouchY = 0;
  document.addEventListener("touchstart", (event) => {{
    if (!browserPanel?.open || event.touches.length !== 1) return;
    browserTouchY = event.touches[0].clientY;
  }}, {{passive:true}});
  document.addEventListener("touchmove", (event) => {{
    if (!browserPanel?.open || event.touches.length !== 1) return;
    const scrollTarget = closestElement(event.target, ".browser-scroll");
    if (!scrollTarget) {{
      event.preventDefault();
      return;
    }}
    const y = event.touches[0].clientY;
    const delta = y - browserTouchY;
    browserTouchY = y;
    const atTop = scrollTarget.scrollTop <= 0;
    const atBottom = Math.ceil(scrollTarget.scrollTop + scrollTarget.clientHeight) >= scrollTarget.scrollHeight;
    if ((atTop && delta > 0) || (atBottom && delta < 0)) event.preventDefault();
  }}, {{passive:false}});
  document.addEventListener("wheel", (event) => {{
    if (!browserPanel?.open) return;
    const scrollTarget = closestElement(event.target, ".browser-scroll");
    if (!scrollTarget) {{
      event.preventDefault();
      return;
    }}
    const atTop = scrollTarget.scrollTop <= 0;
    const atBottom = Math.ceil(scrollTarget.scrollTop + scrollTarget.clientHeight) >= scrollTarget.scrollHeight;
    if ((atTop && event.deltaY < 0) || (atBottom && event.deltaY > 0)) event.preventDefault();
  }}, {{passive:false}});
  document.querySelectorAll("[data-load-more]").forEach((button) => {{
    button.addEventListener("click", () => {{
      const group = button.closest("[data-project-group]");
      if (!group) return;
      group.querySelectorAll("[data-extra-conversation]").forEach((row) => row.hidden = false);
      button.hidden = true;
    }});
  }});
  document.querySelector("[data-reset-form]")?.addEventListener("submit", (event) => {{
    const ok = window.confirm("Use a Codex reset now? This cannot be undone.");
    if (!ok) event.preventDefault();
  }});
  let dirty = false;
  input.addEventListener("input", () => {{ dirty = input.value.length > 0; }});
  input.addEventListener("focus", () => setTimeout(() => input.scrollIntoView({{block:"nearest"}}), 80));
  async function poll() {{
    if (!list || dirty || document.activeElement === input) {{
      setTimeout(poll, 1800);
      return;
    }}
    const nearBottom = list.scrollTop + list.clientHeight >= list.scrollHeight - 90;
    try {{
      const response = await fetch("/agents/slots/{}/state", {{cache:"no-store"}});
      if (response.ok) {{
        const data = await response.json();
        list.innerHTML = data.messages_html;
        status.textContent = data.active_status || (data.running ? (data.current || "running") : "idle");
        count.textContent = data.message_count + " messages";
        if (nearBottom) list.scrollTop = list.scrollHeight;
        setTimeout(poll, data.running ? 1200 : 4000);
        return;
      }}
    }} catch (_) {{}}
    setTimeout(poll, 4000);
  }}
  if (list) list.scrollTop = list.scrollHeight;
  if (!viewingTranscript) setTimeout(poll, 1200);
}})();
</script>
"##,
            html_escape(&runtime.label),
            active_slot.id,
            active_slot.id
        ),
    )
}

async fn agent_conversation_load(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Form(form): Form<AgentConversationForm>,
) -> Response {
    if let Some(response) = page_guard(&state, &headers) {
        return response;
    }
    let mut codex = codex_index_snapshot(&state);
    if codex
        .as_ref()
        .is_none_or(|index| codex_conversation_by_id(index, &form.thread_id).is_none())
    {
        codex = Some(refresh_codex_index_blocking(&state));
    }
    let codex_loaded = codex.is_some();
    let conversation = codex
        .as_ref()
        .and_then(|index| codex_conversation_by_id(index, &form.thread_id));
    if codex_loaded && conversation.is_none() {
        return Redirect::to(&agent_location(Some(id))).into_response();
    }
    let requested_workdir = expand_local_path(&form.workdir);
    let workdir = if requested_workdir.is_dir() {
        requested_workdir
            .canonicalize()
            .unwrap_or_else(|_| requested_workdir.clone())
    } else if let Some(conversation) = conversation {
        expand_local_path(&conversation.cwd)
    } else {
        return Redirect::to(&agent_location(Some(id))).into_response();
    };
    if !workdir.is_dir() {
        return Redirect::to(&agent_location(Some(id))).into_response();
    }
    {
        let db = state.db.lock().unwrap();
        let _ = db.execute(
            "UPDATE agent_slots SET workdir = ?1 WHERE id = ?2",
            params![workdir.to_string_lossy(), id],
        );
        let _ = set_agent_session(&db, id, &form.thread_id, &workdir.to_string_lossy());
    }
    Redirect::to(&format!(
        "{}&thread={}",
        agent_location(Some(id)),
        url_encode(&form.thread_id)
    ))
    .into_response()
}

async fn codex_reset_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<CodexResetForm>,
) -> Response {
    if let Some(response) = page_guard(&state, &headers) {
        return response;
    }
    if form.confirm.trim() != "USE_RESET" {
        return (StatusCode::BAD_REQUEST, "confirmation required").into_response();
    }
    if let Some(command) = &state.config.codex_reset_command {
        let Some((program, args)) = command.split_first() else {
            return (StatusCode::CONFLICT, "no reset command configured").into_response();
        };
        let output = StdCommand::new(program).args(args).output();
        return match output {
            Ok(output) if output.status.success() => {
                refresh_codex_index(state.clone());
                Redirect::to("/agents").into_response()
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("reset command failed: {}", truncate_text(&stderr, 1000)),
                )
                    .into_response()
            }
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not start reset command: {err}"),
            )
                .into_response(),
        };
    }
    match consume_codex_rate_limit_reset_credit(&state.config) {
        Some(outcome) if outcome == "reset" || outcome == "alreadyRedeemed" => {
            refresh_codex_index(state.clone());
            Redirect::to("/agents").into_response()
        }
        Some(outcome) if outcome == "nothingToReset" => (
            StatusCode::CONFLICT,
            "no current Codex limit window is eligible for reset",
        )
            .into_response(),
        Some(outcome) if outcome == "noCredit" => (
            StatusCode::CONFLICT,
            "Codex reports no reset credits available",
        )
            .into_response(),
        Some(outcome) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected Codex reset outcome: {outcome}"),
        )
            .into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not ask Codex to use a reset credit",
        )
            .into_response(),
    }
}

async fn agent_message_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Some(response) = page_guard(&state, &headers) {
        return response;
    }
    let mut slot_id = None;
    let mut body = String::new();
    let mut upload = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "slot_id" => {
                if let Ok(text) = field.text().await {
                    slot_id = text.trim().parse::<i64>().ok();
                }
            }
            "body" => {
                if let Ok(text) = field.text().await {
                    body = text;
                }
            }
            "attachment" | "file" => {
                let filename = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "attachment".into());
                let content_type = field
                    .content_type()
                    .map(str::to_string)
                    .unwrap_or_else(|| "application/octet-stream".into());
                if let Ok(bytes) = field.bytes().await
                    && !bytes.is_empty()
                    && bytes.len() <= MAX_AGENT_UPLOAD_BYTES
                {
                    upload = Some(PendingUpload {
                        filename,
                        content_type,
                        bytes: bytes.to_vec(),
                    });
                }
            }
            _ => {}
        }
    }

    let slot_id = slot_id.unwrap_or(1);
    let slot = {
        let db = state.db.lock().unwrap();
        get_agent_slot(&db, slot_id).unwrap_or(None)
    };
    let Some(slot) = slot else {
        return Redirect::to("/agents").into_response();
    };
    let body = body.trim().to_string();
    if body.len() > MAX_AGENT_MESSAGE_CHARS {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    let attachment_id = if let Some(upload) = upload {
        save_agent_upload(&state, &slot, upload).await.ok()
    } else {
        None
    };
    if body.is_empty() && attachment_id.is_none() {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    {
        let db = state.db.lock().unwrap();
        let _ = append_agent_message(&db, slot.id, "user", &body, attachment_id);
    }
    if handle_agent_control(&state, &slot, &body) {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    if state.agent_jobs.lock().unwrap().contains_key(&slot.id) {
        let db = state.db.lock().unwrap();
        let _ = append_agent_message(
            &db,
            slot.id,
            "assistant",
            &format!(
                "{} is already running. Use another slot or send `!stop` first.",
                slot.name
            ),
            None,
        );
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    let request_body = if body.is_empty() {
        "Please inspect the attached file.".to_string()
    } else {
        body
    };
    start_agent_job(state.clone(), slot.id, request_body, attachment_id);
    Redirect::to(&agent_location(Some(slot.id))).into_response()
}

async fn agent_slot_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    let messages = {
        let db = state.db.lock().unwrap();
        list_agent_messages(&db, id).unwrap_or_default()
    };
    let run = agent_run_for(&state, id);
    let active_status = {
        let db = state.db.lock().unwrap();
        get_agent_slot(&db, id)
            .unwrap_or(None)
            .map(|slot| agent_slot_runtime(&state, &slot).label)
            .unwrap_or_else(|| "idle".into())
    };
    Json(AgentSlotPoll {
        running: run.is_some(),
        current: run.map(|run| run.current).unwrap_or_default(),
        message_count: messages.len(),
        messages_html: agent_messages_html(&messages),
        active_status,
    })
    .into_response()
}

async fn agent_attachment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    let row = {
        let db = state.db.lock().unwrap();
        db.query_row(
            "SELECT original_name, content_type, file_path, size_bytes FROM agent_attachments WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    PathBuf::from(row.get::<_, String>(2)?),
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .unwrap_or(None)
    };
    let Some((filename, content_type, path, size_bytes)) = row else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if size_bytes < 0 || size_bytes as usize > MAX_AGENT_UPLOAD_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "file too large").into_response();
    }
    let Ok(path) = path.canonicalize() else {
        return (StatusCode::NOT_FOUND, "missing file").into_response();
    };
    let Ok(root) = state.config.agent_upload_dir.canonicalize() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "bad upload dir").into_response();
    };
    if !path.starts_with(root) || !path.is_file() {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return (StatusCode::NOT_FOUND, "missing file").into_response();
    };
    let ascii_name = filename
        .chars()
        .filter(|ch| ch.is_ascii() && *ch != '"')
        .collect::<String>();
    let disposition = if content_type.starts_with("image/") {
        format!(
            "inline; filename=\"{}\"",
            if ascii_name.is_empty() {
                "attachment"
            } else {
                &ascii_name
            }
        )
    } else {
        format!(
            "attachment; filename=\"{}\"",
            if ascii_name.is_empty() {
                "attachment"
            } else {
                &ascii_name
            }
        )
    };
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(&content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&disposition).unwrap(),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        bytes,
    )
        .into_response()
}

fn agent_run_for(state: &AppState, slot_id: i64) -> Option<AgentRun> {
    state.agent_jobs.lock().unwrap().get(&slot_id).cloned()
}

fn agent_slot_runtime(state: &AppState, slot: &AgentSlotRow) -> SlotRuntime {
    if let Some(run) = agent_run_for(state, slot.id) {
        let label = if run.current.trim().is_empty() {
            run.status
        } else {
            run.current
        };
        return SlotRuntime { label };
    }
    SlotRuntime {
        label: "idle".into(),
    }
}

fn agent_messages_html(messages: &[AgentMessageRow]) -> String {
    if messages.is_empty() {
        return r#"<p class="empty">No messages in this slot yet.</p>"#.into();
    }
    messages
        .iter()
        .rev()
        .map(|message| {
            let avatar = if message.role == "user" { "U" } else { "A" };
            let body = if message.body.trim().is_empty() {
                String::new()
            } else {
                format!(
                    r#"<p>{}</p>"#,
                    html_escape(&message.body).replace('\n', "<br>")
                )
            };
            let attachment = message
                .attachment
                .as_ref()
                .map(|attachment| {
                    let name = html_escape(&attachment.name);
                    if attachment.is_image {
                        format!(
                            r#"<a class="attachment-link" href="/agents/attachments/{}">{}</a><img class="message-image" src="/agents/attachments/{}" alt="">"#,
                            attachment.id, name, attachment.id
                        )
                    } else {
                        format!(
                            r#"<a class="attachment-link" href="/agents/attachments/{}">{}</a>"#,
                            attachment.id, name
                        )
                    }
                })
                .unwrap_or_default();
            format!(
                r#"<article class="message">
  <div class="message-avatar">{avatar}</div>
  <div class="message-body">
    <div class="message-meta"><strong>{}</strong><span class="message-log">{}</span></div>
    {}{}
  </div>
</article>"#,
                html_escape(&message.role),
                html_escape(&message.created_at),
                body,
                attachment
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn codex_browser_drawer_html(
    projects: &[CodexProject],
    conversations: &[CodexConversation],
    slot_id: i64,
    active_thread: Option<&str>,
    loaded: bool,
) -> String {
    let mut groups = Vec::new();
    for project in projects
        .iter()
        .filter(|project| project.conversation_count > 0)
        .take(CODEX_PROJECT_DRAWER_LIMIT)
    {
        let project_conversations = conversations
            .iter()
            .filter(|conversation| conversation.cwd == project.path)
            .take(CODEX_PROJECT_CONVERSATION_LIMIT)
            .collect::<Vec<_>>();
        if project_conversations.is_empty() {
            continue;
        }
        let rows = project_conversations
            .iter()
            .enumerate()
            .map(|(index, conversation)| {
                let active_class = if Some(conversation.id.as_str()) == active_thread {
                    " active"
                } else {
                    ""
                };
                let extra_attr = if index >= CODEX_PROJECT_VISIBLE_CONVERSATIONS {
                    " data-extra-conversation hidden"
                } else {
                    ""
                };
                let title = html_escape(&conversation.title);
                let preview = html_escape(&truncate_text(&conversation.preview, 92));
                let updated = html_escape(&short_time(&conversation.updated_at));
                let workdir = html_escape(&conversation.cwd);
                let thread_id = html_escape(&conversation.id);
                format!(
                    r#"<form class="browser-conversation-row{active_class}"{extra_attr} action="/agents/slots/{slot_id}/conversation" method="post">
  <input type="hidden" name="thread_id" value="{thread_id}">
  <input type="hidden" name="workdir" value="{workdir}">
  <button type="submit"><strong>{title}</strong><span>{updated} · {} msgs · {preview}</span></button>
</form>"#,
                    conversation.message_count
                )
            })
            .collect::<Vec<_>>()
            .join("");
        let hidden = project_conversations
            .len()
            .saturating_sub(CODEX_PROJECT_VISIBLE_CONVERSATIONS);
        let more = if hidden > 0 {
            format!(
                r#"<button type="button" class="ghost browser-more" data-load-more>Load more ({hidden})</button>"#
            )
        } else {
            String::new()
        };
        let trust = if project.trusted {
            "trusted"
        } else {
            "project"
        };
        groups.push(format!(
            r#"<section class="browser-project" data-project-group>
  <div class="browser-project-head" title="{}"><strong>{}</strong><span>{} chats · {trust}</span></div>
  <div class="browser-conversations">{rows}</div>
  {more}
</section>"#,
            html_escape(&project.path),
            html_escape(&project.name),
            project.conversation_count
        ));
    }
    let body = if !loaded {
        r#"<p class="empty">Loading saved Codex conversations...</p>"#.into()
    } else if groups.is_empty() {
        r#"<p class="empty">No saved Codex conversations found yet.</p>"#.into()
    } else {
        groups.join("")
    };
    format!(
        r#"<dialog class="browser-drawer" id="conversationDrawer">
  <header><div><strong>Conversations</strong><br><span>Projects with saved Codex sessions</span></div><button type="button" class="icon" data-browser-close aria-label="Close">x</button></header>
  <main class="browser-scroll">{body}</main>
</dialog>"#
    )
}

fn codex_usage_dialog(config: &Config, usage: Option<&CodexUsageSnapshot>, loaded: bool) -> String {
    let command = html_escape(&agent_command_label(config));
    let reset_available = usage
        .and_then(|usage| usage.reset_credits.as_ref())
        .is_some_and(|credits| credits.available_count > 0);
    let reset = if config.codex_reset_command.is_some() || reset_available {
        r#"<form class="reset-form" action="/agents/codex/reset" method="post" data-reset-form>
  <input type="hidden" name="confirm" value="USE_RESET">
  <button type="submit" class="danger-icon ghost">Use reset</button>
</form>"#
            .to_string()
    } else {
        r#"<button type="button" class="ghost" disabled title="No Codex reset credits are currently reported.">Use reset</button>"#.to_string()
    };
    let body = if let Some(usage) = usage {
        let primary = usage_window_card("Primary", usage.primary.as_ref());
        let secondary = usage_window_card("Secondary", usage.secondary.as_ref());
        let reset_credit_text = codex_reset_credit_text(usage.reset_credits.as_ref());
        let reset_lines = [
            usage_reset_line("Primary", usage.primary.as_ref()),
            usage_reset_line("Secondary", usage.secondary.as_ref()),
        ]
        .join("");
        format!(
            r#"<p class="muted">Launcher: <strong>{command}</strong></p>
<section class="usage-total">
  <strong>Total usage</strong>
  <span>Plan: {}</span>
  <span>Total tokens recorded: {}</span>
  <span>Last turn: {} · cached input: {}</span>
  <span>Context window: {}</span>
  <span>Add-on credits: {}</span>
  <span>Usage reset credits: {reset_credit_text}</span>
</section>
<div class="usage-grid">{primary}{secondary}</div>
<section class="usage-total"><strong>Reset windows</strong>{reset_lines}</section>
<p class="muted">Observed {}</p>
            {reset}"#,
            html_escape(&usage.plan_type),
            format_number(usage.total_units),
            format_number(usage.last_units),
            format_number(usage.cached_input_units),
            format_number(usage.context_window),
            html_escape(usage.credits.as_deref().unwrap_or("not reported")),
            html_escape(&short_time(&usage.observed_at)),
        )
    } else if loaded {
        format!(
            r#"<p class="muted">No Codex usage event has been recorded yet. Open `/status` in Codex once and Mobailmux will show the saved rate-limit data here.</p><p>Launcher: <strong>{command}</strong></p>{reset}"#
        )
    } else {
        format!(
            r#"<p class="muted">Loading Codex usage from saved sessions...</p><p>Launcher: <strong>{command}</strong></p>{reset}"#
        )
    };
    format!(
        r#"<dialog class="codex-panel" id="codexPanel">
  <header><strong>Codex Usage</strong><button type="button" class="icon" data-codex-close aria-label="Close">x</button></header>
  <main>{body}</main>
</dialog>"#
    )
}

fn usage_window_card(fallback_label: &str, window: Option<&CodexRateWindow>) -> String {
    let Some(window) = window else {
        return format!(
            r#"<section class="usage-card"><strong>{fallback_label}</strong><span class="muted">not reported</span></section>"#
        );
    };
    let used = window.used_percent.clamp(0.0, 100.0);
    let remaining = window.remaining_percent.clamp(0.0, 100.0);
    let reset = window
        .resets_at
        .and_then(epoch_to_rfc3339)
        .map(|value| short_time(&value))
        .unwrap_or_else(|| "unknown".into());
    format!(
        r#"<section class="usage-card"><strong>{}</strong><span>{:.0}% remaining · {:.0}% used</span><div class="meter" title="{:.0}% remaining"><span style="width:{:.0}%"></span></div><span class="muted">{} window · resets {}</span></section>"#,
        html_escape(&window.label),
        remaining,
        used,
        remaining,
        remaining,
        html_escape(&usage_window_duration(window.window_minutes)),
        html_escape(&reset)
    )
}

fn usage_reset_line(fallback_label: &str, window: Option<&CodexRateWindow>) -> String {
    let Some(window) = window else {
        return format!(r#"<span>{fallback_label}: not reported</span>"#);
    };
    let reset = window
        .resets_at
        .and_then(epoch_to_rfc3339)
        .map(|value| short_time(&value))
        .unwrap_or_else(|| "unknown".into());
    format!(
        r#"<span>{}: {} window resets {}</span>"#,
        html_escape(&window.label),
        html_escape(&usage_window_duration(window.window_minutes)),
        html_escape(&reset)
    )
}

fn codex_reset_credit_text(credits: Option<&CodexResetCreditsSummary>) -> String {
    let Some(credits) = credits else {
        return "not reported by Codex".into();
    };
    let count = credits.available_count;
    let label = if count == 1 { "credit" } else { "credits" };
    let Some(estimate) = &credits.estimate else {
        return format!("{count} {label} available · local expiry tracking not initialized yet");
    };
    let mut parts = vec![format!("{count} {label} available")];
    if estimate.tracked_available_count > 0 {
        let tracked_label = if estimate.tracked_available_count == 1 {
            "tracked credit"
        } else {
            "tracked credits"
        };
        let mut tracked = format!("{} {tracked_label}", estimate.tracked_available_count);
        if let Some(next_expires_at) = &estimate.next_expires_at {
            tracked.push_str(&format!(" · next expires {}", short_time(next_expires_at)));
        }
        parts.push(tracked);
    }
    if estimate.untracked_available_count > 0 {
        let untracked_label = if estimate.untracked_available_count == 1 {
            "existing credit"
        } else {
            "existing credits"
        };
        parts.push(format!(
            "{} {untracked_label} from before tracking; expiry unknown",
            estimate.untracked_available_count
        ));
    }
    if estimate.tracked_available_count == 0 && estimate.untracked_available_count == 0 {
        parts.push("tracking future grants".into());
    }
    parts.join(" · ")
}

fn usage_window_duration(minutes: i64) -> String {
    if minutes >= 1440 && minutes % 1440 == 0 {
        return format!("{}d", minutes / 1440);
    }
    if minutes >= 60 && minutes % 60 == 0 {
        return format!("{}h", minutes / 60);
    }
    format!("{minutes}m")
}

fn codex_usage_text(usage: Option<&CodexUsageSnapshot>) -> String {
    let Some(usage) = usage else {
        return "Codex usage: no saved `/status` data found yet.".into();
    };
    let mut lines = vec![
        "Codex usage:".to_string(),
        format!("- observed: {}", short_time(&usage.observed_at)),
        format!("- plan: {}", usage.plan_type),
        format!("- total tokens: {}", format_number(usage.total_units)),
        format!("- last turn: {}", format_number(usage.last_units)),
        format!(
            "- cached input: {}",
            format_number(usage.cached_input_units)
        ),
        format!("- context window: {}", format_number(usage.context_window)),
        format!(
            "- add-on credits: {}",
            usage.credits.as_deref().unwrap_or("not reported")
        ),
        format!(
            "- usage reset credits: {}",
            codex_reset_credit_text(usage.reset_credits.as_ref())
        ),
    ];
    if let Some(primary) = &usage.primary {
        lines.push(format!(
            "- primary: {:.0}% left, {:.0}% used, resets {}",
            primary.remaining_percent,
            primary.used_percent,
            primary
                .resets_at
                .and_then(epoch_to_rfc3339)
                .map(|value| short_time(&value))
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    if let Some(secondary) = &usage.secondary {
        lines.push(format!(
            "- secondary: {:.0}% left, {:.0}% used, resets {}",
            secondary.remaining_percent,
            secondary.used_percent,
            secondary
                .resets_at
                .and_then(epoch_to_rfc3339)
                .map(|value| short_time(&value))
                .unwrap_or_else(|| "unknown".into())
        ));
    }
    lines.join("\n")
}

fn codex_transcript_html(index: &CodexIndex, thread_id: &str) -> io::Result<String> {
    let Some(conversation) = codex_conversation_by_id(&index, thread_id) else {
        return Ok(r#"<p class="empty">Conversation not found.</p>"#.into());
    };
    let messages = codex_transcript_messages(&conversation.path)?;
    if messages.is_empty() {
        return Ok(r#"<p class="empty">This Codex conversation has no visible user or assistant messages yet.</p>"#.into());
    }
    Ok(agent_messages_html(&messages))
}

fn codex_transcript_count(html: &str) -> Option<usize> {
    let count = html.matches("<article class=\"message\">").count();
    (count > 0).then_some(count)
}

fn list_agent_slots(db: &Connection) -> rusqlite::Result<Vec<AgentSlotRow>> {
    let mut stmt = db.prepare("SELECT id, name, workdir FROM agent_slots ORDER BY id ASC")?;
    stmt.query_map([], |row| {
        Ok(AgentSlotRow {
            id: row.get(0)?,
            name: row.get(1)?,
            workdir: row.get(2)?,
        })
    })?
    .collect()
}

fn get_agent_slot(db: &Connection, id: i64) -> rusqlite::Result<Option<AgentSlotRow>> {
    db.query_row(
        "SELECT id, name, workdir FROM agent_slots WHERE id = ?1",
        params![id],
        |row| {
            Ok(AgentSlotRow {
                id: row.get(0)?,
                name: row.get(1)?,
                workdir: row.get(2)?,
            })
        },
    )
    .optional()
}

fn ensure_agent_slot(db: &Connection, name: &str, workdir: &Path) -> rusqlite::Result<i64> {
    let existing = db
        .query_row(
            "SELECT id FROM agent_slots WHERE name = ?1",
            params![name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    db.execute(
        "INSERT INTO agent_slots (name, workdir, created_at) VALUES (?1, ?2, ?3)",
        params![name, workdir.to_string_lossy(), Utc::now().to_rfc3339()],
    )?;
    Ok(db.last_insert_rowid())
}

fn ensure_agent_slot_seeds(
    db: &Connection,
    seeds: &[AgentSlotSeed],
    default_workdir: &Path,
) -> rusqlite::Result<()> {
    if seeds.is_empty() {
        for name in DEFAULT_AGENT_SLOTS.split(',') {
            ensure_agent_slot(db, name, default_workdir)?;
        }
        return Ok(());
    }
    for seed in seeds {
        ensure_agent_slot(db, &seed.name, &seed.workdir)?;
    }
    Ok(())
}

fn list_agent_messages(db: &Connection, slot_id: i64) -> rusqlite::Result<Vec<AgentMessageRow>> {
    let mut stmt = db.prepare(
        "SELECT m.role, m.body, m.created_at, a.id, a.original_name, a.content_type, a.size_bytes
         FROM agent_messages m
         LEFT JOIN agent_attachments a ON a.id = m.attachment_id
         WHERE m.slot_id = ?1
         ORDER BY m.id DESC
         LIMIT 200",
    )?;
    stmt.query_map(params![slot_id], |row| {
        let attachment_id = row.get::<_, Option<i64>>(3)?;
        let attachment = if let Some(id) = attachment_id {
            let content_type = row.get::<_, String>(5)?;
            Some(AgentAttachmentSummary {
                id,
                name: row.get(4)?,
                is_image: content_type.starts_with("image/"),
                content_type,
                size_bytes: row.get(6)?,
            })
        } else {
            None
        };
        Ok(AgentMessageRow {
            role: row.get(0)?,
            body: row.get(1)?,
            created_at: row.get(2)?,
            attachment,
        })
    })?
    .collect()
}

fn append_agent_message(
    db: &Connection,
    slot_id: i64,
    role: &str,
    body: &str,
    attachment_id: Option<i64>,
) -> rusqlite::Result<i64> {
    db.execute(
        "INSERT INTO agent_messages (slot_id, role, body, attachment_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![slot_id, role, body, attachment_id, Utc::now().to_rfc3339()],
    )?;
    Ok(db.last_insert_rowid())
}

fn append_agent_assistant(state: &AppState, slot_id: i64, body: &str) {
    let db = state.db.lock().unwrap();
    let _ = append_agent_message(&db, slot_id, "assistant", body, None);
}

async fn save_agent_upload(
    state: &AppState,
    slot: &AgentSlotRow,
    upload: PendingUpload,
) -> io::Result<i64> {
    let safe_name = sanitize_filename(&upload.filename);
    let stored_name = format!("{}-{safe_name}", Uuid::new_v4().simple());
    let slot_dir = state
        .config
        .agent_upload_dir
        .join(format!("slot-{}", slot.id));
    tokio::fs::create_dir_all(&slot_dir).await?;
    let path = slot_dir.join(&stored_name);
    tokio::fs::write(&path, &upload.bytes).await?;
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO agent_attachments (slot_id, original_name, stored_name, content_type, file_path, size_bytes, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            slot.id,
            upload.filename,
            stored_name,
            upload.content_type,
            path.to_string_lossy(),
            upload.bytes.len() as i64,
            Utc::now().to_rfc3339()
        ],
    )
    .map_err(io_other)?;
    Ok(db.last_insert_rowid())
}

fn handle_agent_control(state: &Arc<AppState>, slot: &AgentSlotRow, body: &str) -> bool {
    let trimmed = body.trim();
    let Some(text) = trimmed.strip_prefix('!').map(str::trim) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    if matches!(lower.as_str(), "help" | "commands") {
        append_agent_assistant(state, slot.id, &agent_help_text(state));
        return true;
    }
    if matches!(lower.as_str(), "slots" | "list" | "overview") {
        append_agent_assistant(state, slot.id, &agent_slots_status_text(state));
        return true;
    }
    if lower == "pwd" {
        append_agent_assistant(
            state,
            slot.id,
            &format!("{} folder: `{}`", slot.name, slot.workdir),
        );
        return true;
    }
    if lower == "status" {
        let runtime = agent_slot_runtime(state, slot);
        let usage = codex_index_snapshot(state).and_then(|index| index.usage);
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{} is {}. Folder: `{}`\n{}",
                slot.name,
                runtime.label,
                slot.workdir,
                codex_usage_text(usage.as_ref())
            ),
        );
        return true;
    }
    if lower == "usage" || lower == "limits" {
        let usage = codex_index_snapshot(state).and_then(|index| index.usage);
        append_agent_assistant(state, slot.id, &codex_usage_text(usage.as_ref()));
        return true;
    }
    if lower == "model" || lower == "settings" {
        append_agent_assistant(
            state,
            slot.id,
            &format!("Agent command: `{}`", agent_command_label(&state.config)),
        );
        return true;
    }
    if matches!(lower.as_str(), "fresh" | "new" | "stayfresh") {
        let keep_workdir = lower == "stayfresh";
        let stopped = stop_agent_job(state, slot.id);
        let workdir = if keep_workdir {
            slot.workdir.clone()
        } else {
            state
                .config
                .agent_default_workdir
                .to_string_lossy()
                .to_string()
        };
        {
            let db = state.db.lock().unwrap();
            if !keep_workdir {
                let _ = db.execute(
                    "UPDATE agent_slots SET workdir = ?1 WHERE id = ?2",
                    params![workdir, slot.id],
                );
            }
            let _ = db.execute(
                "DELETE FROM agent_sessions WHERE slot_id = ?1",
                params![slot.id],
            );
            let _ = db.execute(
                "DELETE FROM agent_messages WHERE slot_id = ?1",
                params![slot.id],
            );
        }
        let folder_text = if keep_workdir {
            format!("Folder kept at `{workdir}`.")
        } else {
            format!("Folder reset to `{workdir}`.")
        };
        let stop_text = if stopped {
            " Stopped the current job."
        } else {
            ""
        };
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{} chat reset. {folder_text}{stop_text} Your next message starts a new agent chat.",
                slot.name
            ),
        );
        return true;
    }
    if lower == "stop" {
        let stopped = stop_agent_job(state, slot.id);
        append_agent_assistant(
            state,
            slot.id,
            if stopped {
                "Stop requested."
            } else {
                "This slot is not running."
            },
        );
        return true;
    }
    if let Some(arg) = command_arg(text, "cd") {
        let target = if arg.trim().is_empty() {
            default_home_dir()
        } else {
            resolve_agent_path(arg, &slot.workdir)
        };
        if target.is_dir() {
            let db = state.db.lock().unwrap();
            let _ = db.execute(
                "UPDATE agent_slots SET workdir = ?1 WHERE id = ?2",
                params![target.to_string_lossy(), slot.id],
            );
            let _ = db.execute(
                "DELETE FROM agent_sessions WHERE slot_id = ?1",
                params![slot.id],
            );
            drop(db);
            append_agent_assistant(
                state,
                slot.id,
                &format!("{} folder set to `{}`", slot.name, target.display()),
            );
        } else {
            append_agent_assistant(
                state,
                slot.id,
                &format!("Folder does not exist: `{}`", target.display()),
            );
        }
        return true;
    }
    if let Some(arg) = command_arg(text, "ls") {
        append_agent_assistant(state, slot.id, &list_agent_path_text(slot, arg));
        return true;
    }
    append_agent_assistant(
        state,
        slot.id,
        &format!("Unknown Mobailmux agent command: `{text}`. Type `!help` for commands."),
    );
    true
}

fn stop_agent_job(state: &AppState, slot_id: i64) -> bool {
    let cancel = state.agent_cancels.lock().unwrap().remove(&slot_id);
    if let Some(cancel) = cancel {
        let _ = cancel.send(());
        true
    } else {
        false
    }
}

fn agent_help_text(state: &AppState) -> String {
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let mut lines = vec!["Agent commands:".to_string()];
    lines.extend(
        [
            "- `!help` or `!commands`",
            "- `!slots`",
            "- `!pwd`",
            "- `!ls [path]`",
            "- `!cd [path]`",
            "- `!fresh`",
            "- `!stayfresh`",
            "- `!status`",
            "- `!usage`",
            "- `!stop`",
            "- `!model`",
        ]
        .into_iter()
        .map(str::to_string),
    );
    lines.push(String::new());
    lines.push("Slots:".into());
    for slot in slots {
        lines.push(format!("- `{}` at `{}`", slot.name, slot.workdir));
    }
    lines.join("\n")
}

fn agent_slots_status_text(state: &AppState) -> String {
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let jobs = state.agent_jobs.lock().unwrap();
    let lines = slots
        .iter()
        .map(|slot| {
            let state = if jobs.contains_key(&slot.id) {
                "running"
            } else {
                "idle"
            };
            format!("{}: {state} | {}", slot.name, slot.workdir)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Slots:\n```text\n{lines}\n```")
}

fn list_agent_path_text(slot: &AgentSlotRow, arg: &str) -> String {
    let target = if arg.trim().is_empty() {
        PathBuf::from(&slot.workdir)
    } else {
        resolve_agent_path(arg, &slot.workdir)
    };
    if !target.exists() {
        return format!("Path does not exist: `{}`", target.display());
    }
    if target.is_file() {
        return format!("{} file:\n```text\n{}\n```", slot.name, target.display());
    }
    if !target.is_dir() {
        return format!("Path is not a directory: `{}`", target.display());
    }
    let mut rows = match fs::read_dir(&target) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    return None;
                }
                let suffix = if path.is_dir() { "/" } else { "" };
                Some((
                    if path.is_dir() { 0 } else { 1 },
                    name.to_lowercase(),
                    format!("{name}{suffix}"),
                ))
            })
            .collect::<Vec<_>>(),
        Err(err) => return format!("Could not list `{}`: {err}", target.display()),
    };
    rows.sort();
    let mut names = rows
        .into_iter()
        .take(120)
        .map(|(_, _, name)| name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        names.push("(empty)".into());
    }
    format!(
        "{} listing `{}`:\n```text\n{}\n```",
        slot.name,
        target.display(),
        names.join("\n")
    )
}

fn load_codex_index(config: &Config) -> CodexIndex {
    let thread_names = load_codex_thread_names(&config.codex_home);
    let mut files = collect_codex_session_files(&config.codex_home);
    files.sort_by_key(|path| Reverse(file_modified(path)));
    files.truncate(CODEX_SESSION_SCAN_LIMIT);

    let mut conversations = files
        .iter()
        .filter_map(|path| codex_conversation_from_file(path, &thread_names))
        .collect::<Vec<_>>();
    conversations.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    conversations.truncate(MAX_CODEX_CONVERSATIONS);

    let mut projects = configured_codex_projects(&config.codex_home);
    for conversation in &conversations {
        projects
            .entry(conversation.cwd.clone())
            .and_modify(|project| project.conversation_count += 1)
            .or_insert_with(|| CodexProject {
                path: conversation.cwd.clone(),
                name: project_display_name(&conversation.cwd),
                trusted: false,
                conversation_count: 1,
            });
    }
    for project in projects.values_mut() {
        if project.conversation_count == 0 {
            project.conversation_count = conversations
                .iter()
                .filter(|conversation| conversation.cwd == project.path)
                .count();
        }
    }
    let mut projects = projects.into_values().collect::<Vec<_>>();
    projects.sort_by(|left, right| {
        right
            .conversation_count
            .cmp(&left.conversation_count)
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });

    let usage = merge_codex_rate_limit_status(latest_codex_usage(&files), &config);

    CodexIndex {
        usage,
        conversations,
        projects,
    }
}

fn load_codex_thread_names(codex_home: &Path) -> HashMap<String, (String, String)> {
    let path = codex_home.join("session_index.jsonl");
    let Ok(file) = fs::File::open(path) else {
        return HashMap::new();
    };
    let reader = io::BufReader::new(file);
    let mut names = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        let title = value
            .get("thread_name")
            .and_then(|value| value.as_str())
            .unwrap_or("Untitled")
            .to_string();
        let updated_at = value
            .get("updated_at")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        names.insert(id.to_string(), (title, updated_at));
    }
    names
}

fn collect_codex_session_files(codex_home: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_files(&codex_home.join("sessions"), 5, &mut files);
    let index = codex_home.join("history.jsonl");
    if index.exists() {
        files.push(index);
    }
    files
}

fn collect_jsonl_files(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth == 0 || dir.to_string_lossy().contains("/.tmp/") {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, depth - 1, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

fn codex_conversation_from_file(
    path: &Path,
    thread_names: &HashMap<String, (String, String)>,
) -> Option<CodexConversation> {
    if path.file_name().and_then(|name| name.to_str()) == Some("history.jsonl") {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let reader = io::BufReader::new(file);
    let mut id = None::<String>;
    let mut cwd = None::<String>;
    let mut started_at = None::<String>;
    let mut updated_at = None::<String>;
    let mut first_user = None::<String>;
    let mut last_message = None::<String>;
    let mut message_count = 0usize;

    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(timestamp) = value.get("timestamp").and_then(|value| value.as_str()) {
            updated_at = Some(timestamp.to_string());
        }
        if value.get("type").and_then(|value| value.as_str()) == Some("session_meta") {
            let payload = value.get("payload").unwrap_or(&serde_json::Value::Null);
            id = payload
                .get("id")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            cwd = payload
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            started_at = payload
                .get("timestamp")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            continue;
        }
        let Some((role, text)) = codex_visible_message(&value) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        message_count += 1;
        if role == "user" && first_user.is_none() {
            first_user = Some(text.clone());
        }
        last_message = Some(text);
    }

    let id = id?;
    let cwd = cwd.unwrap_or_else(|| default_home_dir().to_string_lossy().into_owned());
    let (indexed_title, indexed_updated) = thread_names
        .get(&id)
        .cloned()
        .unwrap_or_else(|| (String::new(), String::new()));
    let indexed_title = indexed_title.trim();
    let title = if indexed_title.is_empty() || is_codex_synthetic_user_text(indexed_title) {
        first_user
            .as_deref()
            .map(|value| truncate_text(value, 56))
            .unwrap_or_else(|| "Untitled Codex conversation".into())
    } else {
        indexed_title.to_string()
    };
    let updated_at = if !indexed_updated.trim().is_empty() {
        indexed_updated
    } else {
        updated_at
            .or_else(|| started_at.clone())
            .unwrap_or_else(|| system_time_to_rfc3339(file_modified(path)))
    };
    Some(CodexConversation {
        id,
        title,
        cwd,
        updated_at,
        path: path.to_path_buf(),
        preview: first_user.or(last_message).unwrap_or_default(),
        message_count,
    })
}

fn codex_transcript_messages(path: &Path) -> io::Result<Vec<AgentMessageRow>> {
    let file = fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut rows = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let timestamp = value
            .get("timestamp")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let Some((role, text)) = codex_visible_message(&value) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        rows.push(AgentMessageRow {
            role,
            body: text,
            created_at: timestamp,
            attachment: None,
        });
    }
    if rows.len() > MAX_CODEX_TRANSCRIPT_MESSAGES {
        let start = rows.len() - MAX_CODEX_TRANSCRIPT_MESSAGES;
        rows = rows.split_off(start);
    }
    rows.reverse();
    Ok(rows)
}

fn codex_visible_message(value: &serde_json::Value) -> Option<(String, String)> {
    if value.get("type").and_then(|value| value.as_str()) != Some("response_item") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(|value| value.as_str()) != Some("message") {
        return None;
    }
    let role = payload.get("role").and_then(|value| value.as_str())?;
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let text = codex_content_text(payload.get("content")?);
    if role == "user" && is_codex_synthetic_user_text(&text) {
        return None;
    }
    Some((role.to_string(), text))
}

fn is_codex_synthetic_user_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("# AGENTS.md instructions for")
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.starts_with("<permissions instructions>")
        || trimmed.starts_with("<collaboration_mode>")
        || trimmed.starts_with("<apps_instructions>")
        || trimmed.starts_with("<skills_instructions>")
        || trimmed.starts_with("<plugins_instructions>")
        || (trimmed.contains("<environment_context>") && trimmed.contains("<cwd>"))
}

fn codex_content_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let Some(items) = content.as_array() else {
        return String::new();
    };
    items
        .iter()
        .filter_map(|item| {
            item.get("text")
                .or_else(|| item.get("content"))
                .and_then(|value| value.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn latest_codex_usage(files: &[PathBuf]) -> Option<CodexUsageSnapshot> {
    let mut latest = None::<(String, CodexUsageSnapshot)>;
    for path in files.iter().take(CODEX_SESSION_SCAN_LIMIT) {
        let Ok(file) = fs::File::open(path) else {
            continue;
        };
        let reader = io::BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value.get("type").and_then(|value| value.as_str()) != Some("event_msg") {
                continue;
            }
            let payload = value.get("payload").unwrap_or(&serde_json::Value::Null);
            if payload.get("type").and_then(|value| value.as_str()) != Some("token_count") {
                continue;
            }
            let observed_at = value
                .get("timestamp")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let snapshot = codex_usage_from_payload(&observed_at, payload);
            if latest
                .as_ref()
                .is_none_or(|(timestamp, _)| observed_at.as_str() > timestamp.as_str())
            {
                latest = Some((observed_at, snapshot));
            }
        }
    }
    latest.map(|(_, snapshot)| snapshot)
}

fn merge_codex_rate_limit_status(
    usage: Option<CodexUsageSnapshot>,
    config: &Config,
) -> Option<CodexUsageSnapshot> {
    let Some(result) = fetch_codex_rate_limits(config) else {
        return usage;
    };
    let rate_limits = result
        .get("rateLimits")
        .or_else(|| result.get("rate_limits"))
        .unwrap_or(&serde_json::Value::Null);
    let mut usage = usage.unwrap_or_else(|| CodexUsageSnapshot {
        observed_at: Utc::now().to_rfc3339(),
        plan_type: "unknown".into(),
        total_units: 0,
        last_units: 0,
        cached_input_units: 0,
        context_window: 0,
        primary: None,
        secondary: None,
        credits: None,
        reset_credits: None,
    });
    usage.observed_at = Utc::now().to_rfc3339();
    if let Some(plan_type) = rate_limits
        .get("planType")
        .or_else(|| rate_limits.get("plan_type"))
        .and_then(|value| value.as_str())
    {
        usage.plan_type = plan_type.to_string();
    }
    usage.primary = codex_rate_window("Primary", rate_limits.get("primary")).or(usage.primary);
    usage.secondary =
        codex_rate_window("Secondary", rate_limits.get("secondary")).or(usage.secondary);
    usage.credits = codex_credits_text(rate_limits.get("credits")).or(usage.credits);
    usage.reset_credits = codex_reset_credits_summary(
        result
            .get("rateLimitResetCredits")
            .or_else(|| result.get("rate_limit_reset_credits")),
    )
    .or(usage.reset_credits);
    Some(usage)
}

fn fetch_codex_rate_limits(config: &Config) -> Option<serde_json::Value> {
    let initialize = serde_json::json!({
        "id": 0,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "mobailmux", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
        }
    });
    let read_limits = serde_json::json!({
        "id": 1,
        "method": "account/rateLimits/read",
        "params": null
    });
    let input = format!("{initialize}\n{{\"method\":\"initialized\"}}\n{read_limits}\n");
    let output = codex_app_server_request(config, &input)?;
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value.get("id").and_then(|value| value.as_i64()) == Some(1))
        .and_then(|value| value.get("result").cloned())
}

fn consume_codex_rate_limit_reset_credit(config: &Config) -> Option<String> {
    let initialize = serde_json::json!({
        "id": 0,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "mobailmux", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
        }
    });
    let consume = serde_json::json!({
        "id": 1,
        "method": "account/rateLimitResetCredit/consume",
        "params": {"idempotencyKey": Uuid::new_v4().to_string()}
    });
    let input = format!("{initialize}\n{{\"method\":\"initialized\"}}\n{consume}\n");
    let output = codex_app_server_request(config, &input)?;
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value.get("id").and_then(|value| value.as_i64()) == Some(1))
        .and_then(|value| {
            value
                .get("result")
                .and_then(|result| result.get("outcome"))
                .and_then(|outcome| outcome.as_str())
                .map(str::to_string)
        })
}

fn codex_app_server_request(config: &Config, input: &str) -> Option<String> {
    let mut command = StdCommand::new(&config.agent_codex_bin);
    command
        .args(&config.agent_codex_args)
        .arg("app-server")
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    {
        let mut stdin = child.stdin.take()?;
        stdin.write_all(input.as_bytes()).ok()?;
        stdin.flush().ok()?;
        std::thread::sleep(CODEX_APP_SERVER_WRITE_SETTLE);
    }
    let started = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if started.elapsed() >= CODEX_APP_SERVER_TIMEOUT {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(StdDuration::from_millis(100));
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn codex_usage_from_payload(observed_at: &str, payload: &serde_json::Value) -> CodexUsageSnapshot {
    let info = payload.get("info").unwrap_or(&serde_json::Value::Null);
    let rate_limits = payload
        .get("rate_limits")
        .unwrap_or(&serde_json::Value::Null);
    let total = info
        .get("total_token_usage")
        .unwrap_or(&serde_json::Value::Null);
    let last = info
        .get("last_token_usage")
        .unwrap_or(&serde_json::Value::Null);
    CodexUsageSnapshot {
        observed_at: observed_at.to_string(),
        plan_type: rate_limits
            .get("plan_type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string(),
        total_units: total
            .get("total_tokens")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        last_units: last
            .get("total_tokens")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        cached_input_units: total
            .get("cached_input_tokens")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        context_window: info
            .get("model_context_window")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        primary: codex_rate_window("Primary", rate_limits.get("primary")),
        secondary: codex_rate_window("Secondary", rate_limits.get("secondary")),
        credits: codex_credits_text(rate_limits.get("credits")),
        reset_credits: codex_reset_credits_summary(
            rate_limits
                .get("rate_limit_reset_credits")
                .or_else(|| rate_limits.get("rateLimitResetCredits"))
                .or_else(|| payload.get("rate_limit_reset_credits"))
                .or_else(|| payload.get("rateLimitResetCredits")),
        ),
    }
}

fn codex_rate_window(label: &str, value: Option<&serde_json::Value>) -> Option<CodexRateWindow> {
    let value = value?;
    let used = value
        .get("used_percent")
        .or_else(|| value.get("usedPercent"))
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    Some(CodexRateWindow {
        label: label.into(),
        used_percent: used,
        remaining_percent: (100.0 - used).max(0.0),
        window_minutes: value
            .get("window_minutes")
            .or_else(|| value.get("windowDurationMins"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        resets_at: value
            .get("resets_at")
            .or_else(|| value.get("resetsAt"))
            .and_then(|value| value.as_i64()),
    })
}

fn codex_credits_text(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        None
    } else if let Some(text) = value.as_str() {
        Some(text.to_string())
    } else if let Some(balance) = value.get("balance").and_then(|value| value.as_str()) {
        let unlimited = value
            .get("unlimited")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if unlimited {
            Some("unlimited".into())
        } else {
            Some(balance.to_string())
        }
    } else {
        Some(value.to_string())
    }
}

fn codex_reset_credits_summary(
    value: Option<&serde_json::Value>,
) -> Option<CodexResetCreditsSummary> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let available_count = value
        .get("available_count")
        .or_else(|| value.get("availableCount"))
        .and_then(|value| value.as_i64())?;
    Some(CodexResetCreditsSummary {
        available_count,
        estimate: None,
    })
}

fn configured_codex_projects(codex_home: &Path) -> HashMap<String, CodexProject> {
    let mut projects = HashMap::new();
    let Ok(raw) = fs::read_to_string(codex_home.join("config.toml")) else {
        return projects;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&raw) else {
        return projects;
    };
    let Some(table) = value.get("projects").and_then(|value| value.as_table()) else {
        return projects;
    };
    for (path, details) in table {
        let trusted = details
            .get("trust_level")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("trusted"));
        projects.insert(
            path.to_string(),
            CodexProject {
                path: path.to_string(),
                name: project_display_name(path),
                trusted,
                conversation_count: 0,
            },
        );
    }
    projects
}

fn codex_conversation_by_id<'a>(
    index: &'a CodexIndex,
    thread_id: &str,
) -> Option<&'a CodexConversation> {
    index
        .conversations
        .iter()
        .find(|conversation| conversation.id == thread_id)
}

fn start_agent_job(
    state: Arc<AppState>,
    slot_id: i64,
    request_body: String,
    attachment_id: Option<i64>,
) {
    state.agent_jobs.lock().unwrap().insert(
        slot_id,
        AgentRun {
            status: "starting".into(),
            current: "starting".into(),
            started_at: Utc::now().to_rfc3339(),
        },
    );
    tokio::spawn(run_agent_job(state, slot_id, request_body, attachment_id));
}

async fn run_agent_job(
    state: Arc<AppState>,
    slot_id: i64,
    request_body: String,
    attachment_id: Option<i64>,
) {
    let slot = {
        let db = state.db.lock().unwrap();
        get_agent_slot(&db, slot_id).unwrap_or(None)
    };
    let Some(slot) = slot else {
        state.agent_jobs.lock().unwrap().remove(&slot_id);
        return;
    };
    append_agent_assistant(
        &state,
        slot_id,
        &format!("{} started in `{}`.", slot.name, slot.workdir),
    );
    let attachment = attachment_id.and_then(|id| {
        let db = state.db.lock().unwrap();
        agent_attachment_for_prompt(&db, id).unwrap_or(None)
    });
    let prompt = build_agent_prompt(&slot, &request_body, attachment.as_ref());
    let session = {
        let db = state.db.lock().unwrap();
        agent_session(&db, slot_id).unwrap_or(None)
    };
    let use_resume = session
        .as_ref()
        .is_some_and(|(_, workdir)| workdir == &slot.workdir);
    let out_path = env::temp_dir().join(format!(
        "mobailmux-agent-{}-{}.txt",
        slot_id,
        Uuid::new_v4().simple()
    ));
    let mut command = TokioCommand::new(&state.config.agent_codex_bin);
    command.arg("exec");
    if use_resume {
        command.arg("resume").arg("--json");
    } else {
        command.arg("--json");
    }
    command.args(&state.config.agent_codex_args);
    if use_resume {
        if let Some((thread_id, _)) = session {
            command
                .arg("--output-last-message")
                .arg(&out_path)
                .arg(thread_id)
                .arg(prompt);
        }
    } else {
        command
            .arg("--cd")
            .arg(&slot.workdir)
            .arg("--output-last-message")
            .arg(&out_path)
            .arg(prompt);
    }
    command
        .current_dir(&slot.workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            append_agent_assistant(
                &state,
                slot_id,
                &format!("Could not start `{}`: {err}", state.config.agent_codex_bin),
            );
            state.agent_jobs.lock().unwrap().remove(&slot_id);
            return;
        }
    };
    let (cancel_tx, mut cancel_rx) = oneshot::channel();
    state
        .agent_cancels
        .lock()
        .unwrap()
        .insert(slot_id, cancel_tx);
    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(read_agent_stdout(
            state.clone(),
            slot_id,
            slot.workdir.clone(),
            stdout,
        ))
    });
    let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_agent_stderr(stderr_tail.clone(), stderr)));
    let started = Utc::now();
    let stopped;
    let status = tokio::select! {
        result = child.wait() => {
            stopped = false;
            result
        }
        _ = &mut cancel_rx => {
            stopped = true;
            let _ = child.start_kill();
            child.wait().await
        }
    };
    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
    state.agent_cancels.lock().unwrap().remove(&slot_id);
    state.agent_jobs.lock().unwrap().remove(&slot_id);
    let elapsed = (Utc::now() - started).num_seconds().max(0);
    if stopped {
        append_agent_assistant(
            &state,
            slot_id,
            &format!("{} stopped after {elapsed}s.", slot.name),
        );
        let _ = tokio::fs::remove_file(&out_path).await;
        return;
    }
    match status {
        Ok(status) if status.success() => {
            let final_text = tokio::fs::read_to_string(&out_path)
                .await
                .unwrap_or_default()
                .trim()
                .to_string();
            append_agent_assistant(
                &state,
                slot_id,
                &format!(
                    "{} done in {elapsed}s.\n\n{}",
                    slot.name,
                    if final_text.is_empty() {
                        "(Agent completed without a final message.)"
                    } else {
                        &final_text
                    }
                ),
            );
        }
        Ok(status) => {
            let tail = stderr_tail.lock().unwrap().join("\n");
            append_agent_assistant(
                &state,
                slot_id,
                &format!(
                    "{} failed with exit code {} after {elapsed}s.\n\n```text\n{}\n```",
                    slot.name,
                    status.code().unwrap_or(-1),
                    truncate_text(&tail, 2400)
                ),
            );
        }
        Err(err) => {
            append_agent_assistant(
                &state,
                slot_id,
                &format!("{} wait failed after {elapsed}s: {err}", slot.name),
            );
        }
    }
    let _ = tokio::fs::remove_file(&out_path).await;
}

async fn read_agent_stdout(
    state: Arc<AppState>,
    slot_id: i64,
    workdir: String,
    stdout: tokio::process::ChildStdout,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let event_type = event
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if event_type == "thread.started"
            && let Some(thread_id) = event.get("thread_id").and_then(|value| value.as_str())
        {
            let db = state.db.lock().unwrap();
            let _ = set_agent_session(&db, slot_id, thread_id, &workdir);
            continue;
        }
        if !matches!(event_type, "item.started" | "item.completed") {
            continue;
        }
        let item = event.get("item").unwrap_or(&serde_json::Value::Null);
        if item.get("type").and_then(|value| value.as_str()) != Some("command_execution") {
            continue;
        }
        let command = item
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or("(unknown command)");
        if event_type == "item.started" {
            state
                .agent_jobs
                .lock()
                .unwrap()
                .entry(slot_id)
                .and_modify(|run| {
                    run.status = "running".into();
                    run.current = truncate_text(command, 160);
                });
            append_agent_assistant(
                &state,
                slot_id,
                &format!("running: `{}`", truncate_text(command, 700)),
            );
        } else {
            state
                .agent_jobs
                .lock()
                .unwrap()
                .entry(slot_id)
                .and_modify(|run| {
                    run.current.clear();
                });
            let exit_code = item.get("exit_code").and_then(|value| value.as_i64());
            if exit_code.is_some_and(|code| code != 0) {
                let output = item
                    .get("aggregated_output")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                append_agent_assistant(
                    &state,
                    slot_id,
                    &format!(
                        "command exit {}: `{}`\n```text\n{}\n```",
                        exit_code.unwrap_or(-1),
                        truncate_text(command, 500),
                        truncate_text(output, 1200)
                    ),
                );
            }
        }
    }
}

async fn read_agent_stderr(tail: Arc<Mutex<Vec<String>>>, stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut tail = tail.lock().unwrap();
        tail.push(line.to_string());
        if tail.len() > 80 {
            let extra = tail.len() - 80;
            tail.drain(0..extra);
        }
    }
}

#[derive(Debug, Clone)]
struct AgentPromptAttachment {
    filename: String,
    content_type: String,
    file_path: String,
    size_bytes: i64,
}

fn agent_attachment_for_prompt(
    db: &Connection,
    id: i64,
) -> rusqlite::Result<Option<AgentPromptAttachment>> {
    db.query_row(
        "SELECT original_name, content_type, file_path, size_bytes FROM agent_attachments WHERE id = ?1",
        params![id],
        |row| {
            Ok(AgentPromptAttachment {
                filename: row.get(0)?,
                content_type: row.get(1)?,
                file_path: row.get(2)?,
                size_bytes: row.get(3)?,
            })
        },
    )
    .optional()
}

fn agent_session(db: &Connection, slot_id: i64) -> rusqlite::Result<Option<(String, String)>> {
    db.query_row(
        "SELECT thread_id, workdir FROM agent_sessions WHERE slot_id = ?1",
        params![slot_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

fn set_agent_session(
    db: &Connection,
    slot_id: i64,
    thread_id: &str,
    workdir: &str,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO agent_sessions (slot_id, thread_id, workdir, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(slot_id) DO UPDATE SET
           thread_id = excluded.thread_id,
           workdir = excluded.workdir,
           updated_at = excluded.updated_at",
        params![slot_id, thread_id, workdir, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn build_agent_prompt(
    slot: &AgentSlotRow,
    request_body: &str,
    attachment: Option<&AgentPromptAttachment>,
) -> String {
    let mut prompt = format!(
        "You are running from Mobailmux agent slot {}.\nCurrent working folder: {}\nKeep the final reply concise and include what changed plus any verification run.\n\nUser request:\n{}",
        slot.name, slot.workdir, request_body
    );
    if let Some(attachment) = attachment {
        prompt.push_str(&format!(
            "\n\nAttached file:\n- name: {}\n- path: {}\n- type: {}\n- bytes: {}\nUse the file path directly if you need to inspect the upload.",
            attachment.filename,
            attachment.file_path,
            attachment.content_type,
            attachment.size_bytes
        ));
    }
    prompt
}

fn agent_location(slot_id: Option<i64>) -> String {
    slot_id
        .filter(|id| *id > 0)
        .map(|id| format!("/agents?slot={id}"))
        .unwrap_or_else(|| "/agents".into())
}

fn command_arg<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case(command) {
        return Some("");
    }
    if trimmed.len() > command.len()
        && trimmed[..command.len()].eq_ignore_ascii_case(command)
        && trimmed[command.len()..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        return Some(trimmed[command.len()..].trim());
    }
    None
}

fn resolve_agent_path(raw: &str, base: &str) -> PathBuf {
    let expanded = expand_local_path(raw.trim());
    if expanded.is_absolute() {
        expanded
    } else {
        Path::new(base).join(expanded)
    }
}

fn default_home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn expand_local_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if trimmed == "~" {
        return default_home_dir();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return default_home_dir().join(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("$HOME/") {
        return default_home_dir().join(rest);
    }
    PathBuf::from(trimmed)
}

fn default_codex_bin() -> String {
    if command_in_path("codexunsafe") {
        "codexunsafe".into()
    } else {
        let user_alias = default_home_dir().join(".local/bin/codexunsafe");
        if user_alias.is_file() {
            user_alias.to_string_lossy().into_owned()
        } else {
            "codex".into()
        }
    }
}

fn command_in_path(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).is_file();
    }
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|path| path.join(command).is_file()))
        .unwrap_or(false)
}

fn agent_command_label(config: &Config) -> String {
    let mut parts = vec![config.agent_codex_bin.clone()];
    parts.extend(config.agent_codex_args.iter().cloned());
    parts.join(" ")
}

fn split_env_args(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_agent_slot_seeds(raw: Option<String>, default_workdir: &Path) -> Vec<AgentSlotSeed> {
    let raw = raw.unwrap_or_else(|| DEFAULT_AGENT_SLOTS.into());
    raw.split(',')
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let (name, workdir) = item
                .split_once(':')
                .map(|(name, workdir)| (name, expand_local_path(workdir)))
                .unwrap_or_else(|| (item, default_workdir.to_path_buf()));
            let name = normalize_agent_slot_name(name);
            if name.is_empty() || name.chars().count() > MAX_AGENT_SLOT_CHARS {
                None
            } else {
                Some(AgentSlotSeed { name, workdir })
            }
        })
        .collect()
}

fn normalize_agent_slot_name(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn sanitize_filename(value: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "attachment".into()
    } else {
        sanitized.chars().take(120).collect()
    }
}

fn project_display_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".into();
    }
    Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn file_modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

fn system_time_to_rfc3339(value: SystemTime) -> String {
    DateTime::<Utc>::from(value).to_rfc3339()
}

fn epoch_to_rfc3339(epoch: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(epoch, 0).map(|value| value.to_rfc3339())
}

fn short_time(value: &str) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(value) else {
        return if value.trim().is_empty() {
            "unknown".into()
        } else {
            value.to_string()
        };
    };
    let dt = parsed.with_timezone(&Utc);
    let now = Utc::now();
    let delta = dt - now;
    let past = now - dt;
    if delta.num_minutes() > 0 {
        return format_duration(delta.num_seconds(), "in ");
    }
    if past.num_minutes() < 1 {
        return "just now".into();
    }
    if past.num_days() < 2 {
        return format_duration(past.num_seconds(), "") + " ago";
    }
    dt.format("%b %-d %H:%M").to_string()
}

fn format_duration(seconds: i64, prefix: &str) -> String {
    let seconds = seconds.max(0);
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{prefix}{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{prefix}{hours}h");
    }
    format!("{prefix}{}d", hours / 24)
}

fn format_number(value: i64) -> String {
    let mut digits = value.abs().to_string();
    let mut out = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        if out.is_empty() {
            out = tail;
        } else {
            out = format!("{tail},{out}");
        }
    }
    if out.is_empty() {
        out = digits;
    } else {
        out = format!("{digits},{out}");
    }
    if value < 0 { format!("-{out}") } else { out }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(20);
    value.chars().take(keep).collect::<String>() + "\n...[truncated]"
}

fn page(title: &str, body: &str) -> Response {
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, viewport-fit=cover, interactive-widget=resizes-content">
<title>{}</title>
<style>
{PAGE_CSS}
</style>
</head>
<body>{}</body>
</html>"#,
        html_escape(title),
        body
    ))
    .into_response()
}

fn page_guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if authorized(&state.config, headers) {
        None
    } else {
        Some(Redirect::to("/login").into_response())
    }
}

fn raw_guard(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if authorized(&state.config, headers) {
        None
    } else {
        Some((StatusCode::UNAUTHORIZED, "authentication required").into_response())
    }
}

fn authorized(config: &Config, headers: &HeaderMap) -> bool {
    if config.auth_disabled {
        return true;
    }
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        && let Some(raw) = value.strip_prefix("Basic ")
        && let Ok(decoded) = BASE64.decode(raw.trim())
        && let Ok(pair) = String::from_utf8(decoded)
        && let Some((user, password)) = pair.split_once(':')
    {
        return user == config.user && verify_password(config, password);
    }
    let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .is_some_and(|(_, value)| verify_session_cookie(config, value))
}

fn verify_password(config: &Config, password: &str) -> bool {
    if config.auth_disabled {
        return true;
    }
    let Some(hash) = &config.password_hash else {
        return false;
    };
    if hash.starts_with("$argon2") {
        let Ok(parsed) = PasswordHash::new(hash) else {
            return false;
        };
        return Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
    }
    let Some((prefix, rest)) = hash.split_once(':') else {
        return false;
    };
    if prefix != "sha256" {
        return false;
    }
    let Some((salt_hex, expected_hex)) = rest.split_once(':') else {
        return false;
    };
    let Ok(salt) = hex::decode(salt_hex) else {
        return false;
    };
    let Ok(expected) = hex::decode(expected_hex) else {
        return false;
    };
    let actual = password_digest(&salt, password);
    actual.as_slice().ct_eq(expected.as_slice()).into()
}

fn password_digest(salt: &[u8], password: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    hasher.finalize().to_vec()
}

fn make_session_cookie(config: &Config) -> String {
    let expires = (Utc::now() + Duration::days(SESSION_DAYS)).timestamp();
    let signature = session_signature(config, expires);
    format!(
        "{SESSION_COOKIE}={expires}:{signature}; Max-Age={}; Path=/; HttpOnly; SameSite=Lax",
        SESSION_DAYS * 24 * 60 * 60
    )
}

fn verify_session_cookie(config: &Config, value: &str) -> bool {
    let Some((raw_expires, signature)) = value.split_once(':') else {
        return false;
    };
    let Ok(expires) = raw_expires.parse::<i64>() else {
        return false;
    };
    if expires < Utc::now().timestamp() {
        return false;
    }
    let expected = session_signature(config, expires);
    expected.as_bytes().ct_eq(signature.as_bytes()).into()
}

fn session_signature(config: &Config, expires: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(&config.cookie_secret);
    hasher.update(expires.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

fn audit_public_cmd(args: &[String]) -> io::Result<u8> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!(
            r#"Usage:
  mobailmux audit-public
  mobailmux audit-public --install-hook

Checks tracked files for local/private paths, common secret markers,
private-network IP leaks, and host-specific denylist terms.
"#
        );
        return Ok(0);
    }

    let root = git_root()?;
    if args.iter().any(|arg| arg == "--install-hook") {
        install_audit_hooks(&root)?;
        println!("installed .git/hooks/pre-commit and .git/hooks/pre-push");
    }

    let findings = audit_public(&root)?;
    if findings.is_empty() {
        println!("audit-public: ok");
        return Ok(0);
    }

    eprintln!("audit-public: found {} issue(s)", findings.len());
    for finding in &findings {
        match finding.line {
            Some(line) => eprintln!("{}:{}: {}", finding.path, line, finding.message),
            None => eprintln!("{}: {}", finding.path, finding.message),
        }
    }
    Ok(1)
}

fn git_root() -> io::Result<PathBuf> {
    let output = StdCommand::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("not inside a Git repository"));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn install_audit_hooks(root: &Path) -> io::Result<()> {
    let hooks = root.join(".git/hooks");
    fs::create_dir_all(&hooks)?;
    for name in ["pre-commit", "pre-push"] {
        let hook = hooks.join(name);
        fs::write(
            &hook,
            r#"#!/bin/sh
set -eu
repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"
cargo run --quiet -- audit-public
"#,
        )?;
        let mut permissions = fs::metadata(&hook)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions)?;
    }
    Ok(())
}

fn audit_public(root: &Path) -> io::Result<Vec<AuditFinding>> {
    let files = git_tracked_files(root)?;
    let private_terms = load_audit_denylist(root);
    let mut findings = Vec::new();

    for path in files {
        if let Some(message) = audit_path(&path) {
            findings.push(AuditFinding {
                path,
                line: None,
                message,
            });
            continue;
        }

        let full_path = root.join(&path);
        if fs::metadata(&full_path)
            .map(|metadata| metadata.len() > 1_000_000)
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&full_path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            for message in audit_line(line, &private_terms) {
                findings.push(AuditFinding {
                    path: path.clone(),
                    line: Some(index + 1),
                    message,
                });
            }
        }
    }

    Ok(findings)
}

fn git_tracked_files(root: &Path) -> io::Result<Vec<String>> {
    let output = StdCommand::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

fn audit_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    let denied_exact = [
        "AGENTS.md",
        "mobailmux.local.toml",
        ".env",
        ".env.local",
        "id_rsa",
        "id_ed25519",
    ];
    if denied_exact.contains(&normalized.as_str()) || denied_exact.contains(&name) {
        return Some("private file path is tracked".into());
    }
    if name.starts_with(".env.") && name != ".env.example" {
        return Some("private env file is tracked".into());
    }
    if normalized.starts_with("docs/private/")
        || normalized.starts_with(".mobailmux/")
        || normalized.starts_with("backups/")
        || normalized.starts_with("data/")
        || normalized.starts_with("downloads/")
    {
        return Some("ignored private/runtime path is tracked".into());
    }
    if normalized.contains("/data/")
        || normalized.contains("/cache/")
        || normalized.contains("/config/")
        || normalized.contains("/downloads/")
        || normalized.contains("/secrets/")
    {
        return Some("runtime or secret data path is tracked".into());
    }
    if matches!(
        Path::new(name).extension().and_then(|ext| ext.to_str()),
        Some("db" | "sqlite" | "sqlite3" | "log" | "pid" | "pem" | "key" | "p12" | "pfx")
    ) {
        return Some("private state or key-like file is tracked".into());
    }
    None
}

fn load_audit_denylist(root: &Path) -> Vec<String> {
    let mut paths = vec![
        root.join("docs/private/audit-denylist.txt"),
        root.join(".mobailmux/audit-denylist.txt"),
    ];
    if let Ok(path) = env::var("MOBAILMUX_AUDIT_DENYLIST") {
        paths.push(PathBuf::from(path));
    }

    let mut terms = Vec::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            let term = line.trim();
            if term.is_empty() || term.starts_with('#') {
                continue;
            }
            terms.push(term.to_ascii_lowercase());
        }
    }
    terms
}

fn audit_line(line: &str, private_terms: &[String]) -> Vec<String> {
    let mut findings = Vec::new();
    let lower = line.to_ascii_lowercase();
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return findings;
    }

    if line.contains("-----BEGIN ") && line.contains(&["PRIVATE", " KEY"].concat()) {
        findings.push("private key material".into());
    }
    for marker in token_markers() {
        if line.contains(&marker) {
            findings.push(format!("token marker `{marker}`"));
        }
    }
    if contains_tailscale_ipv4(line) {
        findings.push("Tailscale/CGNAT private IP address".into());
    }
    if suspicious_secret_assignment(line) {
        findings.push("non-placeholder secret-looking assignment".into());
    }
    for term in private_terms {
        if !term.is_empty() && lower.contains(term) {
            findings.push("local denylist term".into());
        }
    }

    findings
}

fn token_markers() -> Vec<String> {
    vec![
        ["github", "_pat_"].concat(),
        ["gh", "p_"].concat(),
        ["gh", "o_"].concat(),
        ["gh", "s_"].concat(),
        ["gh", "u_"].concat(),
        ["s", "k-"].concat(),
        ["xo", "xb-"].concat(),
        ["xo", "xp-"].concat(),
    ]
}

fn suspicious_secret_assignment(line: &str) -> bool {
    if line.contains("::") {
        return false;
    }
    let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) else {
        return false;
    };
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty()
        || key.len() > 80
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return false;
    }
    let secret_keys = [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "access_key",
        "client_secret",
        "private_key",
    ];
    if !secret_keys.iter().any(|needle| key.contains(needle)) {
        return false;
    }

    let value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches(',')
        .trim();
    let allowed = [
        "",
        "example",
        "placeholder",
        "changeme",
        "change-me",
        "redacted",
        "dummy",
        "none",
        "null",
        "false",
        "true",
    ];
    if allowed.contains(&value.to_ascii_lowercase().as_str()) {
        return false;
    }
    if value == "String" || value.starts_with("Option<") || value.starts_with("Vec<") {
        return false;
    }
    if value.starts_with("${")
        || value.starts_with('<')
        || value.starts_with("your-")
        || value.contains("...")
        || value.starts_with("Some(")
        || value.starts_with("vec!")
    {
        return false;
    }
    true
}

fn contains_tailscale_ipv4(line: &str) -> bool {
    line.split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .filter(|token| token.matches('.').count() == 3)
        .any(|token| {
            let octets: Vec<u16> = token
                .split('.')
                .filter_map(|part| part.parse::<u16>().ok())
                .collect();
            octets.len() == 4
                && octets[0] == 100
                && (64..=127).contains(&octets[1])
                && octets.iter().all(|octet| *octet <= 255)
        })
}

fn hash_password_cmd(args: &[String]) -> io::Result<()> {
    if !args.iter().any(|arg| arg == "--stdin") {
        eprintln!("usage: mobailmux hash-password --stdin");
        return Ok(());
    }
    let mut password = String::new();
    io::stdin().read_to_string(&mut password)?;
    let password = password.trim_end_matches(['\r', '\n']);
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).map_err(io_other)?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(io_other)?;
    println!("{hash}");
    Ok(())
}

fn random_secret() -> Vec<u8> {
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    secret.to_vec()
}

fn env_flag(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn io_other(err: impl std::fmt::Display) -> io::Error {
    io::Error::other(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_round_trips() {
        let salt = [1u8; 16];
        let hash = format!(
            "sha256:{}:{}",
            hex::encode(salt),
            hex::encode(password_digest(&salt, "secret"))
        );
        let config = Config {
            bind: "127.0.0.1:0".into(),
            db_path: PathBuf::new(),
            agent_default_workdir: PathBuf::new(),
            agent_upload_dir: PathBuf::new(),
            agent_codex_bin: "codex".into(),
            agent_codex_args: Vec::new(),
            codex_home: PathBuf::new(),
            codex_reset_command: None,
            agent_slots: Vec::new(),
            user: "mobailmux".into(),
            password_hash: Some(hash),
            cookie_secret: vec![2u8; 32],
            auth_disabled: false,
        };
        assert!(verify_password(&config, "secret"));
        assert!(!verify_password(&config, "wrong"));
    }

    #[test]
    fn audit_rejects_private_paths() {
        assert!(audit_path("mobailmux.local.toml").is_some());
        assert!(audit_path("data/mobailmux.sqlite").is_some());
        assert!(audit_path("docs/private/notes.md").is_some());
    }

    #[test]
    fn audit_detects_cgnat_private_address() {
        let line = format!("service=http://100.{}.10.5:8789", 80);
        assert!(contains_tailscale_ipv4(&line));
        assert!(!contains_tailscale_ipv4("service=http://127.0.0.1:8789"));
    }

    #[test]
    fn audit_secret_assignment_allows_placeholders() {
        assert!(!suspicious_secret_assignment(
            "MOBAILMUX_PASSWORD_HASH=<hash>"
        ));
        assert!(!suspicious_secret_assignment(
            "MOBAILMUX_COOKIE_SECRET=${SECRET}"
        ));
        assert!(suspicious_secret_assignment(
            "MOBAILMUX_COOKIE_SECRET=abc123"
        ));
    }

    #[test]
    fn codex_conversation_parser_uses_index_title_and_visible_messages() {
        let dir = env::temp_dir().join(format!("mobailmux-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-2026-06-23T09-24-22-thread-1.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-06-23T07:24:22Z","type":"session_meta","payload":{"id":"thread-1","cwd":"/work/app","timestamp":"2026-06-23T07:24:22Z"}}
{"timestamp":"2026-06-23T07:24:23Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"secret instructions"}]}}
{"timestamp":"2026-06-23T07:24:24Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"load this project"}]}}
{"timestamp":"2026-06-23T07:24:25Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}
"#,
        )
        .unwrap();
        let mut names = HashMap::new();
        names.insert(
            "thread-1".into(),
            ("Indexed title".into(), "2026-06-23T07:30:00Z".into()),
        );
        let conversation = codex_conversation_from_file(&path, &names).unwrap();
        assert_eq!(conversation.title, "Indexed title");
        assert_eq!(conversation.cwd, "/work/app");
        assert_eq!(conversation.message_count, 2);
        assert_eq!(conversation.preview, "load this project");

        let messages = codex_transcript_messages(&path).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[1].role, "user");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_conversation_parser_filters_synthetic_codex_context() {
        let dir = env::temp_dir().join(format!("mobailmux-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-2026-06-23T09-24-22-thread-2.jsonl");
        fs::write(
            &path,
            r##"{"timestamp":"2026-06-23T07:24:22Z","type":"session_meta","payload":{"id":"thread-2","cwd":"/work/app","timestamp":"2026-06-23T07:24:22Z"}}
{"timestamp":"2026-06-23T07:24:23Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /work/app"}]}}
{"timestamp":"2026-06-23T07:24:24Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix mobailmux loading"}]}}
{"timestamp":"2026-06-23T07:24:25Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}
"##,
        )
        .unwrap();
        let mut names = HashMap::new();
        names.insert(
            "thread-2".into(),
            (
                "# AGENTS.md instructions for /work/app".into(),
                "2026-06-23T07:30:00Z".into(),
            ),
        );
        let conversation = codex_conversation_from_file(&path, &names).unwrap();
        assert_eq!(conversation.title, "fix mobailmux loading");
        assert_eq!(conversation.preview, "fix mobailmux loading");
        assert_eq!(conversation.message_count, 2);

        let messages = codex_transcript_messages(&path).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[1].role, "user");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_usage_parser_reads_rate_limits() {
        let payload = serde_json::json!({
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "total_tokens": 619644,
                    "cached_input_tokens": 528640
                },
                "last_token_usage": {"total_tokens": 109315},
                "model_context_window": 258400
            },
            "rate_limits": {
                "primary": {"used_percent": 14.0, "window_minutes": 300, "resets_at": 1782210186},
                "secondary": {"used_percent": 38.0, "window_minutes": 10080, "resets_at": 1782380596},
                "rate_limit_reset_credits": {"available_count": 2},
                "credits": null,
                "plan_type": "prolite"
            }
        });
        let usage = codex_usage_from_payload("2026-06-23T07:29:57Z", &payload);
        assert_eq!(usage.plan_type, "prolite");
        assert_eq!(usage.total_units, 619644);
        assert_eq!(usage.last_units, 109315);
        assert_eq!(usage.primary.unwrap().remaining_percent, 86.0);
        assert_eq!(usage.secondary.unwrap().remaining_percent, 62.0);
        assert_eq!(usage.reset_credits.unwrap().available_count, 2);

        let window = serde_json::json!({
            "usedPercent": 35,
            "windowDurationMins": 300,
            "resetsAt": 1782210186
        });
        let window = codex_rate_window("Primary", Some(&window)).unwrap();
        assert_eq!(window.used_percent, 35.0);
        assert_eq!(window.window_minutes, 300);
        assert_eq!(window.resets_at, Some(1782210186));
    }
}

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
use chrono::{DateTime, Datelike, Duration, Local, Utc};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    io::{self, BufRead, Read, Seek, SeekFrom},
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
    time::sleep,
};
use uuid::Uuid;

mod codex_reset_ledger;
mod db_migrations;
mod modules;

#[derive(Copy, Clone, Serialize)]
struct AgentCommandSpec {
    name: &'static str,
    usage: &'static str,
    description: &'static str,
    takes_arg: bool,
}

#[derive(Clone, Serialize)]
struct ComposerSuggestion {
    kind: &'static str,
    name: String,
    insert: String,
    description: String,
    takes_arg: bool,
}

const DEFAULT_AGENT_SLOTS: &str = "codex";
const MAX_AGENT_MESSAGE_CHARS: usize = 128 * 1024;
const MAX_AGENT_SLOT_CHARS: usize = 32;
const MAX_AGENT_GOAL_CHARS: usize = 4000;
const MAX_CODEX_CONVERSATIONS: usize = 120;
const MAX_CODEX_TRANSCRIPT_MESSAGES: usize = 80;
const CODEX_SESSION_SCAN_LIMIT: usize = 180;
const CODEX_INDEX_REFRESH_AFTER: StdDuration = StdDuration::from_secs(30);
const CODEX_APP_SERVER_WRITE_SETTLE: StdDuration = StdDuration::from_secs(5);
const SESSION_COOKIE: &str = "mobailmux_session";
const SESSION_DAYS: i64 = 30;
const PAGE_CSS: &str = include_str!("page.css");
const AGENT_COMMAND_SPECS: &[AgentCommandSpec] = &[
    AgentCommandSpec {
        name: "goal",
        usage: "/goal <objective>",
        description: "Set or show this slot's goal",
        takes_arg: true,
    },
    AgentCommandSpec {
        name: "clear-goal",
        usage: "/clear-goal",
        description: "Clear this slot's goal",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "status",
        usage: "/status",
        description: "Show slot status and Codex usage",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "usage",
        usage: "/usage",
        description: "Show Codex usage and limits",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "model",
        usage: "/model",
        description: "Open the model picker",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "slots",
        usage: "/slots",
        description: "Show all slots",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "stop",
        usage: "/stop",
        description: "Stop the running Codex job",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "queue",
        usage: "/queue",
        description: "Show queued follow-ups for this slot",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "clear-queue",
        usage: "/clear-queue",
        description: "Clear queued follow-ups for this slot",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "fresh",
        usage: "/fresh",
        description: "Reset chat and saved Codex thread",
        takes_arg: false,
    },
    AgentCommandSpec {
        name: "help",
        usage: "/help",
        description: "Show commands",
        takes_arg: false,
    },
];
const AGENT_COMMAND_ALIASES: &[&str] =
    &["commands", "list", "overview", "limits", "settings", "new"];

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
    let conn = open_db(&config.db_path)?;
    ensure_agent_slot_seeds(&conn, &config.agent_slots, &config.agent_default_workdir)
        .map_err(io_other)?;

    let state = Arc::new(AppState {
        db: Mutex::new(conn),
        config,
        agent_jobs: Mutex::new(HashMap::new()),
        agent_cancels: Mutex::new(HashMap::new()),
        agent_queues: Mutex::new(HashMap::new()),
        codex_index: Mutex::new(CodexIndexCache::default()),
        codex_models: Mutex::new(CodexModelCatalogCache::default()),
    });

    mark_interrupted_agent_runs(&state);
    refresh_codex_index(state.clone());
    refresh_codex_model_catalog(state.clone());

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
    agent_codex_bin: String,
    agent_codex_args: Vec<String>,
    agent_progress_notes: bool,
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

#[derive(Default)]
struct AgentStdoutSummary {
    last_assistant_text: Option<String>,
    final_text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AgentRunSettings {
    model: Option<String>,
    reasoning_effort: Option<String>,
}

#[derive(Clone, Debug)]
struct QueuedAgentRequest {
    body: String,
    attachment_id: Option<i64>,
    settings: AgentRunSettings,
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
struct CodexVisibleMessage {
    role: String,
    text: String,
    timestamp: String,
    order: usize,
    fallback: bool,
    final_answer: bool,
    assistant_progress: bool,
}

#[derive(Clone, Debug, Serialize)]
struct CodexReasoningEffort {
    effort: String,
    description: String,
}

#[derive(Clone, Debug, Serialize)]
struct CodexModel {
    model: String,
    display_name: String,
    description: String,
    default_reasoning_effort: String,
    supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    is_default: bool,
}

#[derive(Clone, Debug)]
struct CodexIndex {
    conversations: Vec<CodexConversation>,
    usage: Option<CodexUsageSnapshot>,
}

impl CodexIndex {
    fn empty() -> Self {
        Self {
            conversations: Vec::new(),
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
    credits: Vec<CodexResetCredit>,
    estimate: Option<codex_reset_ledger::ResetCreditEstimate>,
}

#[derive(Clone, Debug)]
struct CodexResetCredit {
    title: String,
    expires_at: Option<i64>,
}

#[derive(Default)]
struct CodexAppServerDashboard {
    rate_limits: Option<serde_json::Value>,
}

#[derive(Default)]
struct CodexIndexCache {
    snapshot: Option<CodexIndex>,
    refreshed_at: Option<Instant>,
    refreshing: bool,
}

#[derive(Default)]
struct CodexModelCatalogCache {
    models: Vec<CodexModel>,
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
        let agent_codex_bin =
            env::var("MOBAILMUX_AGENT_CODEX_BIN").unwrap_or_else(|_| default_codex_bin());
        let agent_codex_args = env::var("MOBAILMUX_AGENT_CODEX_ARGS")
            .ok()
            .map(|value| split_env_args(&value))
            .unwrap_or_default();
        let agent_progress_notes = env_flag("MOBAILMUX_AGENT_PROGRESS_NOTES", false);
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
            agent_codex_bin,
            agent_codex_args,
            agent_progress_notes,
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
    agent_queues: Mutex<HashMap<i64, VecDeque<QueuedAgentRequest>>>,
    codex_index: Mutex<CodexIndexCache>,
    codex_models: Mutex<CodexModelCatalogCache>,
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

fn codex_model_catalog_snapshot(state: &Arc<AppState>) -> Vec<CodexModel> {
    let (models, should_refresh) = {
        let cache = state.codex_models.lock().unwrap();
        let stale = cache
            .refreshed_at
            .is_none_or(|refreshed_at| refreshed_at.elapsed() >= CODEX_INDEX_REFRESH_AFTER);
        let should_refresh = stale && !cache.refreshing;
        (cache.models.clone(), should_refresh)
    };
    if should_refresh {
        refresh_codex_model_catalog(state.clone());
    }
    models
}

fn refresh_codex_model_catalog(state: Arc<AppState>) {
    {
        let mut cache = state.codex_models.lock().unwrap();
        if cache.refreshing {
            return;
        }
        cache.refreshing = true;
    }
    tokio::task::spawn_blocking(move || {
        let models = fetch_codex_model_catalog(&state.config);
        let mut cache = state.codex_models.lock().unwrap();
        cache.models = models;
        cache.refreshed_at = (!cache.models.is_empty()).then(Instant::now);
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
    refresh: Option<String>,
    usage: Option<String>,
}

#[derive(Deserialize)]
struct CodexResetForm {
    confirm: String,
    slot_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct AgentSlotRow {
    id: i64,
    name: String,
    workdir: String,
    goal: String,
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
    id: i64,
    role: String,
    body: String,
    created_at: String,
    attachment: Option<AgentAttachmentSummary>,
}

struct AgentProgress {
    dir: PathBuf,
    file: PathBuf,
}

#[derive(Serialize)]
struct AgentSlotPoll {
    running: bool,
    current: String,
    message_count: usize,
    messages_html: String,
    active_status: String,
}

#[derive(Serialize)]
struct AgentSlotSummary {
    id: i64,
    name: String,
    running: bool,
    current: String,
    status: String,
}

#[derive(Serialize)]
struct AgentSlotsPoll {
    slots: Vec<AgentSlotSummary>,
}

#[derive(Serialize)]
struct AgentModelCatalogPoll {
    models: Vec<CodexModel>,
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
    let mut codex_snapshot = if query.refresh.is_some() {
        Some(refresh_codex_index_blocking(&state))
    } else {
        codex_index_snapshot(&state)
    };
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
    let message_count = if selected_thread.is_some() {
        codex_transcript_count(&messages_html).unwrap_or(messages.len())
    } else {
        messages.len()
    };
    let slot_rail = agent_slot_rail_html(&state, &slots, active_slot.id);
    let usage_dialog = codex_usage_dialog(
        &state.config,
        codex.usage.as_ref(),
        codex_loaded,
        active_slot.id,
    );
    let reopen_usage = query.usage.is_some();
    let viewing_transcript = selected_thread.is_some();
    let refresh_thread_input = selected_thread
        .as_deref()
        .map(|thread| {
            format!(
                r#"<input type="hidden" name="thread" value="{}">"#,
                html_attr_escape(thread)
            )
        })
        .unwrap_or_default();
    let composer_suggestions_json = agent_composer_suggestions_json(&state.config);
    let model_catalog_json = json_for_inline_script(&codex_model_catalog_snapshot(&state));
    let execution_mode = agent_execution_mode_html(&state.config);
    let active_running = agent_run_for(&state, active_slot.id).is_some();
    let cancel_disabled = if active_running { "" } else { " disabled" };
    page(
        "Agents",
        &format!(
            r##"
<nav><a href="/">Mobailmux</a><div class="nav-actions"><button type="button" class="ghost nav-icon" data-codex-open aria-label="Usage" title="Usage">📊</button><form action="/agents" method="get" data-refresh-form><input type="hidden" name="slot" value="{}"><input type="hidden" name="refresh" value="1">{refresh_thread_input}<button type="submit" class="ghost nav-icon" aria-label="Refresh" title="Refresh" data-refresh-button>↻</button></form><strong>Agents</strong></div></nav>
<main class="chat-shell agent-shell">
  {slot_rail}
  <section class="chat-pane agent-pane">
    <header class="chat-head">
      <div class="chat-title"><strong>{active_title}</strong></div>
      <div class="chat-stats"><span data-agent-count>{message_count} messages</span><span class="agent-status" data-agent-status>{}</span></div>
    </header>
    <div class="message-list" data-agent-messages>{messages_html}</div>
    <section class="agent-compose-wrap">
      <form action="/agents" method="post" enctype="multipart/form-data" class="agent-composer">
        <input name="slot_id" type="hidden" value="{}">
        <input name="edit_message_id" id="editMessageId" type="hidden" value="">
        <textarea id="agentBody" name="body" maxlength="{MAX_AGENT_MESSAGE_CHARS}" placeholder="Message Codex"></textarea>
        <div class="command-suggestions" id="commandSuggestions" role="listbox" hidden></div>
        <div class="edit-strip" id="editStrip" hidden><span>Editing message</span><button type="button" class="ghost" data-edit-clear>Discard</button></div>
        <div class="agent-settings" data-agent-settings>
          <label class="agent-setting"><span>Model</span><select name="model" data-agent-model aria-label="Codex model" disabled><option>Loading models…</option></select></label>
          <label class="agent-setting"><span>Thinking</span><select name="reasoning_effort" data-agent-reasoning aria-label="Thinking difficulty" disabled><option>Loading…</option></select></label>
          {execution_mode}
        </div>
        <button type="submit" name="control" value="stop" class="ghost cancel-button" data-cancel-button{cancel_disabled}>Cancel</button>
        <button type="submit" data-send-button>Send</button>
      </form>
    </section>
  </section>
</main>
{usage_dialog}
<script>
(() => {{
  const list = document.querySelector("[data-agent-messages]");
  const status = document.querySelector("[data-agent-status]");
  const count = document.querySelector("[data-agent-count]");
  const form = document.querySelector(".agent-composer");
  const input = document.getElementById("agentBody");
  const editMessageId = document.getElementById("editMessageId");
  const editStrip = document.getElementById("editStrip");
  const sendButton = document.querySelector("[data-send-button]");
  const cancelButton = document.querySelector("[data-cancel-button]");
  const suggestionBox = document.getElementById("commandSuggestions");
  const activeCwd = document.querySelector("[data-active-cwd]");
  const composerSuggestions = {composer_suggestions_json};
  const modelPicker = document.querySelector("[data-agent-model]");
  const reasoningPicker = document.querySelector("[data-agent-reasoning]");
  const initialModelCatalog = {model_catalog_json};
  let modelCatalog = initialModelCatalog;
  const modelStorageKey = "mobailmux.agent.model";
  const reasoningStorageKey = "mobailmux.agent.reasoning";
  let selectedSuggestion = 0;
  const viewingTranscript = {viewing_transcript};
  const activeSlotId = "{}";
  const slotRows = new Map();
  function storedValue(key) {{
    try {{ return window.localStorage.getItem(key) || ""; }} catch (_) {{ return ""; }}
  }}
  function storeValue(key, value) {{
    try {{ window.localStorage.setItem(key, value); }} catch (_) {{}}
  }}
  function catalogModel(models, name) {{
    return (models || []).find((model) => model.model === name) || null;
  }}
  function defaultCatalogModel(models) {{
    return (models || []).find((model) => model.is_default) || (models || [])[0] || null;
  }}
  function setOption(select, value, label, title) {{
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    if (title) option.title = title;
    select.append(option);
  }}
  function syncReasoningPicker(model, preferredEffort) {{
    if (!reasoningPicker) return;
    reasoningPicker.replaceChildren();
    const efforts = model?.supported_reasoning_efforts || [];
    if (!model || !efforts.length) {{
      setOption(reasoningPicker, "", "Unavailable");
      reasoningPicker.disabled = true;
      return;
    }}
    const selected = efforts.some((item) => item.effort === preferredEffort)
      ? preferredEffort
      : model.default_reasoning_effort || efforts[0].effort;
    efforts.forEach((item) => setOption(reasoningPicker, item.effort, item.effort, item.description));
    reasoningPicker.value = selected;
    reasoningPicker.disabled = false;
    storeValue(reasoningStorageKey, selected);
  }}
  function syncModelPickers(models) {{
    if (!modelPicker) return;
    modelCatalog = models || [];
    if (!models.length) {{
      modelPicker.replaceChildren();
      setOption(modelPicker, "", "Models unavailable");
      modelPicker.disabled = true;
      syncReasoningPicker(null, "");
      return;
    }}
    const previousModel = modelPicker.value || storedValue(modelStorageKey);
    const previousEffort = reasoningPicker?.value || storedValue(reasoningStorageKey);
    const selectedModel = catalogModel(models, previousModel) || defaultCatalogModel(models);
    modelPicker.replaceChildren();
    models.forEach((model) => setOption(modelPicker, model.model, model.display_name || model.model, model.description));
    modelPicker.value = selectedModel.model;
    modelPicker.disabled = false;
    storeValue(modelStorageKey, selectedModel.model);
    syncReasoningPicker(selectedModel, previousEffort);
  }}
  let modelCatalogPollTimer = 0;
  function scheduleModelCatalogPoll(delay) {{
    window.clearTimeout(modelCatalogPollTimer);
    modelCatalogPollTimer = window.setTimeout(loadModelCatalog, delay);
  }}
  async function loadModelCatalog() {{
    try {{
      const response = await fetch("/agents/models", {{cache:"no-store"}});
      if (response.ok) {{
        const data = await response.json();
        if (Array.isArray(data.models) && data.models.length) {{
          syncModelPickers(data.models);
          return;
        }}
      }}
    }} catch (_) {{}}
    scheduleModelCatalogPoll(2500);
  }}
  if (modelPicker) {{
    modelPicker.addEventListener("change", () => {{
      const model = catalogModel(modelCatalog, modelPicker.value);
      storeValue(modelStorageKey, modelPicker.value);
      syncReasoningPicker(model, storedValue(reasoningStorageKey));
    }});
  }}
  reasoningPicker?.addEventListener("change", () => storeValue(reasoningStorageKey, reasoningPicker.value));
  if (initialModelCatalog.length) syncModelPickers(initialModelCatalog);
  else scheduleModelCatalogPoll(0);
  document.querySelectorAll("[data-slot-row]").forEach((row) => {{
    const id = row.getAttribute("data-slot-id");
    if (!id) return;
    const entry = {{
      row,
      status: row.querySelector("[data-slot-status]"),
      badge: row.querySelector("[data-slot-badge]"),
      wasRunning: row.getAttribute("data-slot-running") === "true"
    }};
    row.addEventListener("click", () => {{
      row.classList.remove("done");
      if (entry.badge) entry.badge.hidden = true;
    }});
    slotRows.set(id, entry);
  }});
  function syncDialogLock() {{
    const locked = Array.from(document.querySelectorAll("dialog")).some((dialog) => dialog.open);
    document.documentElement.classList.toggle("drawer-open", locked);
    document.body.classList.toggle("drawer-open", locked);
  }}
  function openDialog(dialog) {{
    if (!dialog) return;
    if (!dialog.open) {{
      if (typeof dialog.showModal === "function") dialog.showModal();
      else dialog.setAttribute("open", "");
    }}
    syncDialogLock();
  }}
  function closeDialog(dialog) {{
    if (!dialog) return;
    if (typeof dialog.close === "function") dialog.close();
    else dialog.removeAttribute("open");
    syncDialogLock();
  }}
  document.querySelectorAll("dialog").forEach((dialog) => {{
    dialog.addEventListener("close", syncDialogLock);
    dialog.addEventListener("cancel", () => setTimeout(syncDialogLock, 0));
  }});
  const codexPanel = document.getElementById("codexPanel");
  document.querySelector("[data-codex-open]")?.addEventListener("click", () => openDialog(codexPanel));
  document.querySelector("[data-codex-close]")?.addEventListener("click", () => closeDialog(codexPanel));
  if ({reopen_usage}) openDialog(codexPanel);
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
  const openLockedDialog = (dialog) => {{
    if (!dialog) return;
    if (!dialog.open) {{
      if (typeof dialog.showModal === "function") dialog.showModal();
      else dialog.setAttribute("open", "");
    }}
    lockPageScroll();
    syncDialogLock();
  }};
  document.querySelectorAll("[data-refresh-form]").forEach((form) => {{
    form.addEventListener("submit", () => {{
      const button = form.querySelector("[data-refresh-button]");
      if (!button) return;
      button.classList.add("is-busy");
      button.setAttribute("aria-busy", "true");
      button.setAttribute("aria-label", "Refreshing");
      button.setAttribute("title", "Refreshing");
    }});
  }});
   document.querySelector("[data-reset-form]")?.addEventListener("submit", (event) => {{
    const ok = window.confirm("Use a Codex reset now? This cannot be undone.");
    if (!ok) event.preventDefault();
  }});
  let dirty = false;
  let agentPollTimer = 0;
  let slotPollTimer = 0;
  let selectionHoldUntil = 0;
  let lastMessagesHtml = list ? list.innerHTML : "";
  function scheduleAgentPoll(delay) {{
    window.clearTimeout(agentPollTimer);
    agentPollTimer = window.setTimeout(poll, delay);
  }}
  function scheduleSlotPoll(delay) {{
    window.clearTimeout(slotPollTimer);
    slotPollTimer = window.setTimeout(pollSlots, delay);
  }}
  function captureOpenFolds() {{
    const keys = new Set();
    list?.querySelectorAll("details[data-fold-key]").forEach((details) => {{
      if (details.open) {{
        const key = details.getAttribute("data-fold-key");
        if (key) keys.add(key);
      }}
    }});
    return keys;
  }}
  function nodeInsideMessageList(node) {{
    if (!list || !node) return false;
    const element = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
    return !!element && list.contains(element);
  }}
  function messageSelectionActive() {{
    const selection = window.getSelection?.();
    if (!selection || selection.isCollapsed || !selection.toString().trim()) return false;
    return nodeInsideMessageList(selection.anchorNode) || nodeInsideMessageList(selection.focusNode);
  }}
  function holdMessageUpdates(ms = 9000) {{
    selectionHoldUntil = Math.max(selectionHoldUntil, Date.now() + ms);
  }}
  function canReplaceMessages() {{
    if (messageSelectionActive()) {{
      holdMessageUpdates();
      return false;
    }}
    return Date.now() >= selectionHoldUntil;
  }}
  document.addEventListener("selectionchange", () => {{
    if (messageSelectionActive()) holdMessageUpdates();
  }});
  list?.addEventListener("contextmenu", () => holdMessageUpdates(12000));
  list?.addEventListener("touchstart", () => {{
    if (messageSelectionActive()) holdMessageUpdates(5000);
  }}, {{passive:true}});
  function replaceMessages(html) {{
    if (!list || html === lastMessagesHtml) return true;
    if (!canReplaceMessages()) return false;
    const openFolds = captureOpenFolds();
    const nearBottom = list.scrollTop + list.clientHeight >= list.scrollHeight - 90;
    list.innerHTML = html;
    lastMessagesHtml = html;
    list.querySelectorAll("details[data-fold-key]").forEach((details) => {{
      const key = details.getAttribute("data-fold-key");
      if (key && openFolds.has(key)) details.open = true;
    }});
    if (nearBottom) list.scrollTop = list.scrollHeight;
    return true;
  }}
   function activeCompletionToken() {{
    if (!input) return null;
    const value = input.value;
    const cursor = input.selectionStart ?? value.length;
    const before = value.slice(0, cursor);
    const match = before.match(/(^|[\s([{{])([/!#$])([A-Za-z0-9:_-]*)$/);
    if (!match) return null;
    const symbol = match[2];
    const kind = symbol === "$" ? "skill" : symbol === "#" ? "plugin" : "command";
    return {{
      start: before.length - match[2].length - match[3].length,
      end: cursor,
      symbol,
      kind,
      typed: match[3].toLowerCase()
    }};
  }}
  function matchingSuggestions() {{
    const token = activeCompletionToken();
    if (!token) return [];
    return composerSuggestions
      .filter((item) => item.kind === token.kind)
      .filter((item) => {{
        const typed = token.typed;
        if (!typed) return true;
        const haystack = `${{item.name}} ${{item.description}} ${{item.insert}}`.toLowerCase();
        return haystack.includes(typed);
      }})
      .slice(0, 24)
      .map((item) => ({{...item, token}}));
  }}
  function suggestionInsert(item) {{
    if (item.kind === "command") return `${{item.token.symbol}}${{item.name}}`;
    return item.insert;
  }}
  function applyCommandSuggestion(item) {{
    if (!input || !item || !item.token) return;
    const insert = suggestionInsert(item);
    const before = input.value.slice(0, item.token.start);
    const after = input.value.slice(item.token.end).replace(/^\s+/, "");
    const spacer = item.takes_arg ? " " : "";
    input.value = `${{before}}${{insert}}${{spacer}}${{after}}`;
    const caret = before.length + insert.length + spacer.length;
    input.focus();
    input.setSelectionRange(caret, caret);
    dirty = input.value.length > 0;
    renderCommandSuggestions();
  }}
  function renderCommandSuggestions() {{
    if (!suggestionBox) return;
    const matches = matchingSuggestions();
    if (!matches.length) {{
      suggestionBox.hidden = true;
      suggestionBox.innerHTML = "";
      return;
    }}
    selectedSuggestion = Math.min(selectedSuggestion, matches.length - 1);
    suggestionBox.hidden = false;
    suggestionBox.innerHTML = "";
    matches.forEach((command, index) => {{
      const button = document.createElement("button");
      button.type = "button";
      button.className = "command-suggestion" + (index === selectedSuggestion ? " active" : "");
      button.setAttribute("role", "option");
      const label = command.kind === "command" ? `${{command.token.symbol}}${{command.name}}` : command.insert;
      button.innerHTML = `<strong>${{label}}</strong><span>${{command.kind}} | ${{command.description}}</span>`;
      button.addEventListener("click", (event) => {{
        event.preventDefault();
        selectedSuggestion = index;
        applyCommandSuggestion(command);
      }});
      suggestionBox.appendChild(button);
      if (index === selectedSuggestion) button.scrollIntoView({{block:"nearest"}});
    }});
  }}
  function setEditMode(id, body) {{
    if (!input || !editMessageId) return;
    editMessageId.value = id;
    input.value = body || "";
    dirty = input.value.length > 0;
    if (editStrip) editStrip.hidden = false;
    if (sendButton) sendButton.textContent = "Save";
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
    input.scrollIntoView({{block:"nearest"}});
    renderCommandSuggestions();
  }}
  function clearEditMode() {{
    if (editMessageId) editMessageId.value = "";
    if (editStrip) editStrip.hidden = true;
    if (sendButton) sendButton.textContent = "Send";
    if (input) {{
      input.value = "";
      dirty = false;
      renderCommandSuggestions();
      input.focus();
    }}
  }}
  async function copyText(value) {{
    if (!value) return false;
    if (navigator.clipboard?.writeText) {{
      await navigator.clipboard.writeText(value);
      return true;
    }}
    const area = document.createElement("textarea");
    area.value = value;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.left = "-9999px";
    area.style.top = "0";
    document.body.appendChild(area);
    area.focus();
    area.select();
    area.setSelectionRange(0, value.length);
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  }}
  document.querySelector("[data-edit-clear]")?.addEventListener("click", clearEditMode);
  list?.addEventListener("click", (event) => {{
    const copyButton = event.target.closest("[data-copy-code]");
    if (copyButton) {{
      event.preventDefault();
      const code = copyButton.closest(".message-code")?.querySelector("code")?.textContent || "";
      copyText(code).then((ok) => {{
        const original = copyButton.textContent || "Copy";
        copyButton.textContent = ok ? "Copied" : "Copy failed";
        window.setTimeout(() => copyButton.textContent = original, 1400);
      }}).catch(() => {{
        copyButton.textContent = "Copy failed";
        window.setTimeout(() => copyButton.textContent = "Copy", 1400);
      }});
      return;
    }}
    const button = event.target.closest("[data-edit-message]");
    if (!button) return;
    event.preventDefault();
    setEditMode(button.getAttribute("data-edit-message"), button.getAttribute("data-edit-body") || "");
  }});
  form?.addEventListener("submit", (event) => {{
    if (event.submitter?.getAttribute("name") === "control") {{
      dirty = false;
      return;
    }}
    if (input?.value.trim().toLowerCase() === "/model") {{
      event.preventDefault();
      modelPicker?.focus();
    }}
  }});
  input.addEventListener("input", () => {{
    dirty = input.value.length > 0;
    selectedSuggestion = 0;
    renderCommandSuggestions();
  }});
  input.addEventListener("focus", () => setTimeout(() => input.scrollIntoView({{block:"nearest"}}), 80));
  input.addEventListener("keydown", (event) => {{
    if (!suggestionBox || suggestionBox.hidden) return;
    const matches = matchingSuggestions();
    if (!matches.length) return;
    if (event.key === "ArrowDown") {{
      event.preventDefault();
      selectedSuggestion = (selectedSuggestion + 1) % matches.length;
      renderCommandSuggestions();
    }} else if (event.key === "ArrowUp") {{
      event.preventDefault();
      selectedSuggestion = (selectedSuggestion + matches.length - 1) % matches.length;
      renderCommandSuggestions();
    }} else if (event.key === "Tab" || event.key === "ArrowRight") {{
      event.preventDefault();
      applyCommandSuggestion(matches[selectedSuggestion]);
    }} else if (event.key === "Escape") {{
      suggestionBox.hidden = true;
    }}
  }});
  async function poll() {{
    if (!list || dirty) {{
      scheduleAgentPoll(1800);
      return;
    }}
    try {{
      const response = await fetch("/agents/slots/{}/state", {{cache:"no-store"}});
      if (response.ok) {{
        const data = await response.json();
        const replaced = replaceMessages(data.messages_html);
        status.textContent = data.active_status || (data.running ? (data.current || "running") : "idle");
        count.textContent = data.message_count + " messages";
        if (cancelButton) cancelButton.disabled = !data.running;
        scheduleAgentPoll(!replaced ? 1200 : data.running ? 1200 : 4000);
        return;
      }}
    }} catch (_) {{}}
    scheduleAgentPoll(4000);
  }}
  function renderSlotStates(slots) {{
    let anyRunning = false;
    for (const slot of slots || []) {{
      const id = String(slot.id);
      const entry = slotRows.get(id);
      if (!entry) continue;
      const label = slot.running ? (slot.current || "running") : (slot.status || "idle");
      anyRunning = anyRunning || !!slot.running;
      if (entry.status) entry.status.textContent = label;
      if (id === activeSlotId && cancelButton) cancelButton.disabled = !slot.running;
      entry.row.setAttribute("data-slot-running", slot.running ? "true" : "false");
      entry.row.classList.toggle("running", !!slot.running);
      if (slot.running) {{
        entry.row.classList.remove("done");
        if (entry.badge) entry.badge.hidden = true;
      }} else if (entry.wasRunning) {{
        entry.row.classList.add("done");
        if (entry.badge) {{
          entry.badge.textContent = "done";
          entry.badge.hidden = false;
        }}
        if (id === activeSlotId && status) status.textContent = "done";
      }}
      entry.wasRunning = !!slot.running;
    }}
    return anyRunning;
  }}
  async function pollSlots() {{
    try {{
      const response = await fetch("/agents/slots/state", {{cache:"no-store"}});
      if (response.ok) {{
        const data = await response.json();
        const anyRunning = renderSlotStates(data.slots || []);
        scheduleSlotPoll(anyRunning ? 1200 : 4000);
        return;
      }}
    }} catch (_) {{}}
    scheduleSlotPoll(5000);
  }}
  function refreshVisibleState() {{
    if (document.visibilityState === "hidden") return;
    scheduleSlotPoll(0);
    if (!viewingTranscript) scheduleAgentPoll(0);
  }}
  if (list) list.scrollTop = list.scrollHeight;
  window.addEventListener("pageshow", refreshVisibleState);
  window.addEventListener("focus", refreshVisibleState);
  document.addEventListener("visibilitychange", () => {{
    if (document.visibilityState === "visible") refreshVisibleState();
  }});
  scheduleSlotPoll(1000);
  if (!viewingTranscript) scheduleAgentPoll(1200);
}})();
</script>
"##,
            active_slot.id,
            html_escape(&runtime.label),
            active_slot.id,
            active_slot.id,
            active_slot.id
        ),
    )
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
    let return_location = format!(
        "/agents?slot={}&refresh=1&usage=1",
        form.slot_id.unwrap_or_default()
    );
    if let Some(command) = &state.config.codex_reset_command {
        let Some((program, args)) = command.split_first() else {
            return (StatusCode::CONFLICT, "no reset command configured").into_response();
        };
        let output = StdCommand::new(program).args(args).output();
        return match output {
            Ok(output) if output.status.success() => {
                refresh_codex_index(state.clone());
                Redirect::to(&return_location).into_response()
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
            Redirect::to(&return_location).into_response()
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
    let mut edit_message_id = None;
    let mut body = String::new();
    let mut control = String::new();
    let mut requested_model = String::new();
    let mut requested_reasoning_effort = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "slot_id" => {
                if let Ok(text) = field.text().await {
                    slot_id = text.trim().parse::<i64>().ok();
                }
            }
            "edit_message_id" => {
                if let Ok(text) = field.text().await {
                    edit_message_id = text.trim().parse::<i64>().ok().filter(|id| *id > 0);
                }
            }
            "control" => {
                if let Ok(text) = field.text().await {
                    control = text.trim().to_ascii_lowercase();
                }
            }
            "body" => {
                if let Ok(text) = field.text().await {
                    body = text;
                }
            }
            "model" => {
                if let Ok(text) = field.text().await {
                    requested_model = text;
                }
            }
            "reasoning_effort" => {
                if let Ok(text) = field.text().await {
                    requested_reasoning_effort = text;
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
    let settings =
        requested_agent_run_settings(&state, &requested_model, &requested_reasoning_effort);
    if control == "stop" {
        let stopped = stop_agent_job(&state, slot.id);
        let cleared = clear_agent_queue(&state, slot.id);
        let queue_text = if cleared == 0 {
            String::new()
        } else if cleared == 1 {
            " Cleared 1 queued follow-up.".into()
        } else {
            format!(" Cleared {cleared} queued follow-ups.")
        };
        append_agent_assistant(
            &state,
            slot.id,
            &if stopped {
                format!("Stop requested.{queue_text}")
            } else {
                format!("This slot is not running.{queue_text}")
            },
        );
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    let body = body.trim().to_string();
    if body.len() > MAX_AGENT_MESSAGE_CHARS {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    if body.is_empty() {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    if let Some(message_id) = edit_message_id {
        if state.agent_jobs.lock().unwrap().contains_key(&slot.id) {
            append_agent_assistant(&state, slot.id, "Cancel the running job before editing.");
            return Redirect::to(&agent_location(Some(slot.id))).into_response();
        }
        let existing_attachment_id = {
            let db = state.db.lock().unwrap();
            agent_user_message_attachment_id(&db, slot.id, message_id).unwrap_or(None)
        };
        let Some(existing_attachment_id) = existing_attachment_id else {
            return Redirect::to(&agent_location(Some(slot.id))).into_response();
        };
        let _ = existing_attachment_id;
        {
            let db = state.db.lock().unwrap();
            let _ = update_agent_user_message(&db, slot.id, message_id, &body, None);
            let _ = delete_agent_messages_after(&db, slot.id, message_id);
            let _ = db.execute(
                "DELETE FROM agent_sessions WHERE slot_id = ?1",
                params![slot.id],
            );
        }
        let _ = clear_agent_queue(&state, slot.id);
        if handle_agent_control(&state, &slot, &body) {
            return Redirect::to(&agent_location(Some(slot.id))).into_response();
        }
        start_agent_job(state.clone(), slot.id, body, None, settings);
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    {
        let db = state.db.lock().unwrap();
        let _ = append_agent_message(&db, slot.id, "user", &body, None);
    }
    if handle_agent_control(&state, &slot, &body) {
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    let request_body = body;
    if state.agent_jobs.lock().unwrap().contains_key(&slot.id) {
        let queued_count = queue_agent_request(
            &state,
            slot.id,
            QueuedAgentRequest {
                body: request_body,
                attachment_id: None,
                settings,
            },
        );
        let queued_text = if queued_count == 1 {
            "1 queued follow-up".to_string()
        } else {
            format!("{queued_count} queued follow-ups")
        };
        append_agent_assistant(
            &state,
            slot.id,
            &format!("Queued behind the current Codex turn. {queued_text}."),
        );
        return Redirect::to(&agent_location(Some(slot.id))).into_response();
    }
    start_agent_job(state.clone(), slot.id, request_body, None, settings);
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

async fn agent_slots_state(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let summaries = slots
        .iter()
        .map(|slot| agent_slot_summary(&state, slot))
        .collect();
    Json(AgentSlotsPoll { slots: summaries }).into_response()
}

async fn agent_model_catalog(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(response) = raw_guard(&state, &headers) {
        return response;
    }
    let models = codex_model_catalog_snapshot(&state);
    Json(AgentModelCatalogPoll { models }).into_response()
}

fn agent_run_for(state: &AppState, slot_id: i64) -> Option<AgentRun> {
    state.agent_jobs.lock().unwrap().get(&slot_id).cloned()
}

fn agent_slot_summary(state: &AppState, slot: &AgentSlotRow) -> AgentSlotSummary {
    if let Some(run) = agent_run_for(state, slot.id) {
        let label = if run.current.trim().is_empty() {
            run.status.clone()
        } else {
            run.current.clone()
        };
        let label = format!("{}{}", label, queue_suffix(agent_queue_len(state, slot.id)));
        return AgentSlotSummary {
            id: slot.id,
            name: slot.name.clone(),
            running: true,
            current: label.clone(),
            status: label,
        };
    }
    let idle = format!("idle{}", queue_suffix(agent_queue_len(state, slot.id)));
    AgentSlotSummary {
        id: slot.id,
        name: slot.name.clone(),
        running: false,
        current: String::new(),
        status: idle,
    }
}

fn agent_slot_runtime(state: &AppState, slot: &AgentSlotRow) -> SlotRuntime {
    if let Some(run) = agent_run_for(state, slot.id) {
        let label = if run.current.trim().is_empty() {
            run.status
        } else {
            run.current
        };
        return SlotRuntime {
            label: format!("{}{}", label, queue_suffix(agent_queue_len(state, slot.id))),
        };
    }
    SlotRuntime {
        label: format!("idle{}", queue_suffix(agent_queue_len(state, slot.id))),
    }
}

fn agent_slot_rail_html(state: &AppState, slots: &[AgentSlotRow], active_id: i64) -> String {
    let rows = slots.iter().map(|slot| {
        let summary = agent_slot_summary(state, slot);
        let active_class = if slot.id == active_id { " active" } else { "" };
        let running_class = if summary.running { " running" } else { "" };
        format!(r#"<div class="channel-row{active_class}{running_class}" data-slot-row data-slot-id="{}" data-slot-running="{}"><a class="channel-link" href="/agents?slot={}" aria-label="Open {}"><strong>{}</strong><span data-slot-status>{}</span><span class="slot-badge" data-slot-badge hidden></span></a></div>"#, summary.id, summary.running, summary.id, html_escape(&summary.name), html_escape(&summary.name), html_escape(&summary.status))
    }).collect::<Vec<_>>().join("");
    format!(
        r#"<aside class="channel-rail" aria-label="Agent sessions"><div class="rail-title">Sessions</div><div class="channel-list">{rows}</div></aside>"#
    )
}

fn agent_messages_html(messages: &[AgentMessageRow]) -> String {
    if messages.is_empty() {
        return r#"<p class="empty">No messages in this slot yet.</p>"#.into();
    }
    let ordered = messages.iter().rev().collect::<Vec<_>>();
    let mut rendered = String::new();
    let mut index = 0usize;
    while index < ordered.len() {
        if agent_activity_kind(ordered[index]).is_some() {
            let start = index;
            while index < ordered.len() && agent_activity_kind(ordered[index]).is_some() {
                index += 1;
            }
            rendered.push_str(&agent_activity_stack_html(&ordered[start..index]));
        } else {
            rendered.push_str(&agent_message_html(ordered[index]));
            index += 1;
        }
    }
    rendered
}

#[derive(Copy, Clone)]
enum AgentActivityKind {
    Start,
    Run,
    Exit,
}

fn agent_activity_kind(message: &AgentMessageRow) -> Option<AgentActivityKind> {
    if message.role != "assistant" || message.attachment.is_some() {
        return None;
    }
    let body = message.body.trim();
    if body.starts_with("running: `") {
        return Some(AgentActivityKind::Run);
    }
    if body.starts_with("command exit ") {
        return Some(AgentActivityKind::Exit);
    }
    if body.contains(" started in `") && body.ends_with("`.") {
        return Some(AgentActivityKind::Start);
    }
    None
}

fn agent_message_html(message: &AgentMessageRow) -> String {
    let role_class = agent_role_class(&message.role);
    let avatar = if role_class == "user" { "U" } else { "A" };
    let actions = if role_class == "user" && message.id > 0 {
        format!(
            r#"<button type="button" class="message-edit" data-edit-message="{}" data-edit-body="{}">Edit</button>"#,
            message.id,
            html_attr_escape(&message.body)
        )
    } else {
        String::new()
    };
    let body = message_body_html(&message.body);
    format!(
        r#"<article class="message message-{role_class}" data-message-entry>
  <div class="message-avatar">{avatar}</div>
  <div class="message-body">
    <div class="message-meta"><strong>{}</strong><span class="message-log">{}</span>{actions}</div>
    {}
  </div>
</article>"#,
        html_escape(agent_role_label(&message.role)),
        html_escape(&compact_local_time(&message.created_at)),
        body
    )
}

fn message_body_html(body: &str) -> String {
    if body.trim().is_empty() {
        return String::new();
    }
    let normalized = normalize_markdown_fences(body);
    let parser = Parser::new_ext(&normalized, markdown_options()).map(markdown_event);
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    format!(r#"<div class="message-content">{rendered}</div>"#)
}

fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options
}

fn markdown_event(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(Tag::CodeBlock(kind)) => Event::Html(CowStr::from(code_block_open_html(kind))),
        Event::End(TagEnd::CodeBlock) => Event::Html(CowStr::Borrowed("</code></pre></div>")),
        Event::Html(value) | Event::InlineHtml(value) => Event::Text(value),
        _ => event,
    }
}

fn code_block_open_html(kind: CodeBlockKind<'_>) -> String {
    let language = match kind {
        CodeBlockKind::Fenced(info) => info
            .split_whitespace()
            .next()
            .map(|value| {
                value
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+'))
                    .collect::<String>()
            })
            .unwrap_or_default(),
        CodeBlockKind::Indented => String::new(),
    };
    let label = if language.is_empty() {
        "code"
    } else {
        language.as_str()
    };
    let class_attr = if language.is_empty() {
        String::new()
    } else {
        format!(r#" class="language-{}""#, html_attr_escape(&language))
    };
    format!(
        r#"<div class="message-code"><div class="message-code-head"><span>{}</span><button type="button" class="message-copy" data-copy-code>Copy</button></div><pre><code{}>"#,
        html_escape(label),
        class_attr
    )
}

fn normalize_markdown_fences(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut changed = false;
    for line in value.split_inclusive('\n') {
        let without_spaces = line.trim_start_matches(' ');
        let space_count = line.len().saturating_sub(without_spaces.len());
        let quote_count = without_spaces.chars().take_while(|ch| *ch == '\'').count();
        if space_count <= 3 && quote_count >= 3 {
            normalized.extend(std::iter::repeat_n(' ', space_count));
            normalized.push_str("```");
            normalized.push_str(&without_spaces[quote_count..]);
            changed = true;
        } else {
            normalized.push_str(line);
        }
    }
    if changed {
        normalized
    } else {
        value.to_string()
    }
}

fn agent_role_class(role: &str) -> &'static str {
    if role == "user" { "user" } else { "assistant" }
}

fn agent_role_label(role: &str) -> &str {
    if role == "user" { "You" } else { "Codex" }
}

fn agent_activity_stack_html(messages: &[&AgentMessageRow]) -> String {
    let rows = messages
        .iter()
        .enumerate()
        .map(|(index, message)| agent_activity_row_html(index + 1, message))
        .collect::<Vec<_>>()
        .join("");
    let event_count = messages.len();
    let event_label = if event_count == 1 {
        "1 event".to_string()
    } else {
        format!("{event_count} events")
    };
    let preview = messages
        .iter()
        .find_map(|message| agent_activity_preview(message))
        .unwrap_or_else(|| "Codex command activity".into());
    let started_at = messages
        .first()
        .map(|message| html_escape(&compact_local_time(&message.created_at)))
        .unwrap_or_default();
    let fold_key = html_attr_escape(&agent_activity_fold_key(messages));
    format!(
        r#"<article class="message message-activity" data-message-entry>
  <div class="message-avatar">$</div>
  <div class="message-body">
    <div class="message-meta"><strong>Activity</strong><span class="message-log">{started_at}</span></div>
    <details class="tool-fold" data-fold-key="{fold_key}">
      <summary><span>{}</span><code>{}</code></summary>
      <ol class="tool-stack" aria-label="Codex command activity">{rows}</ol>
    </details>
  </div>
 </article>"#,
        html_escape(&event_label),
        html_escape(&truncate_text(&preview, 140))
    )
}

fn agent_activity_fold_key(messages: &[&AgentMessageRow]) -> String {
    messages
        .first()
        .map(|message| message_fold_key("activity", message))
        .unwrap_or_else(|| "activity-empty".into())
}

fn message_fold_key(prefix: &str, message: &AgentMessageRow) -> String {
    if message.id > 0 {
        return format!("{prefix}-{}", message.id);
    }
    let mut hasher = Sha256::new();
    hasher.update(message.created_at.as_bytes());
    hasher.update([0]);
    hasher.update(message.role.as_bytes());
    hasher.update([0]);
    hasher.update(message.body.as_bytes());
    let digest = hasher.finalize();
    format!("{prefix}-{}", hex::encode(&digest[..8]))
}

fn agent_activity_preview(message: &AgentMessageRow) -> Option<String> {
    let body = message.body.trim();
    match agent_activity_kind(message)? {
        AgentActivityKind::Start => Some(body.to_string()),
        AgentActivityKind::Run => first_backtick_text(body)
            .or_else(|| body.strip_prefix("running:"))
            .map(|value| value.trim().to_string()),
        AgentActivityKind::Exit => {
            let exit_code = body
                .strip_prefix("command exit ")
                .and_then(|rest| rest.split_once(':').map(|(code, _)| code.trim()))
                .unwrap_or("?");
            let command = first_backtick_text(body).unwrap_or("(unknown command)");
            Some(format!("exit {exit_code}: {command}"))
        }
    }
}

fn agent_activity_row_html(index: usize, message: &AgentMessageRow) -> String {
    let Some(kind) = agent_activity_kind(message) else {
        return String::new();
    };
    let body = message.body.trim();
    let number = format!("{index:02}");
    match kind {
        AgentActivityKind::Start => format!(
            r#"<li class="tool-row tool-row-start"><span class="tool-index">{number}</span><span class="tool-label">start</span><code>{}</code></li>"#,
            html_escape(body)
        ),
        AgentActivityKind::Run => {
            let command = first_backtick_text(body)
                .or_else(|| body.strip_prefix("running:"))
                .unwrap_or(body)
                .trim();
            format!(
                r#"<li class="tool-row tool-row-run"><span class="tool-index">{number}</span><span class="tool-label">run</span><code>{}</code></li>"#,
                html_escape(command)
            )
        }
        AgentActivityKind::Exit => {
            let exit_code = body
                .strip_prefix("command exit ")
                .and_then(|rest| rest.split_once(':').map(|(code, _)| code.trim()))
                .unwrap_or("?");
            let command = first_backtick_text(body).unwrap_or("(unknown command)");
            let output = fenced_text(body);
            let output_html = if output.trim().is_empty() {
                String::new()
            } else {
                let fold_key = html_attr_escape(&message_fold_key("output", message));
                format!(
                    r#"<details class="tool-output" data-fold-key="{fold_key}"><summary>output</summary><pre>{}</pre></details>"#,
                    html_escape(output.trim())
                )
            };
            format!(
                r#"<li class="tool-row tool-row-exit"><span class="tool-index">{number}</span><span class="tool-label">exit {}</span><code>{}</code>{output_html}</li>"#,
                html_escape(exit_code),
                html_escape(command)
            )
        }
    }
}

fn first_backtick_text(text: &str) -> Option<&str> {
    let start = text.find('`')? + 1;
    let end = text[start..].find('`')?;
    Some(&text[start..start + end])
}

fn fenced_text(text: &str) -> &str {
    text.split_once("```text\n")
        .map(|(_, output)| output.strip_suffix("\n```").unwrap_or(output))
        .unwrap_or("")
}

fn codex_usage_dialog(
    config: &Config,
    usage: Option<&CodexUsageSnapshot>,
    loaded: bool,
    slot_id: i64,
) -> String {
    let command = html_escape(&agent_command_label(config));
    let reset_available = usage
        .and_then(|usage| usage.reset_credits.as_ref())
        .is_some_and(|credits| credits.available_count > 0);
    let reset = if config.codex_reset_command.is_some() || reset_available {
        format!(
            r#"<form class="reset-form" action="/agents/codex/reset" method="post" data-reset-form>
  <input type="hidden" name="confirm" value="USE_RESET">
  <input type="hidden" name="slot_id" value="{slot_id}">
  <button type="submit" class="danger-icon ghost">Use reset</button>
</form>"#
        )
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
  <header><strong>Codex Usage</strong><div class="usage-head-actions"><form action="/agents" method="get" data-refresh-form><input type="hidden" name="slot" value="{slot_id}"><input type="hidden" name="refresh" value="1"><input type="hidden" name="usage" value="1"><button type="submit" class="ghost nav-icon usage-refresh" aria-label="Refresh Codex usage" title="Refresh Codex usage" data-refresh-button>↻</button></form><button type="button" class="icon" data-codex-close aria-label="Close">x</button></div></header>
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
        .map(format_epoch_date)
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
        .map(format_epoch_date)
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
    for (index, credit) in credits.credits.iter().enumerate() {
        let expiry = credit
            .expires_at
            .map(format_epoch_date)
            .unwrap_or_else(|| "expiry unknown".into());
        parts.push(format!("{} {} · expires {expiry}", index + 1, credit.title));
    }
    if !credits.credits.is_empty() {
        return parts.join(" · ");
    }
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
    let count = html.matches("data-message-entry").count();
    (count > 0).then_some(count)
}

fn list_agent_slots(db: &Connection) -> rusqlite::Result<Vec<AgentSlotRow>> {
    let mut stmt = db.prepare("SELECT id, name, workdir, goal FROM agent_slots ORDER BY id ASC")?;
    stmt.query_map([], |row| {
        Ok(AgentSlotRow {
            id: row.get(0)?,
            name: row.get(1)?,
            workdir: row.get(2)?,
            goal: row.get(3)?,
        })
    })?
    .collect()
}

fn get_agent_slot(db: &Connection, id: i64) -> rusqlite::Result<Option<AgentSlotRow>> {
    db.query_row(
        "SELECT id, name, workdir, goal FROM agent_slots WHERE id = ?1",
        params![id],
        |row| {
            Ok(AgentSlotRow {
                id: row.get(0)?,
                name: row.get(1)?,
                workdir: row.get(2)?,
                goal: row.get(3)?,
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
        "SELECT m.id, m.role, m.body, m.created_at, a.id, a.original_name, a.content_type, a.size_bytes
         FROM agent_messages m
         LEFT JOIN agent_attachments a ON a.id = m.attachment_id
         WHERE m.slot_id = ?1
         ORDER BY m.id DESC
         LIMIT 200",
    )?;
    stmt.query_map(params![slot_id], |row| {
        let attachment_id = row.get::<_, Option<i64>>(4)?;
        let attachment = if let Some(id) = attachment_id {
            let content_type = row.get::<_, String>(6)?;
            Some(AgentAttachmentSummary {
                id,
                name: row.get(5)?,
                is_image: content_type.starts_with("image/"),
                content_type,
                size_bytes: row.get(7)?,
            })
        } else {
            None
        };
        Ok(AgentMessageRow {
            id: row.get(0)?,
            role: row.get(1)?,
            body: row.get(2)?,
            created_at: row.get(3)?,
            attachment,
        })
    })?
    .collect()
}

fn last_agent_message(db: &Connection, slot_id: i64) -> rusqlite::Result<Option<AgentMessageRow>> {
    db.query_row(
        "SELECT id, role, body, created_at
         FROM agent_messages
         WHERE slot_id = ?1
         ORDER BY id DESC
         LIMIT 1",
        params![slot_id],
        |row| {
            Ok(AgentMessageRow {
                id: row.get(0)?,
                role: row.get(1)?,
                body: row.get(2)?,
                created_at: row.get(3)?,
                attachment: None,
            })
        },
    )
    .optional()
}

fn agent_user_message_attachment_id(
    db: &Connection,
    slot_id: i64,
    message_id: i64,
) -> rusqlite::Result<Option<Option<i64>>> {
    db.query_row(
        "SELECT attachment_id
         FROM agent_messages
         WHERE id = ?1 AND slot_id = ?2 AND role = 'user'",
        params![message_id, slot_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()
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

fn update_agent_user_message(
    db: &Connection,
    slot_id: i64,
    message_id: i64,
    body: &str,
    attachment_id: Option<i64>,
) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE agent_messages
         SET body = ?1, attachment_id = COALESCE(?2, attachment_id), created_at = ?3
         WHERE id = ?4 AND slot_id = ?5 AND role = 'user'",
        params![
            body,
            attachment_id,
            Utc::now().to_rfc3339(),
            message_id,
            slot_id
        ],
    )?;
    Ok(())
}

fn delete_agent_messages_after(
    db: &Connection,
    slot_id: i64,
    message_id: i64,
) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM agent_messages WHERE slot_id = ?1 AND id > ?2",
        params![slot_id, message_id],
    )?;
    Ok(())
}

fn append_agent_assistant(state: &AppState, slot_id: i64, body: &str) {
    let db = state.db.lock().unwrap();
    let _ = append_agent_message(&db, slot_id, "assistant", body, None);
}

fn mark_interrupted_agent_runs(state: &AppState) {
    let db = state.db.lock().unwrap();
    let slots = list_agent_slots(&db).unwrap_or_default();
    for slot in slots {
        let Ok(Some(message)) = last_agent_message(&db, slot.id) else {
            continue;
        };
        if agent_activity_kind(&message).is_some() {
            let _ = append_agent_message(
                &db,
                slot.id,
                "assistant",
                &format!(
                    "Mobailmux restarted while `{}` was running, so this local web transcript ended before Codex returned a final answer. Send a new message to continue from the saved Codex session.",
                    slot.name
                ),
                None,
            );
        }
    }
}

fn set_agent_goal(db: &Connection, slot_id: i64, goal: &str) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE agent_slots SET goal = ?1 WHERE id = ?2",
        params![goal, slot_id],
    )?;
    Ok(())
}

fn reset_agent_slot_chat(state: &AppState, slot_id: i64, workdir: &Path) -> bool {
    let stopped = stop_agent_job(state, slot_id);
    let _ = clear_agent_queue(state, slot_id);
    let workdir = workdir.to_string_lossy().to_string();
    let db = state.db.lock().unwrap();
    let _ = db.execute(
        "UPDATE agent_slots SET workdir = ?1 WHERE id = ?2",
        params![workdir, slot_id],
    );
    let _ = db.execute(
        "DELETE FROM agent_sessions WHERE slot_id = ?1",
        params![slot_id],
    );
    let _ = db.execute(
        "DELETE FROM agent_messages WHERE slot_id = ?1",
        params![slot_id],
    );
    stopped
}

fn handle_agent_control(state: &Arc<AppState>, slot: &AgentSlotRow, body: &str) -> bool {
    let trimmed = body.trim();
    let Some((prefix, raw_text)) = agent_control_text(trimmed) else {
        return false;
    };
    let text = normalize_agent_command_text(raw_text);
    let lower = text.to_ascii_lowercase();
    if matches!(lower.as_str(), "help" | "commands") {
        append_agent_assistant(state, slot.id, &agent_help_text(state));
        return true;
    }
    if let Some(arg) = command_arg(&text, "goal") {
        let goal = arg.trim();
        if goal.is_empty() {
            let current = slot.goal.trim();
            let response = if current.is_empty() {
                "No goal is set for this slot. Use `/goal <objective>` to set one.".to_string()
            } else {
                format!(
                    "Current goal:\n```text\n{}\n```",
                    truncate_text(current, 2000)
                )
            };
            append_agent_assistant(state, slot.id, &response);
            return true;
        }
        if matches!(
            goal.to_ascii_lowercase().as_str(),
            "clear" | "none" | "unset" | "off"
        ) {
            let db = state.db.lock().unwrap();
            let _ = set_agent_goal(&db, slot.id, "");
            drop(db);
            append_agent_assistant(state, slot.id, "Goal cleared for this slot.");
            return true;
        }
        let goal = truncate_text(goal, MAX_AGENT_GOAL_CHARS);
        let db = state.db.lock().unwrap();
        let _ = set_agent_goal(&db, slot.id, &goal);
        drop(db);
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "Goal set for this slot. Future Codex messages will include:\n```text\n{goal}\n```"
            ),
        );
        return true;
    }
    if lower == "clear-goal" {
        let db = state.db.lock().unwrap();
        let _ = set_agent_goal(&db, slot.id, "");
        drop(db);
        append_agent_assistant(state, slot.id, "Goal cleared for this slot.");
        return true;
    }
    if matches!(lower.as_str(), "slots" | "list" | "overview") {
        append_agent_assistant(state, slot.id, &agent_slots_status_text(state));
        return true;
    }
    if lower == "status" {
        let runtime = agent_slot_runtime(state, slot);
        let usage = refresh_codex_index_blocking(state).usage;
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{} is {}.\n{}",
                slot.name,
                runtime.label,
                codex_usage_text(usage.as_ref())
            ),
        );
        return true;
    }
    if lower == "usage" || lower == "limits" {
        let usage = refresh_codex_index_blocking(state).usage;
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{}\n\nOpen the 📊 Usage panel to refresh again or use a reset credit manually.",
                codex_usage_text(usage.as_ref())
            ),
        );
        return true;
    }
    if lower == "model" || lower == "settings" {
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "Choose a model and thinking difficulty in the composer. Agent command: `{}`",
                agent_command_label(&state.config)
            ),
        );
        return true;
    }
    if matches!(lower.as_str(), "fresh" | "new") {
        let workdir = slot.workdir.clone();
        let stopped = reset_agent_slot_chat(state, slot.id, Path::new(&workdir));
        let stop_text = if stopped {
            " Stopped the current job."
        } else {
            ""
        };
        append_agent_assistant(
            state,
            slot.id,
            &format!(
                "{} chat reset.{stop_text} Your next message starts a new agent chat.",
                slot.name
            ),
        );
        return true;
    }
    if lower == "stop" {
        let stopped = stop_agent_job(state, slot.id);
        let cleared = clear_agent_queue(state, slot.id);
        let queue_text = if cleared == 0 {
            String::new()
        } else if cleared == 1 {
            " Cleared 1 queued follow-up.".into()
        } else {
            format!(" Cleared {cleared} queued follow-ups.")
        };
        append_agent_assistant(
            state,
            slot.id,
            &if stopped {
                format!("Stop requested.{queue_text}")
            } else {
                format!("This slot is not running.{queue_text}")
            },
        );
        return true;
    }
    if matches!(lower.as_str(), "queue" | "queued") {
        append_agent_assistant(state, slot.id, &agent_queue_text(state, slot));
        return true;
    }
    if matches!(lower.as_str(), "clear-queue" | "clearqueue" | "queue clear") {
        let cleared = clear_agent_queue(state, slot.id);
        append_agent_assistant(
            state,
            slot.id,
            &format!("Cleared {cleared} queued follow-up(s) for {}.", slot.name),
        );
        return true;
    }
    append_agent_assistant(
        state,
        slot.id,
        &format!("Unknown command: `{prefix}{raw_text}`. Type `/help` for commands."),
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

fn queue_agent_request(state: &AppState, slot_id: i64, request: QueuedAgentRequest) -> usize {
    let mut queues = state.agent_queues.lock().unwrap();
    let queue = queues.entry(slot_id).or_default();
    queue.push_back(request);
    queue.len()
}

fn pop_queued_agent_request(state: &AppState, slot_id: i64) -> Option<QueuedAgentRequest> {
    let mut queues = state.agent_queues.lock().unwrap();
    queues.entry(slot_id).or_default().pop_front()
}

fn clear_agent_queue(state: &AppState, slot_id: i64) -> usize {
    let mut queues = state.agent_queues.lock().unwrap();
    let queue = queues.entry(slot_id).or_default();
    let count = queue.len();
    queue.clear();
    count
}

fn agent_queue_len(state: &AppState, slot_id: i64) -> usize {
    state
        .agent_queues
        .lock()
        .unwrap()
        .get(&slot_id)
        .map(VecDeque::len)
        .unwrap_or(0)
}

fn agent_queue_text(state: &AppState, slot: &AgentSlotRow) -> String {
    let items = state
        .agent_queues
        .lock()
        .unwrap()
        .get(&slot.id)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return format!("{} queue is empty.", slot.name);
    }
    let rows = items
        .iter()
        .enumerate()
        .map(|(index, request)| {
            format!(
                "{}. {}",
                index + 1,
                truncate_text(&request.body, 220).replace('\n', " ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{} queued follow-ups:\n```text\n{rows}\n```", slot.name)
}

fn queue_suffix(count: usize) -> String {
    if count == 0 {
        String::new()
    } else if count == 1 {
        " · 1 queued".into()
    } else {
        format!(" · {count} queued")
    }
}

fn requested_agent_run_settings(
    state: &Arc<AppState>,
    requested_model: &str,
    requested_reasoning_effort: &str,
) -> AgentRunSettings {
    let models = codex_model_catalog_snapshot(state);
    validate_agent_run_settings(&models, requested_model, requested_reasoning_effort)
}

fn validate_agent_run_settings(
    models: &[CodexModel],
    requested_model: &str,
    requested_reasoning_effort: &str,
) -> AgentRunSettings {
    let requested_model = requested_model.trim();
    let Some(model) = models.iter().find(|model| model.model == requested_model) else {
        return AgentRunSettings::default();
    };
    let requested_reasoning_effort = requested_reasoning_effort.trim();
    let reasoning_effort = model
        .supported_reasoning_efforts
        .iter()
        .any(|option| option.effort == requested_reasoning_effort)
        .then(|| requested_reasoning_effort.to_string());
    AgentRunSettings {
        model: Some(model.model.clone()),
        reasoning_effort,
    }
}

fn agent_run_settings_label(settings: &AgentRunSettings) -> String {
    let mut parts = Vec::new();
    if let Some(model) = &settings.model {
        parts.push(format!("model `{model}`"));
    }
    if let Some(effort) = &settings.reasoning_effort {
        parts.push(format!("thinking `{effort}`"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn apply_agent_run_settings(command: &mut TokioCommand, settings: &AgentRunSettings) {
    if let Some(model) = &settings.model {
        command.arg("--model").arg(model);
    }
    if let Some(effort) = &settings.reasoning_effort {
        let value = serde_json::to_string(effort).unwrap_or_else(|_| "\"medium\"".into());
        command
            .arg("--config")
            .arg(format!("model_reasoning_effort={value}"));
    }
}

fn start_next_queued_agent_job(state: Arc<AppState>, slot_id: i64) {
    let Some(next) = pop_queued_agent_request(&state, slot_id) else {
        return;
    };
    let remaining = agent_queue_len(&state, slot_id);
    let remaining_text = if remaining == 0 {
        String::new()
    } else if remaining == 1 {
        " 1 follow-up remains queued.".into()
    } else {
        format!(" {remaining} follow-ups remain queued.")
    };
    append_agent_assistant(
        &state,
        slot_id,
        &format!("Starting queued follow-up.{remaining_text}"),
    );
    start_agent_job(state, slot_id, next.body, next.attachment_id, next.settings);
}

fn agent_help_text(state: &AppState) -> String {
    let slots = {
        let db = state.db.lock().unwrap();
        list_agent_slots(&db).unwrap_or_default()
    };
    let mut lines = vec![
        "Agent commands:".to_string(),
        "Use `/` in the web chatbox. The old `!` prefix still works.".to_string(),
    ];
    lines.extend(
        [
            "- `/goal <objective>` sets a goal that is included in future Codex prompts",
            "- `/goal` shows the current goal",
            "- `/goal clear` or `/clear-goal` clears it",
            "- `/slots`",
            "- `/fresh`",
            "- `/status`",
            "- `/usage`",
            "- `/stop`",
            "- `/queue`",
            "- `/clear-queue`",
            "- `/model`",
            "- `/help` or `/commands`",
        ]
        .into_iter()
        .map(str::to_string),
    );
    lines.push(String::new());
    lines.push(format!("{} slots configured.", slots.len()));
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
            let status = if jobs.contains_key(&slot.id) {
                "running"
            } else {
                "idle"
            };
            let status = format!(
                "{}{}",
                status,
                queue_suffix(agent_queue_len(state, slot.id))
            );
            let goal = slot.goal.trim();
            if goal.is_empty() {
                format!("{}: {status}", slot.name)
            } else {
                format!(
                    "{}: {status} | goal: {}",
                    slot.name,
                    truncate_text(goal, 120)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Slots:\n```text\n{lines}\n```")
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

    let dashboard = fetch_codex_app_server_dashboard(config);
    let usage =
        merge_codex_rate_limit_status(latest_codex_usage(&files), dashboard.rate_limits.as_ref());

    CodexIndex {
        usage,
        conversations,
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

    let mut visible_messages = Vec::new();
    for (order, line) in reader.lines().map_while(Result::ok).enumerate() {
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
        if let Some(message) = codex_visible_message_event(&value, order)
            && !message.text.trim().is_empty()
        {
            visible_messages.push(message);
        }
    }

    for message in dedupe_codex_visible_messages(visible_messages) {
        message_count += 1;
        if message.role == "user" && first_user.is_none() {
            first_user = Some(message.text.clone());
        }
        last_message = Some(message.text);
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
    let mut messages = Vec::new();
    for (order, line) in reader.lines().map_while(Result::ok).enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(message) = codex_visible_message_event(&value, order)
            && !message.text.trim().is_empty()
        {
            messages.push(message);
        }
    }
    let mut visible_messages = dedupe_codex_visible_messages(messages);
    if codex_transcript_interrupted(&visible_messages) {
        let timestamp = visible_messages
            .last()
            .map(|message| message.timestamp.clone())
            .unwrap_or_default();
        visible_messages.push(CodexVisibleMessage {
            role: "assistant".into(),
            text: "This saved Codex transcript ended before Codex returned a final answer. The Mobailmux service or Codex process was likely interrupted; send a new message in the agent slot to continue.".into(),
            timestamp,
            order: usize::MAX,
            fallback: false,
            final_answer: false,
            assistant_progress: false,
        });
    }
    let mut rows = visible_messages
        .into_iter()
        .map(|message| AgentMessageRow {
            id: 0,
            role: message.role,
            body: message.text,
            created_at: message.timestamp,
            attachment: None,
        })
        .collect::<Vec<_>>();
    if rows.len() > MAX_CODEX_TRANSCRIPT_MESSAGES {
        let start = rows.len() - MAX_CODEX_TRANSCRIPT_MESSAGES;
        rows = rows.split_off(start);
    }
    rows.reverse();
    Ok(rows)
}

fn codex_transcript_interrupted(messages: &[CodexVisibleMessage]) -> bool {
    messages.iter().any(|message| message.assistant_progress)
        && !messages.iter().any(|message| message.final_answer)
}

fn codex_visible_message_event(
    value: &serde_json::Value,
    order: usize,
) -> Option<CodexVisibleMessage> {
    let timestamp = value
        .get("timestamp")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    if let Some((role, text, final_answer, assistant_progress)) = codex_visible_message(value) {
        return Some(CodexVisibleMessage {
            role,
            text,
            timestamp,
            order,
            fallback: false,
            final_answer,
            assistant_progress,
        });
    }
    let (role, text, final_answer, assistant_progress) = codex_event_visible_message(value)?;
    Some(CodexVisibleMessage {
        role,
        text,
        timestamp,
        order,
        fallback: true,
        final_answer,
        assistant_progress,
    })
}

fn codex_visible_message(value: &serde_json::Value) -> Option<(String, String, bool, bool)> {
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
    let phase = payload.get("phase").and_then(|value| value.as_str());
    let final_answer = phase.is_some_and(is_final_agent_phase);
    let assistant_progress =
        role == "assistant" && phase.is_some_and(|value| !is_final_agent_phase(value));
    Some((role.to_string(), text, final_answer, assistant_progress))
}

fn codex_event_visible_message(value: &serde_json::Value) -> Option<(String, String, bool, bool)> {
    let event_type = value.get("type").and_then(|value| value.as_str())?;
    let payload = if event_type == "event_msg" {
        value.get("payload")?
    } else {
        value
    };
    let payload_type = payload.get("type").and_then(|value| value.as_str())?;
    let role = match payload_type {
        "user_message" => "user",
        "agent_message" => "assistant",
        _ => return None,
    };
    let text = payload
        .get("message")
        .or_else(|| payload.get("text"))
        .and_then(|value| value.as_str())?
        .to_string();
    if role == "user" && is_codex_synthetic_user_text(&text) {
        return None;
    }
    let phase = payload.get("phase").and_then(|value| value.as_str());
    let final_answer = phase.is_some_and(is_final_agent_phase);
    let assistant_progress =
        role == "assistant" && phase.is_some_and(|value| !is_final_agent_phase(value));
    Some((role.to_string(), text, final_answer, assistant_progress))
}

fn dedupe_codex_visible_messages(messages: Vec<CodexVisibleMessage>) -> Vec<CodexVisibleMessage> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            if message.fallback
                && messages.iter().enumerate().any(|(other_index, other)| {
                    other_index != index
                        && !other.fallback
                        && same_codex_visible_message_near(message, other)
                })
            {
                None
            } else {
                Some(message.clone())
            }
        })
        .collect()
}

fn same_codex_visible_message_near(
    left: &CodexVisibleMessage,
    right: &CodexVisibleMessage,
) -> bool {
    if left.role != right.role || left.text.trim() != right.text.trim() {
        return false;
    }
    match (
        codex_timestamp_seconds(&left.timestamp),
        codex_timestamp_seconds(&right.timestamp),
    ) {
        (Some(left), Some(right)) => (left - right).abs() <= 2,
        _ => left.order.abs_diff(right.order) <= 2,
    }
}

fn codex_timestamp_seconds(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp())
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
    result: Option<&serde_json::Value>,
) -> Option<CodexUsageSnapshot> {
    let Some(result) = result else {
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

fn fetch_codex_app_server_dashboard(config: &Config) -> CodexAppServerDashboard {
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
    let Some(output) = codex_app_server_request(config, &input) else {
        return CodexAppServerDashboard::default();
    };
    let responses = output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect::<Vec<_>>();
    let rate_limits = responses
        .iter()
        .find(|value| value.get("id").and_then(|value| value.as_i64()) == Some(1))
        .and_then(|value| value.get("result").cloned());
    CodexAppServerDashboard { rate_limits }
}

fn fetch_codex_model_catalog(config: &Config) -> Vec<CodexModel> {
    let initialize = serde_json::json!({
        "id": 0,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "mobailmux", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
        }
    });
    let list_models = serde_json::json!({
        "id": 1,
        "method": "model/list",
        "params": {"includeHidden": false}
    });
    let input = format!("{initialize}\n{{\"method\":\"initialized\"}}\n{list_models}\n");
    let Some(output) = codex_app_server_request(config, &input) else {
        return Vec::new();
    };
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value.get("id").and_then(|value| value.as_i64()) == Some(1))
        .and_then(|value| value.get("result").cloned())
        .map(|payload| codex_models_from_payload(&payload))
        .unwrap_or_default()
}

fn codex_models_from_payload(payload: &serde_json::Value) -> Vec<CodexModel> {
    payload
        .get("data")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let model = item
                .get("model")
                .or_else(|| item.get("id"))
                .and_then(|value| value.as_str())?
                .trim();
            if model.is_empty() {
                return None;
            }
            let supported_reasoning_efforts = item
                .get("supportedReasoningEfforts")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    let effort = option.get("reasoningEffort")?.as_str()?.trim();
                    if effort.is_empty() {
                        return None;
                    }
                    Some(CodexReasoningEffort {
                        effort: effort.to_string(),
                        description: option
                            .get("description")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                    })
                })
                .collect::<Vec<_>>();
            Some(CodexModel {
                model: model.to_string(),
                display_name: item
                    .get("displayName")
                    .and_then(|value| value.as_str())
                    .unwrap_or(model)
                    .trim()
                    .to_string(),
                description: item
                    .get("description")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                default_reasoning_effort: item
                    .get("defaultReasoningEffort")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                supported_reasoning_efforts,
                is_default: item
                    .get("isDefault")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
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
    let mut command_parts = vec![config.agent_codex_bin.as_str()];
    command_parts.extend(agent_codex_args_for_command(config));
    command_parts.extend(["app-server", "--stdio"]);
    let command = command_parts
        .into_iter()
        .map(shell_single_quote)
        .collect::<Vec<_>>()
        .join(" ");
    let settle_seconds = CODEX_APP_SERVER_WRITE_SETTLE.as_secs_f64();
    let script = format!(
        "{{ printf '%s' {}; sleep {settle_seconds}; }} | {command}",
        shell_single_quote(input)
    );
    let output = StdCommand::new("bash")
        .args(["-lc", &script])
        .output()
        .ok()?;
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
    let credits = value
        .get("credits")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|credit| {
            credit
                .get("status")
                .and_then(|value| value.as_str())
                .is_none_or(|status| status == "available")
        })
        .map(|credit| CodexResetCredit {
            title: credit
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or("Reset credit")
                .to_string(),
            expires_at: credit
                .get("expiresAt")
                .or_else(|| credit.get("expires_at"))
                .and_then(|value| value.as_i64()),
        })
        .collect();
    Some(CodexResetCreditsSummary {
        available_count,
        credits,
        estimate: None,
    })
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

fn prepare_agent_progress(slot_id: i64) -> io::Result<AgentProgress> {
    let dir = env::temp_dir().join(format!(
        "mobailmux-agent-progress-{}-{}",
        slot_id,
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&dir)?;
    let file = dir.join("progress.log");
    fs::File::create(&file)?;
    let helper = dir.join("aiprogress");
    let progress_file = shell_single_quote(&file.to_string_lossy());
    fs::write(
        &helper,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" >> {progress_file}\n"
        ),
    )?;
    let mut permissions = fs::metadata(&helper)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&helper, permissions)?;
    Ok(AgentProgress { dir, file })
}

fn progress_path_env(progress_dir: &Path) -> Option<std::ffi::OsString> {
    let mut paths = vec![progress_dir.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).ok()
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn watch_agent_progress_file(
    state: Arc<AppState>,
    slot_id: i64,
    path: PathBuf,
    mut done_rx: oneshot::Receiver<()>,
) {
    let mut offset = 0u64;
    loop {
        tokio::select! {
            _ = &mut done_rx => {
                let _ = drain_agent_progress_file(&state, slot_id, &path, &mut offset);
                break;
            }
            _ = sleep(StdDuration::from_secs(1)) => {
                let _ = drain_agent_progress_file(&state, slot_id, &path, &mut offset);
            }
        }
    }
}

fn drain_agent_progress_file(
    state: &AppState,
    slot_id: i64,
    path: &Path,
    offset: &mut u64,
) -> io::Result<()> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(*offset))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    *offset = file.stream_position()?;
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            append_agent_assistant(
                state,
                slot_id,
                &format!("note: {}", truncate_text(line, 1200)),
            );
        }
    }
    Ok(())
}

fn start_agent_job(
    state: Arc<AppState>,
    slot_id: i64,
    request_body: String,
    attachment_id: Option<i64>,
    settings: AgentRunSettings,
) {
    state.agent_jobs.lock().unwrap().insert(
        slot_id,
        AgentRun {
            status: "starting".into(),
            current: "starting".into(),
            started_at: Utc::now().to_rfc3339(),
        },
    );
    tokio::spawn(run_agent_job(
        state,
        slot_id,
        request_body,
        attachment_id,
        settings,
    ));
}

async fn run_agent_job(
    state: Arc<AppState>,
    slot_id: i64,
    request_body: String,
    attachment_id: Option<i64>,
    settings: AgentRunSettings,
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
        &format!(
            "{} started in `{}`{}.",
            slot.name,
            slot.workdir,
            agent_run_settings_label(&settings)
        ),
    );
    let attachment = attachment_id.and_then(|id| {
        let db = state.db.lock().unwrap();
        agent_attachment_for_prompt(&db, id).unwrap_or(None)
    });
    let progress = if state.config.agent_progress_notes {
        prepare_agent_progress(slot_id).ok()
    } else {
        None
    };
    let prompt = build_agent_prompt(
        &slot,
        &request_body,
        attachment.as_ref(),
        progress.is_some(),
    );
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
    command.args(agent_codex_args_for_command(&state.config));
    apply_agent_run_settings(&mut command, &settings);
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
    if let Some(progress) = &progress
        && let Some(path) = progress_path_env(&progress.dir)
    {
        command.env("PATH", path);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            append_agent_assistant(
                &state,
                slot_id,
                &format!("Could not start `{}`: {err}", state.config.agent_codex_bin),
            );
            state.agent_jobs.lock().unwrap().remove(&slot_id);
            if let Some(progress) = progress {
                let _ = tokio::fs::remove_dir_all(progress.dir).await;
            }
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
    let (progress_done_tx, progress_task) = if let Some(progress) = &progress {
        let (done_tx, done_rx) = oneshot::channel();
        (
            Some(done_tx),
            Some(tokio::spawn(watch_agent_progress_file(
                state.clone(),
                slot_id,
                progress.file.clone(),
                done_rx,
            ))),
        )
    } else {
        (None, None)
    };
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
    let mut stdout_summary = AgentStdoutSummary::default();
    if let Some(task) = stdout_task {
        stdout_summary = task.await.unwrap_or_default();
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
    if let Some(done_tx) = progress_done_tx {
        let _ = done_tx.send(());
    }
    if let Some(task) = progress_task {
        let _ = task.await;
    }
    if let Some(progress) = progress {
        let _ = tokio::fs::remove_dir_all(progress.dir).await;
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
            let streamed_final = stdout_summary
                .final_text
                .as_deref()
                .or(stdout_summary.last_assistant_text.as_deref())
                .is_some_and(|streamed| streamed.trim() == final_text.trim());
            if final_text.is_empty() {
                if stdout_summary.last_assistant_text.is_none() {
                    append_agent_assistant(
                        &state,
                        slot_id,
                        "(Codex completed without a final message.)",
                    );
                }
            } else if !streamed_final {
                append_agent_assistant(&state, slot_id, &final_text);
            }
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
    start_next_queued_agent_job(state.clone(), slot_id);
}

async fn read_agent_stdout(
    state: Arc<AppState>,
    slot_id: i64,
    workdir: String,
    stdout: tokio::process::ChildStdout,
) -> AgentStdoutSummary {
    let mut lines = BufReader::new(stdout).lines();
    let mut summary = AgentStdoutSummary::default();
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
        if let Some((message, final_answer)) = codex_stdout_agent_message(&event) {
            let message = truncate_text(&message, MAX_AGENT_MESSAGE_CHARS);
            if !message.trim().is_empty()
                && summary.last_assistant_text.as_deref() != Some(message.as_str())
            {
                state
                    .agent_jobs
                    .lock()
                    .unwrap()
                    .entry(slot_id)
                    .and_modify(|run| {
                        run.status = if final_answer {
                            "finishing".into()
                        } else {
                            "responding".into()
                        };
                        run.current.clear();
                    });
                append_agent_assistant(&state, slot_id, &message);
                summary.last_assistant_text = Some(message.clone());
                if final_answer {
                    summary.final_text = Some(message);
                }
            }
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
    summary
}

fn codex_stdout_agent_message(value: &serde_json::Value) -> Option<(String, bool)> {
    let event_type = value.get("type").and_then(|value| value.as_str())?;
    if event_type == "response_item" {
        let payload = value.get("payload")?;
        if payload.get("type").and_then(|value| value.as_str()) != Some("message") {
            return None;
        }
        if payload.get("role").and_then(|value| value.as_str()) != Some("assistant") {
            return None;
        }
        let text = codex_content_text(payload.get("content")?);
        let final_answer = payload
            .get("phase")
            .and_then(|value| value.as_str())
            .is_some_and(is_final_agent_phase);
        return Some((text, final_answer));
    }

    let payload = if event_type == "event_msg" {
        value.get("payload")?
    } else {
        value
    };
    let payload_type = payload
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or(event_type);
    if payload_type != "agent_message" {
        return None;
    }
    let text = payload
        .get("message")
        .or_else(|| payload.get("text"))
        .and_then(|value| value.as_str())?
        .to_string();
    let final_answer = payload
        .get("phase")
        .and_then(|value| value.as_str())
        .is_some_and(is_final_agent_phase);
    Some((text, final_answer))
}

fn is_final_agent_phase(value: &str) -> bool {
    matches!(value, "final" | "final_answer")
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
    progress_notes_enabled: bool,
) -> String {
    let goal = slot.goal.trim();
    let mut sections = Vec::new();
    if !goal.is_empty() {
        sections.push(format!("Current slot goal:\n{goal}"));
    }
    if progress_notes_enabled {
        sections.push(
            "Mobailmux has an optional `aiprogress 'message'` command for short human progress notes. Use it only when useful."
                .to_string(),
        );
    }
    sections.push(request_body.to_string());
    let mut prompt = sections.join("\n\n");
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

fn agent_control_text(text: &str) -> Option<(char, &str)> {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let prefix = chars.next()?;
    if !matches!(prefix, '!' | '/') {
        return None;
    }
    Some((prefix, chars.as_str().trim()))
}

#[cfg(test)]
fn looks_like_agent_control_request(body: &str) -> bool {
    agent_control_text(body).is_some()
}

fn normalize_agent_command_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let (name, rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(name, rest)| (name, Some(rest)))
        .unwrap_or((trimmed, None));
    let lower = name.to_ascii_lowercase();
    if known_agent_command_names()
        .iter()
        .any(|command| *command == lower)
    {
        return trimmed.to_string();
    }
    let matches = known_agent_command_names()
        .into_iter()
        .filter(|command| command.starts_with(&lower))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return trimmed.to_string();
    }
    match rest {
        Some(rest) if !rest.trim_start().is_empty() => {
            format!("{} {}", matches[0], rest.trim_start())
        }
        _ => matches[0].to_string(),
    }
}

fn known_agent_command_names() -> Vec<&'static str> {
    let mut names = AGENT_COMMAND_SPECS
        .iter()
        .map(|command| command.name)
        .collect::<Vec<_>>();
    names.extend(AGENT_COMMAND_ALIASES);
    names
}

fn agent_composer_suggestions_json(config: &Config) -> String {
    let mut suggestions = AGENT_COMMAND_SPECS
        .iter()
        .map(|command| ComposerSuggestion {
            kind: "command",
            name: command.name.to_string(),
            insert: format!("/{}", command.name),
            description: command.description.to_string(),
            takes_arg: command.takes_arg,
        })
        .collect::<Vec<_>>();
    suggestions.extend(discover_codex_skill_suggestions(&config.codex_home));
    suggestions.extend(discover_codex_plugin_suggestions(&config.codex_home));
    let mut seen = HashSet::new();
    suggestions.retain(|suggestion| {
        seen.insert(format!(
            "{}:{}",
            suggestion.kind,
            suggestion.name.to_ascii_lowercase()
        ))
    });
    suggestions.sort_by(|left, right| {
        suggestion_kind_rank(left.kind)
            .cmp(&suggestion_kind_rank(right.kind))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });
    json_for_inline_script(&suggestions)
}

fn json_for_inline_script<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "[]".into())
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn suggestion_kind_rank(kind: &str) -> usize {
    match kind {
        "command" => 0,
        "skill" => 1,
        "plugin" => 2,
        _ => 3,
    }
}

fn discover_codex_skill_suggestions(codex_home: &Path) -> Vec<ComposerSuggestion> {
    let mut files = Vec::new();
    collect_named_files(&codex_home.join("skills"), "SKILL.md", 5, &mut files);
    collect_named_files(&codex_home.join("plugins"), "SKILL.md", 9, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let skill_dir = path.parent()?.file_name()?.to_str()?.to_string();
            let plugin_root = plugin_root_for_path(&path);
            let name = if let Some(root) = plugin_root {
                let plugin = plugin_manifest_field(&root, "name")?;
                format!("{plugin}:{skill_dir}")
            } else {
                skill_dir
            };
            let description =
                skill_description_from_file(&path).unwrap_or_else(|| "Codex skill".into());
            Some(ComposerSuggestion {
                kind: "skill",
                insert: format!("${name}"),
                name,
                description: compact_text(&description, 120),
                takes_arg: false,
            })
        })
        .collect()
}

fn discover_codex_plugin_suggestions(codex_home: &Path) -> Vec<ComposerSuggestion> {
    let mut files = Vec::new();
    collect_named_files(&codex_home.join("plugins"), "plugin.json", 9, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let plugin_dir = path.parent()?.parent()?;
            let name = plugin_manifest_field(plugin_dir, "name")?;
            let description =
                plugin_manifest_description(plugin_dir).unwrap_or_else(|| "Codex plugin".into());
            Some(ComposerSuggestion {
                kind: "plugin",
                insert: format!("#{name}"),
                name,
                description: compact_text(&description, 120),
                takes_arg: false,
            })
        })
        .collect()
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_text(&compact, max_chars).replace('\n', " ")
}

fn collect_named_files(dir: &Path, file_name: &str, depth: usize, files: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, file_name, depth - 1, files);
        } else if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            files.push(path);
        }
    }
}

fn plugin_root_for_path(path: &Path) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.join(".codex-plugin/plugin.json").is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn skill_description_from_file(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .find_map(|line| yaml_string_field(line, "description"))
}

fn plugin_manifest_description(plugin_root: &Path) -> Option<String> {
    let manifest = plugin_manifest_json(plugin_root)?;
    manifest
        .get("interface")
        .and_then(|interface| interface.get("shortDescription"))
        .and_then(|value| value.as_str())
        .or_else(|| manifest.get("description").and_then(|value| value.as_str()))
        .map(str::to_string)
}

fn plugin_manifest_field(plugin_root: &Path, field: &str) -> Option<String> {
    plugin_manifest_json(plugin_root)?
        .get(field)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn plugin_manifest_json(plugin_root: &Path) -> Option<serde_json::Value> {
    let raw = fs::read_to_string(plugin_root.join(".codex-plugin/plugin.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

fn yaml_string_field(line: &str, field: &str) -> Option<String> {
    let value = line.trim().strip_prefix(&format!("{field}:"))?.trim();
    if value.is_empty() {
        return None;
    }
    Some(
        value
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string(),
    )
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
    parts.extend(
        agent_codex_args_for_command(config)
            .into_iter()
            .map(str::to_string),
    );
    parts.join(" ")
}

fn agent_codex_args_for_command(config: &Config) -> Vec<&str> {
    let wrapper_adds_yolo = Path::new(&config.agent_codex_bin)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "codexunsafe");
    config
        .agent_codex_args
        .iter()
        .map(String::as_str)
        .filter(|arg| !(wrapper_adds_yolo && *arg == "--dangerously-bypass-approvals-and-sandbox"))
        .collect()
}

fn agent_execution_mode_html(config: &Config) -> String {
    let wrapper_adds_yolo = Path::new(&config.agent_codex_bin)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "codexunsafe");
    if wrapper_adds_yolo
        || config
            .agent_codex_args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
    {
        r#"<span class="agent-execution-mode yolo" data-yolo-mode title="YOLO mode: Codex bypasses approvals and sandboxing for this service" aria-label="YOLO mode: approvals and sandbox bypassed"><span aria-hidden="true">🔓</span><span>YOLO</span></span>"#.into()
    } else {
        r#"<span class="agent-execution-mode" title="Codex is using its configured approvals and sandbox policy" aria-label="Configured approvals and sandbox policy"><span aria-hidden="true">🔒</span><span>Guarded</span></span>"#.into()
    }
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

fn format_epoch_date(epoch: i64) -> String {
    let Some(value) = epoch_to_rfc3339(epoch) else {
        return "unknown".into();
    };
    let exact = DateTime::parse_from_rfc3339(&value)
        .map(|date| {
            date.with_timezone(&Utc)
                .format("%Y-%m-%d %H:%M UTC")
                .to_string()
        })
        .unwrap_or_else(|_| value.clone());
    format!("{} ({exact})", short_time(&value))
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

fn compact_local_time(value: &str) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(value) else {
        return if value.trim().is_empty() {
            "unknown".into()
        } else {
            value.to_string()
        };
    };
    let dt = parsed.with_timezone(&Local);
    let now = Local::now();
    if dt.date_naive() == now.date_naive() {
        return dt.format("%H:%M").to_string();
    }
    if dt.year() == now.year() {
        return dt.format("%d-%m %H:%M").to_string();
    }
    dt.format("%Y-%m-%d %H:%M").to_string()
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
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, viewport-fit=cover">
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

fn html_attr_escape(value: &str) -> String {
    html_escape(value).replace('\r', "").replace('\n', "&#10;")
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
            agent_codex_bin: "codex".into(),
            agent_codex_args: Vec::new(),
            agent_progress_notes: false,
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
    fn slash_command_prefixes_autocorrect_when_unambiguous() {
        assert_eq!(normalize_agent_command_text("go ship it"), "goal ship it");
        assert_eq!(normalize_agent_command_text("mod"), "model");
        assert_eq!(normalize_agent_command_text("sta"), "status");
    }

    #[test]
    fn codex_model_catalog_keeps_supported_thinking_levels() {
        let payload = serde_json::json!({
            "data": [{
                "model": "gpt-test",
                "displayName": "GPT Test",
                "description": "Test model",
                "isDefault": true,
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "low", "description": "Fast"},
                    {"reasoningEffort": "medium", "description": "Balanced"},
                    {"reasoningEffort": "high", "description": "Deep"}
                ]
            }]
        });

        let models = codex_models_from_payload(&payload);

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "gpt-test");
        assert_eq!(models[0].default_reasoning_effort, "medium");
        assert_eq!(
            models[0]
                .supported_reasoning_efforts
                .iter()
                .map(|effort| effort.effort.as_str())
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high"]
        );
    }

    #[test]
    fn agent_run_settings_only_accept_catalog_options() {
        let models = vec![CodexModel {
            model: "gpt-test".into(),
            display_name: "GPT Test".into(),
            description: String::new(),
            default_reasoning_effort: "medium".into(),
            supported_reasoning_efforts: vec![
                CodexReasoningEffort {
                    effort: "low".into(),
                    description: String::new(),
                },
                CodexReasoningEffort {
                    effort: "high".into(),
                    description: String::new(),
                },
            ],
            is_default: true,
        }];

        let settings = validate_agent_run_settings(&models, "gpt-test", "high");
        assert_eq!(settings.model.as_deref(), Some("gpt-test"));
        assert_eq!(settings.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            validate_agent_run_settings(&models, "gpt-test", "ultra").reasoning_effort,
            None
        );
        assert_eq!(
            validate_agent_run_settings(&models, "other", "high"),
            AgentRunSettings::default()
        );

        let mut command = TokioCommand::new("codex");
        apply_agent_run_settings(&mut command, &settings);
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                "--model",
                "gpt-test",
                "--config",
                "model_reasoning_effort=\"high\""
            ]
        );
    }

    #[test]
    fn codexunsafe_wrapper_drops_duplicate_yolo_flag() {
        let config = Config {
            bind: "127.0.0.1:0".into(),
            db_path: PathBuf::new(),
            agent_default_workdir: PathBuf::new(),
            agent_codex_bin: "/usr/local/bin/codexunsafe".into(),
            agent_codex_args: vec![
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "--color".into(),
                "never".into(),
            ],
            agent_progress_notes: false,
            codex_home: PathBuf::new(),
            codex_reset_command: None,
            agent_slots: Vec::new(),
            user: "mobailmux".into(),
            password_hash: None,
            cookie_secret: vec![2u8; 32],
            auth_disabled: true,
        };

        assert_eq!(
            agent_codex_args_for_command(&config),
            vec!["--color", "never"]
        );
        assert!(agent_execution_mode_html(&config).contains("data-yolo-mode"));
    }

    #[test]
    fn inline_script_json_escapes_html_terminators() {
        let json = json_for_inline_script(&serde_json::json!({"name": "</script>&"}));
        assert!(!json.contains("</script>"));
        assert!(json.contains("\\u003c/script\\u003e\\u0026"));
    }

    #[test]
    fn agent_prompt_uses_plain_request_without_slot_context_by_default() {
        let slot = AgentSlotRow {
            id: 1,
            name: "codex".into(),
            workdir: "/work/app".into(),
            goal: String::new(),
        };

        let prompt = build_agent_prompt(&slot, "fix the bug", None, false);

        assert_eq!(prompt, "fix the bug");
        assert!(!prompt.contains("Mobailmux"));
        assert!(!prompt.contains("User request:"));
    }

    #[test]
    fn agent_prompt_includes_slot_goal_and_optional_progress_notes() {
        let slot = AgentSlotRow {
            id: 1,
            name: "codex".into(),
            workdir: "/work/app".into(),
            goal: "Keep the app deployable.".into(),
        };

        let prompt = build_agent_prompt(&slot, "fix the bug", None, false);

        assert!(prompt.contains("Current slot goal:\nKeep the app deployable."));
        assert!(prompt.ends_with("fix the bug"));
        assert!(!prompt.contains("User request:"));
        let prompt_with_progress = build_agent_prompt(&slot, "fix the bug", None, true);
        assert!(prompt_with_progress.contains("aiprogress 'message'"));
    }

    #[test]
    fn agent_messages_group_command_activity() {
        let messages = vec![
            test_message("assistant", "Done with the requested change."),
            test_message("assistant", "running: `/bin/bash -lc 'cargo test'`"),
            test_message("assistant", "running: `/bin/bash -lc 'cargo fmt'`"),
            test_message("assistant", "codex started in `/work/app`."),
            test_message("user", "please fix this"),
        ];

        let html = agent_messages_html(&messages);

        assert_eq!(html.matches("message-activity").count(), 1);
        assert_eq!(html.matches("tool-fold").count(), 1);
        assert!(html.contains(r#"data-fold-key="activity-1""#));
        assert!(html.contains("3 events"));
        assert_eq!(html.matches("tool-row-run").count(), 2);
        assert!(html.contains("message-user"));
        assert!(html.contains("message-assistant"));
        assert!(
            html.find("message-user").unwrap() < html.find("message-activity").unwrap()
                && html.find("message-activity").unwrap() < html.find("Done with").unwrap()
        );
    }

    #[test]
    fn agent_messages_keep_progress_notes_outside_activity_folds() {
        let messages = vec![
            test_message("assistant", "Done."),
            test_message("assistant", "running: `/bin/bash -lc 'cargo test'`"),
            test_message("assistant", "note: finished investigation"),
            test_message("assistant", "running: `/bin/bash -lc 'rg bug'`"),
            test_message("assistant", "codex started in `/work/app`."),
            test_message("user", "please fix this"),
        ];

        let html = agent_messages_html(&messages);

        assert_eq!(html.matches("message-activity").count(), 2);
        assert_eq!(html.matches("tool-fold").count(), 2);
        assert!(html.contains("note: finished investigation"));
        let first_activity = html.find("message-activity").unwrap();
        let note = html.find("note: finished investigation").unwrap();
        let second_activity = html.rfind("message-activity").unwrap();
        assert!(first_activity < note && note < second_activity);
    }

    #[test]
    fn agent_messages_render_markdown_code_blocks_with_copy() {
        let html = agent_messages_html(&[test_message(
            "assistant",
            "Run this:\n\n```bash\ncargo test\n```",
        )]);

        assert!(html.contains(r#"<div class="message-content">"#));
        assert!(html.contains(r#"<div class="message-code">"#));
        assert!(html.contains(r#"data-copy-code"#));
        assert!(html.contains(r#"class="language-bash""#));
        assert!(html.contains("cargo test"));
        assert!(!html.contains("```bash"));
    }

    #[test]
    fn agent_messages_accept_single_quote_code_fences() {
        let html = message_body_html("Run this:\n\n'''bash\ncargo test\n'''\n");

        assert!(html.contains(r#"<div class="message-code">"#));
        assert!(html.contains("cargo test"));
        assert!(!html.contains("'''bash"));
    }

    #[test]
    fn agent_markdown_escapes_raw_html() {
        let html = message_body_html("<script>alert(1)</script>\n\n`safe`");

        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("<code>safe</code>"));
    }

    #[test]
    fn prefixed_messages_are_control_requests() {
        assert!(looks_like_agent_control_request("/status"));
        assert!(looks_like_agent_control_request("!stop"));
        assert!(looks_like_agent_control_request("/unknown"));
        assert!(!looks_like_agent_control_request("fix the app"));
    }

    #[test]
    fn composer_suggestions_include_skills_and_plugins() {
        let dir = env::temp_dir().join(format!("mobailmux-test-{}", Uuid::new_v4().simple()));
        let skill_dir = dir.join("skills/repo-starter");
        let plugin_dir = dir.join("plugins/cache/openai-curated/github/hash");
        let plugin_skill_dir = plugin_dir.join("skills/yeet");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::create_dir_all(&plugin_skill_dir).unwrap();
        fs::create_dir_all(plugin_dir.join(".codex-plugin")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: repo-starter\ndescription: Start repos safely.\n---\n",
        )
        .unwrap();
        fs::write(
            plugin_dir.join(".codex-plugin/plugin.json"),
            r#"{"name":"github","description":"GitHub workflows"}"#,
        )
        .unwrap();
        fs::write(
            plugin_skill_dir.join("SKILL.md"),
            "---\nname: yeet\ndescription: Publish changes.\n---\n",
        )
        .unwrap();

        let skills = discover_codex_skill_suggestions(&dir);
        let plugins = discover_codex_plugin_suggestions(&dir);

        assert!(skills.iter().any(|item| item.insert == "$repo-starter"));
        assert!(skills.iter().any(|item| item.insert == "$github:yeet"));
        assert!(plugins.iter().any(|item| item.insert == "#github"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn edited_user_message_prunes_later_chat_and_session() {
        let db = Connection::open_in_memory().unwrap();
        db_migrations::migrate(&db).unwrap();
        let slot_id = ensure_agent_slot(&db, "codex", Path::new("/tmp")).unwrap();
        let message_id = append_agent_message(&db, slot_id, "user", "old prompt", None).unwrap();
        append_agent_message(&db, slot_id, "assistant", "old answer", None).unwrap();
        set_agent_session(&db, slot_id, "thread-old", "/tmp").unwrap();

        update_agent_user_message(&db, slot_id, message_id, "new prompt", None).unwrap();
        delete_agent_messages_after(&db, slot_id, message_id).unwrap();
        db.execute(
            "DELETE FROM agent_sessions WHERE slot_id = ?1",
            params![slot_id],
        )
        .unwrap();

        let body: String = db
            .query_row(
                "SELECT body FROM agent_messages WHERE id = ?1",
                params![message_id],
                |row| row.get(0),
            )
            .unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM agent_messages WHERE slot_id = ?1",
                params![slot_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(body, "new prompt");
        assert_eq!(count, 1);
        assert!(agent_session(&db, slot_id).unwrap().is_none());
    }

    fn test_message(role: &str, body: &str) -> AgentMessageRow {
        AgentMessageRow {
            id: 1,
            role: role.into(),
            body: body.into(),
            created_at: "2026-06-29T12:00:00Z".into(),
            attachment: None,
        }
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
    fn codex_conversation_parser_reads_event_messages_without_duplicates() {
        let dir = env::temp_dir().join(format!("mobailmux-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-2026-06-23T09-24-22-thread-3.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-06-23T07:24:22Z","type":"session_meta","payload":{"id":"thread-3","cwd":"/work/app","timestamp":"2026-06-23T07:24:22Z"}}
{"timestamp":"2026-06-23T07:24:23.000Z","type":"event_msg","payload":{"type":"user_message","message":"fix the web chat"}}
{"timestamp":"2026-06-23T07:24:23.001Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the web chat"}]}}
{"timestamp":"2026-06-23T07:24:24Z","type":"event_msg","payload":{"type":"agent_message","message":"I am checking the UI now.","phase":"commentary"}}
{"timestamp":"2026-06-23T07:24:25Z","type":"event_msg","payload":{"type":"agent_message","message":"Done.","phase":"final_answer"}}
"#,
        )
        .unwrap();
        let conversation = codex_conversation_from_file(&path, &HashMap::new()).unwrap();
        assert_eq!(conversation.title, "fix the web chat");
        assert_eq!(conversation.preview, "fix the web chat");
        assert_eq!(conversation.message_count, 3);

        let messages = codex_transcript_messages(&path).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].body, "Done.");
        assert_eq!(messages[1].body, "I am checking the UI now.");
        assert_eq!(messages[2].body, "fix the web chat");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_conversation_parser_marks_interrupted_transcripts() {
        let dir = env::temp_dir().join(format!("mobailmux-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-2026-06-23T09-24-22-thread-4.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-06-23T07:24:22Z","type":"session_meta","payload":{"id":"thread-4","cwd":"/work/app","timestamp":"2026-06-23T07:24:22Z"}}
{"timestamp":"2026-06-23T07:24:23.000Z","type":"event_msg","payload":{"type":"user_message","message":"fix the web chat"}}
{"timestamp":"2026-06-23T07:24:24Z","type":"event_msg","payload":{"type":"agent_message","message":"I am checking the UI now.","phase":"commentary"}}
"#,
        )
        .unwrap();

        let messages = codex_transcript_messages(&path).unwrap();
        assert_eq!(messages.len(), 3);
        assert!(
            messages[0]
                .body
                .contains("ended before Codex returned a final answer")
        );
        assert_eq!(messages[1].body, "I am checking the UI now.");
        assert_eq!(messages[2].body, "fix the web chat");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_stdout_agent_message_reads_commentary_and_final_text() {
        let commentary = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": "checking layout",
                "phase": "commentary"
            }
        });
        assert_eq!(
            codex_stdout_agent_message(&commentary),
            Some(("checking layout".into(), false))
        );

        let final_answer = serde_json::json!({
            "type": "agent_message",
            "message": "fixed",
            "phase": "final_answer"
        });
        assert_eq!(
            codex_stdout_agent_message(&final_answer),
            Some(("fixed".into(), true))
        );
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

        let reset_credits = serde_json::json!({
            "availableCount": 1,
            "credits": [{
                "status": "available",
                "title": "Full reset (Weekly + 5 hr)",
                "expiresAt": 1785527935
            }]
        });
        let summary = codex_reset_credits_summary(Some(&reset_credits)).unwrap();
        assert_eq!(summary.available_count, 1);
        assert_eq!(summary.credits.len(), 1);
        assert_eq!(summary.credits[0].expires_at, Some(1785527935));
    }

    #[test]
    fn startup_marks_interrupted_agent_activity_once() {
        let dir = env::temp_dir().join(format!("mobailmux-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let db = Connection::open_in_memory().unwrap();
        db_migrations::migrate(&db).unwrap();
        let slot_id = ensure_agent_slot(&db, "codex-2", &dir).unwrap();
        append_agent_message(&db, slot_id, "user", "fix this", None).unwrap();
        append_agent_message(&db, slot_id, "assistant", "running: `cargo test`", None).unwrap();
        let state = AppState {
            db: Mutex::new(db),
            config: Config {
                bind: "127.0.0.1:0".into(),
                db_path: PathBuf::new(),
                agent_default_workdir: dir.clone(),
                agent_codex_bin: "codex".into(),
                agent_codex_args: Vec::new(),
                agent_progress_notes: false,
                codex_home: PathBuf::new(),
                codex_reset_command: None,
                agent_slots: Vec::new(),
                user: "mobailmux".into(),
                password_hash: None,
                cookie_secret: vec![2u8; 32],
                auth_disabled: true,
            },
            agent_jobs: Mutex::new(HashMap::new()),
            agent_cancels: Mutex::new(HashMap::new()),
            agent_queues: Mutex::new(HashMap::new()),
            codex_index: Mutex::new(CodexIndexCache::default()),
            codex_models: Mutex::new(CodexModelCatalogCache::default()),
        };

        mark_interrupted_agent_runs(&state);
        mark_interrupted_agent_runs(&state);

        let db = state.db.lock().unwrap();
        let messages = list_agent_messages(&db, slot_id).unwrap();
        assert_eq!(messages.len(), 3);
        assert!(
            messages[0]
                .body
                .contains("Mobailmux restarted while `codex-2` was running")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reset_agent_slot_chat_clears_local_chat() {
        let old_dir = env::temp_dir().join(format!("mobailmux-old-{}", Uuid::new_v4().simple()));
        let new_dir = env::temp_dir().join(format!("mobailmux-new-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&old_dir).unwrap();
        fs::create_dir_all(&new_dir).unwrap();
        let db = Connection::open_in_memory().unwrap();
        db_migrations::migrate(&db).unwrap();
        let slot_id = ensure_agent_slot(&db, "codex", &old_dir).unwrap();
        append_agent_message(&db, slot_id, "user", "hello", None).unwrap();
        set_agent_session(
            &db,
            slot_id,
            "thread-old",
            old_dir.to_string_lossy().as_ref(),
        )
        .unwrap();
        let state = AppState {
            db: Mutex::new(db),
            config: Config {
                bind: "127.0.0.1:0".into(),
                db_path: PathBuf::new(),
                agent_default_workdir: old_dir.clone(),
                agent_codex_bin: "codex".into(),
                agent_codex_args: Vec::new(),
                agent_progress_notes: false,
                codex_home: PathBuf::new(),
                codex_reset_command: None,
                agent_slots: Vec::new(),
                user: "mobailmux".into(),
                password_hash: None,
                cookie_secret: vec![2u8; 32],
                auth_disabled: true,
            },
            agent_jobs: Mutex::new(HashMap::new()),
            agent_cancels: Mutex::new(HashMap::new()),
            agent_queues: Mutex::new(HashMap::new()),
            codex_index: Mutex::new(CodexIndexCache::default()),
            codex_models: Mutex::new(CodexModelCatalogCache::default()),
        };
        assert_eq!(
            queue_agent_request(
                &state,
                slot_id,
                QueuedAgentRequest {
                    body: "next".into(),
                    attachment_id: None,
                    settings: AgentRunSettings::default(),
                },
            ),
            1
        );

        assert!(!reset_agent_slot_chat(&state, slot_id, &new_dir));

        let db = state.db.lock().unwrap();
        let slot = get_agent_slot(&db, slot_id).unwrap().unwrap();
        assert_eq!(slot.workdir, new_dir.to_string_lossy());
        assert!(agent_session(&db, slot_id).unwrap().is_none());
        assert!(list_agent_messages(&db, slot_id).unwrap().is_empty());
        drop(db);
        assert_eq!(agent_queue_len(&state, slot_id), 0);
        fs::remove_dir_all(old_dir).unwrap();
        fs::remove_dir_all(new_dir).unwrap();
    }
}

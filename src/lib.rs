use argon2::{
    Argon2,
    password_hash::{PasswordHash, SaltString},
};
use axum::{
    Json,
    extract::{Form, Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, Redirect, Response},
};
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Duration, Local, Utc};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};
use rusqlite::{Connection, params};
use sha2::Sha256;
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, SeekFrom},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::{Arc, Mutex},
    time::{Duration as StdDuration, Instant, SystemTime},
};
use tokio::{io::BufReader, process::Command as TokioCommand, sync::oneshot, time::sleep};
use uuid::Uuid;

mod app;
mod features;
mod integrations;
mod interfaces;
mod persistence;
mod security;
mod shared;

use app::{AgentSlotSeed, AppState, Config, serve};
#[cfg(test)]
use features::agents::{
    build_agent_prompt, codex_stdout_agent_message, discover_codex_plugin_suggestions,
    discover_codex_skill_suggestions, looks_like_agent_control_request, message_body_html,
    validate_agent_run_settings,
};
use features::{
    agents::{
        AgentCommandSpec, AgentMessageRow, AgentProgress, AgentRun, AgentRunSettings, AgentSlotRow,
        AgentSlotSummary, AgentStdoutSummary, ComposerSuggestion, SlotRuntime, agent_activity_kind,
        agent_composer_suggestions_json, agent_control_text, agent_location, agent_messages_html,
        agent_run_for, agent_run_settings_label, agent_slot_rail_html, agent_slot_runtime,
        agent_slot_summary, apply_agent_run_settings, codex_transcript_count,
        codex_transcript_html, command_arg, handle_agent_control, is_final_agent_phase,
        json_for_inline_script, normalize_agent_command_text, requested_agent_run_settings,
        shell_single_quote, start_agent_job, stop_agent_job,
    },
    usage::{codex_reset_post, codex_usage_dialog, codex_usage_text},
};
use integrations::codex::{
    CodexAppServerDashboard, CodexConversation, CodexIndex, CodexIndexCache, CodexModel,
    CodexModelCatalogCache, CodexRateWindow, CodexReasoningEffort, CodexResetCredit,
    CodexResetCreditsSummary, CodexUsageSnapshot, CodexVisibleMessage, codex_content_text,
    codex_conversation_by_id, codex_index_snapshot, codex_model_catalog_snapshot,
    codex_transcript_messages, codex_usage_from_payload, consume_codex_rate_limit_reset_credit,
    fetch_codex_app_server_dashboard, fetch_codex_model_catalog, load_codex_index,
    merge_codex_rate_limit_status, open_db, refresh_codex_index, refresh_codex_index_blocking,
    refresh_codex_model_catalog,
};
#[cfg(test)]
use integrations::codex::{
    codex_conversation_from_file, codex_models_from_payload, codex_rate_window,
    codex_reset_credits_summary,
};
use interfaces::web::{
    agent_message_create, agent_model_catalog, agent_project_create, agent_slot_state,
    agent_slots_state, agents_page,
};
#[cfg(test)]
use persistence::ensure_agent_slot;
use persistence::{
    agent_session, agent_user_message_exists, append_agent_assistant, append_agent_message,
    create_agent_slot, create_parallel_agent_slot, delete_agent_messages_after,
    delete_agent_session, ensure_agent_slot_seeds, get_agent_slot, list_agent_messages,
    list_agent_slots, mark_interrupted_agent_runs, reset_agent_slot_chat, set_agent_goal,
    set_agent_session, set_agent_workdir, update_agent_user_message,
};
#[cfg(test)]
use security::{
    audit_path, contains_tailscale_ipv4, password_digest, suspicious_secret_assignment,
    verify_password,
};
use security::{
    audit_public_cmd, hash_password_cmd, login_page, login_post, logout_post, page_guard,
    random_secret, raw_guard,
};
use shared::{
    agent_codex_args_for_command, agent_command_label, agent_execution_mode_html,
    compact_local_time, default_codex_bin, default_home_dir, env_flag, epoch_to_rfc3339,
    expand_local_path, file_modified, format_epoch_date, format_number, html_attr_escape,
    html_escape, io_other, normalize_agent_slot_name, page, parse_agent_slot_seeds, short_time,
    split_env_args, system_time_to_rfc3339, truncate_text,
};

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
const PAGE_CSS: &str = include_str!("interfaces/web/page.css");
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

pub async fn run() -> io::Result<()> {
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

#[cfg(test)]
include!("tests.rs");

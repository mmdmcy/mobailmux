use crate::PathBuf;
use crate::default_codex_bin;
use crate::default_home_dir;
use crate::env;
use crate::env_flag;
use crate::expand_local_path;
use crate::io;
use crate::parse_agent_slot_seeds;
use crate::random_secret;
use crate::split_env_args;

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) bind: String,
    pub(crate) db_path: PathBuf,
    pub(crate) agent_default_workdir: PathBuf,
    pub(crate) agent_codex_bin: String,
    pub(crate) agent_codex_args: Vec<String>,
    pub(crate) agent_progress_notes: bool,
    pub(crate) codex_home: PathBuf,
    pub(crate) codex_reset_command: Option<Vec<String>>,
    pub(crate) agent_slots: Vec<AgentSlotSeed>,
    pub(crate) user: String,
    pub(crate) password_hash: Option<String>,
    pub(crate) cookie_secret: Vec<u8>,
    pub(crate) auth_disabled: bool,
}

#[derive(Clone)]
pub(crate) struct AgentSlotSeed {
    pub(crate) name: String,
    pub(crate) workdir: PathBuf,
}

impl Config {
    pub(crate) fn from_env() -> io::Result<Self> {
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

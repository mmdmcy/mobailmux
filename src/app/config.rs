use crate::AgentHarness;
use crate::PathBuf;
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
    pub(crate) default_harness: AgentHarness,
    pub(crate) pi_bin: String,
    pub(crate) pi_args: Vec<String>,
    pub(crate) opencode_bin: String,
    pub(crate) opencode_args: Vec<String>,
    pub(crate) agent_progress_notes: bool,
    #[cfg(test)]
    pub(crate) legacy_codex_bin: String,
    #[cfg(test)]
    pub(crate) legacy_codex_args: Vec<String>,
    #[cfg(test)]
    pub(crate) legacy_codex_home: PathBuf,
    #[cfg(test)]
    pub(crate) legacy_codex_reset_command: Option<Vec<String>>,
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
        let default_harness = env::var("MOBAILMUX_DEFAULT_HARNESS")
            .ok()
            .and_then(|value| AgentHarness::parse(&value))
            .filter(|harness| harness.is_runnable())
            .unwrap_or_default();
        let pi_bin = env::var("MOBAILMUX_PI_BIN").unwrap_or_else(|_| "pi".into());
        let pi_args = env::var("MOBAILMUX_PI_ARGS")
            .ok()
            .map(|value| split_env_args(&value))
            .unwrap_or_else(|| vec!["--approve".into()]);
        let opencode_bin = env::var("MOBAILMUX_OPENCODE_BIN").unwrap_or_else(|_| "opencode".into());
        let opencode_args = env::var("MOBAILMUX_OPENCODE_ARGS")
            .ok()
            .map(|value| split_env_args(&value))
            .unwrap_or_else(|| vec!["--auto".into()]);
        let agent_progress_notes = env_flag("MOBAILMUX_AGENT_PROGRESS_NOTES", false);
        #[cfg(test)]
        let legacy_codex_bin =
            env::var("MOBAILMUX_LEGACY_CODEX_BIN").unwrap_or_else(|_| "codex".into());
        #[cfg(test)]
        let legacy_codex_args = env::var("MOBAILMUX_LEGACY_CODEX_ARGS")
            .ok()
            .map(|value| split_env_args(&value))
            .unwrap_or_default();
        #[cfg(test)]
        let legacy_codex_home = env::var("MOBAILMUX_LEGACY_CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_home_dir().join(".codex"));
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
            default_harness,
            pi_bin,
            pi_args,
            opencode_bin,
            opencode_args,
            agent_progress_notes,
            #[cfg(test)]
            legacy_codex_bin,
            #[cfg(test)]
            legacy_codex_args,
            #[cfg(test)]
            legacy_codex_home,
            #[cfg(test)]
            legacy_codex_reset_command: None,
            agent_slots,
            user,
            password_hash,
            cookie_secret,
            auth_disabled,
        })
    }
}

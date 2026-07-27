use crate::Config;
use crate::AgentHarness;
use crate::PathBuf;
use crate::audit_path;
use crate::contains_tailscale_ipv4;
use crate::password_digest;
use crate::suspicious_secret_assignment;
use crate::verify_password;


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
            default_harness: AgentHarness::Pi,
            pi_bin: "pi".into(),
            pi_args: Vec::new(),
            opencode_bin: "opencode".into(),
            opencode_args: Vec::new(),
            agent_progress_notes: false,
            legacy_codex_bin: "codex".into(),
            legacy_codex_args: Vec::new(),
            legacy_codex_home: PathBuf::new(),
            legacy_codex_reset_command: None,
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

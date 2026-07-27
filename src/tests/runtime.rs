use crate::AppState;
use crate::AgentHarness;
use crate::CodexIndexCache;
use crate::CodexModelCatalogCache;
use crate::Config;
use crate::Connection;
use crate::HashMap;
use crate::Mutex;
use crate::PathBuf;
use crate::Uuid;
use crate::agent_session;
use crate::append_agent_message;
use crate::create_parallel_agent_slot;
use crate::ensure_agent_slot;
use crate::env;
use crate::fs;
use crate::get_agent_slot;
use crate::list_agent_messages;
use crate::mark_interrupted_agent_runs;
use crate::reset_agent_slot_chat;
use crate::set_agent_goal;
use crate::set_agent_session;

use crate::persistence;

    #[test]
    fn startup_marks_interrupted_agent_activity_once() {
        let dir = env::temp_dir().join(format!("mobailmux-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let db = Connection::open_in_memory().unwrap();
        persistence::migrations::migrate(&db).unwrap();
        let slot_id = ensure_agent_slot(&db, "codex-2", &dir).unwrap();
        append_agent_message(&db, slot_id, "user", "fix this").unwrap();
        append_agent_message(&db, slot_id, "assistant", "running: `cargo test`").unwrap();
        let state = AppState {
            db: Mutex::new(db),
            config: Config {
                bind: "127.0.0.1:0".into(),
                db_path: PathBuf::new(),
                agent_default_workdir: dir.clone(),
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
                password_hash: None,
                cookie_secret: vec![2u8; 32],
                auth_disabled: true,
            },
            agent_jobs: Mutex::new(HashMap::new()),
            agent_cancels: Mutex::new(HashMap::new()),
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
        persistence::migrations::migrate(&db).unwrap();
        let slot_id = ensure_agent_slot(&db, "codex", &old_dir).unwrap();
        append_agent_message(&db, slot_id, "user", "hello").unwrap();
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
                password_hash: None,
                cookie_secret: vec![2u8; 32],
                auth_disabled: true,
            },
            agent_jobs: Mutex::new(HashMap::new()),
            agent_cancels: Mutex::new(HashMap::new()),
            codex_index: Mutex::new(CodexIndexCache::default()),
            codex_models: Mutex::new(CodexModelCatalogCache::default()),
        };
        assert!(!reset_agent_slot_chat(&state, slot_id, &new_dir));

        let db = state.db.lock().unwrap();
        let slot = get_agent_slot(&db, slot_id).unwrap().unwrap();
        assert_eq!(slot.workdir, new_dir.to_string_lossy());
        assert!(agent_session(&db, slot_id).unwrap().is_none());
        assert!(list_agent_messages(&db, slot_id).unwrap().is_empty());
        drop(db);
        fs::remove_dir_all(old_dir).unwrap();
        fs::remove_dir_all(new_dir).unwrap();
    }

    #[test]
    fn parallel_agent_slots_get_unique_names_and_copy_context() {
        let dir = env::temp_dir().join(format!("mobailmux-lanes-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let db = Connection::open_in_memory().unwrap();
        persistence::migrations::migrate(&db).unwrap();
        let slot_id = ensure_agent_slot(&db, "project", &dir).unwrap();
        set_agent_goal(&db, slot_id, "ship the project").unwrap();
        let source = get_agent_slot(&db, slot_id).unwrap().unwrap();

        let second = create_parallel_agent_slot(&db, &source).unwrap();
        let third = create_parallel_agent_slot(&db, &source).unwrap();

        assert_eq!(second.name, "project-2");
        assert_eq!(third.name, "project-3");
        assert_eq!(second.workdir, source.workdir);
        assert_eq!(second.goal, source.goal);
        assert_eq!(third.workdir, source.workdir);
        assert_eq!(third.goal, source.goal);
        fs::remove_dir_all(dir).unwrap();
    }

use crate::AgentMessageRow;
use crate::AgentHarness;
use crate::AgentRunSettings;
use crate::AgentSlotRow;
use crate::CodexModel;
use crate::CodexReasoningEffort;
use crate::Config;
use crate::Connection;
use crate::Path;
use crate::PathBuf;
use crate::TokioCommand;
use crate::Uuid;
use crate::agent_command_label;
use crate::agent_execution_mode_html;
use crate::agent_messages_html;
use crate::agent_session;
use crate::append_agent_message;
use crate::apply_agent_run_settings;
use crate::build_agent_prompt;
use crate::codex_models_from_payload;
use crate::delete_agent_messages_after;
use crate::discover_codex_plugin_suggestions;
use crate::discover_codex_skill_suggestions;
use crate::ensure_agent_slot;
use crate::env;
use crate::fs;
use crate::harness_session_id;
use crate::harness_stdout_agent_message;
use crate::json_for_inline_script;
use crate::looks_like_agent_control_request;
use crate::message_body_html;
use crate::normalize_agent_command_text;
use crate::params;
use crate::set_agent_session;
use crate::update_agent_user_message;
use crate::validate_agent_run_settings;

use crate::persistence;

    #[test]
    fn slash_command_prefixes_autocorrect_when_unambiguous() {
        assert_eq!(normalize_agent_command_text("go ship it"), "goal ship it");
        assert_eq!(normalize_agent_command_text("mod"), "model");
        assert_eq!(normalize_agent_command_text("sta"), "status");
    }

    #[test]
    fn harness_json_parsers_keep_sessions_and_final_text() {
        let pi_session = serde_json::json!({
            "type": "session",
            "id": "pi-session"
        });
        let pi_message = serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Pi done"}]
            }
        });
        assert_eq!(
            harness_session_id(AgentHarness::Pi, &pi_session),
            Some("pi-session")
        );
        assert_eq!(
            harness_stdout_agent_message(AgentHarness::Pi, &pi_message),
            Some(("Pi done".into(), true))
        );

        let opencode_message = serde_json::json!({
            "type": "text",
            "sessionID": "oc-session",
            "part": {
                "text": "OpenCode done",
                "metadata": {"openai": {"phase": "final_answer"}}
            }
        });
        assert_eq!(
            harness_session_id(AgentHarness::OpenCode, &opencode_message),
            Some("oc-session")
        );
        assert_eq!(
            harness_stdout_agent_message(AgentHarness::OpenCode, &opencode_message),
            Some(("OpenCode done".into(), true))
        );
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

        let mut command = TokioCommand::new("pi");
        apply_agent_run_settings(&mut command, &settings, AgentHarness::Pi);
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
                "--thinking",
                "high"
            ]
        );
    }

    #[test]
    fn harness_command_labels_are_explicit() {
        let config = Config {
            bind: "127.0.0.1:0".into(),
            db_path: PathBuf::new(),
            agent_default_workdir: PathBuf::new(),
            default_harness: AgentHarness::Pi,
            pi_bin: "/usr/local/bin/pi".into(),
            pi_args: vec!["--approve".into()],
            opencode_bin: "/usr/local/bin/opencode".into(),
            opencode_args: vec!["--auto".into()],
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
        };

        assert_eq!(
            agent_command_label(&config, AgentHarness::Pi),
            "/usr/local/bin/pi --approve"
        );
        assert!(
            agent_execution_mode_html(&config, AgentHarness::OpenCode).contains("OpenCode")
        );
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
            harness: AgentHarness::Pi,
        };

        let prompt = build_agent_prompt(&slot, "fix the bug", false);

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
            harness: AgentHarness::Pi,
        };

        let prompt = build_agent_prompt(&slot, "fix the bug", false);

        assert!(prompt.contains("Current slot goal:\nKeep the app deployable."));
        assert!(prompt.ends_with("fix the bug"));
        assert!(!prompt.contains("User request:"));
        let prompt_with_progress = build_agent_prompt(&slot, "fix the bug", true);
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
        persistence::migrations::migrate(&db).unwrap();
        let slot_id = ensure_agent_slot(&db, "codex", Path::new("/tmp")).unwrap();
        let message_id = append_agent_message(&db, slot_id, "user", "old prompt").unwrap();
        append_agent_message(&db, slot_id, "assistant", "old answer").unwrap();
        set_agent_session(&db, slot_id, "thread-old", "/tmp").unwrap();

        update_agent_user_message(&db, slot_id, message_id, "new prompt").unwrap();
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
        }
    }

use crate::HashMap;
use crate::Uuid;
use crate::codex_conversation_from_file;
use crate::codex_rate_window;
use crate::codex_reset_credits_summary;
use crate::codex_stdout_agent_message;
use crate::codex_transcript_messages;
use crate::codex_usage_from_payload;
use crate::env;
use crate::fs;


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

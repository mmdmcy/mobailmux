use crate::CODEX_APP_SERVER_WRITE_SETTLE;
use crate::CodexAppServerDashboard;
use crate::CodexConversation;
use crate::CodexIndex;
use crate::CodexModel;
use crate::CodexRateWindow;
use crate::CodexReasoningEffort;
use crate::CodexResetCredit;
use crate::CodexResetCreditsSummary;
use crate::CodexUsageSnapshot;
use crate::Config;
use crate::StdCommand;
use crate::Utc;
use crate::Uuid;
use crate::shell_single_quote;

pub(crate) fn merge_codex_rate_limit_status(
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

pub(crate) fn fetch_codex_app_server_dashboard(config: &Config) -> CodexAppServerDashboard {
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

pub(crate) fn fetch_codex_model_catalog(config: &Config) -> Vec<CodexModel> {
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

pub(crate) fn codex_models_from_payload(payload: &serde_json::Value) -> Vec<CodexModel> {
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

pub(crate) fn consume_codex_rate_limit_reset_credit(config: &Config) -> Option<String> {
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

pub(crate) fn codex_app_server_request(config: &Config, input: &str) -> Option<String> {
    let mut command_parts = vec![config.legacy_codex_bin.as_str()];
    command_parts.extend(config.legacy_codex_args.iter().map(String::as_str));
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

pub(crate) fn codex_usage_from_payload(
    observed_at: &str,
    payload: &serde_json::Value,
) -> CodexUsageSnapshot {
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

pub(crate) fn codex_rate_window(
    label: &str,
    value: Option<&serde_json::Value>,
) -> Option<CodexRateWindow> {
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

pub(crate) fn codex_credits_text(value: Option<&serde_json::Value>) -> Option<String> {
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

pub(crate) fn codex_reset_credits_summary(
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

pub(crate) fn codex_conversation_by_id<'a>(
    index: &'a CodexIndex,
    thread_id: &str,
) -> Option<&'a CodexConversation> {
    index
        .conversations
        .iter()
        .find(|conversation| conversation.id == thread_id)
}

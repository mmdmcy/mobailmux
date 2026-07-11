use crate::AppState;
use crate::Arc;
use crate::CodexRateWindow;
use crate::CodexResetCreditsSummary;
use crate::CodexUsageSnapshot;
use crate::Config;
use crate::Form;
use crate::HeaderMap;
use crate::Redirect;
use crate::Response;
use crate::State;
use crate::StatusCode;
use crate::StdCommand;
use crate::agent_command_label;
use crate::consume_codex_rate_limit_reset_credit;
use crate::epoch_to_rfc3339;
use crate::format_epoch_date;
use crate::format_number;
use crate::html_escape;
use crate::page_guard;
use crate::refresh_codex_index;
use crate::short_time;
use crate::truncate_text;
use axum::response::IntoResponse;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct CodexResetForm {
    confirm: String,
    slot_id: Option<i64>,
}

pub(crate) async fn codex_reset_post(
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

pub(crate) fn codex_usage_dialog(
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

pub(crate) fn usage_window_card(fallback_label: &str, window: Option<&CodexRateWindow>) -> String {
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

pub(crate) fn usage_reset_line(fallback_label: &str, window: Option<&CodexRateWindow>) -> String {
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

pub(crate) fn codex_reset_credit_text(credits: Option<&CodexResetCreditsSummary>) -> String {
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

pub(crate) fn usage_window_duration(minutes: i64) -> String {
    if minutes >= 1440 && minutes % 1440 == 0 {
        return format!("{}d", minutes / 1440);
    }
    if minutes >= 60 && minutes % 60 == 0 {
        return format!("{}h", minutes / 60);
    }
    format!("{minutes}m")
}

pub(crate) fn codex_usage_text(usage: Option<&CodexUsageSnapshot>) -> String {
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

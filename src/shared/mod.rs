use crate::AgentHarness;
use crate::AgentSlotSeed;
use crate::Config;
use crate::DEFAULT_AGENT_SLOTS;
use crate::DateTime;
use crate::Html;
use crate::Local;
use crate::MAX_AGENT_SLOT_CHARS;
use crate::PAGE_CSS;
use crate::Path;
use crate::PathBuf;
use crate::Response;
#[cfg(test)]
use crate::SystemTime;
#[cfg(test)]
use crate::Utc;
use crate::env;
#[cfg(test)]
use crate::fs;
use crate::io;
use axum::response::IntoResponse;
use chrono::Datelike;

pub(crate) fn default_home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub(crate) fn expand_local_path(value: &str) -> PathBuf {
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

pub(crate) fn agent_command_label(config: &Config, harness: AgentHarness) -> String {
    let (binary, args) = match harness {
        AgentHarness::Pi => (&config.pi_bin, &config.pi_args),
        AgentHarness::OpenCode => (&config.opencode_bin, &config.opencode_args),
        AgentHarness::LegacyCodex => return "legacy-codex (read-only)".into(),
    };
    let mut parts = vec![binary.clone()];
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

pub(crate) fn agent_execution_mode_html(config: &Config, harness: AgentHarness) -> String {
    let label = harness.display_name();
    let command = html_attr_escape(&agent_command_label(config, harness));
    format!(
        r#"<span class="agent-execution-mode" title="{command}" aria-label="{label} harness"><span>Harness</span><strong>{label}</strong></span>"#
    )
}

pub(crate) fn split_env_args(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn parse_agent_slot_seeds(
    raw: Option<String>,
    default_workdir: &Path,
) -> Vec<AgentSlotSeed> {
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

pub(crate) fn normalize_agent_slot_name(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
pub(crate) fn file_modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

#[cfg(test)]
pub(crate) fn system_time_to_rfc3339(value: SystemTime) -> String {
    DateTime::<Utc>::from(value).to_rfc3339()
}

#[cfg(test)]
pub(crate) fn epoch_to_rfc3339(epoch: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(epoch, 0).map(|value| value.to_rfc3339())
}

#[cfg(test)]
pub(crate) fn format_epoch_date(epoch: i64) -> String {
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

#[cfg(test)]
pub(crate) fn short_time(value: &str) -> String {
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

pub(crate) fn compact_local_time(value: &str) -> String {
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

#[cfg(test)]
pub(crate) fn format_duration(seconds: i64, prefix: &str) -> String {
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

#[cfg(test)]
pub(crate) fn format_number(value: i64) -> String {
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

pub(crate) fn truncate_text(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(20);
    value.chars().take(keep).collect::<String>() + "\n...[truncated]"
}

pub(crate) fn page(title: &str, body: &str) -> Response {
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

pub(crate) fn env_flag(name: &str, default: bool) -> bool {
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

pub(crate) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn html_attr_escape(value: &str) -> String {
    html_escape(value).replace('\r', "").replace('\n', "&#10;")
}

pub(crate) fn io_other(err: impl std::fmt::Display) -> io::Error {
    io::Error::other(err.to_string())
}

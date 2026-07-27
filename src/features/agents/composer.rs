use crate::AGENT_COMMAND_ALIASES;
use crate::AGENT_COMMAND_SPECS;
use crate::ComposerSuggestion;
use crate::Config;
use crate::HashSet;
#[cfg(test)]
use crate::Path;
#[cfg(test)]
use crate::PathBuf;
#[cfg(test)]
use crate::fs;
#[cfg(test)]
use crate::truncate_text;
use serde::Serialize;

pub(crate) fn agent_location(slot_id: Option<i64>) -> String {
    slot_id
        .filter(|id| *id > 0)
        .map(|id| format!("/agents?slot={id}"))
        .unwrap_or_else(|| "/agents".into())
}

pub(crate) fn command_arg<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case(command) {
        return Some("");
    }
    if trimmed.len() > command.len()
        && trimmed[..command.len()].eq_ignore_ascii_case(command)
        && trimmed[command.len()..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        return Some(trimmed[command.len()..].trim());
    }
    None
}

pub(crate) fn agent_control_text(text: &str) -> Option<(char, &str)> {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let prefix = chars.next()?;
    if !matches!(prefix, '!' | '/') {
        return None;
    }
    Some((prefix, chars.as_str().trim()))
}

#[cfg(test)]
pub(crate) fn looks_like_agent_control_request(body: &str) -> bool {
    agent_control_text(body).is_some()
}
pub(crate) fn normalize_agent_command_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let (name, rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(name, rest)| (name, Some(rest)))
        .unwrap_or((trimmed, None));
    let lower = name.to_ascii_lowercase();
    if known_agent_command_names()
        .iter()
        .any(|command| *command == lower)
    {
        return trimmed.to_string();
    }
    let matches = known_agent_command_names()
        .into_iter()
        .filter(|command| command.starts_with(&lower))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return trimmed.to_string();
    }
    match rest {
        Some(rest) if !rest.trim_start().is_empty() => {
            format!("{} {}", matches[0], rest.trim_start())
        }
        _ => matches[0].to_string(),
    }
}

pub(crate) fn known_agent_command_names() -> Vec<&'static str> {
    let mut names = AGENT_COMMAND_SPECS
        .iter()
        .map(|command| command.name)
        .collect::<Vec<_>>();
    names.extend(AGENT_COMMAND_ALIASES);
    names
}

pub(crate) fn agent_composer_suggestions_json(_config: &Config) -> String {
    let mut suggestions = AGENT_COMMAND_SPECS
        .iter()
        .map(|command| ComposerSuggestion {
            kind: "command",
            name: command.name.to_string(),
            insert: format!("/{}", command.name),
            description: command.description.to_string(),
            takes_arg: command.takes_arg,
        })
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    suggestions.retain(|suggestion| {
        seen.insert(format!(
            "{}:{}",
            suggestion.kind,
            suggestion.name.to_ascii_lowercase()
        ))
    });
    suggestions.sort_by(|left, right| {
        suggestion_kind_rank(left.kind)
            .cmp(&suggestion_kind_rank(right.kind))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });
    json_for_inline_script(&suggestions)
}

pub(crate) fn json_for_inline_script<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "[]".into())
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub(crate) fn suggestion_kind_rank(kind: &str) -> usize {
    match kind {
        "command" => 0,
        "skill" => 1,
        "plugin" => 2,
        _ => 3,
    }
}

#[cfg(test)]
pub(crate) fn discover_codex_skill_suggestions(codex_home: &Path) -> Vec<ComposerSuggestion> {
    let mut files = Vec::new();
    collect_named_files(&codex_home.join("skills"), "SKILL.md", 5, &mut files);
    collect_named_files(&codex_home.join("plugins"), "SKILL.md", 9, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let skill_dir = path.parent()?.file_name()?.to_str()?.to_string();
            let plugin_root = plugin_root_for_path(&path);
            let name = if let Some(root) = plugin_root {
                let plugin = plugin_manifest_field(&root, "name")?;
                format!("{plugin}:{skill_dir}")
            } else {
                skill_dir
            };
            let description =
                skill_description_from_file(&path).unwrap_or_else(|| "Codex skill".into());
            Some(ComposerSuggestion {
                kind: "skill",
                insert: format!("${name}"),
                name,
                description: compact_text(&description, 120),
                takes_arg: false,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn discover_codex_plugin_suggestions(codex_home: &Path) -> Vec<ComposerSuggestion> {
    let mut files = Vec::new();
    collect_named_files(&codex_home.join("plugins"), "plugin.json", 9, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let plugin_dir = path.parent()?.parent()?;
            let name = plugin_manifest_field(plugin_dir, "name")?;
            let description =
                plugin_manifest_description(plugin_dir).unwrap_or_else(|| "Codex plugin".into());
            Some(ComposerSuggestion {
                kind: "plugin",
                insert: format!("#{name}"),
                name,
                description: compact_text(&description, 120),
                takes_arg: false,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn compact_text(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_text(&compact, max_chars).replace('\n', " ")
}

#[cfg(test)]
pub(crate) fn collect_named_files(
    dir: &Path,
    file_name: &str,
    depth: usize,
    files: &mut Vec<PathBuf>,
) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, file_name, depth - 1, files);
        } else if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            files.push(path);
        }
    }
}

#[cfg(test)]
pub(crate) fn plugin_root_for_path(path: &Path) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.join(".codex-plugin/plugin.json").is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn skill_description_from_file(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .find_map(|line| yaml_string_field(line, "description"))
}

#[cfg(test)]
pub(crate) fn plugin_manifest_description(plugin_root: &Path) -> Option<String> {
    let manifest = plugin_manifest_json(plugin_root)?;
    manifest
        .get("interface")
        .and_then(|interface| interface.get("shortDescription"))
        .and_then(|value| value.as_str())
        .or_else(|| manifest.get("description").and_then(|value| value.as_str()))
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) fn plugin_manifest_field(plugin_root: &Path, field: &str) -> Option<String> {
    plugin_manifest_json(plugin_root)?
        .get(field)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) fn plugin_manifest_json(plugin_root: &Path) -> Option<serde_json::Value> {
    let raw = fs::read_to_string(plugin_root.join(".codex-plugin/plugin.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
pub(crate) fn yaml_string_field(line: &str, field: &str) -> Option<String> {
    let value = line.trim().strip_prefix(&format!("{field}:"))?.trim();
    if value.is_empty() {
        return None;
    }
    Some(
        value
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string(),
    )
}

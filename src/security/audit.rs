use crate::Path;
use crate::PathBuf;
use crate::StdCommand;
use crate::env;
use crate::fs;
use crate::io;
use std::os::unix::fs::PermissionsExt;

#[derive(Debug)]
pub(crate) struct AuditFinding {
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

pub(crate) fn audit_public_cmd(args: &[String]) -> io::Result<u8> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!(
            r#"Usage:
  mobailmux audit-public
  mobailmux audit-public --install-hook

Checks tracked files for local/private paths, common secret markers,
private-network IP leaks, and host-specific denylist terms.
"#
        );
        return Ok(0);
    }

    let root = git_root()?;
    if args.iter().any(|arg| arg == "--install-hook") {
        install_audit_hooks(&root)?;
        println!("installed .git/hooks/pre-commit and .git/hooks/pre-push");
    }

    let findings = audit_public(&root)?;
    if findings.is_empty() {
        println!("audit-public: ok");
        return Ok(0);
    }

    eprintln!("audit-public: found {} issue(s)", findings.len());
    for finding in &findings {
        match finding.line {
            Some(line) => eprintln!("{}:{}: {}", finding.path, line, finding.message),
            None => eprintln!("{}: {}", finding.path, finding.message),
        }
    }
    Ok(1)
}

pub(crate) fn git_root() -> io::Result<PathBuf> {
    let output = StdCommand::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("not inside a Git repository"));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

pub(crate) fn install_audit_hooks(root: &Path) -> io::Result<()> {
    let hooks = root.join(".git/hooks");
    fs::create_dir_all(&hooks)?;
    for name in ["pre-commit", "pre-push"] {
        let hook = hooks.join(name);
        fs::write(
            &hook,
            r#"#!/bin/sh
set -eu
repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"
cargo run --quiet -- audit-public
"#,
        )?;
        let mut permissions = fs::metadata(&hook)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions)?;
    }
    Ok(())
}

pub(crate) fn audit_public(root: &Path) -> io::Result<Vec<AuditFinding>> {
    let files = git_tracked_files(root)?;
    let private_terms = load_audit_denylist(root);
    let mut findings = Vec::new();

    for path in files {
        if let Some(message) = audit_path(&path) {
            findings.push(AuditFinding {
                path,
                line: None,
                message,
            });
            continue;
        }

        let full_path = root.join(&path);
        if fs::metadata(&full_path)
            .map(|metadata| metadata.len() > 1_000_000)
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&full_path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            for message in audit_line(line, &private_terms) {
                findings.push(AuditFinding {
                    path: path.clone(),
                    line: Some(index + 1),
                    message,
                });
            }
        }
    }

    Ok(findings)
}

pub(crate) fn git_tracked_files(root: &Path) -> io::Result<Vec<String>> {
    let output = StdCommand::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

pub(crate) fn audit_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    let denied_exact = [
        "AGENTS.md",
        "mobailmux.local.toml",
        ".env",
        ".env.local",
        "id_rsa",
        "id_ed25519",
    ];
    if denied_exact.contains(&normalized.as_str()) || denied_exact.contains(&name) {
        return Some("private file path is tracked".into());
    }
    if name.starts_with(".env.") && name != ".env.example" {
        return Some("private env file is tracked".into());
    }
    if normalized.starts_with("docs/private/")
        || normalized.starts_with(".mobailmux/")
        || normalized.starts_with("backups/")
        || normalized.starts_with("data/")
        || normalized.starts_with("downloads/")
    {
        return Some("ignored private/runtime path is tracked".into());
    }
    if normalized.contains("/data/")
        || normalized.contains("/cache/")
        || normalized.contains("/config/")
        || normalized.contains("/downloads/")
        || normalized.contains("/secrets/")
    {
        return Some("runtime or secret data path is tracked".into());
    }
    if matches!(
        Path::new(name).extension().and_then(|ext| ext.to_str()),
        Some("db" | "sqlite" | "sqlite3" | "log" | "pid" | "pem" | "key" | "p12" | "pfx")
    ) {
        return Some("private state or key-like file is tracked".into());
    }
    None
}

pub(crate) fn load_audit_denylist(root: &Path) -> Vec<String> {
    let mut paths = vec![
        root.join("docs/private/audit-denylist.txt"),
        root.join(".mobailmux/audit-denylist.txt"),
    ];
    if let Ok(path) = env::var("MOBAILMUX_AUDIT_DENYLIST") {
        paths.push(PathBuf::from(path));
    }

    let mut terms = Vec::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            let term = line.trim();
            if term.is_empty() || term.starts_with('#') {
                continue;
            }
            terms.push(term.to_ascii_lowercase());
        }
    }
    terms
}

pub(crate) fn audit_line(line: &str, private_terms: &[String]) -> Vec<String> {
    let mut findings = Vec::new();
    let lower = line.to_ascii_lowercase();
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return findings;
    }

    if line.contains("-----BEGIN ") && line.contains(&["PRIVATE", " KEY"].concat()) {
        findings.push("private key material".into());
    }
    for marker in token_markers() {
        if line.contains(&marker) {
            findings.push(format!("token marker `{marker}`"));
        }
    }
    if contains_tailscale_ipv4(line) {
        findings.push("Tailscale/CGNAT private IP address".into());
    }
    if suspicious_secret_assignment(line) {
        findings.push("non-placeholder secret-looking assignment".into());
    }
    for term in private_terms {
        if !term.is_empty() && lower.contains(term) {
            findings.push("local denylist term".into());
        }
    }

    findings
}

pub(crate) fn token_markers() -> Vec<String> {
    vec![
        ["github", "_pat_"].concat(),
        ["gh", "p_"].concat(),
        ["gh", "o_"].concat(),
        ["gh", "s_"].concat(),
        ["gh", "u_"].concat(),
        ["s", "k-"].concat(),
        ["xo", "xb-"].concat(),
        ["xo", "xp-"].concat(),
    ]
}

pub(crate) fn suspicious_secret_assignment(line: &str) -> bool {
    if line.contains("::") {
        return false;
    }
    let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) else {
        return false;
    };
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty()
        || key.len() > 80
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return false;
    }
    let secret_keys = [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "access_key",
        "client_secret",
        "private_key",
    ];
    if !secret_keys.iter().any(|needle| key.contains(needle)) {
        return false;
    }

    let value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches(',')
        .trim();
    let allowed = [
        "",
        "example",
        "placeholder",
        "changeme",
        "change-me",
        "redacted",
        "dummy",
        "none",
        "null",
        "false",
        "true",
    ];
    if allowed.contains(&value.to_ascii_lowercase().as_str()) {
        return false;
    }
    if value == "String" || value.starts_with("Option<") || value.starts_with("Vec<") {
        return false;
    }
    if value.starts_with("${")
        || value.starts_with('<')
        || value.starts_with("your-")
        || value.contains("...")
        || value.starts_with("Some(")
        || value.starts_with("vec!")
    {
        return false;
    }
    true
}

pub(crate) fn contains_tailscale_ipv4(line: &str) -> bool {
    line.split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .filter(|token| token.matches('.').count() == 3)
        .any(|token| {
            let octets: Vec<u16> = token
                .split('.')
                .filter_map(|part| part.parse::<u16>().ok())
                .collect();
            octets.len() == 4
                && octets[0] == 100
                && (64..=127).contains(&octets[1])
                && octets.iter().all(|octet| *octet <= 255)
        })
}

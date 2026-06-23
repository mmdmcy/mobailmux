# Security

Mobailmux is a bridge from a chat/browser control surface to a local CLI AI agent. Treat it like remote command execution.

## Recommended Boundary

- Keep Mobailmux private: loopback, trusted LAN, private VPN, or a reverse proxy with TLS and access control.
- For the Rust web UI, set `MOBAILMUX_PASSWORD_HASH` and keep `MOBAILMUX_AUTH_DISABLED=0` outside local-only development.
- For Mattermost, use a dedicated bot token and restrict execution to one user with `MOBAILMUX_OWNER_USERNAME` or `MOBAILMUX_OWNER_USER_ID`.
- Keep `.env` out of git.
- Use project-specific workdirs instead of defaulting every slot to your entire home directory.
- Keep each slot on a separate project or branch when jobs can edit files.
- Do not expose the Rust web UI or Mattermost directly to the public internet unless you add a proper access layer.

## Web UI Notes

The Rust web UI uses password-hash auth and a signed HTTP-only cookie. It does not provide TLS by itself. Keep it on `127.0.0.1`, a private interface, or behind a TLS reverse proxy.

The agent transcript is stored in the Rust service SQLite database configured by `MOBAILMUX_DB`.

## Mattermost Notes

Mattermost channel history is stored by Mattermost, not by Mobailmux. `!fresh` and `!stayfresh` reset Mobailmux's Codex thread and local `logs` buffer, but they do not delete old Mattermost posts.

The included Compose stack binds Mattermost to `127.0.0.1` by default. For phone access, expose it only through a trusted private network or VPN.

## Codex Autonomy

The example config uses:

```text
MOBAILMUX_CODEX_ARGS=--dangerously-bypass-approvals-and-sandbox
```

That is convenient, but it lets Codex execute shell commands without approval. Use it only on machines and repositories where you understand the blast radius.

Safer options:

- remove the dangerous flag and handle approvals manually where possible
- run Mobailmux in a container or VM
- restrict workdirs to disposable clones
- keep secrets out of project folders

## What Not To Commit

Never commit:

- `.env`
- bot tokens
- web password hashes and cookie secrets
- admin passwords
- Mattermost database/data directories
- Mobailmux runtime state
- Codex auth files
- private hostnames, private IPs, or personal restore docs

Before publishing, run a secret scanner such as `gitleaks` or `trufflehog`.

## Commit Guardrails

Maintainers should install the repository hook before committing:

```bash
scripts/install-git-hooks.sh
```

The hook blocks staged `.env` files, virtualenvs, generated state, known local identifiers, and runs `gitleaks protect --staged` before Git accepts the commit.

# Security

Mobailmux is a bridge from Mattermost chat messages to a local CLI AI agent. Treat it like remote command execution.

## Recommended Boundary

- Run Mattermost behind a private network boundary such as a VPN or trusted LAN.
- Use a dedicated bot token for Mobailmux.
- Restrict execution to one Mattermost user with `MOBAILMUX_OWNER_USERNAME` or `MOBAILMUX_OWNER_USER_ID`.
- Keep `.env` out of git.
- Use project-specific workdirs instead of defaulting to your entire home directory.
- Keep each slot on a separate project or branch when jobs can edit files.

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
- admin passwords
- Mattermost database/data directories
- Codex auth files
- private hostnames, private IPs, or personal restore docs

Before publishing, run a secret scanner such as `gitleaks` or `trufflehog`.

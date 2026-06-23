# Mobailmux

Mobailmux is the private control surface for local AI agent lanes. It includes
a Rust browser UI and the existing `mbx` terminal command assets.

Use it when you want a browser and tmux-oriented way to start, resume, monitor,
and stop Codex work.

## Current Shape

- Rust web service for Agents at `/` and `/agents`.
- SQLite storage for agent messages, attachments, and saved Codex threads.
- Saved Codex conversation browser and usage/reset panel.
- Existing `commands/bin/mbx` tmux helper remains available for terminal slots.
- Private-by-default: run behind localhost, LAN, VPN, or tailnet access.

## Run The Rust Web UI

```sh
cargo run -- hash-password --stdin
MOBAILMUX_PASSWORD_HASH='$argon2id$...' cargo run -- serve
```

Default bind: `127.0.0.1:8765`.

For local-only development without auth:

```sh
MOBAILMUX_AUTH_DISABLED=1 cargo run -- serve
```

## Configuration

```text
MOBAILMUX_BIND=127.0.0.1:8765
MOBAILMUX_DB=data/mobailmux.sqlite
MOBAILMUX_AGENT_DEFAULT_WORKDIR=~
MOBAILMUX_AGENT_UPLOAD_DIR=data/agent-uploads
MOBAILMUX_AGENT_SLOTS=codex
MOBAILMUX_AGENT_CODEX_BIN=codex
MOBAILMUX_AGENT_CODEX_ARGS=--dangerously-bypass-approvals-and-sandbox
MOBAILMUX_PASSWORD_HASH=<argon2 hash>
MOBAILMUX_COOKIE_SECRET=<random hex>
```

## Terminal `mbx`

The existing shell command lives at:

```sh
commands/bin/mbx
```

It manages tmux-backed Codex sessions. Run:

```sh
commands/bin/mbx help
```

## Checks

```sh
cargo fmt --check
cargo test
cargo run -- audit-public
```

Keep real `.env` files, databases, uploads, logs, local service units, and
host-specific deployment state out of this repository.

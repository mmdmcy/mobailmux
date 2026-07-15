# Mobailmux

Mobailmux is the private control surface for local AI agent lanes. It includes
a Rust browser UI and the existing `mbx` terminal command assets.

Use it when you want a browser and tmux-oriented way to start, resume, monitor,
and stop Codex work.

## Current Shape

- Rust web service for Agents at `/` and `/agents`.
- SQLite storage for agent messages and saved Codex thread identifiers.
- Codex usage and manually confirmed reset controls.
- Embedded web terminal, isolated as its own capability.
- Existing `commands/bin/mbx` tmux helper remains available for terminal slots.
- Private-by-default: run behind localhost, LAN, VPN, or tailnet access.

Architecture and feature boundaries are documented in
[`docs/architecture.md`](docs/architecture.md).

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
MOBAILMUX_AGENT_SLOTS=codex
MOBAILMUX_AGENT_CODEX_BIN=codex
MOBAILMUX_AGENT_CODEX_ARGS=--dangerously-bypass-approvals-and-sandbox
MOBAILMUX_AGENT_PROGRESS_NOTES=0
MOBAILMUX_PASSWORD_HASH=<argon2 hash>
MOBAILMUX_COOKIE_SECRET=<random hex>
```

## Terminal `mbx`

The existing shell command lives at:

```sh
commands/bin/mbx
```

It manages reusable tmux workspaces (`a` through `j`) and enables tmux mouse
mode for scrolling. `mbx r <slot>` attaches to that exact tmux workspace;
`mbx q <slot>` stops one, and `mbx s`/`mbx start`/`mbx new` remain optional
Codex-launcher conveniences. Run:

```sh
commands/bin/mbx help
```

## Manual Live Deploy

There is no background auto-deploy watcher. Deploy only when a human or agent
explicitly runs:

```sh
scripts/deploy-live.sh
```

The command refuses to run if `mobailmux-autodeploy.service` is active. It runs
the host's one-shot deploy helper, which checks, builds, installs the live
binary, restarts `mobailmux.service`, and verifies that it came back active.

## iPhone WebKit Smoke Test

Set up the private Playwright/WebKit toolchain once:

```sh
scripts/ensure-playwright-webkit.sh
```

Then test a Mobailmux page with the iPhone 13 WebKit profile:

```sh
PLAYWRIGHT_BROWSERS_PATH=private/playwright-webkit/browsers \
  private/playwright-webkit/venv/bin/python \
  scripts/smoke-iphone-webkit.py \
  --url http://127.0.0.1:8765/agents \
  --expect-selector '[data-agent-messages]' \
  --expect-selector '.agent-composer'
```

The Playwright virtualenv, browser binaries, screenshots, and logs live under
ignored `private/playwright-webkit/`.

## Checks

```sh
cargo fmt --check
cargo test
cargo run -- audit-public
cargo katrust check
```

Keep real `.env` files, databases, logs, local service units, and
host-specific deployment state out of this repository.

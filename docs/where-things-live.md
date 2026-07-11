# Where Things Live

This is the quick map for Mobailmux source files, runtime state, and the live
service layout. Keep this document public-safe: describe locations and
responsibilities, but do not paste secrets, host-specific env values, private
IPs, tokens, databases, uploads, logs, or local-only notes into the repo.

## Source Checkout

The source checkout is the place to edit code, docs, tests, scripts, and command
assets.

- `src/main.rs`: Rust web server, routes, page rendering, agent slot handling,
  Codex conversation indexing, usage/reset UI, auth, and audit checks.
- `src/page.css`: shared CSS for the Rust web UI. It is embedded into the Rust
  binary with `include_str!("page.css")`, so CSS changes require a rebuild.
- `src/db_migrations.rs`: SQLite schema migrations for the Rust web service.
- `src/codex_reset_ledger.rs`: reset-credit tracking helper for Codex usage.
- `src/modules.rs`: route/module wiring used by the Rust service.
- `src/mobailmux/`: Python package for the Mattermost bridge.
- `commands/bin/mbx`: terminal tmux helper for local agent sessions.
- `commands/COMMANDS.md`: command behavior notes for `mbx`.
- `tests/`: Python tests for command and Mattermost behavior.
- `docs/`: human documentation.
- `mattermost/`: Mattermost-specific notes.
- `systemd/`: example service unit files only.
- `scripts/`: bootstrap, install, and development helper scripts.
- `scripts/ensure-playwright-webkit.sh`: provisions the ignored private
  Playwright/WebKit toolchain used for iPhone 13 smoke tests.
- `scripts/smoke-iphone-webkit.py`: runs a WebKit smoke check with Playwright's
  iPhone 13 device profile.
- `compose.yaml`: local Mattermost stack example.
- `.env.example`: public template only. Real `.env` files stay private.

## Rust Web UI

The browser UI is generated mostly from `src/main.rs`.

- Main page and Agents page: `agents_page`.
- Browse drawer: `codex_browser_drawer_html` plus the embedded script in
  `agents_page`.
- Chat/message polling: embedded script in `agents_page` and the
  `/agents/slots/.../state` handler.
- Visual layout, mobile behavior, and drawer sizing: `src/page.css`.
- Auth/session behavior: `login_page`, `login_post`, `page_guard`, and cookie
  helpers in `src/main.rs`.

## Local Development State

Default development state is relative to the checkout unless env vars override
it.

- `MOBAILMUX_DB`: defaults to `data/mobailmux.sqlite`.
- `MOBAILMUX_AGENT_UPLOAD_DIR`: defaults to `data/agent-uploads`.
- `MOBAILMUX_AGENT_DEFAULT_WORKDIR`: defaults to the user's home directory.
- `MOBAILMUX_AGENT_SLOTS`: seeds the browser agent slots.
- `MOBAILMUX_AUTH_DISABLED=1`: useful only for local development.
- `.env`: local-only and ignored. Do not commit it.
- `private/playwright-webkit/`: ignored private Playwright virtualenv, WebKit
  browser cache, wheel cache, hashes, and screenshots for mobile smoke tests.

## Codex Data Read By Mobailmux

Mobailmux does not own saved Codex conversations. It reads Codex state from
`CODEX_HOME`, or from the user's default `~/.codex` directory when `CODEX_HOME`
is unset.

- `CODEX_HOME/session_index.jsonl`: optional thread titles and update times.
- `CODEX_HOME/sessions/**/*.jsonl`: saved Codex session files scanned for the
  Browse drawer.
- `CODEX_HOME/history.jsonl`: legacy or fallback conversation history.
- `CODEX_HOME/config.toml`: project trust/config data used to label projects.

Do not move or copy Codex data into this repo.

## Mattermost Bridge State

The Python Mattermost bridge uses its own env vars from `.env.example`.

- `MOBAILMUX_STATE_DIR`: defaults to `~/.local/state/mobailmux`.
- `MOBAILMUX_MATTERMOST_URL`: Mattermost server URL.
- `MOBAILMUX_BOT_TOKEN`: private bot token. Never commit it.
- `MOBAILMUX_SLOTS`: maps slot names to Mattermost channels.
- `MOBAILMUX_DEFAULT_WORKDIR` and `MOBAILMUX_SLOT_*_WORKDIR`: folder routing
  for Mattermost-driven slots.

Mattermost database and Docker data belong to the local compose/runtime
environment, not this source repo.

## Live Service Layout

On a Plugroot-managed host, live service data belongs under that host's private
Plugroot state root, not in this checkout. Treat everything there as runtime
state.

Common live locations, with placeholders:

- `<plugroot-state>/.env`: host env file loaded by services.
- `<plugroot-state>/services/mobailmux/`: Mobailmux service data root.
- `<plugroot-state>/services/mobailmux/mobailmux.sqlite`: live SQLite DB when
  configured by the service unit.
- `<plugroot-state>/services/mobailmux/agent-uploads`: live upload storage when
  configured by the service unit.
- `<plugroot-state>/releases/mobailmux/current/`: current release directory when
  the service runs from a Plugroot release.

Always inspect the live unit before deploying because the current host may
override paths:

```bash
systemctl status mobailmux --no-pager
systemctl cat mobailmux --no-pager
```

## Deploy And Restart Safety

Before deploying, publishing, pulling into production, or restarting services on
a Plugroot-managed host, run:

```bash
<plugroot-install>/bin/plugroot --root <plugroot-install> boundary --strict
<plugroot-install>/bin/plugroot --root <plugroot-install> audit-public
```

If either command fails, stop and fix the boundary first.

For Rust web UI changes, the local check flow is:

```bash
cargo fmt --check
cargo test
cargo run --quiet -- audit-public
cargo build --release
```

On the Plugroot-managed live host, use the repo's manual deploy command only
after the change is done:

```bash
scripts/deploy-live.sh
```

Do not run a background source watcher for Mobailmux. A watcher can restart the
web service while Mobailmux is hosting active Codex agent sessions.

When replacing a live binary manually, keep the previous binary as a timestamped
backup, preserve ownership and mode, restart the service, and verify it is
active.

```bash
systemctl is-active mobailmux
systemctl status mobailmux --no-pager
```

## Mobile WebKit Smoke Tests

Install the private Playwright/WebKit toolchain from the repo root:

```bash
scripts/ensure-playwright-webkit.sh
```

This creates an ignored local toolchain under `private/playwright-webkit/`.
It pins the Playwright Python package version, records downloaded wheel hashes,
and installs only the WebKit browser into the private browser cache.

Run an iPhone 13 smoke check against a local Mobailmux page:

```bash
PLAYWRIGHT_BROWSERS_PATH=private/playwright-webkit/browsers \
  private/playwright-webkit/venv/bin/python \
  scripts/smoke-iphone-webkit.py \
  --url http://127.0.0.1:8765/agents \
  --expect-selector '[data-agent-messages]' \
  --expect-selector '.agent-composer'
```

## What Not To Store Here

Do not commit or paste these into this repo:

- real `.env` files
- local service units
- SQLite databases
- uploads and generated files
- Mattermost data
- Codex conversation data
- release binaries
- tokens, cookies, password hashes, private keys, or secrets
- private hostnames, private IPs, local-only ports, or machine-specific notes

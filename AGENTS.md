# Mobailmux Agent Notes

This repo is the GitHub-bound source for Mobailmux, a private browser and
tmux control surface for local agent lanes.

## Orientation

- Human source/runtime map: `docs/where-things-live.md`.
- Rust web UI entry point: `src/main.rs`.
- Shared web styling: `src/page.css`, embedded into the Rust binary with
  `include_str!("page.css")`.
- Python/Mattermost bridge: `src/mobailmux/app.py`.
- Terminal helper command: `commands/bin/mbx`.
- Systemd example: `systemd/mobailmux.service.example`.
- Runtime data, real env files, uploads, databases, release binaries, and local
  service state belong outside this repo.

## Common Task Routing

- Browser UI bugs on `/` or `/agents`: start with `src/main.rs` and
  `src/page.css`.
- Conversation browser, Browse drawer, Codex transcript, usage, or reset UI:
  start around `agents_page`, `codex_browser_drawer_html`, and the embedded
  script in `src/main.rs`.
- Mobile scroll, layout, or visual issues: check `src/page.css` first, then the
  page script in `src/main.rs`.
- Agent slot state, message polling, uploads, or SQLite persistence: inspect
  `src/main.rs` and `src/db_migrations.rs`.
- Mattermost command behavior: inspect `src/mobailmux/app.py` and Python tests
  under `tests/`.
- `mbx` terminal behavior: inspect `commands/bin/mbx` and
  `commands/COMMANDS.md`.

## Checks

Before considering a source change done, run the relevant subset:

```bash
cargo fmt --check
cargo test
cargo run --quiet -- audit-public
```

For Python/Mattermost changes, also run the focused pytest file if available.

## Live Service Notes

This repo can be used to build the live Mobailmux binary, but the running
service is host state. Do not assume the service runs directly from the repo.

Before deploying or restarting a live Plugroot-managed host:

```bash
<plugroot-install>/bin/plugroot --root <plugroot-install> boundary --strict
<plugroot-install>/bin/plugroot --root <plugroot-install> audit-public
```

If either check fails, do not deploy or restart.

To understand the active installation, inspect systemd instead of guessing:

```bash
systemctl status mobailmux --no-pager
systemctl cat mobailmux --no-pager
```

If the unit points at a root-owned release binary, build with
`cargo build --release`, install the new binary into the release location with
the existing ownership and mode, keep a timestamped backup of the previous
binary, then restart and verify:

```bash
systemctl is-active mobailmux
systemctl status mobailmux --no-pager
```

Do not copy files from private Plugroot runtime state into this repo.

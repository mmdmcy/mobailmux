# Mobailmux Architecture

Mobailmux follows the KatRust ownership map. `src/main.rs` is only the binary
entry point; `src/lib.rs` composes the application and exposes `run`.

## Ownership map

| Responsibility | Source | Owns |
| --- | --- | --- |
| Application | `src/lib.rs` | Process startup, configuration, and global state wiring |
| Agent chat | `src/features/agents/` | Lanes, messages, queues, commands, prompts, and rendering |
| Harness runtime | `src/features/agents/runtime.rs` | Short-lived Pi and OpenCode subprocess adapters |
| Terminal | `src/features/terminal/` | Authenticated embedded command execution |
| Web | `src/interfaces/web/` | Routes, forms, polling responses, HTML, CSS, and JavaScript |
| Legacy parsers | `src/integrations/codex/` | Test-only compatibility for archived Codex data |
| Persistence | `src/persistence/` | SQLite schema migrations and reset-credit ledger |
| Security | `src/security/` | Authentication and public-source privacy audit |
| Shared | `src/shared/` | Small helpers used by at least three ownership areas |

`commands/bin/mbx` is an independent tmux-oriented command interface.

## Dependency direction

```text
main -> application wiring -> interfaces
interfaces -> features -> integrations / persistence
security guards every web entry point
```

Code belongs with the capability that changes for the same reason. HTTP parsing
stays in `interfaces`, product behavior in `features`, external process/API
protocols in `integrations`, SQL and migrations in `persistence`, and trust
decisions in `security`. Do not create a generic `utils.rs`.

## Product boundaries

File uploads, file browsing, folder browsing, and Mattermost are not Mobailmux
capabilities. The working directory remains because the harness and embedded
terminal execute inside it. Refreshing usage never consumes a reset credit;
resets require a separate, explicit confirmation.

## Change procedure

1. Use `cargo katrust inspect` and this ownership map to find the owner.
2. Move one responsibility cluster at a time without redesigning behavior.
3. Run focused tests after each move.
4. Update this map when ownership changes.
5. Run the full verification set before publishing.

## Verification

```bash
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo katrust check
cargo run --quiet -- audit-public
```

# Security

Mobailmux is a browser control surface for a local CLI AI agent. Treat it like remote command execution.

## Recommended Boundary

- Keep Mobailmux private: loopback, trusted LAN, private VPN, or a reverse proxy with TLS and access control.
- For the Rust web UI, set `MOBAILMUX_PASSWORD_HASH` and keep `MOBAILMUX_AUTH_DISABLED=0` outside local-only development.
- Keep `.env` out of git.
- Configure the service's internal execution directory conservatively.
- Do not expose the Rust web UI directly to the public internet unless you add a proper access layer.

## Web UI Notes

The Rust web UI uses password-hash auth and a signed HTTP-only cookie. It does not provide TLS by itself. Keep it on `127.0.0.1`, a private interface, or behind a TLS reverse proxy.

The agent transcript is stored in the Rust service SQLite database configured by `MOBAILMUX_DB`.

## Harness autonomy

The example config uses:

```text
MOBAILMUX_PI_ARGS=--approve
MOBAILMUX_OPENCODE_ARGS=--auto
```

OpenCode's `--auto` enables automatic approval, so use it only on machines and
repositories where you understand the blast radius. Pi's `--approve` has a
narrower meaning: it pre-approves project trust and does not bypass general tool
permissions. Project trust still allows project-local Pi configuration and
extensions to load, so it should only be granted to trusted repositories.

Safer options:

- remove OpenCode `--auto` and handle approvals manually where possible
- remove Pi `--approve` to keep its project-trust prompt
- run Mobailmux in a container or VM
- restrict the configured execution directory to a disposable clone
- keep secrets out of the execution environment

## What Not To Commit

Never commit:

- `.env`
- web password hashes and cookie secrets
- admin passwords
- Mobailmux runtime state
- Pi and OpenCode auth files
- private hostnames, private IPs, or personal restore docs

Before publishing, run a secret scanner such as `gitleaks` or `trufflehog`.

## Commit Guardrails

Maintainers should install the repository hook before committing:

```bash
scripts/install-git-hooks.sh
```

The hook blocks staged `.env` files, virtualenvs, generated state, known local identifiers, and runs `gitleaks protect --staged` before Git accepts the commit.

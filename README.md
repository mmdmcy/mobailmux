# Mobailmux

Mobailmux turns a browser UI or Mattermost channels into mobile control slots for CLI AI agents.

Each slot is an independent agent lane. Send a message, and Mobailmux runs a Codex job locally, streams command progress back to the UI, and keeps that slot's agent chat until you reset it with `fresh`.

```text
Browser or Mattermost
  -> Mobailmux
  -> codex exec --json
  -> local workspace
```

Mobailmux is useful when you want to kick off several independent AI coding jobs from iOS, Android, desktop, or a small server without juggling SSH sessions.

## Features

- built-in lightweight web UI
- Mattermost channel per agent slot
- multiple slots running in parallel
- per-channel continuing Codex chat
- `fresh` reset command
- `stop`, `status`, `pwd`, `ls`, and `cd` controls
- command start/exit progress from `codex exec --json`
- optional explicit progress notes through `aiprogress 'message'`
- owner allowlist so only one Mattermost user can trigger jobs
- no public callback URL required for either frontend

## Status

The current driver is Codex. The built-in web UI and Mattermost adapter use the same slot model. The code is structured so other CLI agents can be added later, but Codex is the first supported runtime.

## Platform Support

- Mobile client: any modern browser, plus iOS, Android, desktop, or web Mattermost clients.
- Host runner: intended for Linux, macOS, and Windows wherever Python 3.11+ and the configured CLI agent are available.
- Quickstart scripts: Bash-based, so they target Linux, macOS, or WSL. Native Windows can still run the Python package, but PowerShell bootstrap scripts are not included yet.
- Background service: the included install scripts and systemd examples are Linux/systemd-specific. macOS launchd and Windows service examples are not included yet.

## Quick Start

### Lightweight Web UI

Prerequisites:

- Python 3.11+
- Codex CLI installed and logged in with `codex login`

Create config, set a password, install, and run:

```bash
cp .env.example .env
$EDITOR .env
python3 -m venv .venv
. .venv/bin/activate
pip install -e .
mobailmux web
```

Or install the web UI as a user systemd service:

```bash
scripts/install-web-user-service.sh
```

Open `http://127.0.0.1:8765`, sign in with `MOBAILMUX_WEB_PASSWORD`, choose a slot, and send:

```text
help
```

For phone access, put the web UI behind a private VPN, private reverse proxy, or trusted LAN endpoint. Keep the default `127.0.0.1` binding unless you intentionally expose it.

### Mattermost

Prerequisites:

- Docker with Docker Compose v2
- Python 3.11+
- Bash, `curl`, and `jq` for the included quickstart scripts
- Codex CLI installed and logged in with `codex login`

Start a local Mattermost, create an admin user, create the bot, and create the slot channels:

```bash
cp .env.example .env
scripts/quickstart-docker.sh
```

Install Mobailmux:

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install -e .
```

Run it:

```bash
mobailmux
```

Or install it as a user systemd service:

```bash
scripts/install-user-service.sh
```

The installer writes a user service with paths based on the current clone. `systemd/mobailmux.service.example` is only a template for manual installs.

Open Mattermost, join `agent-one`, `agent-two`, or `agent-three`, and send:

```text
help
```

## Existing Mattermost

If you already have Mattermost, skip Docker Compose and fill in `.env` manually:

```text
MOBAILMUX_MATTERMOST_URL=https://mattermost.example.com
MOBAILMUX_TEAM_NAME=agents
MOBAILMUX_OWNER_USERNAME=your-mattermost-username
MOBAILMUX_ADMIN_USERNAME=admin
MOBAILMUX_ADMIN_PASSWORD=<admin-password>
```

Then run `scripts/bootstrap-mattermost.sh` to create the team, bot token, and slot channels.

## Channel Commands

Type commands as normal messages, not slash commands:

```text
help
slots
pwd
ls
ls src
cd /path/to/project
fresh
status
stop
```

Any other message continues that channel's agent chat in the current folder.

Advanced commands:

```text
logs
model
next <request>
queue
clearqueue
```

## Configuration

Required runtime values:

```text
MOBAILMUX_MATTERMOST_URL=http://mattermost.example.local
MOBAILMUX_TEAM_NAME=agents
MOBAILMUX_OWNER_USERNAME=your-mattermost-username
MOBAILMUX_BOT_TOKEN=<bot-token>
MOBAILMUX_SLOTS=one:agent-one,two:agent-two,three:agent-three
```

Slot format:

```text
name:channel[:default_workdir]
```

Examples:

```text
MOBAILMUX_SLOTS=one:agent-one,two:agent-two,docs:agent-docs
MOBAILMUX_SLOT_ONE_WORKDIR=~/code/app
MOBAILMUX_SLOT_TWO_WORKDIR=~/code/site
```

## Codex

Mobailmux uses:

```bash
codex exec --json --output-last-message <file>
```

If a channel already has a saved Codex thread, Mobailmux uses:

```bash
codex exec resume --json <thread-id>
```

For fully autonomous coding jobs, set:

```text
MOBAILMUX_CODEX_ARGS=--dangerously-bypass-approvals-and-sandbox
```

That is powerful and risky. Read [docs/security.md](docs/security.md) before using it.

## Privacy

Mobailmux does not need Telegram or a public webhook. It polls Mattermost over the URL you configure. The recommended deployment is a private Mattermost instance reachable only over a VPN, private network, or trusted LAN.

The included Compose file binds Mattermost to `127.0.0.1` by default. For phone access, put it behind a private VPN or change `MOBAILMUX_MATTERMOST_BIND` to a trusted private interface address.

## Name

Mobailmux is short for mobile AI multiplexer. A multiplexer is a tool that routes several independent inputs through one control surface; here, Mattermost channels map to separate local AI agent slots.

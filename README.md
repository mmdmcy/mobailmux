# Mobailmux

Mobailmux turns Mattermost channels into mobile control slots for CLI AI agents.

Each slot is a Mattermost channel. Send a message, and Mobailmux runs a Codex job locally, streams command progress back into the channel, and keeps that channel's agent chat until you reset it with `fresh`.

```text
iPhone / Mattermost
  -> private Mattermost server
  -> Mobailmux bot
  -> codex exec --json
  -> local workspace
```

Mobailmux is useful when you want to kick off several independent AI coding jobs from a phone without juggling SSH sessions.

## Features

- Mattermost channel per agent slot
- multiple slots running in parallel
- per-channel continuing Codex chat
- `fresh` reset command
- `stop`, `status`, `pwd`, and `cd` controls
- command start/exit progress from `codex exec --json`
- optional explicit progress notes through `aiprogress 'message'`
- owner allowlist so only one Mattermost user can trigger jobs
- no public callback URL required

## Status

The current driver is Codex. The code is structured so other CLI agents can be added later, but Codex is the first supported runtime.

## Quick Start

Prerequisites:

- Docker with Docker Compose v2
- Python 3.11+
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

Mobailmux is short for mobile AI multiplexer.

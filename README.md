# Mobailmux

Mobailmux turns a browser UI or Mattermost channels into mobile control slots for CLI AI agents.

Each slot is an independent agent lane. Send a message, and Mobailmux runs a Codex job locally, streams command progress back to the UI, and keeps that slot's agent chat until you reset it with `!fresh` or `!stayfresh`.

```text
Browser or Mattermost
  -> Mobailmux
  -> codex exec --json
  -> local workspace
```

Use it when you want to kick off several independent AI coding jobs from iOS, Android, desktop, or a small server without juggling SSH or tmux sessions.

## Repository Layout

Mobailmux is organized into independent surfaces:

- `commands/` - upstream terminal interface for tmux-backed Codex sessions.
- `src/mobailmux/web.py`, `docs/web.md`, and `web/` - browser UI surface.
- `src/mobailmux/app.py`, `docs/mattermost.md`, and `mattermost/` - Mattermost connector surface.

The command package is the upstream contract for direct terminal automation.
Run `mbx commands` for the live command list.

## Current Shape

- Codex is the only implemented agent driver.
- The built-in web UI is the lightest frontend and does not require Docker.
- Mattermost is optional for teams or people who want a full chat app.
- Both frontends use the same slot model, Codex prompt, workdir state, progress stream, and fresh-session reset behavior.
- The web UI is dark by default and keeps the message composer fixed while only the transcript scrolls.

## Features

- built-in lightweight browser UI
- optional Mattermost channel per slot
- multiple slots running in parallel
- continuing Codex thread per slot
- `!fresh` and `!stayfresh` reset commands that start a new agent chat and clear local slot history
- `!stop`, `!status`, `!pwd`, `!ls`, and `!cd` controls
- queued follow-up requests with `!next <request>`
- command start/exit progress from `codex exec --json`
- explicit human progress notes through `aiprogress 'message'`
- uncapped progress posts by default
- owner allowlist for Mattermost
- no Telegram, public webhook, or callback URL required

## Platform Support

- Mobile client: any modern browser, plus iOS, Android, desktop, or web Mattermost clients.
- Host runner: intended for Linux, macOS, and Windows wherever Python 3.11+ and the configured CLI agent are available.
- Quickstart scripts: Bash-based, so they target Linux, macOS, or WSL.
- Background services: included install scripts and systemd examples are Linux/systemd-specific.
- Native Windows can run the Python package, but PowerShell bootstrap/service scripts are not included yet.

## Quick Start: Web UI

Prerequisites:

- Python 3.11+
- Codex CLI installed and logged in with `codex login`

Create config, set a web password, install, and run:

```bash
cp .env.example .env
$EDITOR .env
python3 -m venv .venv
. .venv/bin/activate
pip install -e .
mobailmux web
```

Open `http://127.0.0.1:8765`, sign in with `MOBAILMUX_WEB_PASSWORD`, choose a slot, and send:

```text
help
```

For phone access, keep Mobailmux behind a private VPN, trusted LAN, or reverse proxy with TLS. Keep the default loopback bind unless you intentionally expose it to a private interface.

On Linux with systemd user services:

```bash
scripts/install-web-user-service.sh
systemctl --user status mobailmux-web.service
```

The service reads the clone's `.env` file and runs:

```bash
mobailmux web
```

## Optional: Mattermost

Prerequisites:

- Docker with Docker Compose v2 for the included Mattermost stack
- Python 3.11+
- Bash, `curl`, and `jq` for the included setup scripts
- Codex CLI installed and logged in with `codex login`

Start a local Mattermost, create an admin user, create the bot, and create the slot channels:

```bash
cp .env.example .env
scripts/quickstart-docker.sh
```

Install and run Mobailmux:

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install -e .
mobailmux
```

On Linux with systemd user services:

```bash
scripts/install-user-service.sh
systemctl --user status mobailmux.service
```

Open Mattermost, join `agent-one`, `agent-two`, or `agent-three`, and send:

```text
help
```

For a direct mobile setup, the slot and channel names can be the same. For example:

```text
MOBAILMUX_SLOTS=one:one,two:two,three:three
```

Then each Mattermost channel is its own Codex lane: `one`, `two`, and `three`.

Mobailmux also looks for a Mattermost channel named `slots`. If that channel exists, it is status-only: type `slots` there to see the current state of every lane. It will not start Codex jobs.

If you already have Mattermost, skip Docker Compose, fill in the Mattermost values in `.env`, then run:

```bash
scripts/bootstrap-mattermost.sh
```

## Slot Commands

Use `!` for Mobailmux command shortcuts. Mattermost slash commands are not used. Plain messages go to the agent.

```text
!help              show command help and configured slots
!commands          same as help
!slots
!pwd
!ls [path]
!cd [path]
!fresh
!stayfresh
!status
!stop
!logs
!model
!next <request>
!queue
!clearqueue
```

Any other message starts or continues that slot's Codex chat in the current folder.

`!fresh` starts a new Codex thread for that slot and resets the slot folder to its configured default. `!stayfresh` starts a new Codex thread while keeping the slot's current folder. In the web UI both commands clear the visible transcript for that slot. In Mattermost they clear Mobailmux's local `logs` history, but do not delete existing Mattermost channel posts.

`!cd` changes the slot's workdir, and `!cd` with no path goes to your home folder. If the workdir changes while a Codex thread is saved, Mobailmux resets that slot's thread so future work starts in the new folder.

## Configuration

Shared values:

```text
MOBAILMUX_SLOTS=one:agent-one,two:agent-two,three:agent-three
MOBAILMUX_DEFAULT_WORKDIR=~
MOBAILMUX_STATE_DIR=~/.local/state/mobailmux
MOBAILMUX_CODEX_BIN=codex
MOBAILMUX_CODEX_ARGS=--dangerously-bypass-approvals-and-sandbox
```

Slot format:

```text
name:label-or-channel[:default_workdir]
```

Examples:

```text
MOBAILMUX_SLOTS=one:agent-one,two:agent-two,docs:agent-docs
MOBAILMUX_SLOT_ONE_WORKDIR=~/code/app
MOBAILMUX_SLOT_TWO_WORKDIR=~/code/site
```

Web values:

```text
MOBAILMUX_WEB_HOST=127.0.0.1
MOBAILMUX_WEB_PORT=8765
MOBAILMUX_WEB_PASSWORD=<strong-password>
```

Mattermost values:

```text
MOBAILMUX_MATTERMOST_URL=http://mattermost.example.local
MOBAILMUX_TEAM_NAME=agents
MOBAILMUX_OWNER_USERNAME=your-mattermost-username
MOBAILMUX_BOT_TOKEN=<bot-token>
MOBAILMUX_SLOTS_CHANNEL=slots
```

Progress behavior:

```text
MOBAILMUX_STATUS_SECONDS=60
MOBAILMUX_MAX_PROGRESS_POSTS=0
```

`MOBAILMUX_STATUS_SECONDS` controls automatic "still running" updates. `MOBAILMUX_MAX_PROGRESS_POSTS=0` means command progress and `aiprogress` notes are uncapped. Set a positive number only if a chat surface gets too noisy.

## Codex Runtime

Mobailmux uses:

```bash
codex exec --json --output-last-message <file>
```

If a slot already has a saved Codex thread in the same workdir, Mobailmux uses:

```bash
codex exec resume --json <thread-id>
```

For fully autonomous coding jobs, set:

```text
MOBAILMUX_CODEX_ARGS=--dangerously-bypass-approvals-and-sandbox
```

That is powerful and risky. Read [docs/security.md](docs/security.md) before using it.

## State And Storage

Mobailmux stores runtime state under `MOBAILMUX_STATE_DIR`:

```text
state.json
web.sqlite3
web-cookie-secret
```

`state.json` stores slot workdirs, saved Codex thread ids, and visible transcript reset markers. `web.sqlite3` stores the web UI transcript. Mattermost channel history lives in Mattermost; Mobailmux only keeps a small in-memory `logs` buffer for that adapter.

Do not commit runtime state, `.env`, virtualenvs, or service data directories.

## Privacy

Mobailmux does not need Telegram or a public webhook. The web UI is served locally by Mobailmux. The Mattermost adapter polls the Mattermost URL you configure.

The recommended deployment is private network access only: loopback, trusted LAN, private VPN, or a reverse proxy with TLS and access control. The included Mattermost Compose file binds to `127.0.0.1` by default.

## Name

Mobailmux is short for mobile AI multiplexer. A multiplexer routes several independent inputs through one control surface; here, browser slots or Mattermost channels map to separate local AI agent lanes.

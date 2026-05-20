# Built-in Web UI

Mobailmux includes a lightweight browser UI for installs where Mattermost is unnecessary or too large.

## Run

```bash
cp .env.example .env
$EDITOR .env
python3 -m venv .venv
. .venv/bin/activate
pip install -e .
mobailmux web
```

Required web value:

```text
MOBAILMUX_WEB_PASSWORD=<strong-password>
```

Default bind:

```text
MOBAILMUX_WEB_HOST=127.0.0.1
MOBAILMUX_WEB_PORT=8765
```

Keep the service on loopback unless you are putting it behind a private VPN, trusted LAN, or reverse proxy with TLS.

## Background Service

On Linux with systemd user services, install and start the web UI with:

```bash
scripts/install-web-user-service.sh
```

The generated service reads the clone's `.env` file and runs:

```bash
mobailmux web
```

The template at `systemd/mobailmux-web.service.example` is for manual installs.

## Commands

The web UI accepts the same slot commands as the Mattermost adapter:

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

Any other message starts or continues that slot's Codex chat.

## Progress

The web UI shows command start/exit events automatically. Agents can also send human-readable milestone notes with `aiprogress 'message'`.

By default progress posts are uncapped:

```text
MOBAILMUX_MAX_PROGRESS_POSTS=0
```

Set a positive value only when you want to cap noisy chat surfaces.

## Storage

Runtime files are stored under `MOBAILMUX_STATE_DIR`:

```text
state.json
web.sqlite3
web-cookie-secret
```

Do not commit these files.

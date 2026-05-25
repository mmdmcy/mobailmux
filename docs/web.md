# Built-in Web UI

Mobailmux includes a lightweight browser UI for installs where Mattermost is unnecessary or too large.

The web UI is served by the Mobailmux Python process. It provides the same slots, commands, Codex thread handling, command progress, `aiprogress` notes, queueing, and `fresh` reset behavior as the Mattermost adapter.

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

Keep the service on loopback unless you are putting it behind a private VPN, trusted LAN, or reverse proxy with TLS. If you bind to a private interface for phone access, keep password auth enabled.

## Background Service

On Linux with systemd user services, install and start the web UI with:

```bash
scripts/install-web-user-service.sh
```

The generated service reads the clone's `.env` file and runs:

```bash
mobailmux web
```

Useful service commands:

```bash
systemctl --user status mobailmux-web.service
systemctl --user restart mobailmux-web.service
journalctl --user -u mobailmux-web.service -f
```

The template at `systemd/mobailmux-web.service.example` is for manual installs.

## Commands

The web UI accepts commands as plain messages:

```text
help              show command help and configured slots
commands          same as help
slots
pwd
ls [path]
cd /path/to/project
fresh
status
stop
logs
model
next <request>
queue
clearqueue
```

Any other message starts or continues that slot's Codex chat in the current folder.

`fresh` starts a new Codex thread and clears the visible transcript for that slot, so long-running slots do not keep growing forever. `cd` changes the slot's workdir; if a saved thread belongs to a different workdir, Mobailmux resets the thread.

## Progress

The web UI shows command start/exit events automatically from `codex exec --json`. Agents can also send human-readable milestone notes with:

```bash
aiprogress 'message'
```

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

`state.json` stores slot workdirs, saved Codex thread ids, and visible transcript reset markers. `web.sqlite3` stores the web UI transcript. `web-cookie-secret` signs login cookies.

Do not commit these files.

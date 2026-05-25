# Mattermost Setup

Mattermost is optional. Use it when you want Mobailmux inside a full chat app with mobile and desktop clients. For the lightest setup, use the built-in web UI instead.

Mobailmux needs:

- one Mattermost team
- one Mattermost bot account with a personal access token
- one channel per slot
- one allowed owner user

The bootstrap helper can create the bot token and slot channels if your Mattermost admin account can use the REST API:

```bash
cp .env.example .env
$EDITOR .env
scripts/bootstrap-mattermost.sh
```

The helper stores `MOBAILMUX_BOT_TOKEN` in `.env`. Do not commit `.env`.

## Included Docker Compose

For a local battery-included Mattermost:

```bash
cp .env.example .env
scripts/quickstart-docker.sh
```

The quickstart script:

- generates local passwords into `.env`
- starts Mattermost and Postgres with Docker Compose
- creates the admin user when possible
- creates the configured team
- creates the Mobailmux bot token
- creates the slot channels

The Compose stack binds to `127.0.0.1:8065` by default. For phone access, expose it only on a trusted private interface, VPN, or LAN.

## Run Mobailmux

Install and run the Mattermost adapter:

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install -e .
mobailmux
```

On Linux with systemd user services:

```bash
scripts/install-user-service.sh
```

Useful service commands:

```bash
systemctl --user status mobailmux.service
systemctl --user restart mobailmux.service
journalctl --user -u mobailmux.service -f
```

## Manual Setup

If you prefer to do it manually:

1. Create a bot account named `mobailmux`.
2. Generate a bot token.
3. Create channels such as `agent-one`, `agent-two`, and `agent-three`.
4. Add the bot and your user to those channels.
5. Put the token and channel names in `.env`.

Required Mattermost values:

```text
MOBAILMUX_MATTERMOST_URL=https://mattermost.example.com
MOBAILMUX_TEAM_NAME=agents
MOBAILMUX_OWNER_USERNAME=your-mattermost-username
MOBAILMUX_BOT_TOKEN=<bot-token>
```

## Channel Commands

Type commands as plain messages, not slash commands:

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

Any other message continues that channel's agent chat.

For direct mobile channel names, configure slots with the same name as the Mattermost channel:

```text
MOBAILMUX_SLOTS=aione:aione,aitwo:aitwo,aithree:aithree
```

`fresh` resets the Codex thread for that channel and clears Mobailmux's local `logs` history for the slot. It does not delete existing posts from the Mattermost channel.

## Progress

Mattermost receives command start/exit updates automatically from `codex exec --json`. Agents can also send human-readable milestone notes with:

```bash
aiprogress 'message'
```

By default progress posts are uncapped:

```text
MOBAILMUX_MAX_PROGRESS_POSTS=0
```

Set a positive value only when you want to cap noisy chat surfaces.

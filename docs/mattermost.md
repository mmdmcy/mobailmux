# Mattermost Setup

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

## Manual Setup

If you prefer to do it manually:

1. Create a bot account named `mobailmux`.
2. Generate a bot token.
3. Create channels such as `agent-one`, `agent-two`, and `agent-three`.
4. Add the bot and your user to those channels.
5. Put the token and channel names in `.env`.

## Channel Commands

Type commands as plain messages, not slash commands:

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

Any other message continues that channel's agent chat. `fresh` resets the chat for that channel and clears Mobailmux's local `logs` history for the slot. It does not delete existing posts from the Mattermost channel.

Advanced commands:

```text
logs
model
next <request>
queue
clearqueue
```

## Progress

Mattermost receives command start/exit updates automatically. Agents can also send human-readable milestone notes with `aiprogress 'message'`.

By default progress posts are uncapped:

```text
MOBAILMUX_MAX_PROGRESS_POSTS=0
```

Set a positive value only when you want to cap noisy chat surfaces.

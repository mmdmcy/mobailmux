#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${MOBAILMUX_ENV:-$ROOT/.env}"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

need curl
need jq

if [[ ! -r "$ENV_FILE" ]]; then
  echo "Missing readable env file: $ENV_FILE" >&2
  echo "Copy .env.example to .env and fill it in first." >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

: "${MOBAILMUX_MATTERMOST_URL:?MOBAILMUX_MATTERMOST_URL is required}"
: "${MOBAILMUX_TEAM_NAME:?MOBAILMUX_TEAM_NAME is required}"
: "${MOBAILMUX_TEAM_DISPLAY_NAME:=$MOBAILMUX_TEAM_NAME}"
: "${MOBAILMUX_OWNER_USERNAME:?MOBAILMUX_OWNER_USERNAME is required}"
: "${MOBAILMUX_ADMIN_USERNAME:?MOBAILMUX_ADMIN_USERNAME is required}"
: "${MOBAILMUX_ADMIN_PASSWORD:?MOBAILMUX_ADMIN_PASSWORD is required}"
: "${MOBAILMUX_BOT_USERNAME:=mobailmux}"
: "${MOBAILMUX_BOT_DISPLAY_NAME:=Mobailmux}"
: "${MOBAILMUX_BOT_DESCRIPTION:=Mattermost slots for CLI AI agents}"
: "${MOBAILMUX_SLOTS:=one:agent-one,two:agent-two,three:agent-three}"

BASE="${MOBAILMUX_MATTERMOST_URL%/}"

write_env_value() {
  local key="$1"
  local value="$2"
  local tmp
  tmp="$(mktemp)"
  awk -v key="$key" -v value="$value" '
    BEGIN { updated = 0 }
    $0 ~ "^" key "=" {
      print key "=" value
      updated = 1
      next
    }
    { print }
    END {
      if (!updated) {
        print key "=" value
      }
    }
  ' "$ENV_FILE" >"$tmp"
  mv "$tmp" "$ENV_FILE"
  chmod 0600 "$ENV_FILE"
}

admin_token() {
  local headers body token
  headers="$(mktemp)"
  body="$(mktemp)"
  curl -fsS \
    -D "$headers" \
    -o "$body" \
    -H "Content-Type: application/json" \
    -X POST \
    -d "$(jq -cn --arg login_id "$MOBAILMUX_ADMIN_USERNAME" --arg password "$MOBAILMUX_ADMIN_PASSWORD" '{login_id: $login_id, password: $password}')" \
    "$BASE/api/v4/users/login" >/dev/null
  token="$(awk 'tolower($1) == "token:" { print $2 }' "$headers" | tr -d '\r' | tail -n 1)"
  rm -f "$headers" "$body"
  [[ -n "$token" ]] || {
    echo "Mattermost admin login did not return a token." >&2
    return 1
  }
  printf "%s" "$token"
}

api_get() {
  local token="$1"
  local path="$2"
  curl -fsS -H "Authorization: Bearer $token" "$BASE$path"
}

api_post() {
  local token="$1"
  local path="$2"
  local payload="$3"
  curl -fsS \
    -H "Authorization: Bearer $token" \
    -H "Content-Type: application/json" \
    -X POST \
    -d "$payload" \
    "$BASE$path"
}

user_id_by_name() {
  local token="$1"
  local username="$2"
  api_get "$token" "/api/v4/users/username/$username" 2>/dev/null | jq -r ".id // empty" || true
}

team_id_by_name() {
  local token="$1"
  api_get "$token" "/api/v4/teams/name/$MOBAILMUX_TEAM_NAME" 2>/dev/null | jq -r ".id // empty" || true
}

create_team() {
  local token="$1"
  api_post "$token" "/api/v4/teams" "$(
    jq -cn \
      --arg name "$MOBAILMUX_TEAM_NAME" \
      --arg display_name "$MOBAILMUX_TEAM_DISPLAY_NAME" \
      '{name: $name, display_name: $display_name, type: "O"}'
  )" | jq -r ".id // empty"
}

channel_id_by_name() {
  local token="$1"
  local team_id="$2"
  local channel="$3"
  api_get "$token" "/api/v4/teams/$team_id/channels/name/$channel" 2>/dev/null | jq -r ".id // empty" || true
}

create_bot_user() {
  local token="$1"
  api_post "$token" "/api/v4/bots" "$(
    jq -cn \
      --arg username "$MOBAILMUX_BOT_USERNAME" \
      --arg display_name "$MOBAILMUX_BOT_DISPLAY_NAME" \
      --arg description "$MOBAILMUX_BOT_DESCRIPTION" \
      '{username: $username, display_name: $display_name, description: $description}'
  )" | jq -r ".user_id // .id // empty"
}

generate_bot_token() {
  local token="$1"
  local bot_user_id="$2"
  api_post "$token" "/api/v4/users/$bot_user_id/tokens" '{"description":"mobailmux"}' | jq -r ".token // empty"
}

ensure_team_member() {
  local token="$1"
  local team_id="$2"
  local user_id="$3"
  api_post "$token" "/api/v4/teams/$team_id/members" "$(jq -cn --arg team_id "$team_id" --arg user_id "$user_id" '{team_id: $team_id, user_id: $user_id}')" >/dev/null 2>&1 || true
}

ensure_channel() {
  local token="$1"
  local team_id="$2"
  local owner_user_id="$3"
  local bot_user_id="$4"
  local slot_name="$5"
  local channel_name="$6"
  local channel_id

  channel_id="$(channel_id_by_name "$token" "$team_id" "$channel_name")"
  if [[ -z "$channel_id" ]]; then
    echo "Creating channel: $channel_name"
    channel_id="$(api_post "$token" "/api/v4/channels" "$(
      jq -cn \
        --arg team_id "$team_id" \
        --arg name "$channel_name" \
        --arg display_name "$slot_name" \
        '{team_id: $team_id, name: $name, display_name: $display_name, type: "O"}'
    )" | jq -r ".id // empty")"
  else
    echo "Channel exists: $channel_name"
  fi

  [[ -n "$channel_id" ]] || {
    echo "Could not resolve channel id for $channel_name" >&2
    exit 1
  }
  api_post "$token" "/api/v4/channels/$channel_id/members" "$(jq -cn --arg user_id "$owner_user_id" '{user_id: $user_id}')" >/dev/null 2>&1 || true
  api_post "$token" "/api/v4/channels/$channel_id/members" "$(jq -cn --arg user_id "$bot_user_id" '{user_id: $user_id}')" >/dev/null 2>&1 || true
}

token="$(admin_token)"
team_id="$(team_id_by_name "$token")"
if [[ -z "$team_id" ]]; then
  echo "Creating team: $MOBAILMUX_TEAM_NAME"
  team_id="$(create_team "$token")"
fi

[[ -n "$team_id" ]] || {
  echo "Mattermost team id could not be resolved: $MOBAILMUX_TEAM_NAME" >&2
  exit 1
}

owner_user_id="$(user_id_by_name "$token" "$MOBAILMUX_OWNER_USERNAME")"
[[ -n "$owner_user_id" ]] || {
  echo "Mattermost owner user not found: $MOBAILMUX_OWNER_USERNAME" >&2
  exit 1
}

bot_user_id="$(user_id_by_name "$token" "$MOBAILMUX_BOT_USERNAME")"
if [[ -z "$bot_user_id" ]]; then
  echo "Creating bot: $MOBAILMUX_BOT_USERNAME"
  bot_user_id="$(create_bot_user "$token")"
else
  echo "Bot exists: $MOBAILMUX_BOT_USERNAME"
fi

[[ -n "$bot_user_id" ]] || {
  echo "Could not resolve bot user id." >&2
  exit 1
}

ensure_team_member "$token" "$team_id" "$owner_user_id"
ensure_team_member "$token" "$team_id" "$bot_user_id"

if [[ -z "${MOBAILMUX_BOT_TOKEN:-}" ]]; then
  bot_token="$(generate_bot_token "$token" "$bot_user_id")"
  [[ -n "$bot_token" ]] || {
    echo "Mattermost did not return a bot token. Check personal access token settings." >&2
    exit 1
  }
  write_env_value MOBAILMUX_BOT_TOKEN "$bot_token"
  echo "Stored MOBAILMUX_BOT_TOKEN in $ENV_FILE"
else
  echo "MOBAILMUX_BOT_TOKEN already exists in $ENV_FILE"
fi

IFS="," read -r -a slots <<<"$MOBAILMUX_SLOTS"
for item in "${slots[@]}"; do
  item="${item//[[:space:]]/}"
  [[ -n "$item" ]] || continue
  IFS=":" read -r slot_name channel_name _rest <<<"$item"
  [[ -n "$slot_name" && -n "$channel_name" ]] || {
    echo "Invalid slot spec: $item" >&2
    exit 1
  }
  ensure_channel "$token" "$team_id" "$owner_user_id" "$bot_user_id" "$slot_name" "$channel_name"
done

echo "Done. Mobailmux Mattermost channels are ready."

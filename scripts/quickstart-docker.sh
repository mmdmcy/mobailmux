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

need docker
need jq
need python3

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
else
  echo "Docker Compose v2 is required: docker compose ..." >&2
  exit 1
fi

generate_secret() {
  python3 - <<'PY'
import secrets
print(secrets.token_urlsafe(32))
PY
}

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

if [[ ! -f "$ENV_FILE" ]]; then
  cp "$ROOT/.env.example" "$ENV_FILE"
  chmod 0600 "$ENV_FILE"
  echo "Created $ENV_FILE"
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

if [[ -z "${POSTGRES_PASSWORD:-}" ]]; then
  write_env_value POSTGRES_PASSWORD "$(generate_secret)"
fi

if [[ -z "${MOBAILMUX_ADMIN_PASSWORD:-}" ]]; then
  write_env_value MOBAILMUX_ADMIN_PASSWORD "$(generate_secret)"
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

echo "Starting Mattermost with Docker Compose..."
"${COMPOSE[@]}" -f "$ROOT/compose.yaml" --env-file "$ENV_FILE" up -d

echo "Waiting for Mattermost..."
deadline=$((SECONDS + 180))
until curl -fsS "${MOBAILMUX_MATTERMOST_URL%/}/api/v4/system/ping" >/dev/null 2>&1; do
  if (( SECONDS > deadline )); then
    echo "Mattermost did not become ready within 180 seconds." >&2
    exit 1
  fi
  sleep 3
done

mmctl() {
  "${COMPOSE[@]}" -f "$ROOT/compose.yaml" --env-file "$ENV_FILE" exec -T mattermost /mattermost/bin/mmctl --local --suppress-warnings "$@"
}

echo "Creating admin user/team if needed..."
mmctl user create \
  --email "$MOBAILMUX_ADMIN_EMAIL" \
  --username "$MOBAILMUX_ADMIN_USERNAME" \
  --password "$MOBAILMUX_ADMIN_PASSWORD" \
  --system-admin >/dev/null 2>&1 || true

mmctl team create \
  --name "$MOBAILMUX_TEAM_NAME" \
  --display-name "${MOBAILMUX_TEAM_DISPLAY_NAME:-$MOBAILMUX_TEAM_NAME}" >/dev/null 2>&1 || true

mmctl team users add "$MOBAILMUX_TEAM_NAME" "$MOBAILMUX_ADMIN_USERNAME" >/dev/null 2>&1 || true

"$ROOT/scripts/bootstrap-mattermost.sh"

cat <<EOF

Mattermost is ready:
  ${MOBAILMUX_MATTERMOST_URL%/}

Admin login:
  username: $MOBAILMUX_ADMIN_USERNAME
  password: stored in $ENV_FILE as MOBAILMUX_ADMIN_PASSWORD

Next:
  python3 -m venv .venv
  . .venv/bin/activate
  pip install -e .
  mobailmux
EOF

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
SERVICE_FILE="$SERVICE_DIR/mobailmux-web.service"

if [[ ! -x "$ROOT/.venv/bin/mobailmux" ]]; then
  echo "Expected $ROOT/.venv/bin/mobailmux to exist." >&2
  echo "Run: python3 -m venv .venv && . .venv/bin/activate && pip install -e ." >&2
  exit 1
fi

mkdir -p "$SERVICE_DIR"
cat >"$SERVICE_FILE" <<EOF
[Unit]
Description=Mobailmux web agent slots
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$ROOT
Environment=PYTHONUNBUFFERED=1
Environment=MOBAILMUX_ENV=$ROOT/.env
ExecStart=$ROOT/.venv/bin/mobailmux web
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now mobailmux-web.service
systemctl --user status mobailmux-web.service --no-pager

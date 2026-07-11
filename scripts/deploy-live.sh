#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEPLOY_HELPER="${MOBAILMUX_DEPLOY_HELPER:-}"

cd "$ROOT"

if systemctl is-active --quiet mobailmux-autodeploy.service 2>/dev/null; then
  echo "Refusing to deploy while mobailmux-autodeploy.service is active." >&2
  echo "Stop the watcher first: sudo systemctl disable --now mobailmux-autodeploy.service" >&2
  exit 1
fi

if [[ -z "$DEPLOY_HELPER" || ! -f "$DEPLOY_HELPER" ]]; then
  echo "Set MOBAILMUX_DEPLOY_HELPER to this host's one-shot deploy helper." >&2
  exit 1
fi

if ! sudo -n true 2>/dev/null; then
  echo "Manual deploy needs passwordless sudo for install and service restart." >&2
  exit 1
fi

exec sudo -n /usr/bin/python3 "$DEPLOY_HELPER" once

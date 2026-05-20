#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

chmod 0755 "$ROOT/.githooks/pre-commit"
git -C "$ROOT" config core.hooksPath .githooks

if ! command -v gitleaks >/dev/null 2>&1; then
  cat <<'EOF' >&2
warning: gitleaks is not installed.
The pre-commit hook is installed, but commits will fail until gitleaks is available
unless you explicitly set MOBAILMUX_SKIP_GITLEAKS=1.
EOF
fi

echo "Installed Mobailmux git hooks."

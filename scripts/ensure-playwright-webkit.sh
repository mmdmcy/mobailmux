#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOL_HOME="${MOBAILMUX_PLAYWRIGHT_HOME:-$ROOT/private/playwright-webkit}"
PLAYWRIGHT_VERSION="${MOBAILMUX_PLAYWRIGHT_VERSION:-1.61.0}"
VENV="$TOOL_HOME/venv"
WHEELHOUSE="$TOOL_HOME/wheels"
BROWSERS="$TOOL_HOME/browsers"
PYTHON_BIN="${PYTHON_BIN:-python3}"
PY_DEPS=(
  "playwright==$PLAYWRIGHT_VERSION"
  "greenlet==3.5.3"
  "pyee==13.0.1"
  "typing-extensions==4.16.0"
)

if [[ "$(id -u)" == "0" ]]; then
  echo "Refusing to create the private Playwright toolchain as root." >&2
  exit 1
fi

umask 077
mkdir -p "$TOOL_HOME" "$WHEELHOUSE" "$BROWSERS"
chmod 700 "$TOOL_HOME" "$WHEELHOUSE" "$BROWSERS"

if [[ ! -x "$VENV/bin/python" ]]; then
  "$PYTHON_BIN" -m venv "$VENV"
fi

PY="$VENV/bin/python"

echo "Preparing pinned Playwright Python package: $PLAYWRIGHT_VERSION"
"$PY" -m pip download \
  --disable-pip-version-check \
  --only-binary=:all: \
  --dest "$WHEELHOUSE" \
  "${PY_DEPS[@]}"

echo "Recording wheel hashes in $TOOL_HOME/wheels.sha256"
(
  cd "$WHEELHOUSE"
  sha256sum ./*.whl | sort
) > "$TOOL_HOME/wheels.sha256.current"
if [[ -f "$TOOL_HOME/wheels.sha256.expected" ]]; then
  diff -u "$TOOL_HOME/wheels.sha256.expected" "$TOOL_HOME/wheels.sha256.current"
else
  cp "$TOOL_HOME/wheels.sha256.current" "$TOOL_HOME/wheels.sha256.expected"
fi
cp "$TOOL_HOME/wheels.sha256.current" "$TOOL_HOME/wheels.sha256"

"$PY" -m pip install \
  --disable-pip-version-check \
  --no-index \
  --find-links "$WHEELHOUSE" \
  "${PY_DEPS[@]}"

INSTALLED="$("$PY" - <<'PY'
import importlib.metadata
print(importlib.metadata.version("playwright"))
PY
)"
if [[ "$INSTALLED" != "$PLAYWRIGHT_VERSION" ]]; then
  echo "Expected Playwright $PLAYWRIGHT_VERSION but installed $INSTALLED" >&2
  exit 1
fi

echo "Installing WebKit browser into private cache: $BROWSERS"
PLAYWRIGHT_BROWSERS_PATH="$BROWSERS" "$PY" -m playwright install webkit

echo "Running private WebKit/iPhone 13 self-test"
PLAYWRIGHT_BROWSERS_PATH="$BROWSERS" "$PY" "$ROOT/scripts/smoke-iphone-webkit.py" \
  --url "data:text/html,<title>mobailmux-playwright-self-test</title><main data-agent-messages>ok</main><form class=agent-composer><textarea placeholder='Message Codex'></textarea></form>" \
  --expect "ok" \
  --expect-selector "[data-agent-messages]" \
  --expect-selector ".agent-composer" \
  --screenshot "$TOOL_HOME/self-test-iphone13-webkit.png"

cat <<EOF
Playwright WebKit is ready.

Tool home:
  $TOOL_HOME

Use:
  PLAYWRIGHT_BROWSERS_PATH="$BROWSERS" "$PY" scripts/smoke-iphone-webkit.py --url http://127.0.0.1:8765/agents
EOF

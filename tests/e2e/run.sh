#!/usr/bin/env bash
# End-to-end test of the money flow and the agent loop against local stand-ins for the PayPal sandbox
# (mock_paypal.py) and the Claude API (mock_claude.py). No real keys are used and nothing leaves the machine.
#
# Requirements: PostgreSQL reachable through DATABASE_URL (environment or .env), python3, curl, Rust.
# Usage: tests/e2e/run.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT/tests/e2e"
TMP="$(mktemp -d)"
APP_PORT="${APP_PORT:-18793}"
PAYPAL_PORT="${PAYPAL_PORT:-18792}"
CLAUDE_PORT="${CLAUDE_PORT:-18790}"

cd "$ROOT"
cargo build --quiet || exit 1

python3 -I "$HERE/mock_paypal.py" "$PAYPAL_PORT" "http://127.0.0.1:$APP_PORT" &
PAYPAL_PID=$!
python3 -I "$HERE/mock_claude.py" "$CLAUDE_PORT" "$TMP/claude_requests.jsonl" &
CLAUDE_PID=$!
# The stand-in's path contains "/sandbox": the server refuses any PayPal base URL that is not a sandbox.
PAYPAL_CLIENT_ID=test PAYPAL_CLIENT_SECRET=test PAYPAL_WEBHOOK_ID=WH-ID-MOCK \
PAYPAL_BASE_URL="http://127.0.0.1:$PAYPAL_PORT/sandbox" \
ANTHROPIC_API_KEY=test ANTHROPIC_BASE_URL="http://127.0.0.1:$CLAUDE_PORT" \
PORT="$APP_PORT" RUN_JOBS=1 "$ROOT/target/debug/misfy-dropcard" serve > "$TMP/server.log" 2>&1 &
APP_PID=$!
trap 'kill $PAYPAL_PID $CLAUDE_PID $APP_PID 2>/dev/null; true' EXIT

for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$APP_PORT/healthz" >/dev/null && break
  sleep 0.25
done

python3 -I "$HERE/scenario.py" "http://127.0.0.1:$APP_PORT" "http://127.0.0.1:$PAYPAL_PORT" "$TMP/claude_requests.jsonl"
status=$?
if [ $status -ne 0 ]; then
  echo "--- server log: $TMP/server.log"
  grep -E "ERROR|WARN" "$TMP/server.log" | tail -20
fi
exit $status

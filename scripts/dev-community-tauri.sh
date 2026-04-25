#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_DIR="$ROOT_DIR/apps/client"
FRONTEND_PORT="${CTENO_DEV_FRONTEND_PORT:-8081}"

if [ -n "${CTENO_LOG_FILE:-}" ] && [ "${CTENO_LOGGING_WRAPPED:-0}" != "1" ]; then
  mkdir -p "$(dirname "$CTENO_LOG_FILE")"
  export CTENO_LOGGING_WRAPPED=1
  exec > >(tee -a "$CTENO_LOG_FILE") 2>&1
fi

export EXPO_PUBLIC_CLOUD_SYNC_ENABLED="${EXPO_PUBLIC_CLOUD_SYNC_ENABLED:-false}"
export EXPO_PUBLIC_HAPPY_SERVER_URL="${EXPO_PUBLIC_HAPPY_SERVER_URL:-${CTENO_COMMUNITY_AUTH_SERVER_URL:-https://dev.frontfidelity.cn}}"

SIDECAR_TARGET_DIR="$ROOT_DIR/apps/client/desktop/target"
SIDECAR_BIN="$SIDECAR_TARGET_DIR/debug/cteno-agent"
case "$(uname -s 2>/dev/null)" in
  MINGW*|MSYS*) SIDECAR_BIN="$SIDECAR_TARGET_DIR/debug/cteno-agent.exe" ;;
esac

echo "🤖 构建 cteno-agent sidecar..."
CARGO_TARGET_DIR="$SIDECAR_TARGET_DIR" \
  cargo build \
    --manifest-path "$ROOT_DIR/packages/agents/rust/crates/cteno-agent-stdio/Cargo.toml" \
    --bin cteno-agent
export CTENO_AGENT_PATH="$SIDECAR_BIN"
echo "  ✓ $CTENO_AGENT_PATH"

cd "$APP_DIR"

exec ./node_modules/.bin/tauri dev \
  --config desktop/tauri.dev.conf.json \
  -- \
  --no-default-features \
  --features community

#!/bin/bash

APP_DIR="${1:?app dir is required}"
APP_LOG="${2:-/tmp/cteno.log}"
FRONTEND_PORT="${3:-8081}"
RUST_LOG_VALUE="${4:-info,multi_agent_runtime_cteno=debug}"

now() {
  date "+%Y-%m-%dT%H:%M:%S%z"
}

echo "[$(now)] launchd supervisor started pid=$$ app_dir=$APP_DIR"

cd "$APP_DIR" || exit 127
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export CTENO_DEV_FRONTEND_PORT="$FRONTEND_PORT"
export RUST_LOG="$RUST_LOG_VALUE"

yarn tauri:dev >>"$APP_LOG" 2>&1
exit_code=$?

echo "[$(now)] yarn tauri:dev exited code=$exit_code"
exit "$exit_code"

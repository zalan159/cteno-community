#!/bin/bash

MODE="${1:-tauri}"  # tauri (默认) 或 metro (仅 Metro，用于 iOS 真机调试)
ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
UNIFIED_SECRETS_FILE="$ROOT_DIR/config/unified.secrets.json"
SECRETS_PROFILE="${CTENO_SECRETS_PROFILE:-dev}"

echo "🚀 启动 Cteno (模式: ${MODE})..."
DEV_FRONTEND_PORT=8081
STARTED_PID=""

# 统一密钥同步（开发环境）
if [ -f "$UNIFIED_SECRETS_FILE" ]; then
  echo "🔐 同步密钥 (profile: ${SECRETS_PROFILE})..."
  SYNC_CMD="secrets:sync"
  if [ "$SECRETS_PROFILE" = "dev" ]; then
    SYNC_CMD="secrets:sync:dev"
  fi
  if ! (cd "$ROOT_DIR" && yarn "$SYNC_CMD" >/dev/null); then
    echo "❌ 密钥同步失败，请检查 config/unified.secrets.json"
    exit 1
  fi
fi

# 停止旧进程（包括 Expo/Metro bundler）
echo "🛑 停止旧进程..."
if [ "$MODE" = "tauri" ]; then
  pkill -9 -f "cteno|tauri" 2>/dev/null
fi
# Kill Metro/Expo bundler on port 8081
METRO_PIDS=$(lsof -ti:${DEV_FRONTEND_PORT} 2>/dev/null)
if [ -n "$METRO_PIDS" ]; then
  echo "$METRO_PIDS" | xargs kill -9 2>/dev/null
  echo "  ✓ Metro bundler (${DEV_FRONTEND_PORT}) 已终止"
else
  echo "  - 无 Metro 进程"
fi
sleep 1

# 日志轮转（保留最近 3 份旧日志）
rotate_log() {
  local log_file="$1"
  if [ -f "$log_file" ] && [ -s "$log_file" ]; then
    # 删除最旧的
    rm -f "${log_file}.3"
    # 轮转
    [ -f "${log_file}.2" ] && mv "${log_file}.2" "${log_file}.3"
    [ -f "${log_file}.1" ] && mv "${log_file}.1" "${log_file}.2"
    mv "$log_file" "${log_file}.1"
  fi
}
rotate_log /tmp/cteno.log
rotate_log /tmp/cteno-metro.log
rotate_log /tmp/cteno-supervisor.log

start_detached() {
  local log_file="$1"
  shift

  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" </dev/null >>"$log_file" 2>&1 &
  else
    nohup "$@" </dev/null >>"$log_file" 2>&1 &
  fi

  STARTED_PID=$!
  disown "$STARTED_PID" 2>/dev/null || true
}

start_supervised_tauri() {
  local app_dir="$1"
  local app_log="$2"
  local supervisor_log="$3"
  local frontend_port="$4"
  local rust_log="$5"

  start_detached "$supervisor_log" bash -lc '
app_dir="$1"
app_log="$2"
frontend_port="$3"
rust_log="$4"
child_pid=""

now() {
  date "+%Y-%m-%dT%H:%M:%S%z"
}

terminate() {
  echo "[$(now)] supervisor stopping"
  if [ -n "$child_pid" ] && kill -0 "$child_pid" 2>/dev/null; then
    kill "$child_pid" 2>/dev/null || true
    wait "$child_pid" 2>/dev/null || true
  fi
  rm -f /tmp/cteno-child.pid
  exit 0
}

trap terminate TERM INT
echo "[$(now)] supervisor started pid=$$ app_dir=$app_dir"

while true; do
  echo "[$(now)] launching yarn tauri:dev"
  (
    cd "$app_dir" || exit 127
    CTENO_DEV_FRONTEND_PORT="$frontend_port" RUST_LOG="$rust_log" yarn tauri:dev
  ) >>"$app_log" 2>&1 &
  child_pid=$!
  echo "$child_pid" > /tmp/cteno-child.pid
  wait "$child_pid"
  exit_code=$?
  rm -f /tmp/cteno-child.pid
  echo "[$(now)] yarn tauri:dev exited code=$exit_code"
  sleep 2
done
' cteno-dev-supervisor "$app_dir" "$app_log" "$frontend_port" "$rust_log"
}

ensure_pid_alive() {
  local pid="$1"
  local name="$2"
  local log_file="$3"
  local wait_secs="${4:-8}"

  sleep "$wait_secs"
  if ! ps -p "$pid" >/dev/null 2>&1; then
    echo "❌ ${name} 启动后很快退出 (pid=${pid})"
    echo "🧾 最近日志（${log_file}）："
    tail -n 60 "$log_file" 2>/dev/null || true
    return 1
  fi
  return 0
}

# ── 预构建 cteno-memory-mcp（跨 vendor 共享记忆的 MCP stdio server）──
# 产物会被 agent_sync_bridge 定位并写进每个 vendor 的 MCP 配置；dev 模式下
# 通过 CTENO_MEMORY_MCP_BIN 固化路径，避免 daemon 找不到。
MEMORY_MCP_TARGET="$ROOT_DIR/packages/host/rust/target/debug/cteno-memory-mcp"
echo "🧠 构建 cteno-memory-mcp..."
if ! (cd "$ROOT_DIR/packages/host/rust" && cargo build -p cteno-host-memory-mcp --bin cteno-memory-mcp >/tmp/cteno-memory-mcp-build.log 2>&1); then
  echo "⚠️  cteno-memory-mcp 构建失败（详见 /tmp/cteno-memory-mcp-build.log）；跨 vendor 记忆会被跳过"
else
  # Tauri sidecar-style symlink so `externalBin` lookups in dev also resolve.
  TRIPLE="$(rustc -vV 2>/dev/null | sed -n 's/host: //p')"
  if [ -n "$TRIPLE" ] && [ -f "$MEMORY_MCP_TARGET" ]; then
    ln -sf "$MEMORY_MCP_TARGET" "$ROOT_DIR/packages/host/rust/target/debug/cteno-memory-mcp-$TRIPLE" 2>/dev/null || true
  fi
  export CTENO_MEMORY_MCP_BIN="$MEMORY_MCP_TARGET"
  echo "  ✓ $CTENO_MEMORY_MCP_BIN"
fi

# 进入项目目录
cd "$ROOT_DIR/apps/client"

# 检查依赖是否已安装（独立仓库结构需要）
if [ ! -d "node_modules" ]; then
  echo "📦 首次启动，正在安装依赖..."
  yarn install
  if [ $? -ne 0 ]; then
    echo "❌ 依赖安装失败"
    exit 1
  fi
  echo "✅ 依赖安装完成"
fi

if [ "$MODE" = "metro" ]; then
  # 仅启动 Metro，用于 iOS 真机调试
  echo "🎯 启动 Metro bundler (仅前端，用于 iOS 真机调试)..."
  echo "   - Metro: http://localhost:${DEV_FRONTEND_PORT}"
  echo ""

  start_detached /tmp/cteno-metro.log npx expo start --port ${DEV_FRONTEND_PORT}
  METRO_PID="$STARTED_PID"
  echo $METRO_PID > /tmp/cteno-metro.pid

  echo "⏳ 等待 Metro 启动..."
  ensure_pid_alive "$METRO_PID" "Metro" "/tmp/cteno-metro.log" 5 || exit 1

  echo ""
  echo "✅ Metro 启动完成！"
  echo ""
  echo "📝 日志："
  echo "  - 实时日志: tail -f /tmp/cteno-metro.log"
  echo "  - 完整日志: /tmp/cteno-metro.log"
  echo ""
  echo "🛑 停止："
  echo "  kill \$(cat /tmp/cteno-metro.pid)"
  echo "  或: lsof -ti:${DEV_FRONTEND_PORT} | xargs kill -9"

else
  # 完整启动: Tauri + Metro + 后端
  echo "🎯 启动 Tauri 开发服务器（包含前后端）..."
  echo "   这会自动启动："
  echo "   - Expo 前端服务器 (http://localhost:${DEV_FRONTEND_PORT})"
  echo "   - Tauri 桌面应用窗口"
  echo ""

  # 用 shell supervisor 记录退出码，并在 tauri dev 意外退出时重启。
  # RUST_LOG：父进程默认 info；把 multi_agent_runtime_cteno 调到 debug，
  # 这样 cteno-agent subprocess 的 stderr（由父进程 log::debug! 转发）也会进 /tmp/cteno.log。
  start_supervised_tauri "$ROOT_DIR/apps/client" /tmp/cteno.log /tmp/cteno-supervisor.log \
    "${DEV_FRONTEND_PORT}" "${RUST_LOG:-info,multi_agent_runtime_cteno=debug}"
  TAURI_PID="$STARTED_PID"

  # 记录 PID 方便后续停止
  echo $TAURI_PID > /tmp/cteno.pid

  echo "⏳ 等待服务启动..."
  if [ -z "$TAURI_PID" ]; then
    echo "❌ Tauri dev supervisor 未启动"
    echo "🧾 Supervisor 日志："
    tail -n 60 /tmp/cteno-supervisor.log 2>/dev/null || true
    exit 1
  fi
  ensure_pid_alive "$TAURI_PID" "Tauri dev supervisor" "/tmp/cteno-supervisor.log" 8 || exit 1

  echo ""
  echo "✅ Cteno 启动完成！"
  echo ""
  echo "📊 服务信息："
  echo "  - 前端开发: http://localhost:${DEV_FRONTEND_PORT}"
  echo "  - Tauri 窗口: 应该已打开"
  echo ""
  echo "📝 日志："
  echo "  - 实时日志: tail -f /tmp/cteno.log"
  echo "  - 完整日志: /tmp/cteno.log"
  echo "  - Supervisor: /tmp/cteno-supervisor.log"
  echo ""
  echo "🛑 停止："
  echo "  bash stop-cteno.sh"
  echo "  或: pkill -f 'cteno-dev-supervisor|cteno|tauri'; lsof -ti:${DEV_FRONTEND_PORT} | xargs kill -9"
fi

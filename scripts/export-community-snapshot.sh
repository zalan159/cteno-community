#!/usr/bin/env bash
# 把 monorepo 当前 HEAD 的 community 范围导出到 ./community-snapshot/
# 用法：
#   ./scripts/export-community-snapshot.sh [target-dir] [--publish]
# 默认 target-dir = ./community-snapshot
# --publish: 导出后直接 init/commit/force-push 到 COMMUNITY_REMOTE（默认
#   https://github.com/zalan159/cteno-community.git，main 分支）

set -euo pipefail

PUBLISH=0
POSITIONAL=()
for arg in "$@"; do
  case "$arg" in
    --publish) PUBLISH=1 ;;
    --help|-h)
      sed -n '2,7p' "$0"
      exit 0 ;;
    *) POSITIONAL+=("$arg") ;;
  esac
done

REPO_ROOT="$(git rev-parse --show-toplevel)"
TARGET="${POSITIONAL[0]:-${REPO_ROOT}/community-snapshot}"
HEAD_COMMIT=$(git rev-parse HEAD)
COMMUNITY_REMOTE="${COMMUNITY_REMOTE:-https://github.com/zalan159/cteno-community.git}"
COMMUNITY_BRANCH="${COMMUNITY_BRANCH:-main}"

echo "=== Cteno community snapshot export ==="
echo "Source: $REPO_ROOT"
echo "Target: $TARGET"
echo "HEAD:   $HEAD_COMMIT"
echo ""

if [ -e "$TARGET" ]; then
  echo "Target exists; clearing"
  rm -rf "$TARGET"
fi
mkdir -p "$TARGET"

# 用 rsync + 白名单 + exclude。这里导出的是公开客户端 / 本地 agent runtime；
# 官方 Happy Server、landing/console、计费、生产运维材料留在私有仓。
RSYNC_EXCLUDES=(
  # Closed-source cloud service and billing surface
  --exclude='apps/happy-server/'
  --exclude='apps/landing-page/'
  --exclude='packages/commercial/'
  --exclude='commercial/'
  --exclude='landing/'
  --exclude='ops/'
  --exclude='/config/'
  --exclude='docs/archive/'
  --exclude='/archive/'
  --exclude='docs/CI-BUILD-MACHINE.md'
  --exclude='CI-BUILD-MACHINE.md'
  --exclude='docs/community-commercial-boundary.md'
  --exclude='community-commercial-boundary.md'
  --exclude='docs/openrouter-migration-plan.md'
  --exclude='openrouter-migration-plan.md'
  --exclude='docs/server-relay-refactor.md'
  --exclude='server-relay-refactor.md'
  --exclude='docs/embedding-memory-architecture.md'
  --exclude='embedding-memory-architecture.md'
  --exclude='docs/PRODUCT_SPEC.md'
  --exclude='PRODUCT_SPEC.md'
  --exclude='tests/eval/auth-*.md'
  --exclude='eval/auth-*.md'
  --exclude='tests/eval/openrouter-*.md'
  --exclude='eval/openrouter-*.md'
  --exclude='scripts/sync-unified-secrets.mjs'
  --exclude='sync-unified-secrets.mjs'
  --exclude='scripts/run-server-relay-gate.sh'
  --exclude='run-server-relay-gate.sh'
  --exclude='apps/client/app/changelog/changelog.json'
  --exclude='app/changelog/changelog.json'

  # Closed-source mobile native shells and native plugin glue. The shared RN
  # frontend stays in the community snapshot for desktop/web.
  --exclude='apps/client/ios/'
  --exclude='ios/'
  --exclude='apps/client/android/'
  --exclude='android/'
  --exclude='apps/client/modules/'
  --exclude='modules/'
  --exclude='apps/client/plugins/'
  --exclude='plugins/'

  # Production release / hosted OTA plumbing
  --exclude='apps/client/.build-release.env'
  --exclude='.build-release.env'
  --exclude='apps/client/build-release.sh'
  --exclude='build-release.sh'
  --exclude='apps/client/build-release.ps1'
  --exclude='build-release.ps1'
  --exclude='apps/client/build-ios.sh'
  --exclude='build-ios.sh'
  --exclude='apps/client/publish-ota.sh'
  --exclude='publish-ota.sh'
  --exclude='apps/client/publish-ota.ps1'
  --exclude='publish-ota.ps1'
  --exclude='apps/client/scripts/publish-ota.ts'
  --exclude='scripts/publish-ota.ts'
  --exclude='apps/client/eas.json'
  --exclude='eas.json'
  --exclude='apps/client/ios/Podfile.lock'
  --exclude='ios/Podfile.lock'
  --exclude='apps/client/desktop/test-mobile-auth-debug.sh'
  --exclude='desktop/test-mobile-auth-debug.sh'

  # Secrets and local state
  --exclude='config/unified.secrets.json'
  --exclude='config/unified.secrets.example.json'
  --exclude='.env'
  --exclude='.env.*'
  --exclude='*.env'
  --exclude='*.env.*'
  --exclude='apikeys.txt'
  --exclude='00-management/'
  --exclude='credentials.json'
  --exclude='credentials/'
  --exclude='target/'
  --exclude='node_modules/'
  --exclude='.git/'
  --exclude='.gitea/'
  --exclude='.github/'
  --exclude='.claude/'
  --exclude='.gemini/'
  --exclude='.agents/'
  --exclude='.mcp.json'
  --exclude='.cteno/'
  --exclude='Cargo.lock.bak'
  --exclude='community-snapshot/'
  --exclude='.DS_Store'
  --exclude='dist/'
  --exclude='build/'
  --exclude='.next/'
  --exclude='.expo/'
  --exclude='.turbo/'
  --exclude='tsconfig.tsbuildinfo'
)

INCLUDE_PATHS=(
  apps/client
  packages
  docs
  scripts
  tests
  third_party
  start-cteno.sh
  start-cteno.ps1
  package.json
  package-lock.json
  .gitignore
  README.md
  README.zh-CN.md
  refactor_p1_host_plan.md
  refactor_p0_wave2_plan.md
  agent_executor_plan.md
)

for p in "${INCLUDE_PATHS[@]}"; do
  src="$REPO_ROOT/$p"
  if [ ! -e "$src" ]; then
    continue
  fi
  parent_rel="$(dirname "$p")"
  if [ "$parent_rel" = "." ]; then
    dest_dir="$TARGET/"
  else
    dest_dir="$TARGET/$parent_rel/"
    mkdir -p "$dest_dir"
  fi
  if [ -d "$src" ]; then
    rsync -a "${RSYNC_EXCLUDES[@]}" "$src/" "$dest_dir$(basename "$p")/"
  else
    rsync -a "${RSYNC_EXCLUDES[@]}" "$src" "$dest_dir"
  fi
done

# The internal monorepo uses a real Cargo feature gate: desktop defaults to
# `commercial-cloud`, while `community` compiles the local-only path. Cargo still
# resolves disabled optional path dependencies during manifest loading, so the
# public snapshot must normalize its manifest after removing packages/commercial.
DESKTOP_CARGO="$TARGET/apps/client/desktop/Cargo.toml"
if [ -f "$DESKTOP_CARGO" ]; then
  perl -0pi -e 's/default = \["commercial-cloud"\]/default = ["community"]/g; s/commercial-cloud = \[\n.*?\n\]/commercial-cloud = []/s; s/^cteno-happy-client[^\n]*\n//mg' "$DESKTOP_CARGO"
fi

DESKTOP_TAURI_CONFIG="$TARGET/apps/client/desktop/tauri.conf.json"
if [ -f "$DESKTOP_TAURI_CONFIG" ]; then
  perl -0pi -e 's/,\n      "\.\.\/\.\.\/\.\.\/packages\/commercial\/rust"//g' "$DESKTOP_TAURI_CONFIG"
fi

CLIENT_APP_CONFIG="$TARGET/apps/client/app.config.js"
if [ -f "$CLIENT_APP_CONFIG" ]; then
  perl -0pi -e 's/\n\s*require\("\.\/plugins\/withEinkCompatibility\.js"\),//g; s/\n\s*require\("\.\/plugins\/withRemoveMotionPermission\.js"\),//g' "$CLIENT_APP_CONFIG"
fi

# 写一个 SNAPSHOT_META 标记导出时间和源 commit
cat > "$TARGET/.snapshot-meta.json" <<EOF
{
  "source_repo": "(internal monorepo)",
  "source_commit": "$HEAD_COMMIT",
  "exported_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "exported_by": "scripts/export-community-snapshot.sh",
  "edition": "community",
  "desktop_cargo_features": "--no-default-features --features community",
  "note": "Auto-exported from internal monorepo. Official Cteno Cloud / Happy Server, landing/console, billing, hosted sync, and production operations are NOT included."
}
EOF

assert_absent_path() {
  local path="$1"
  local label="$2"
  if [ -e "$path" ]; then
    echo "ERROR: $label leaked into snapshot: $path"
    exit 1
  fi
}

assert_no_match() {
  local pattern="$1"
  local label="$2"
  if grep -rI -E "$pattern" "$TARGET" 2>/dev/null | grep -v "/scripts/export-community-snapshot.sh" | grep -q .; then
    echo "ERROR: $label marker found in snapshot:"
    grep -rI -E "$pattern" "$TARGET" 2>/dev/null | grep -v "/scripts/export-community-snapshot.sh" | head -20
    exit 1
  fi
}

# Sanity check：确认官方云服务、landing、计费与运维材料没漏出来
assert_absent_path "$TARGET/apps/happy-server" "Happy Server"
assert_absent_path "$TARGET/apps/landing-page" "Landing page"
assert_absent_path "$TARGET/packages/commercial" "commercial client crates"
assert_absent_path "$TARGET/landing" "legacy landing page"
assert_absent_path "$TARGET/ops" "production ops scripts"
assert_absent_path "$TARGET/config" "production config"
assert_absent_path "$TARGET/docs/archive" "internal archived plans"
assert_absent_path "$TARGET/CLAUDE.md" "internal agent instructions"
assert_absent_path "$TARGET/apps/client/app/changelog/changelog.json" "internal release notes"
assert_absent_path "$TARGET/apps/client/ios" "iOS native project"
assert_absent_path "$TARGET/apps/client/android" "Android native project"
assert_absent_path "$TARGET/apps/client/modules" "mobile native modules"
assert_absent_path "$TARGET/apps/client/plugins" "mobile native plugins"

if grep -q 'packages/commercial\|cteno-happy-client' "$DESKTOP_CARGO" 2>/dev/null; then
  echo "ERROR: commercial Cargo dependency leaked into desktop community manifest:"
  grep -n 'packages/commercial\|cteno-happy-client' "$DESKTOP_CARGO" | head -20
  exit 1
fi
if grep -q 'packages/commercial' "$DESKTOP_TAURI_CONFIG" 2>/dev/null; then
  echo "ERROR: private bundle config leaked into desktop community Tauri config:"
  grep -n 'packages/commercial' "$DESKTOP_TAURI_CONFIG" | head -20
  exit 1
fi
if grep -q './plugins/' "$CLIENT_APP_CONFIG" 2>/dev/null; then
  echo "ERROR: native config plugin leaked into community app config:"
  grep -n './plugins/' "$CLIENT_APP_CONFIG" | head -20
  exit 1
fi

# Sanity check：密钥文件不该出现
for forbidden in \
  "$TARGET/config/unified.secrets.json" \
  "$TARGET/config/unified.secrets.example.json" \
  "$TARGET/apikeys.txt" \
  "$TARGET/.env" \
  "$TARGET/apps/client/.build-release.env"; do
  assert_absent_path "$forbidden" "forbidden file"
done

assert_absent_path "$TARGET/00-management" "task-gate/internal management state"
assert_absent_path "$TARGET/apps/client/scripts/publish-ota.ts" "hosted OTA publish script"

assert_no_match 'PaymentOrder|BalanceLedger|ALIPAY_|OPENROUTER_MANAGEMENT_KEY|OFOX_API_KEY|WECHAT_APP_SECRET|REVENUE_CAT|RevenueCat' "billing/server secret"
assert_no_match '免费模型余额|OTA pipeline|server manifest|Secure end-to-end encrypted communication via Happy Server|版本说明上传|上传版本说明' "internal release note"

if grep -rI "ESCROW_KEY" "$TARGET" 2>/dev/null \
  | grep -v "/docs/" \
  | grep -v "ESCROW_KEY配置在" \
  | grep -v "scripts/export-community-snapshot.sh" \
  | grep -q .; then
  echo "WARN: ESCROW_KEY mentioned in snapshot (might be docs/config). Review."
fi

# 总文件数
FILE_COUNT=$(find "$TARGET" -type f | wc -l | tr -d ' ')
SIZE=$(du -sh "$TARGET" | cut -f1)
echo ""
echo "Snapshot ready: $TARGET"
echo "  Files: $FILE_COUNT"
echo "  Size:  $SIZE"
echo ""

if [ "$PUBLISH" = "1" ]; then
  echo "=== Publishing to $COMMUNITY_REMOTE ($COMMUNITY_BRANCH) ==="
  cd "$TARGET"
  git init -b "$COMMUNITY_BRANCH" -q
  git add .
  git -c user.email="$(git -C "$REPO_ROOT" config user.email)" \
      -c user.name="$(git -C "$REPO_ROOT" config user.name)" \
      commit -q -m "Snapshot from monorepo $HEAD_COMMIT"
  git remote add origin "$COMMUNITY_REMOTE"
  git push --force origin "$COMMUNITY_BRANCH"
  echo "Published $HEAD_COMMIT to $COMMUNITY_REMOTE ($COMMUNITY_BRANCH)"
else
  echo "Next steps (manual) — or rerun with --publish to auto-push:"
  echo "  1. cd $TARGET"
  echo "  2. git init && git add . && git commit -m 'Snapshot from monorepo HEAD: $HEAD_COMMIT'"
  echo "  3. git remote add origin $COMMUNITY_REMOTE"
  echo "  4. git push -u origin $COMMUNITY_BRANCH"
fi

#!/bin/bash
# Cteno E2E Test Helpers v2
#
# 设计原则：能脚本判的绝不交给 AI
# 验证优先级：store query > DOM 断言 > 可访问性树 > 截图存档
#
# 用法：source tests/e2e/browser-helpers.sh

set -euo pipefail

# ============================================================
# 基础设施
# ============================================================

e2e_check() {
    if ! ctenoctl status &>/dev/null; then
        echo '{"ok":false,"error":"daemon not running"}'
        return 1
    fi
    echo '{"ok":true}'
}

# 在 webview 内执行 JS，返回 JSON
e2e_eval() {
    ctenoctl webview eval "$1" 2>/dev/null
}

# 在 webview 内执行 JS，要求返回 JSON 字符串（自动 parse 验证）
e2e_json() {
    local result
    result=$(e2e_eval "JSON.stringify($1)")
    echo "$result"
}

e2e_screenshot() {
    ctenoctl webview screenshot "${@}"
}

# ============================================================
# Store 查询（Layer 1 — 数据是否到了前端）
# ============================================================

# 列出所有 session
e2e_sessions() {
    e2e_json "window.__e2e.sessions()"
}

# 读指定 session 的消息
e2e_messages() {
    local session_id="$1"
    e2e_json "window.__e2e.messages('$session_id')"
}

# 列出所有 persona
e2e_personas() {
    e2e_json "window.__e2e.personas()"
}

# ============================================================
# DOM 断言（Layer 2 — React 是否正确渲染了）
# ============================================================

# 断言元素可见
e2e_assert_visible() {
    local selector="$1"
    local text="${2:-}"
    local result
    if [ -n "$text" ]; then
        result=$(e2e_json "window.__e2e.assertVisible('$selector', '$text')")
    else
        result=$(e2e_json "window.__e2e.assertVisible('$selector')")
    fi
    local ok
    ok=$(echo "$result" | python3 -c "import sys,json; print(json.load(sys.stdin).get('ok',False))" 2>/dev/null)
    if [ "$ok" = "True" ]; then
        return 0
    else
        echo "FAIL: $result" >&2
        return 1
    fi
}

# 断言元素不可见/不存在
e2e_assert_hidden() {
    local selector="$1"
    local result
    result=$(e2e_json "window.__e2e.assertHidden('$selector')")
    local ok
    ok=$(echo "$result" | python3 -c "import sys,json; print(json.load(sys.stdin).get('ok',False))" 2>/dev/null)
    [ "$ok" = "True" ]
}

# 获取可见文本列表
e2e_visible_texts() {
    local selector="$1"
    e2e_json "window.__e2e.visibleTexts('$selector')"
}

# 元素计数
e2e_count() {
    local selector="$1"
    e2e_eval "window.__e2e.count('$selector')"
}

# 获取 UI 快照（可访问性树）
e2e_snapshot() {
    e2e_json "window.__e2e.snapshot()"
}

# ============================================================
# 等待（Layer 1+2 — 异步时序处理）
# ============================================================

# 等待 session 有回复（store 级别）
e2e_wait_response() {
    local session_id="$1"
    local timeout="${2:-60000}"
    e2e_json "await window.__e2e.waitForResponse('$session_id', $timeout)"
}

# 等待 DOM 元素出现
e2e_wait_element() {
    local selector="$1"
    local timeout="${2:-15000}"
    e2e_json "await window.__e2e.waitForElement('$selector', $timeout)"
}

# ============================================================
# 操作（和 UI 走同一条代码路径）
# ============================================================

# 创建 persona
e2e_create_persona() {
    local vendor="$1"
    local workdir="${2:-~/}"
    e2e_json "await window.__e2e.createPersona('$vendor', '$workdir')"
}

# 发消息
e2e_send_message() {
    local session_id="$1"
    local text="$2"
    e2e_json "await window.__e2e.sendMessage('$session_id', '$text')"
}

# 导航
e2e_navigate() {
    local path="$1"
    e2e_json "window.__e2e.navigate('$path')"
}

# ============================================================
# 高级组合（一步到位端到端验证）
# ============================================================

# 测试单个 vendor 完整链路
e2e_test_vendor() {
    local vendor="$1"
    local message="${2:-say hello in one word}"
    local timeout="${3:-60000}"
    e2e_json "await window.__e2e.testVendor('$vendor', '$message', $timeout)"
}

# 测试所有可用 vendor
e2e_test_all_vendors() {
    local message="${1:-say hello in one word}"
    e2e_json "await window.__e2e.testAllVendors('$message')"
}

# ============================================================
# 报告（Agent 友好的结构化输出）
# ============================================================

# 完整 E2E 健康检查
e2e_health() {
    local daemon_ok vendors sessions
    daemon_ok=$(e2e_check)
    if [ $? -ne 0 ]; then echo "$daemon_ok"; return 1; fi

    vendors=$(e2e_json "window.__e2e.vendors()" 2>/dev/null || echo '{"error":"driver not installed"}')
    sessions=$(e2e_json "window.__e2e.sessions()" 2>/dev/null || echo '[]')
    personas=$(e2e_json "window.__e2e.personas()" 2>/dev/null || echo '[]')

    python3 -c "
import json, sys
print(json.dumps({
    'daemon': True,
    'driverInstalled': 'error' not in '''$vendors''',
    'vendors': json.loads('''$vendors'''),
    'sessionCount': len(json.loads('''$sessions''')),
    'personaCount': len(json.loads('''$personas''')),
}, indent=2))
"
}

echo '[E2E] Helpers v2 loaded. Key commands:'
echo '  e2e_health                         — 系统健康检查'
echo '  e2e_test_vendor claude             — 单 vendor 端到端测试'
echo '  e2e_test_all_vendors               — 全 vendor 测试'
echo '  e2e_sessions / e2e_personas        — 查看 store 状态'
echo '  e2e_assert_visible "selector"      — DOM 可见性断言'
echo '  e2e_snapshot                        — UI 可访问性树快照'

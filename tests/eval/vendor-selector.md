# Vendor Selector — discovery / auth state matrix

验证 vendor selector 及其相邻入口在 discovery/auth 流上的状态边界是否清晰一致：只看安装状态、登录状态、可执行动作入口，以及登录变化后的自动刷新。

本文件只覆盖 selector 前后的入口语义，不验证运行中 session 的 model / permission hotswap，也不验证 host / injected tool 的展示。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-vendor-selector
- max-turns: 18

## setup

```bash
rm -rf /tmp/cteno-vendor-selector
mkdir -p /tmp/cteno-vendor-selector
printf 'vendor selector eval fixture\n' > /tmp/cteno-vendor-selector/context.txt
# 如走桌面端真机路径，先确认:
# - `list_available_vendors` 可返回 `{vendors:[...]}`
# - `cteno_auth_get_snapshot` / `cteno_auth_save_credentials` / `cteno_auth_clear_credentials` 已注册
```

## state matrix

| state | source of truth | expected UI / behavior | primary action |
| --- | --- | --- | --- |
| `installed` | `list_available_vendors` / `VendorMeta.available` | 已安装 vendor 可点击；未安装项灰显并显示 `not installed` / `Not installed` | 选择 vendor；或走 `View install instructions` |
| `logged-in` | `cteno_auth_get_snapshot().isLoggedIn` / `AuthStore` | 已登录时 host 持有 Happy auth；未登录时维持 community mode | 登录、登出、等待 refresh |
| `ready` | 安装状态 + 当前 auth 前置条件 | 满足前置条件的路径可直接进入下一步；不满足时给明确 gate，而不是 silent fallback | 打开 selector、创建 session、进入 cloud-only/profile 入口 |
| `auto-refresh` | `saveCredentials` / refresh guard / `auth-require-login` | 登录、token 轮换、被迫重新登录后，selector 相邻入口无需重启 app 即更新 | 重新打开 selector 或重试同一路径 |

## out-of-scope

- runtime `set_model` / `set_permission_mode` / restart-resume 语义
- 跨 vendor hotswap 或 selector 内“切完马上热迁移已有 session”
- host-owned / injected tool 的 tool-call UI、`Host Tool` badge、tool-result 归并
- Claude / Codex / Gemini 各自 CLI 内部的登录向导或浏览器授权细节

## cases

### [skip: 2026-04-18 QA 无可连接 daemon/webview，无法执行 live VendorSelector 渲染与加载态验证] 初次打开 selector 时，先显示 discovery 加载态，再落已安装矩阵
- **message**: 在新建 session 或 persona 入口打开 `VendorSelector`，让 `listAvailableVendors(machineId)` 首次走异步加载
- **expect**: 请求未返回前显示 `Detecting installed CLIs…` / `Detecting…`；返回后稳定渲染 `cteno / claude / codex / gemini` 列表；缺失 CLI 的项显示 `not installed` 或 `Not installed`，不会直接消失
- **anti-pattern**: 未加载完成就把空列表当最终结果；缺失 vendor 被整个隐藏；loading 结束后列表闪回 mock/旧值
- **severity**: medium

### [skip: 2026-04-18 QA 无可连接 daemon/webview，无法执行 live selector 与 setup wizard 点击验证] `installed` 状态直接决定 selector 行是否可点，未安装项只能走安装入口
- **message**: 让 `list_available_vendors` 返回一个混合矩阵，例如 `cteno=true`、`claude=true`、`codex=false`、`gemini=false`；依次点击各行，并在 setup wizard 查看辅助入口
- **expect**: `available=true` 的行可选中并触发 `onChange(vendor)`；`available=false` 的行灰显、点击无效；setup wizard 中同一矩阵显示 `Installed` / `Not installed`，并保留 `View install instructions` 作为唯一安装动作入口
- **anti-pattern**: 未安装 vendor 仍可被选中；selector 和 setup wizard 对同一 vendor 的安装状态不一致；安装入口缺失，只剩无反馈的 disabled 行
- **severity**: high

### [skip: 2026-04-18 QA 无可连接 daemon/AuthStore，无法执行 live community/auth gate 分流验证] 未登录时保持 community mode，但 cloud-only 路径必须给出明确 auth gate
- **message**: 确认 `cteno_auth_get_snapshot().isLoggedIn=false`，然后分别测试两类路径：1) 直接选择 `cteno` 发起本地 session；2) 选择依赖 Happy auth 的 cloud-only/profile 路径
- **expect**: 本地 `cteno` 路径仍可用，不要求先登录；依赖 Happy auth 的路径返回明确 gate，例如 `proxy profiles require logged-in Happy Server auth` 或等价提示，而不是静默降级成别的 vendor / profile
- **anti-pattern**: 未登录时整个 selector 被误锁死；cloud-only 路径偷偷 fallback 到本地模式且无提示；错误只写日志不反馈到调用方
- **severity**: high

### [skip: 2026-04-18 QA 无可连接 daemon/AuthStore，无法执行 saveCredentials→cteno_auth_save_credentials live 验证] 登录动作写入 AuthStore 后，原先被 auth gate 的路径变为 ready
- **message**: 先在未登录状态复现一次 auth gate；随后通过浏览器 OAuth / email 登录或测试钩，触发 `saveCredentials` → `cteno_auth_save_credentials`，再重新打开 selector 或重试同一路径
- **expect**: `cteno_auth_get_snapshot().isLoggedIn=true`；新开的 Cteno session 会把 `auth` 块注入 `agent_config`；刚才被 gate 的 cloud-only/profile 入口现在可继续，不再报 “require logged-in Happy Server auth”
- **anti-pattern**: JS 侧已登录但 daemon `AuthStore` 未更新；必须重启 app/daemon 才解锁；登录后 selector 仍展示旧的 blocked 行为
- **severity**: high

### [skip: 2026-04-18 QA 无可连接 daemon/AuthStore，无法执行 ensureFreshAccess/refresh guard live 验证] token 自动刷新或 host 侧 token 轮换后，ready 路径无需手动重登
- **message**: 在已登录状态让 access token 接近过期，触发 `ensureFreshAccess` 或桌面 refresh guard 的一次成功轮换；随后继续走同一个 selector 下游动作，例如新开一个 `cteno` session 或重试同一个 cloud-only profile
- **expect**: 新 token 通过 `saveCredentials` / `AuthStore::set_tokens` 落盘并同步到 daemon；Cteno 侧能收到刷新后的 token（必要时经 `broadcast_token_refresh`）；用户不需要先手动 logout/login，ready 路径持续可用
- **anti-pattern**: refresh 成功但 selector 下游仍拿旧 token；下一次动作立刻 401；只有重启后才恢复 ready
- **severity**: medium

### [skip: 2026-04-18 QA 无可连接 daemon/AuthStore，无法执行 clearCredentials/auth-require-login live 验证] refresh 终态失败或显式登出后，selector 相邻入口自动回到 not-logged-in 状态
- **message**: 模拟 refresh token 被撤销，或显式执行 `clearCredentials` / `cteno_auth_clear_credentials`；随后重新打开 selector 或重试刚才的 cloud-only 路径
- **expect**: daemon 清空 `AuthStore` 并发出 `auth-require-login`；需要 Happy auth 的路径重新显示登录 gate；本地 community-mode 路径仍可继续；整个过程不要求手动清缓存或重启应用
- **anti-pattern**: UI 继续把 cloud-only 路径当 ready；登出后旧 token 还能继续被注入新 session；必须重启才能看到 not-logged-in 状态
- **severity**: high

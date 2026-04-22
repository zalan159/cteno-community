# VendorSelector 连接状态 badge

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-vendor-badge
- max-turns: 10

## setup
```bash
mkdir -p /tmp/cteno-vendor-badge
```

## cases

### [pending] 启动 1.5s 后 cteno/codex/gemini 显示 connected 徽章
- **message**: "打开创建 persona 的面板，等待 2s 后读取 vendor 选择器里 cteno/codex/gemini 三家的 badge 文案。"
- **expect**: 三家都显示 "Ready" 或 "Connected"（latency 附加 " · Xms" 可选）；store 里 `resolveVendorMeta` 返回的 `status.connection.state` 为 `connected`；每家的 `checkedAtUnixMs > 0` 且 `latencyMs` 是正整数
- **anti-pattern**: 1.5s 后仍停留在 `Connecting…` / Unknown；`checkedAtUnixMs === 0`；UI 阻止选择 (canSelect=false)
- **severity**: high

### [pending] Claude 在支持 1:1 模式下不显示 failed
- **message**: "读取 claude 行的 badge；store 里 capabilities.supportsMultiSessionPerProcess。"
- **expect**: badge `Ready`（connection.state 为 `connected`，因 `open_connection` 仅做 version check / `--help` 探测）；capabilities.supportsMultiSessionPerProcess=false；canSelect=true
- **anti-pattern**: badge 显示 `Connection failed` 或 UI 阻止选择 (canSelect=false)；probing 一直不结束
- **severity**: high

### [pending] 连接失败的 vendor 显示 Retry 按钮并可重试
- **setup addendum**: 把 CODEX_PATH 指向一个 `sleep 60` 的脚本（或用一个不存在的路径），确保 `open_connection` 在 spawn_timeout 或 spawn error 路径失败
- **message**: "观察 codex 行的 badge、detail、actions；点击 Retry，RPC 调用返回新的 VendorConnectionMeta，UI 是否刷新为新的 state。"
- **expect**:
    - badge 文案 `Connection failed`，tone=warning
    - detail 含 daemon 返回的 `reason`（timeout/spawn failure 类 message），不是硬编码的 fallback
    - Retry 按钮存在（`action.id === 'retry-probe'`, variant=primary）
    - 点击 Retry 触发 `probe_vendor_connection` RPC，payload `{ vendor: 'codex' }`
    - RPC 返回后 UI 立即 reflow 到新 state（如果仍失败继续显示 failed；若已变 connected 则切 Ready）
- **anti-pattern**: Retry 按钮缺失 / 点击无反应 / RPC 未发出；Retry 后 state 依旧；detail 写死 "Connection check failed."
- **severity**: high

### [pending] probing 状态不阻止选择
- **message**: "mock 一个 VendorMeta，其 `status.connection.state='probing'`；VendorSelector 渲染后的 badge、canSelect、actions。"
- **expect**: badge `Connecting…`；tone=muted；canSelect=**true**（spawn 层自己排队），actions=[]
- **anti-pattern**: probing 状态阻止点击（canSelect=false）；显示 Retry 按钮（probing 还没失败）；tone=warning
- **severity**: medium

### [pending] 缺少 connection 字段的旧 payload 回落 unknown 且不 regress
- **message**: "mock 一个 VendorMeta，不带 status.connection；resolveVendorMeta 后的 connection.state；VendorSelector 渲染行为。"
- **expect**: `state='unknown'`；`checkedAtUnixMs=0`；UI 行为跟 Phase 2 之前一致（installed + loggedIn → Ready / loggedOut → Login required / !installed → Not installed）
- **anti-pattern**: undefined 字段导致 UI 崩溃 / ReferenceError；全部显示 `Connection failed`；强制走 Connecting… 分支
- **severity**: high

### [pending] 部署后 deferred refresh 能拿到 preheat 的 connected 状态
- **message**: "挂起 daemon 的 preheat 约 1s（backend 在 `open_connection` 里加 1s sleep），打开 VendorSelector，观察 1.2s 后的状态变化。"
- **expect**: 初次 fetch 返回 probing → 1.2s 后第二次 fetch 到 connected；UI 从 `Connecting…` 自动切到 `Ready`，无需手动 Retry
- **anti-pattern**: 1.2s timer 未触发 / 永远停留在 Connecting…；timer 触发但 state 未更新；component unmount 后 timer 仍在跑（setState on unmounted component）
- **severity**: medium

### [pending] 切换 machineId 后应重新 probe
- **message**: "在两个 machineId 之间切换，观察 VendorSelector 是否对新 machine 重新 fetch（两次，含 deferred）"
- **expect**: 换 machineId 触发 useEffect cleanup，旧的 setTimeout 被 clearTimeout；新 machineId 的 probing → connected 独立进行
- **anti-pattern**: 旧 machineId 的 timer 仍写入新 machine 的 state；看到错位的 "connected from previous machine"
- **severity**: low

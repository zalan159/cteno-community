# 模型选择器对接经验

本文总结 `apps/client` 与 desktop runtime 在模型选择器上的接入经验，重点覆盖 Claude Code / Codex / Gemini / Cteno 四类 agent 的统一模型语义、默认值、热切换与排障方式。

## 1. 对外统一成 `model`，不要再暴露 `profile`

这次接入里最核心的经验是：

- `profile` 只适合继续留在 Cteno agent / BYOK 内部，当作本机 LLM 配置容器
- 前端 selector、persona/session 元数据、host RPC、runtime 切换能力，对外都应该统一暴露为 `model` / `modelId`
- Claude / Codex / Gemini 的原生模型 id 不是本机 profile，不能走 profile store 解析

因此模型选择器的公共链路应遵循：

- UI 只收 `models: ModelOptionDisplay[]`
- 当前值只看 `modelId`
- 默认值只看当前 vendor 的默认 `modelId`
- RPC 使用 `set-model` / `switch-session-model`

如果把外部概念继续写成 `profile`，最容易出现的回归就是：

- selector 展示出旧 BYOK profile
- 新建 session 时错误回落到机器默认 profile
- Claude/Codex 传入原生 model id 后，被 profile store 错误覆盖

## 2. 列表来源必须按 vendor 拉取

模型列表不能只靠本机 profile store 推导，必须根据当前 agent vendor 走不同来源：

- `claude`: 走 Claude Code SDK / CLI 可枚举的 supported models
- `codex`: 走 Codex app-server / CLI 暴露的 model list
- `gemini`: 走 Gemini runtime 的原生模型来源
- `cteno`: 走 app server 公共 proxy model 列表，再与本机内部能力做必要合并

经验上要避免两种错误：

1. 先拿到本机 profile，再“猜”哪些能给 Claude/Codex 用
2. 把 app server 返回的模型又映射回本地旧 profile 名称

正确做法是：

- app server / vendor SDK 拉到什么模型，selector 就显示什么模型
- 本机 profile 仅作为 Cteno/BYOK 内部解析层，不参与 Claude/Codex/Gemini 对外展示语义

关键前端入口：

- `apps/client/app/sync/ops.ts`
- `apps/client/app/app/(app)/persona/[id].tsx`
- `apps/client/app/app/(app)/session/[id]/info.tsx`

## 3. 默认模型必须是“按 vendor 缓存”的

新建 persona / session 时，如果前端还没显式选模型，就必须先解析当前 vendor 的默认模型，否则很容易掉回机器默认 profile，例如：

- Claude 新建会话未选模型，但后端回落到 `deepseek-reasoner`
- Codex 新建会话未选模型，但 session 启动仍用旧 profile 默认值

建议固定成下面这套策略：

- 以 `machineId + vendor` 为 key 缓存模型列表与默认模型
- 新建前先预取当前 vendor 的模型列表
- 如果用户未显式选择，直接使用该 vendor 的 `defaultModelId`
- 前端与后端都要做兜底，不能只信任一侧

换句话说，默认值的责任不是“某台机器的默认 profile”，而是“当前 vendor 的默认 model”。

## 4. 热切换能力要区分“原生模型”与“本机 profile”

模型热切换这条链最容易出问题的地方，是 runtime 把 `modelId` 当成 profile id 去查本地 profile store。

实际经验：

- Claude/Codex/Gemini 的原生 `modelId`，需要优先保留为 vendor-native model
- 只有 Cteno/BYOK 路径，才需要继续解析本机 profile store
- spawn 路径和 live switch 路径都要统一这个规则，不能只修其中一条

否则会出现两类典型故障：

1. 新建时没问题，切模型时错
2. 切模型时没问题，发首条消息时 turn preparation/execution 又掉回旧 profile

关键 runtime 入口：

- `apps/client/desktop/src/happy_client/session/spawn.rs`
- `apps/client/desktop/src/happy_client/session/connection.rs`
- `apps/client/desktop/src/happy_client/session/execution.rs`

## 5. effort 选项不能全 vendor 共用一套

`reasoning effort` 必须跟着当前模型能力走，不能对 Claude/Codex/Gemini 共用同一组固定选项。

应遵循：

- effort selector 放在模型 selector 旁边
- 当前模型返回哪些 `supportedReasoningEfforts`，就显示哪些
- 未返回时再降级到 vendor 的合理默认集合

否则用户会看到：

- Claude 和 Codex 显示完全一样的 effort 选项
- 切模型后 effort 可选项没有变化
- 已选 effort 不被当前模型支持，运行时报错

## 6. 日志要打在真正会进 `cteno.log` 的链路上

模型选择器问题很多时候不是“没拉到”，而是：

- fetch 拉对了，state 被旧值覆盖
- modal 收到新值了，但渲染层仍在读旧 props
- 前端传对了，runtime 又把它解析回旧 profile

排障时最有效的办法是把日志打在这几层：

- `machineListModels`
- persona/session 页面拿到的 models state
- 点开 selector 前传给 modal 的 props
- 列表组件最终渲染的 items
- runtime 的 spawn / switch / execution model resolution

本地启动脚本会把前后端日志汇到 `cteno.log`，所以优先看 `start-cteno.sh` 里定义的日志位置，再让前端通过统一日志入口写进去。

## 7. 图标与显示语义也应该跟着 vendor 走

模型选择器和 session/persona 的图标，应该表达当前 agent vendor，而不是旧 profile 类型。

建议：

- Claude → `icon-claude.png`
- Codex → `icon-gpt.png`
- Gemini → `icon-gemini.png`
- Cteno → `icon.png`

共享 vendor icon 映射，避免 selector、avatar overlay、new session wizard 各自维护一套。

## 8. 一份可执行的对接检查单

每次改模型选择器时，至少检查下面这些点：

1. 列表是否直接来源于当前 vendor 的真实模型枚举接口
2. selector 对外是否只暴露 `modelId`
3. persona/session 当前值是否仍偷偷读 `profileId`
4. 默认模型是否按 `machineId + vendor` 缓存
5. spawn / live switch / turn preparation 是否都保留 vendor-native model
6. effort 是否根据当前模型动态过滤
7. 前端日志是否已经接到 `cteno.log`
8. 图标是否按 vendor 显示，而不是按旧 profile 显示

## 9. 推荐阅读

- `CLAUDE.md`
- `docs/claude-permission-integration.md`
- `apps/client/app/sync/ops.ts`
- `apps/client/app/components/VendorSelector.tsx`
- `apps/client/app/components/LlmProfileList.tsx`
- `apps/client/desktop/src/happy_client/session/spawn.rs`
- `apps/client/desktop/src/happy_client/session/connection.rs`
- `apps/client/desktop/src/happy_client/session/execution.rs`

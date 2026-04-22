# Multi-Agent Runtime

[English](./README.md) | [简体中文](./README.zh-CN.md)

A small, extraction-friendly runtime for running role-based multi-agent workspaces on top of the official Claude Agent SDK and Codex SDK.

This package gives us one unified protocol for:
- defining role agents and coordinator roles
- accepting a workspace-level user turn and routing it across the team
- modeling workspace members, public activity, and claim policies
- declaring template-level workflows with stages, nodes, edges, artifacts, and completion rules
- observing task lifecycle as events
- validating generated artifacts in end-to-end tests

The repository now contains:
- a TypeScript reference implementation
- a Rust protocol/core implementation intended for embedding into Cteno

The current adapters are:
- `ClaudeAgentWorkspace` for Anthropic Claude Agent SDK
- `CodexSdkWorkspace` for OpenAI Codex SDK

The protocol is intentionally generic enough to support a future Cteno-native adapter.

## What It Does

`@cteno/multi-agent-runtime` treats a workspace as:
- one coordinator role
- multiple named workspace members
- explicit role dispatches such as `prd`, `finance`, or `scout`
- optional claim-based assignment rules
- a unified event stream for `workspace`, `member`, `activity`, `dispatch`, `message`, `tool.progress`, and `result`

That maps well onto Claude's session-centric model while keeping our authoring model role-centric.

## Included Templates

### `Coding Studio`
A software delivery workspace.

Roles:
- `pm`
- `prd`
- `architect`
- `coder`
- `tester`
- `reviewer`

Typical outcome:
- PRDs
- implementation notes
- code changes
- test reports

### `OPC Solo Company`
A one-person company staffed by specialist digital operators.

Roles:
- `ceo`
- `finance`
- `tax`
- `admin`
- `recruiter`

Typical outcome:
- operating checklists
- finance docs
- tax prep handoff notes
- admin SOPs

### `Autoresearch`
A research-oriented workspace for scouting, synthesis, and lightweight experiment framing.

Roles:
- `lead`
- `scout`
- `experimenter`
- `critic`

Typical outcome:
- sourced briefs
- comparison notes
- experiment outlines
- research critiques

### `Three Departments Six Ministries`
A governance-style multi-agent template for planning, review, execution, oversight, and release coordination.

Roles include:
- `shangshu`
- `zhongshu`
- `menxia`
- `gongbu`
- `hubu`
- `libu`
- `xingbu`
- `bingbu`

## Install

```bash
npm install @anthropic-ai/claude-agent-sdk @openai/codex-sdk
```

This package currently assumes:
- Node `>=20`
- a working Claude Code / Claude Agent SDK environment
- a working Codex CLI / Codex SDK environment
- local Claude authentication already configured on the machine running the tests
- local Codex authentication already configured on the machine running the tests

## Quick Start

```ts
import {
  ClaudeAgentWorkspace,
  createClaudeWorkspaceProfile,
  createCodingStudioTemplate,
  instantiateWorkspace,
} from '@cteno/multi-agent-runtime';

const workspace = new ClaudeAgentWorkspace({
  spec: instantiateWorkspace(
    createCodingStudioTemplate(),
    {
      id: 'demo-coding-studio',
      name: 'Demo Coding Studio',
      cwd: process.cwd(),
    },
    createClaudeWorkspaceProfile(),
  ),
});

workspace.onEvent(event => {
  console.log(event.type, event);
});

await workspace.start();

const turn = await workspace.runWorkspaceTurn({
  message:
    'We need a short PRD for a group-chat mention feature. Please create it at 10-prd/group-mentions.md with sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria.',
  workflowEntry: 'direct',
});

console.log(turn.plan);
console.log(turn.dispatches[0]?.status);
console.log(turn.dispatches[0]?.resultText);
await workspace.close();
```

### Codex SDK Example

```ts
import {
  CodexSdkWorkspace,
  createCodingStudioTemplate,
  createCodexWorkspaceProfile,
  instantiateWorkspace,
} from '@cteno/multi-agent-runtime';

const workspace = new CodexSdkWorkspace({
  spec: instantiateWorkspace(
    createCodingStudioTemplate(),
    {
      id: 'demo-codex-coding-studio',
      name: 'Demo Codex Coding Studio',
      cwd: process.cwd(),
    },
    createCodexWorkspaceProfile({
      model: 'gpt-5.1-codex-mini',
    }),
  ),
  skipGitRepoCheck: true,
  approvalPolicy: 'never',
  sandboxMode: 'workspace-write',
});

await workspace.start();
const turn = await workspace.runWorkspaceTurn({
  message:
    'We need a short PRD for a group-chat mention feature. Please create it at 10-prd/group-mentions.md with sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria.',
  workflowEntry: 'direct',
});

console.log(turn.plan);
console.log(turn.dispatches[0]?.status);
console.log(turn.dispatches[0]?.resultText);
await workspace.close();
```

## Hybrid Claude + Codex Pattern

The runtime now supports role-level provider mixing through `HybridWorkspace`.

Use this when you want one workspace where:
- Claude-owned roles handle planning, coordination, or status tracking
- Codex-owned roles handle coding and validation
- the event stream still looks like one workspace

The key requirement is:
- run a spec whose resolved providers span more than one provider
- set explicit `provider` only where a role or workflow node differs from the workspace default

Resolution order is now:
- template-level default provider/model
- profile-level override of the template default
- role-level provider/model override
- workflow-node-level provider/model override for dispatches created from that node

If the final resolved providers span more than one provider, `instantiateWorkspace()` now infers `spec.provider = 'hybrid'`.

There is a reference TypeScript example at [`src/examples/hybridClaudeCodex.ts`](./src/examples/hybridClaudeCodex.ts), runnable with:

```bash
npm run smoke:hybrid -- /absolute/workspace/path "your task here"
```

## Runtime API

### `runWorkspaceTurn()`
Takes a single workspace-level user message, lets the coordinator interpret it, emits public workspace activity, and returns the routed role dispatches.

This is now the primary API for the full multi-agent effect:
- one user message enters the workspace
- the coordinator decides who should claim it
- the selected member(s) run through the provider adapter
- public activity is emitted for user messages, coordinator responses, claims, progress, and delivery

If the workspace already has a fixed workflow template and you want to bypass the group-chat proposal/vote path, pass `workflowEntry: 'direct'`. That starts the configured workflow executor immediately from its entry node.

### `assignRoleTask()`
Queues a task for a role and returns immediately with the local dispatch record.

### `runRoleTask()`
Queues a task and waits until:
- the dispatch reaches a terminal state
- Claude returns final result text for the delegated task, when available

This remains useful for deterministic low-level tests and adapter debugging.

### `onEvent()`
Subscribes to the workspace event stream.

Useful event types:
- `workspace.started`
- `workspace.initialized`
- `workspace.state.changed`
- `member.registered`
- `member.state.changed`
- `activity.published`
- `dispatch.claimed`

## Workflow Template v3

Templates can now carry a declarative workflow in addition to roles and claim policy. The runtime does not yet execute every workflow node natively, but the protocol and built-in templates now preserve the structure needed for reliable loops and gates.

Current workflow objects:
- `workflow.mode`
- `workflow.stages`
- `workflow.nodes`
- `workflow.edges`
- `artifacts`
- `completionPolicy`

Current node vocabulary:
- `announce`
- `assign`
- `claim`
- `shell`
- `evaluate`
- `review`
- `branch`
- `loop`
- `artifact`
- `commit`
- `revert`
- `merge`
- `complete`

That lets us express patterns such as:
- coding loop: `claim -> prd -> review -> architecture -> implement -> test -> review -> complete`
- autoresearch loop: `frame -> claim evidence -> shell run -> evaluate -> keep/discard -> loop`
- governance flow: `draft -> review -> dispatch -> execute -> compliance -> final review`
- workflow nodes may now optionally override provider/model for the role they dispatch
- `dispatch.queued`
- `dispatch.started`
- `dispatch.progress`
- `dispatch.completed`
- `dispatch.result`
- `tool.progress`
- `result`
- `error`

## Development

```bash
npm install
npm run typecheck
npm run build
```

Helpful commands:

```bash
npm test
npm run clean
```

### Rust Workspace

```bash
cd rust
cargo test
```

Rust crates:
- `multi-agent-protocol`
- `multi-agent-runtime-core`
- `multi-agent-runtime-claude`
- `multi-agent-runtime-codex`
- `multi-agent-runtime-cteno`

Run Rust live provider checks explicitly:

```bash
cd rust
cargo test -p multi-agent-runtime-claude --test live_claude_e2e -- --ignored --nocapture
cargo test -p multi-agent-runtime-codex --test live_codex_e2e -- --ignored --nocapture
```

## Smoke Commands

These are useful for manual exploration, but they are not our release-quality validation layer.

```bash
npm run smoke:coding
npm run smoke:opc
npm run smoke:autoresearch
```

## Live E2E Tests

These are the important ones.

They make real Claude calls and assert:
- the workspace initializes correctly
- the dispatch is queued, started, completed, and produces a final result
- the delegated role matches the expected role
- the expected file is actually generated
- the generated content matches template-specific acceptance checks

Run them individually:

```bash
npm run e2e:coding
npm run e2e:codex
npm run e2e:opc
npm run e2e:autoresearch
```

Run the full suite sequentially:

```bash
npm run e2e
```

### Current E2E Coverage

#### Coding Studio
Checks that:
- the `prd` role is used
- `10-prd/group-mentions.md` is created
- the file contains `Goal`, `User Story`, `Scope`, `Non-Goals`, and `Acceptance Criteria`

#### Codex Coding Studio
Checks that:
- the `CodexSdkWorkspace` adapter can reuse a role thread
- the `prd` role writes `10-prd/group-mentions.md`
- the generated PRD contains the expected sections
- the output is concise and implementation-oriented

#### OPC Solo Company
Checks that:
- the `finance` role is used
- `company/10-finance/monthly-close-checklist.md` is created
- the file contains monthly close sections for cash, invoices, subscriptions, payroll, tax prep, and KPIs
- the output is a real checklist with multiple actionable items

#### Autoresearch
Checks that:
- the `scout` role is used
- the workspace emits multiple research progress events
- `research/10-scout/mention-patterns.md` is created
- the brief includes `Implications for Cteno`
- the brief references tools like Slack and GitHub
- the brief includes at least three source links

## Design Notes

### Why this shape?
Claude's public interface behaves more like:
- sessions
- subagents
- task lifecycle notifications

and less like a fully exposed graph runtime.

So this package keeps the runtime thin and practical:
- `WorkspaceSpec` defines the workspace and its roles
- `ClaudeAgentWorkspace` adapts those roles into Claude subagents
- dispatches are provider-neutral records in our protocol
- events are normalized before they reach callers

### Important current limitation
Role-task correlation is still FIFO:
- each `assignRoleTask()` call queues a local dispatch
- the next Claude `task_started` event is matched to the next queued dispatch

This is reliable when the orchestrator is the only source of subagent launches.
If we later allow autonomous, unrelated subagent launches in the same session, we should strengthen correlation beyond FIFO.

## Status

Current status for the Claude adapter:
- live dispatching works
- delegated role tasks produce normalized `dispatch.*` events
- final result text is attached back onto the dispatch as `resultText`
- all three built-in templates have passing live e2e coverage

Current status for the Rust side:
- provider-neutral protocol types implemented
- core dispatch lifecycle runtime implemented
- Cteno adapter traits and bootstrap flow implemented
- `cargo test` passes for protocol/core/cteno workspace

That makes this package good enough to continue hardening toward an open-source split, with Cteno integration as the next adapter layer.

## Open Source Readiness

See:
- [CONTRIBUTING.md](/Users/zal/Cteno/packages/multi-agent-runtime/CONTRIBUTING.md)
- [OPEN_SOURCE_CHECKLIST.md](/Users/zal/Cteno/packages/multi-agent-runtime/OPEN_SOURCE_CHECKLIST.md)

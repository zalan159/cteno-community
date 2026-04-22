# Rust Workspace

This directory contains the Rust implementation of the multi-agent runtime.

## Crates

### `multi-agent-protocol`
Shared provider-neutral objects:
- `WorkspaceSpec`
- `RoleSpec`
- `WorkspaceMember`
- `WorkspaceActivity`
- `TaskDispatch`
- `WorkspaceEvent`
- `WorkspaceTemplate`
- `WorkspaceProfile`
- `instantiate_workspace()`

### `multi-agent-runtime-core`
Core runtime state machine:
- register workspace members
- publish user/coordinator/member activity
- handle direct or claim-based assignment
- queue dispatch
- start dispatch
- progress dispatch
- complete dispatch
- attach final result text

### `multi-agent-runtime-claude`
Claude Code CLI provider adapter:
- runs role tasks through `claude -p --verbose --output-format stream-json`
- captures reusable Claude session IDs per role
- maps tool usage, progress, and final result text back into workspace events

### `multi-agent-runtime-codex`
Codex CLI provider adapter:
- runs role tasks through `codex exec --experimental-json`
- captures reusable Codex thread IDs per role
- maps tool usage, progress, and final result text back into workspace events

### `multi-agent-runtime-cteno`
Trait-based Cteno embedding layer:
- workspace bootstrap
- role agent provisioning
- role session spawning
- session message delivery

This crate is intentionally decoupled from the private Cteno codebase.
It defines the adapter seam now, so the real Cteno integration can implement these traits later.

## Current Shape

Rust now mirrors the TypeScript authoring model:
- templates define organization semantics
- profiles define provider/runtime defaults
- `instantiate_workspace()` binds them into a concrete `WorkspaceSpec`
- runtime state now includes `members`, `activities`, and `claimPolicy`
- templates can also carry declarative `workflow`, `artifacts`, and `completion_policy`

Built-in templates currently included in the protocol crate:
- `create_coding_studio_template()`
- `create_opc_solo_company_template()`
- `create_autoresearch_template()`
- `create_edict_governance_template()`

Workflow node vocabulary currently modeled in the protocol:
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

## Run Tests

```bash
cd rust
cargo test
```

## Run Live Provider E2E

These tests are ignored by default because they require local CLI auth and spend real tokens.

Claude Code CLI:

```bash
cd rust
cargo test -p multi-agent-runtime-claude --test live_claude_e2e -- --ignored --nocapture
```

Codex CLI:

```bash
cd rust
cargo test -p multi-agent-runtime-codex --test live_codex_e2e -- --ignored --nocapture
```

# Cteno Local-First Desktop

Cteno's desktop app is local-first by default. It is:

- **Local-first** — all sessions stored in local SQLite (`~/.cteno/db.sqlite`)
- **BYOK** — Bring Your Own LLM API Keys (Anthropic / OpenAI / DeepSeek / Gemini etc.)
- **Multi-agent** — drive Cteno's native agent runtime, or plug in `claude` / `codex` CLI as alternative agent vendors
- **Local-only community mode** — the public desktop build runs without Happy Server, hosted sync, account billing, or mobile cloud relay

## Install

(Download links / build from source instructions)

## Build Community Desktop

Build the community desktop binary explicitly with the community feature:

```bash
cargo build --manifest-path apps/client/desktop/Cargo.toml \
  --no-default-features \
  --features community \
  --bin cteno
```

Public sidecars are built separately and discovered automatically in dev/source builds:

```bash
cargo build --manifest-path packages/agents/rust/crates/cteno-agent-stdio/Cargo.toml
cargo build --manifest-path packages/host/rust/Cargo.toml -p cteno-host-memory-mcp
```

Run the app:

```bash
apps/client/desktop/target/debug/cteno
```

## Validate the desktop bundle

Use the standard desktop build and inspect the packaged sidecar/resources directly:

```bash
cd apps/client
yarn tauri:build
```

For direct macOS artifact inspection, use the rebuilt bundle paths rather than inferring from config alone:

```bash
find "desktop/target/release/bundle/macos/Cteno.app/Contents/MacOS" -maxdepth 1 -type f
find "desktop/target/release/bundle/macos/Cteno.app/Contents/Resources" -maxdepth 1 -mindepth 1
```

What this validates:

- `tauri:build` points at the single desktop Tauri config
- `desktop/tauri.conf.json` bundles the public `cteno-agent` and `cteno-memory-mcp` sidecars as `externalBin`
- `desktop/src/executor_registry.rs` resolves `cteno-agent` from env/PATH/dev target dirs or as a sibling of the packaged app executable
- `desktop/src/agent_sync_bridge.rs` resolves `cteno-memory-mcp` from env/PATH/dev target dirs or as a sibling of the packaged app executable
- The built macOS app contains `Contents/MacOS/cteno`, `Contents/MacOS/cteno-agent`, `Contents/MacOS/cteno-memory-mcp`, and the packaged resources under `Contents/Resources/{agents,helpers,skills,tools}`

Expected packaged sidecar locations:

- macOS: `<App>.app/Contents/MacOS/cteno-agent` and `<App>.app/Contents/MacOS/cteno-memory-mcp`
- Linux: sibling `cteno-agent` and `cteno-memory-mcp` next to the packaged executable
- Windows: sibling `cteno-agent.exe` and `cteno-memory-mcp.exe` next to the packaged executable

Expected macOS resource layout:

- `<App>.app/Contents/Resources/agents`
- `<App>.app/Contents/Resources/helpers`
- `<App>.app/Contents/Resources/skills`
- `<App>.app/Contents/Resources/tools`

## Community Snapshot Acceptance

Before publishing a community snapshot, verify the local-only loop end to end:

```bash
./scripts/export-community-snapshot.sh /tmp/cteno-community-audit
cd /tmp/cteno-community-audit
cargo build --manifest-path packages/agents/rust/crates/cteno-agent-stdio/Cargo.toml
cargo build --manifest-path packages/host/rust/Cargo.toml -p cteno-host-memory-mcp
cargo build --manifest-path apps/client/desktop/Cargo.toml \
  --no-default-features \
  --features community \
  --bin cteno
```

Acceptance checks:

- `packages/commercial/`, `apps/happy-server/`, `apps/client/ios/`, and `apps/client/android/` are absent from the snapshot.
- `apps/client/desktop/Cargo.toml` defaults to `community`, and no `cteno-happy-client*` dependency remains.
- `apps/client/desktop/tauri.conf.json` keeps the public sidecars and has no `packages/commercial` watch path.
- Launching `apps/client/desktop/target/debug/cteno` opens the desktop window.
- Startup logs do not contain `cteno-agent binary not found`, `cteno-memory-mcp binary not found`, or `No handler registered` for `list-personas`, `list-agent-workspaces`, or `list-scheduled-tasks`.
- In local mode, creating a session and sending one message succeeds without Happy Server auth.

## Configure your LLM API key

Edit `~/.cteno/profiles.json`:

```json
{ "anthropic": { "api_key": "sk-ant-..." } }
```

See [docs/byok.md](./byok.md) for the full BYOK configuration reference.

## Use other agent vendors

Cteno can drive `claude` and `codex` CLI subprocesses. Install them separately:

- Claude: https://docs.anthropic.com/claude-code
- Codex: https://github.com/openai/codex

Cteno auto-discovers `claude` / `codex` in PATH; override with `CLAUDE_PATH` / `CODEX_PATH` env vars.

## Privacy

Community desktop mode stores session data locally. Happy Server, hosted multi-device sync, account billing, and mobile relay are not part of the community snapshot.

See [docs/privacy.md](./privacy.md) for the full privacy statement.

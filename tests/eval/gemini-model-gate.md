# Gemini model-gate adapter regression

## meta
- kind: host-runtime
- adapter: `multi-agent-runtime-gemini`
- cli: `gemini --acp` (0.38.2+)
- workdir: `/tmp`

## setup

All cases are unit + integration tests backed by a mock `gemini --acp`
shell script. One case also exercises the real binary — gated behind
`GEMINI_PATH` so CI without the CLI skips it.

```bash
cd packages/multi-agent-runtime/rust
cargo test -p multi-agent-runtime-gemini --test integration_connection_reuse
# Live check (requires ~/.gemini/oauth_creds.json or GEMINI_API_KEY env):
GEMINI_PATH="$(command -v gemini)" \
  cargo test -p multi-agent-runtime-gemini --test integration_connection_reuse \
  -- --ignored live_smoke
```

## Background

Cteno's shared `resolve_spawn_model()` returns a `ModelSpec` tagged with a
`provider` field (`"anthropic"` / `"openai"` / `"gemini"`). When a user
picks a profile that targets e.g. `deepseek-reasoner` (OpenAI format) and
starts a gemini session, the host passes the profile's `model_id`
verbatim to the gemini adapter with `provider="openai"`.

The previous adapter forwarded the id into `session/set_model`
unconditionally. Gemini's ACP server accepts the call with `{ result: {} }`
(no validation), but the underlying session gets poisoned — the next
`session/prompt` fails with:

```
error.code = 500
error.message = "Requested entity was not found."
```

Confirmed live against `gemini --acp` 0.38.2 by driving it with raw
ndJSON JSON-RPC over stdin/stdout.

Additionally, `collect_vendor_models("gemini")` returned an empty list,
so `is_vendor_native_model_id()` always returned `false` for gemini —
forcing the fallback branch that tagged every profile id as a gemini
model.

## cases

### [pass] session/set_model must be skipped when provider != "gemini"
- **message**: n/a — integration test `apply_model_skips_set_model_when_provider_is_not_gemini`
- **expect**: spawning a session with `ModelSpec { provider: "openai", model_id: "deepseek-reasoner" }` must not emit any `session/set_model` call. The mock records every call to a log file; test asserts the log is empty. The subsequent `session/prompt` completes with `TurnComplete`.
- **anti-pattern**: blindly forwarding `set_model` with any non-empty `model_id`; trusting the `{ result: {} }` ack as validation; relying on `session/prompt` to error out so the user re-picks a model.
- **severity**: high

### [pass] session/set_model must be skipped when model_id not in advertised list
- **message**: n/a — `apply_model_skips_set_model_for_unknown_gemini_id`
- **expect**: even when `provider="gemini"`, if the id (e.g. `"gemini-bogus-preview"`) is not present in the `session/new` response's `models.availableModels`, the adapter must skip `set_model`. Log file stays empty, turn completes.
- **anti-pattern**: string-prefix heuristics like "starts with `gemini-`"; assuming the capability probe at `initialize` returned the model list (it does not — the list is in `session/new`/`session/load` responses only).
- **severity**: high

### [pass] recognized gemini id is forwarded verbatim
- **message**: n/a — `apply_model_forwards_recognized_gemini_id`
- **expect**: `ModelSpec { provider: "gemini", model_id: "gemini-2.5-flash" }` must emit one `session/set_model` with `modelId: "gemini-2.5-flash"`. Log file contains exactly that line.
- **anti-pattern**: stripping reasoning suffixes; lower-casing the id; adding `models/` prefix.
- **severity**: medium

### [pass] bogus model on one session doesn't poison sibling sessions on the shared connection
- **message**: n/a — `apply_model_does_not_poison_shared_connection_across_sessions`
- **expect**: given the multi-session shared connection, a bogus model id passed to session A must not leak into session B. Both sessions' `session/prompt` calls complete with `TurnComplete`.
- **anti-pattern**: caching the last-applied model globally on the connection; routing `set_model` at connection scope instead of session scope.
- **severity**: high

### [pass] `collect_vendor_models("gemini")` returns a non-empty baseline
- **message**: n/a — covered by `cargo check` compile-time assertion that the static list has entries; end-to-end UI smoke verified via `list_vendor_models` RPC returning `returnedModelCount > 0`.
- **expect**: `collect_vendor_models("gemini")` returns at least the 8 ids currently advertised by `gemini --acp` 0.38.x (`auto-gemini-3`, `auto-gemini-2.5`, `gemini-3.1-pro-preview`, `gemini-3-flash-preview`, `gemini-3.1-flash-lite-preview`, `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-2.5-flash-lite`). Default is `auto-gemini-3`. Updating gemini CLI may add new models; stale list is tolerated because the adapter validates live against `availableModels` from `session/new`.
- **anti-pattern**: returning `Ok(Vec::new())` (pre-fix behaviour); shelling out to `gemini models list` (no such subcommand); hard-coding a single model.
- **severity**: medium

### [pass] live OAuth-logged-in user can spawn + prompt without explicit authenticate
- **message**: n/a — `live_smoke_initialize_plus_prompt` (ignored by default; run with `GEMINI_PATH="$(command -v gemini)"`)
- **expect**: on a host where `~/.gemini/oauth_creds.json` exists and `~/.gemini/settings.json` has `security.auth.selectedType = "oauth-personal"`, the adapter spawns `gemini --acp`, runs `initialize`, then `session/new`, then `session/prompt` without ever calling `authenticate`. The final stream contains a `StreamDelta { content: "PONG", ... }`.
- **anti-pattern**: pre-emptively calling `authenticate` when OAuth is cached (fails for oauth-personal without the interactive flow); failing loudly when `GEMINI_API_KEY` is unset.
- **severity**: high

### [skip] long-idle shared connection re-uses cached known_models across sessions
- **reason**: the known-models cache lives on the `GeminiAcpConnection`, not the shared executor; per-session freshness is already exercised by `apply_model_skips_set_model_for_unknown_gemini_id` (which runs session/new right before set_model). A dedicated long-idle regression would need a timed mock and adds flake risk without catching a new class of bug.

## status

11 integration tests pass on the mock harness, 1 live-smoke test passes
against `gemini --acp` 0.38.2 with cached OAuth creds. Manual reproduction
of the original `[500] Requested entity was not found.` error with raw
`python3 + gemini --acp` is recorded in the PR description.

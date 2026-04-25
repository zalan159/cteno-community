# Vendor context usage

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-vendor-context-usage
- max-turns: 10

## setup

```bash
mkdir -p /tmp/cteno-vendor-context-usage
```

## cases

### [pending] Claude uses SDK context usage instead of UI fallback
- **message**: "Start a Claude session and ask for a one-sentence reply. After the turn, inspect persisted ACP side-effects for that session."
- **expect**: A `context_usage` ACP record exists with `total_tokens > 0`, `max_tokens > 0`, and `raw_max_tokens > 0` from Claude SDK `get_context_usage()`; the UI denominator comes from `session.contextWindowTokens`.
- **anti-pattern**: Only `token_count` exists, or the UI shows a hard-coded 1M denominator without a persisted `context_usage.max_tokens`.
- **severity**: high

### [pending] Gemini emits context usage with model-derived window
- **message**: "Start a Gemini session with a concrete Gemini model and ask for a one-sentence reply. After the turn, inspect persisted ACP side-effects for that session."
- **expect**: A `context_usage` ACP record exists with `total_tokens > 0`, `max_tokens > 0`, `raw_max_tokens > 0`, and `model` matching `_meta.quota.model_usage[].model` when Gemini reports it.
- **anti-pattern**: Gemini only persists `token_count`; `session.contextWindowTokens` remains empty; the UI denominator appears from a vendor fallback guess.
- **severity**: high

### [pending] Missing context window does not render guessed denominator
- **message**: "Replay a local session history containing only ACP `token_count` records and no `context_usage` record."
- **expect**: `session.contextTokens` may update from `token_count`, but no `context` percentage is rendered until a `context_usage.max_tokens` side-effect supplies `session.contextWindowTokens`.
- **anti-pattern**: The UI renders `xx/256K` or `xx/1M` solely from vendor flavor.
- **severity**: medium

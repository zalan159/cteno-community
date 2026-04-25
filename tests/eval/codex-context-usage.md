# Codex context usage

## meta
- kind: codex
- profile: gpt-5.4
- workdir: /tmp/cteno-codex-context-usage
- max-turns: 8

## setup

```bash
mkdir -p /tmp/cteno-codex-context-usage
printf 'Do not edit files for this eval.\n' > /tmp/cteno-codex-context-usage/AGENTS.md
```

## cases

### [pending] Codex token usage carries the real context window
- **message**: "请只回复一句话，并说明你不会修改文件。"
- **expect**: The Codex session receives `thread/tokenUsage/updated`, persists an ACP `context_usage` side-effect with `total_tokens > 0` and `max_tokens = modelContextWindow`, and the UI denominator uses that `max_tokens` instead of the hard-coded Codex fallback.
- **anti-pattern**: Only `token_count` is persisted; `session.contextWindowTokens` stays empty; the UI keeps showing `context` against a guessed 256K window when Codex reported a different `modelContextWindow`.
- **severity**: high

### [pending] Codex context usage uses last turn usage, not cumulative billing
- **message**: "连续发两轮很短消息，第二轮只问：现在几点？不要调用工具。"
- **expect**: The stored `context_usage.total_tokens` follows Codex `tokenUsage.last.totalTokens` for the latest turn while `token_count` may still represent normalized aggregate usage.
- **anti-pattern**: `context_usage.total_tokens` is derived from cumulative `tokenUsage.total` and drifts above the current context window display used by Codex itself.
- **severity**: medium

# Cteno Context Usage

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-context-usage
- max-turns: 12

## setup
```bash
mkdir -p /tmp/cteno-context-usage
printf 'one two three\n' > /tmp/cteno-context-usage/notes.txt
```

## cases

### [pending] Cteno reports real context window usage
- **message**: "Use the Cteno persona with a profile whose chat context_window_tokens is 200000. Ask it to read notes.txt and answer in one short sentence."
- **expect**: The session persists an ACP `context_usage` side-effect with `total_tokens > 0`, `max_tokens = 200000`, and `auto_compact_token_limit > 0`; the chat input shows context usage against 200K rather than the hard-coded Cteno fallback.
- **anti-pattern**: The UI only receives `token_count`, falls back to `256K`, or uses accumulated multi-call billing tokens as the context-window usage.
- **severity**: high

### [pending] Context usage survives reload
- **message**: "After the previous Cteno turn completes, reload the session messages from local persistence without running another turn."
- **expect**: Replaying persisted messages restores `session.contextTokens` and `session.contextWindowTokens`; the context indicator still shows the same used/window values.
- **anti-pattern**: The indicator disappears after reload, or the denominator reverts to the legacy fallback.
- **severity**: medium

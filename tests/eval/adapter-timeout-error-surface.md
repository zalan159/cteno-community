# Adapter Timeout / Error Surface — Readable Error Verification

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 5

## cases

### [pass] Cteno adapter stderr probe detects panic/fatal lines
- **message**: Unit test `stderr_probe_flags_panic_and_fatal_lines` in `agent_executor.rs`
- **expect**: `stderr_line_is_fatal` returns true for lines containing "panic" or "fatal" (case-insensitive), false for normal warnings
- **anti-pattern**: false positive on benign log lines
- **severity**: high
- **verification**: `cargo test -p multi-agent-runtime-cteno stderr_probe_flags_panic_and_fatal_lines` passes

### [pass] Subprocess exit message includes stderr tail when available
- **message**: Unit test `subprocess_exit_message_includes_stderr_tail_when_available` in `agent_executor.rs`
- **expect**: `subprocess_exit_message(Some(101), "panic: broken state machine")` contains both "code 101" and stderr tail
- **anti-pattern**: stderr tail silently dropped
- **severity**: high
- **verification**: `cargo test -p multi-agent-runtime-cteno subprocess_exit_message` passes

### [pass] Fatal executor errors are persisted as ACP error + task_complete
- **message**: Unit test `fatal_executor_errors_are_persisted_and_close_the_turn` in `executor_normalizer.rs`
- **expect**: Non-recoverable `ExecutorEvent::Error` produces two persisted ACP messages: (1) `{type: "error", recoverable: false}` and (2) `{type: "task_complete"}`, and `process_event` returns `Ok(true)` to stop the turn
- **anti-pattern**: Frontend hangs on thinking indicator because task_complete is never sent; or error is transient-only and lost on reconnect
- **severity**: high
- **verification**: `cargo test -p cteno fatal_executor_errors_are_persisted_and_close_the_turn` passes

### [pass] user_visible_executor_error formats SubprocessExited with stderr context
- **message**: Unit test `user_visible_executor_error_formats_subprocess_exit` in `executor_normalizer.rs`
- **expect**: `user_visible_executor_error(&AgentExecutorError::SubprocessExited { code: Some(101), stderr: "panic: ..." })` produces a human-readable string containing exit code and stderr tail
- **anti-pattern**: Raw `Debug` or `Display` output with internal variant names leaked to user
- **severity**: medium
- **verification**: `cargo test -p cteno user_visible_executor_error_formats_subprocess_exit` passes

### [pass] spawn_session failure emits readable error via emit_spawn_failure
- **message**: Code inspection of `executor_session.rs::run_one_turn`
- **expect**: When `executor.spawn_session(spec)` returns Err, `emit_spawn_failure` sends a persisted ACP error `{type: "error", message: "xxx failed: <reason>"}` + `{type: "task_complete"}` to the session socket, preventing frontend hang
- **anti-pattern**: spawn failure silently swallowed; frontend stuck on loading
- **severity**: high
- **verification**: Code path confirmed by inspection; `cargo check -p cteno` passes

### [pass] cargo check passes with all changes
- **message**: `cargo check -p cteno --manifest-path apps/client/desktop/Cargo.toml`
- **expect**: Exit 0, no errors (warnings from unrelated crates acceptable)
- **anti-pattern**: Compilation failure
- **severity**: high
- **verification**: Confirmed exit 0

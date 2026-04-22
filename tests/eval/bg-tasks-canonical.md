# Background Tasks Canonical: Registry + RPC + Frontend Integration

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 15

## cases

### [pass] task_started registration survives parent turn completion (vendor race)
- **message**: Agent dispatches a subagent producing a task_started frame. IMMEDIATELY after frame ingestion, the parent turn completes. Verify the BackgroundTaskRegistry retains the task record with status=Running both before and after the parent turn ends. Adversarial hook: insert only 0ms delay between task_started and turn_complete to maximize the race window.
- **expect**: Registry contains the task with status=Running after turn_complete. The record persists across subsequent list-background-tasks calls with no loss. started_at timestamp is non-null and monotonically ordered.
- **anti-pattern**: Registry purges or garbage-collects the record when the parent turn completes; task disappears from list-background-tasks after turn ends; status transiently flips to Unknown.
- **severity**: high

### [pass] task_progress for unknown task_id does not fabricate Running status
- **message**: Inject a task_progress frame referencing a task_id ("phantom-task-9999") that never appeared in any prior task_started. Adversarial hook: the progress payload includes plausible fields (progress=50, message="halfway done") that could trick a naive registry into auto-creating a Running record.
- **expect**: Registry either ignores the frame entirely (task_id absent from list output) OR creates a sentinel record with status=Unknown. The task MUST NOT appear as Running in list-background-tasks. No panic, no unwrap failure, no log-level escalation above warn.
- **anti-pattern**: Task appears in list-background-tasks as Running with no real backing task; registry panics on missing key lookup; fabricated record inherits progress fields as if legitimate.
- **severity**: high

### [pass] task_notification terminal=failed preserves output_file despite success-like language
- **message**: Send a task_notification with status="failed", output_file="/tmp/results.json", and message body containing the word "successfully completed the error analysis". Adversarial hook: the message text is deliberately misleading — contains "successfully" to trick keyword-based status inference.
- **expect**: Registry transitions the task to Failed status. output_file is preserved as "/tmp/results.json". The misleading message body is stored verbatim but does NOT override the explicit status="failed" field.
- **anti-pattern**: Status flipped to Completed because notification message keyword-matched "successfully"; output_file discarded on failure; status field ignored in favor of message text heuristics.
- **severity**: high

### [pass] Session socket not released while vendor task Running during grace window
- **message**: Create a session, dispatch a background task (status=Running), then trigger turn_complete. Verify the session's socket/connection is NOT released during the 3-second idle grace window while the vendor task remains Running. Adversarial hook: send a second turn_complete 1s after the first to simulate rapid successive completions — connection must survive both.
- **expect**: Session connection remains alive throughout the grace window. Subsequent task_notification frames (arriving 4s after turn_complete) are successfully received and processed. Connection release only occurs after all tasks reach terminal status AND grace window expires.
- **anti-pattern**: Connection dropped immediately on turn_complete; task_notification lost because socket was released; grace window timer not reset by second turn_complete; task_notification silently discarded with no error log.
- **severity**: high

### [pass] list-background-tasks rejects invalid category with error, not empty array
- **message**: Call list-background-tasks RPC with `{ category: "INVALID" }`. Adversarial hook: also test with category="" (empty string) and category="execution; DROP TABLE" (injection attempt) in separate calls.
- **expect**: All three calls return `{ success: false, error: "Invalid category" }`. The error message is identical regardless of input content. No SQL or command injection side effects. Empty string is not silently treated as "no filter".
- **anti-pattern**: Silently returns empty array (hides client bug); error message varies based on input (information leakage); empty string bypasses validation and returns all tasks; injection payload reflected in error message.
- **severity**: medium

### [pass] get-background-task rejects malicious task_id without echoing input
- **message**: Call get-background-task RPC with task_id values: `"../../../etc/passwd"`, `"task'; DROP TABLE--"`, `"task\x00null"`. Adversarial hook: each payload is designed to test path traversal, SQL injection, and null byte injection respectively.
- **expect**: All three calls return `{ success: false, error: "Task not found" }`. The error message is static and does NOT echo back the attacker's raw input. No file system access, no query manipulation, no null byte propagation.
- **anti-pattern**: Error message echoes the attacker's raw input (XSS/log injection vector); different error messages for different malicious inputs (oracle attack); panic on null byte; path traversal reaches file system.
- **severity**: high

### [fail] Duplicate task_started frames (Claude resume) upsert idempotently
- **message**: Send two task_started frames with identical task_id="claude-resume-task-1" but different timestamps (t1=1000, t2=2000). The second frame simulates Claude CLI re-emitting task_started after `--resume`. Adversarial hook: the second frame also carries a different task_name to test which fields are preserved vs overwritten.
- **expect**: list-background-tasks returns exactly ONE entry for task_id="claude-resume-task-1". started_at remains the ORIGINAL timestamp (t1=1000), not overwritten by the resume frame. Progress accumulated before the duplicate frame is preserved. task_name may update (last-write-wins is acceptable for metadata) but started_at must not.
- **anti-pattern**: started_at overwritten to t2 (breaks duration calculation); duplicate entries in list output; progress counter reset to 0; second frame treated as new task with separate lifecycle.
- **severity**: high

### [pass] Scheduled job and execution task distinguishable in unfiltered list
- **message**: Insert a scheduled_job record (category=ScheduledJob, status=Pending, next_run_at set) AND a running execution task (category=ExecutionTask, status=Running, started_at set) into the registry. Call list-background-tasks with no filter `{}`. Adversarial hook: give both records identical task_name="background-work" to force differentiation by category alone, not by name.
- **expect**: Response contains exactly 2 entries. Each entry has a distinct `category` field ("scheduled_job" vs "execution"). Frontend can distinguish them purely by category without relying on task_name. Both records serialized with their category-specific fields (next_run_at for scheduled_job, started_at for execution).
- **anti-pattern**: Both rendered with identical category; category field missing from serialization; scheduled_job missing next_run_at; execution task missing started_at; one record shadows the other due to name collision.
- **severity**: medium

### [fail] Frontend modal falls back to legacy data when canonical RPC fails
- **message**: Simulate canonical list-background-tasks RPC returning `{ success: false, error: "registry unavailable" }`. Verify the BackgroundRunsModal still renders using the legacy deriveAgentBackgroundTasks fallback AND shows a visible banner/indicator that fallback mode is active. Adversarial hook: the legacy data contains 3 tasks while the failed RPC error message contains the substring "tasks" — ensure the error string is not parsed as task data.
- **expect**: Modal renders with legacy-derived task list (3 items visible). A fallback-mode banner or warning indicator is present in the DOM. The RPC error message is not displayed as a task item. useBackgroundTasks hook returns isFallback=true or equivalent flag.
- **anti-pattern**: Blank/empty modal when RPC fails; no indication that fallback mode is active; error message string rendered as a task row; legacy fallback code path removed or dead; modal crashes with unhandled promise rejection.
- **severity**: high

# bg-05: Background Task RPC Handlers

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 5

## cases

### [pass] MachineRpcMethods struct has list_background_tasks and get_background_task fields
- **message**: Verify MachineRpcMethods includes `list_background_tasks: String` and `get_background_task: String` fields, initialized with `format!("{machine_id}:list-background-tasks")` and `format!("{machine_id}:get-background-task")`
- **expect**: Both fields exist at lines 610-611 of lib.rs; constructor at lines 699-700 uses correct format strings
- **anti-pattern**: Fields missing, wrong format string prefix, typo in method name
- **severity**: high

### [pass] Both handlers registered inside prime_machine_scoped_ui_rpc_handlers
- **message**: Verify list-background-tasks and get-background-task RPC handlers are registered in the same scope as list-runs (inside `register_machine_scoped_ui_rpc_handlers`)
- **expect**: Handlers registered at lines 1842-1895 of manager.rs, using `register_sync`; registration test `local_first_runtime_registers_shared_machine_ui_methods` asserts `has_method` for both (lines 3347-3352)
- **anti-pattern**: Handlers registered in a different scope, missing from registration test
- **severity**: high

### [pass] list-background-tasks with no params returns all tasks
- **message**: Verify `list-background-tasks` RPC with empty params `{}` returns `{ success: true, data: [...all tasks...] }`
- **expect**: Test `shared_machine_ui_background_task_rpcs_validate_filters_and_fetch_records` sends `json!({})` and asserts both running and completed tasks appear in data array (lines 3451-3474)
- **anti-pattern**: Returns empty array, returns error, panics
- **severity**: high

### [pass] list-background-tasks with sessionId filters correctly
- **message**: Verify filtering by sessionId + category + status returns only matching tasks
- **expect**: Test sends `{ sessionId: "bg05-session-a", category: "execution", status: "running" }` and asserts exact match to the one expected record (lines 3476-3500)
- **anti-pattern**: Returns all tasks ignoring filter, returns empty when match exists
- **severity**: high

### [pass] list-background-tasks with invalid category returns error
- **message**: Verify `{ category: "bogus" }` returns `{ success: false, error: "Invalid category" }`
- **expect**: Parser function `parse_background_task_category_param` at line 71-84 returns `Err("Invalid category")` for unknown strings; test asserts exact JSON match (lines 3502-3514)
- **anti-pattern**: Silently ignores invalid category, panics
- **severity**: medium

### [pass] list-background-tasks with invalid status returns error
- **message**: Verify `{ status: "bogus" }` returns `{ success: false, error: "Invalid status" }`
- **expect**: Parser function `parse_background_task_status_param` at line 86-102 returns `Err("Invalid status")`; test asserts exact JSON (lines 3516-3528)
- **anti-pattern**: Silently ignores invalid status
- **severity**: medium

### [pass] get-background-task with empty taskId returns Missing taskId error
- **message**: Verify `{}` or `{ taskId: "" }` returns `{ success: false, error: "Missing taskId" }`
- **expect**: Handler trims and checks empty at lines 1876-1883; test sends `json!({})` and asserts exact error (lines 3530-3542)
- **anti-pattern**: Panics on missing field, returns null data
- **severity**: high

### [pass] get-background-task with unknown taskId returns Task not found error
- **message**: Verify `{ taskId: "bg05-missing-task" }` returns `{ success: false, error: "Task not found" }`
- **expect**: Handler calls `registry.get()` and maps `None` to error at line 1892; test asserts exact JSON (lines 3544-3556)
- **anti-pattern**: Panics, returns success with null data
- **severity**: high

### [pass] get-background-task with known taskId returns task data
- **message**: Verify `{ taskId: "bg05-completed-task" }` returns `{ success: true, data: {..task record..} }`
- **expect**: Handler maps `Some(task)` to success at line 1891; test asserts exact match against `sample_background_task_record` (lines 3558-3578)
- **anti-pattern**: Returns wrong task, missing fields in serialization
- **severity**: high

### [pass] cargo check passes
- **message**: Run `cargo check -p cteno` and verify no compilation errors
- **expect**: Exits with status 0, only warnings (unrelated dead_code in codex workspace)
- **anti-pattern**: Compilation errors, missing imports, type mismatches
- **severity**: high

### [pass] cargo test passes for background task RPC test
- **message**: Run `cargo test -p cteno shared_machine_ui_background_task_rpcs_validate_filters_and_fetch_records`
- **expect**: 1 test passed, 0 failed
- **anti-pattern**: Test failure, assertion panic
- **severity**: high

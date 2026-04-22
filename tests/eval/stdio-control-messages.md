# stdio control messages (SetModel / SetPermissionMode)

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 5

## cases

### [pass] SetModel inbound message deserialization with new shape
- **message**: Verify `{"type":"set_model","session_id":"s-3","model":"gpt-5.1","effort":"high"}` deserializes to `Inbound::SetModel` with correct fields
- **expect**: Parsed as SetModel variant; session_id="s-3", model="gpt-5.1", effort=Some("high")
- **anti-pattern**: Panic, deserialization error, wrong variant
- **severity**: high
- **verified-by**: `cargo test -p cteno-agent-stdio inbound_set_model_accepts_runtime_control_shape` -- pass (unit test)

### [pass] SetPermissionMode inbound message deserialization
- **message**: Verify `{"type":"set_permission_mode","session_id":"s-4","mode":"accept_edits"}` deserializes correctly
- **expect**: Parsed as SetPermissionMode variant; session_id="s-4", mode="accept_edits"
- **anti-pattern**: Panic, deserialization error, wrong variant
- **severity**: high
- **verified-by**: `cargo test -p cteno-agent-stdio inbound_set_permission_mode_round_trip` -- pass (unit test)

### [pass] SetModel forward-compatibility with unknown fields
- **message**: Verify SetModel JSON with extra unknown fields (e.g. `"ignored_future_field": true`) still deserializes without error
- **expect**: Unknown fields silently ignored per serde defaults
- **anti-pattern**: Deserialization failure on unknown fields
- **severity**: medium
- **verified-by**: `cargo test -p cteno-agent-stdio inbound_set_model_accepts_runtime_control_shape` -- includes `ignored_future_field` and passes

### [pass] apply_model_control updates legacy nested model shape
- **message**: Call apply_model_control on a config with `{"model": {"provider":"openai","model_id":"gpt-4.1","reasoning_effort":"low"}}` and verify both nested and top-level fields update
- **expect**: cfg_model returns new model; cfg_effort returns new effort; unrelated keys (e.g. resume_session_id) preserved
- **anti-pattern**: Clobbering unrelated config keys, losing nested shape structure
- **severity**: high
- **verified-by**: `cargo test -p cteno-agent-stdio apply_model_control_updates_legacy_and_new_shapes` -- pass (unit test)

### [pass] apply_model_control works on null/empty config (no panic)
- **message**: Call apply_model_control on Value::Null and verify it creates a valid object config
- **expect**: Config becomes an object with model set; no panic
- **anti-pattern**: Panic on null config, unwrap failure
- **severity**: high
- **verified-by**: `cargo test -p cteno-agent-stdio apply_model_control_updates_legacy_and_new_shapes` -- tests Value::Null case

### [pass] apply_permission_mode_control works on null config (no panic)
- **message**: Call apply_permission_mode_control on Value::Null
- **expect**: Config becomes `{"permission_mode":"accept_edits"}`; no panic
- **anti-pattern**: Panic on null config
- **severity**: high
- **verified-by**: `cargo test -p cteno-agent-stdio apply_permission_mode_control_updates_config_without_panicking` -- pass

### [pass] Init.agent_config backward compat with string model
- **message**: Verify cfg_model parses `{"model":"gpt-5.1","effort":"medium"}` (new flat shape)
- **expect**: model="gpt-5.1", effort="medium"
- **anti-pattern**: Fails on flat string model shape
- **severity**: high
- **verified-by**: `cargo test -p cteno-agent-stdio cfg_model_accepts_new_shape` -- pass

### [pass] Init.agent_config backward compat with nested object model
- **message**: Verify cfg_model parses `{"model":{"provider":"openai","model_id":"gpt-5.1","reasoning_effort":"high"}}` (legacy nested shape)
- **expect**: model="gpt-5.1", effort="high"
- **anti-pattern**: Fails on nested object model shape
- **severity**: high
- **verified-by**: `cargo test -p cteno-agent-stdio cfg_model_accepts_legacy_nested_shape` -- pass

### [pass] Unknown inbound message type is tolerated
- **message**: Verify `{"type":"future_protocol_message","some_field":"val"}` deserializes to Inbound::Unknown
- **expect**: Parsed as Unknown variant, no error
- **anti-pattern**: Hard failure on unknown message type
- **severity**: medium
- **verified-by**: `cargo test -p cteno-agent-stdio inbound_unknown_type_is_tolerated` -- pass

### [pass] Main dispatch: SetModel on unknown session returns error (not panic)
- **message**: Main loop receives SetModel for a session_id that was never Init'd
- **expect**: Emits Outbound::Error with message containing "unknown session_id"
- **anti-pattern**: Panic, silent drop, or HashMap key-not-found crash
- **severity**: high
- **verified-by**: Code inspection: main.rs lines 179-196 -- match arm checks `sessions.get_mut(&session_id)`, sends Error on None

### [pass] Main dispatch: SetPermissionMode on unknown session returns error (not panic)
- **message**: Main loop receives SetPermissionMode for a session_id that was never Init'd
- **expect**: Emits Outbound::Error with message containing "unknown session_id"
- **anti-pattern**: Panic, silent drop
- **severity**: high
- **verified-by**: Code inspection: main.rs lines 198-211 -- same pattern as SetModel

### [pass] cargo check -p cteno-agent-stdio passes
- **message**: Run cargo check
- **expect**: Exit code 0, no errors
- **anti-pattern**: Compile errors, warnings treated as errors
- **severity**: high
- **verified-by**: `cargo check` in crate dir -- `Finished dev profile` with exit 0

### [pass] cargo test -p cteno-agent-stdio passes (16/16)
- **message**: Run cargo test
- **expect**: All 16 tests pass, 0 failures
- **anti-pattern**: Any test failure
- **severity**: high
- **verified-by**: `cargo test` -- `test result: ok. 16 passed; 0 failed; 0 ignored`

# Persona dispatch_task respects persona.agent vendor

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 10

## cases

### [pass] dispatch_task_async resolves vendor from persona.agent, not hardcoded
- **message**: "Verify that PersonaManager::dispatch_task_async reads persona.agent via AgentOwnerInfo.agent_flavor instead of hardcoding 'claude'"
- **expect**: resolve_dispatch_agent_flavor uses owner.agent_flavor as default; no literal "claude" in dispatch path
- **anti-pattern**: Hardcoded "claude" string in dispatch_task_async or resolve_dispatch_agent_flavor
- **severity**: high
- **verified-by**: grep confirms zero matches for literal "claude" in persona/manager.rs; code reads owner.agent_flavor which comes from persona.agent

### [pass] agent_flavor_override takes precedence over persona.agent
- **message**: "Verify that an explicit agent_flavor_override param overrides the persona's own vendor"
- **expect**: resolve_dispatch_agent_flavor returns override when Some, persona.agent when None
- **anti-pattern**: Override ignored, always using persona.agent
- **severity**: high
- **verified-by**: Unit test dispatch_override_takes_precedence_over_persona_agent at line 690

### [pass] Unavailable vendor produces clear error
- **message**: "Verify that dispatching to a vendor not installed on the host returns a descriptive error"
- **expect**: ensure_dispatch_vendor_available returns Err("vendor X not available on this host")
- **anti-pattern**: Silent fallback to another vendor, panic, or generic error
- **severity**: high
- **verified-by**: Code at line 51 returns formatted error string with vendor name

### [pass] dispatch_task tool executor passes agent_flavor param
- **message**: "Verify DispatchTaskExecutor reads agent_flavor from input and passes it to dispatch_task_async"
- **expect**: dispatch_task.rs line 64 reads input.get("agent_flavor"), passes as agent_flavor_override
- **anti-pattern**: Parameter ignored or hardcoded
- **severity**: medium
- **verified-by**: Code inspection of dispatch_task.rs lines 64, 83

### [pass] RPC handler accepts agentFlavor param
- **message**: "Verify the RPC dispatch path in happy_client/manager.rs reads agentFlavor from params"
- **expect**: params.get("agentFlavor") at line 2229, passed to dispatch_task_async
- **anti-pattern**: Missing parameter, hardcoded vendor
- **severity**: medium
- **verified-by**: Code inspection of happy_client/manager.rs line 2229

### [pass] AgentOwnerInfo carries resolved persona vendor
- **message**: "Verify resolve_owner populates agent_flavor from persona.agent with cteno as default"
- **expect**: agent_owner.rs resolve_owner reads persona.agent, trims, defaults to "cteno"
- **anti-pattern**: Hardcoded "claude" default, empty string allowed
- **severity**: high
- **verified-by**: Code inspection of agent_owner.rs lines 84-90

### [pass] TOOL.md documents agent_flavor parameter
- **message**: "Verify dispatch_task TOOL.md schema includes agent_flavor with correct description"
- **expect**: agent_flavor field present in input_schema with description mentioning vendor override
- **anti-pattern**: Missing from schema, wrong type
- **severity**: low
- **verified-by**: TOOL.md lines 62-64

### [pass] Existing Claude persona regression: default behavior preserved
- **message**: "Verify that a persona with agent='claude' still dispatches to claude vendor"
- **expect**: resolve_owner returns agent_flavor='claude' for such persona; resolve_dispatch_agent_flavor with None override returns 'claude'
- **anti-pattern**: Claude persona silently switched to cteno
- **severity**: high
- **verified-by**: Unit test dispatch_uses_persona_agent_when_override_missing at line 682 (tests with arbitrary flavor); resolve_owner faithfully reads persona.agent

### [pass] cargo check -p cteno passes
- **message**: "Run cargo check to verify compilation"
- **expect**: No compilation errors related to this change
- **anti-pattern**: Type mismatch, missing params, wrong signature
- **severity**: high
- **verified-by**: QA ran `cargo check -p cteno` from worktree desktop dir; finished with 0 errors (only pre-existing warnings in multi-agent-runtime-codex)

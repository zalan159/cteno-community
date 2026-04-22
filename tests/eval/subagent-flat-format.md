# Cteno subagent flat-format loader

Verifies that Cteno reads `{dir}/{name}.md` (flat, aligned with Claude/Gemini)
in addition to the legacy `{dir}/{name}/AGENT.md` nested layout.

Low-level coverage: `cargo test -p cteno service_init::subagent_loader_tests`
(5 unit tests — flat load, nested compat, flat-wins on collision, coexistence,
README/hidden filtering).

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-subagent-flat
- max-turns: 5

## setup
```bash
rm -rf /tmp/cteno-subagent-flat
mkdir -p /tmp/cteno-subagent-flat/.cteno/agents
# flat-format subagent
cat > /tmp/cteno-subagent-flat/.cteno/agents/critic.md <<'EOF'
---
name: "critic"
description: "Aggressively challenges the user's assumptions."
version: "1.0.0"
type: "autonomous"
---
Push back hard on unsupported claims. Ask for evidence.
EOF
```

## cases

### [pending] Flat subagent loads and is dispatchable
- **message**: "Dispatch the 'critic' sub-agent with the task 'evaluate this claim: GraphQL is always faster than REST'."
- **expect**: The sub-agent is invoked (uses `agent_critic` or similar tool); its reply clearly challenges the claim (asks for context/benchmarks). Proves the flat `.cteno/agents/critic.md` was discovered.
- **anti-pattern**: Agent says "no sub-agent named critic available" — loader missed the flat file.
- **severity**: high

### [pending] Frontmatter parsing tolerates superset fields
- **setup**: edit `.cteno/agents/critic.md` frontmatter to add Gemini-only fields `temperature: 0.2` and `kind: local`, plus Claude-only `effort: medium` and `permissionMode: acceptEdits`.
- **message**: "List your sub-agents."
- **expect**: Agent lists `critic` with the same description. Extra fields are ignored by Cteno's parser (per spec, unknown fields are `#[serde(default)]`-dropped).
- **anti-pattern**: Loader errors out with "unknown field"; agent disappears from the list.
- **severity**: high

### [pending] Legacy nested format still discovered in same directory
- **setup**: add `.cteno/agents/legacy-agent/AGENT.md` alongside the flat `critic.md`.
- **message**: "List your sub-agents. I expect both 'critic' and 'legacy-agent'."
- **expect**: Both appear.
- **severity**: medium

### [pending] README.md in agents dir doesn't get loaded as an agent
- **setup**: drop a `.cteno/agents/README.md` file.
- **message**: "List your sub-agents."
- **expect**: No agent called "README" appears. Loader skipped it.
- **severity**: low

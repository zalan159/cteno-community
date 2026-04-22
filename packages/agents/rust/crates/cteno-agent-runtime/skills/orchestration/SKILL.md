---
id: orchestration
name: "Orchestration"
description: "Comprehensive multi-step orchestration using user-facing ctenoctl commands (persona, run, agent run) with sequential, parallel, conditional, and loop control"
when_to_use: "Use when a request needs multiple coordinated steps, role separation, retries, branching, or script automation"
version: "3.2.0"
tags:
  - orchestration
  - workflow
  - automation
  - ctenoctl
  - multi-agent
---

# Orchestration Skill

Build reliable multi-step workflows by generating scripts that use **user-facing** `ctenoctl` commands.

## Core Policy

- Use only user-facing commands in this skill:
  - `ctenoctl persona ...`
  - `ctenoctl run --kind ... --message ...`
  - `ctenoctl agent run <id> -m ...`
- Persona creation is explicitly allowed and recommended when task ownership/context matters.
- Keep workflow guidance at the CLI level; do not describe internal task transport internals.
- For long-running workflows, avoid process-level hard kill timeouts. Prefer small resumable steps, retries, and checkpoints.
- Every generated script should include:
  - clear step boundaries
  - explicit retry limits
  - explicit failure handling
  - saved outputs for traceability

## Mode Selection Matrix

Choose mode **before** writing the script.

| Mode | Best For | Stateful Context | Primary Command(s) | Avoid When |
|---|---|---|---|---|
| Persona Mode | Long, iterative collaboration (implement -> review -> fix) | Yes | `ctenoctl persona create/chat/tasks` | You only need one isolated execution |
| Run Mode | Stateless, script-friendly one-shot tasks (batch/CI style) | No | `ctenoctl run --kind ... --message ...` | You need rich multi-turn memory in one owner thread |
| Custom Agent Run Mode | One-shot execution with a specific Markdown custom agent | No (per invocation) | `ctenoctl agent run <id> -m ...` | A builtin kind (`worker`, `browser`) is enough |

## ctenoctl Command Reference

### 1) Persona Mode Commands

`ctenoctl persona list`
- Purpose: list available personas.

`ctenoctl persona create [options]`
- Purpose: create a new orchestration owner.
- Optional flags:
  - `--name, -n <name>`
  - `--description, -d <desc>`
  - `--model <model>`
  - `--avatar <id>`
  - `--profile, -p <id>`
  - `--workdir, -w <dir>`

`ctenoctl persona chat <id> -m <message>`
- Purpose: send one turn to a persona.
- Required:
  - `<id>`
  - `--message, -m <message>`

`ctenoctl persona tasks <id>`
- Purpose: inspect task sessions associated with the persona.

`ctenoctl persona delete <id>`
- Purpose: remove persona when lifecycle ends.

Scripting I/O expectations:
- Output is JSON text. Always parse JSON fields with `jq`/`json` parser where possible.
- For robust scripting, tolerate field shape differences by using fallbacks.

Example (safe persona id extraction):
```bash
PID=$(ctenoctl persona create --name "Workflow Owner" | jq -r '.persona.id // .id // empty')
[ -n "$PID" ] || { echo "Failed to create persona"; exit 1; }
```

Common combinations:
- Create once, then multiple `chat` turns.
- Use `tasks` between phases when you need state/progress inspection.

### 2) Run Mode Commands

Primary execution:

`ctenoctl run --kind <kind> --message <msg> [options]`
- Purpose: run one task and block until completion.
- Required:
  - `--kind, -k <kind>`
  - `--message, -m <msg>`
- Optional:
  - `--profile, -p <id>`
  - `--workdir, -w <dir>`

Run management:
- `ctenoctl run list [--session <id>]`
- `ctenoctl run get <run_id>`
- `ctenoctl run logs <run_id> [--lines <n>]`
- `ctenoctl run stop <run_id>`

Timeout semantics for orchestration:
- Default scripts should not surface timeout/tuning flags.
- Avoid wrapping long tasks with shell hard-kill wrappers (for example `timeout 300 ...`) unless you explicitly want forced termination.
- For long workflows, use checkpoint files and resume-friendly step design instead of global hard timeouts.

Scripting I/O expectations:
- Output is JSON text.
- Non-zero exit code means invocation-level failure (command or execution path failure).
- Prefer storing each step output to a file.

Common combinations:
- `run` for execution + `run logs` for postmortem.
- Keep each command focused and add retry/fallback in the script.

### 3) Custom Agent Run Mode Commands

`ctenoctl agent run <id> -m <message> [--profile <id>] [--workdir <dir>]`
- Purpose: execute a specific Markdown custom agent once.
- Required:
  - `<id>`
  - `--message, -m <message>`
- Optional:
  - `--profile, -p <id>`
  - `--workdir, -w <dir>`

Useful supporting commands:
- `ctenoctl agent list [--workdir <dir>]`
- `ctenoctl agent show <id> [--workdir <dir>]`

Common combinations:
- `agent list` -> choose ID -> `agent run` for deterministic role behavior.

## Orchestration Pattern Library

Each pattern includes Bash and Python templates.

### Pattern A: Sequential Steps

**Bash**
```bash
#!/usr/bin/env bash
set -euo pipefail

ctenoctl run --kind worker -m "Implement feature A" > /tmp/step1.json
ctenoctl run --kind worker -m "Write tests for feature A" > /tmp/step2.json
ctenoctl run --kind worker -m "Summarize implementation + test results" > /tmp/summary.json
```

**Python**
```python
#!/usr/bin/env python3
import subprocess

def run(kind: str, msg: str, out: str) -> None:
    with open(out, "w", encoding="utf-8") as f:
        subprocess.run(
            ["ctenoctl", "run", "--kind", kind, "--message", msg],
            check=True,
            stdout=f,
            text=True,
        )

run("worker", "Implement feature A", "/tmp/step1.json")
run("worker", "Write tests for feature A", "/tmp/step2.json")
run("worker", "Summarize implementation + test results", "/tmp/summary.json")
```

### Pattern B: Parallel Fan-Out + Merge

**Bash**
```bash
#!/usr/bin/env bash
set -euo pipefail

ctenoctl run --kind worker -m "Research subtopic A" > /tmp/a.json &
PID_A=$!
ctenoctl run --kind worker -m "Research subtopic B" > /tmp/b.json &
PID_B=$!
ctenoctl run --kind worker -m "Research subtopic C" > /tmp/c.json &
PID_C=$!

wait "$PID_A" "$PID_B" "$PID_C"

ctenoctl run --kind worker -m "Merge findings from A/B/C into one report" > /tmp/merged.json
```

**Python**
```python
#!/usr/bin/env python3
import subprocess
from concurrent.futures import ThreadPoolExecutor


def run_task(msg: str, out: str) -> None:
    with open(out, "w", encoding="utf-8") as f:
        subprocess.run(
            ["ctenoctl", "run", "--kind", "worker", "--message", msg],
            check=True,
            stdout=f,
            text=True,
        )

jobs = [
    ("Research subtopic A", "/tmp/a.json"),
    ("Research subtopic B", "/tmp/b.json"),
    ("Research subtopic C", "/tmp/c.json"),
]

with ThreadPoolExecutor(max_workers=3) as pool:
    for msg, out in jobs:
        pool.submit(run_task, msg, out)

run_task("Merge findings from A/B/C into one report", "/tmp/merged.json")
```

### Pattern C: Conditional Branching

**Bash**
```bash
#!/usr/bin/env bash
set -euo pipefail

ctenoctl run --kind worker -m "Evaluate release readiness. Output PASS or FAIL." > /tmp/verdict.json

if rg -q "PASS" /tmp/verdict.json; then
  ctenoctl run --kind worker -m "Proceed with release checklist" > /tmp/release.json
else
  ctenoctl run --kind worker -m "Create remediation plan for failed checks" > /tmp/remediation.json
fi
```

**Python**
```python
#!/usr/bin/env python3
import subprocess

verdict = subprocess.run(
    ["ctenoctl", "run", "--kind", "worker", "--message", "Evaluate release readiness. Output PASS or FAIL."],
    capture_output=True,
    text=True,
    check=True,
)

if "PASS" in verdict.stdout:
    subprocess.run(
        ["ctenoctl", "run", "--kind", "worker", "--message", "Proceed with release checklist"],
        check=True,
    )
else:
    subprocess.run(
        ["ctenoctl", "run", "--kind", "worker", "--message", "Create remediation plan for failed checks"],
        check=True,
    )
```

### Pattern D: Retry Loop with Max Attempts

**Bash**
```bash
#!/usr/bin/env bash
set -euo pipefail

MAX_RETRIES=3
for i in $(seq 1 "$MAX_RETRIES"); do
  ctenoctl run --kind worker -m "Attempt $i: fix failing tests" > "/tmp/fix-$i.json"
  ctenoctl run --kind worker -m "Attempt $i: output PASS if tests are clean, otherwise FAIL" > "/tmp/retest-$i.json"

  if rg -q "PASS" "/tmp/retest-$i.json"; then
    echo "Succeeded at attempt $i"
    exit 0
  fi
done

echo "All retries exhausted"
exit 1
```

**Python**
```python
#!/usr/bin/env python3
import subprocess

MAX_RETRIES = 3
for i in range(1, MAX_RETRIES + 1):
    subprocess.run(
        ["ctenoctl", "run", "--kind", "worker", "--message", f"Attempt {i}: fix failing tests"],
        check=True,
    )
    retest = subprocess.run(
        ["ctenoctl", "run", "--kind", "worker", "--message", f"Attempt {i}: output PASS if tests are clean, otherwise FAIL"],
        capture_output=True,
        text=True,
        check=True,
    )
    if "PASS" in retest.stdout:
        print(f"Succeeded at attempt {i}")
        break
else:
    raise SystemExit("All retries exhausted")
```

### Pattern E: Long-Running Steps Without Hard Timeout + Failure Fallback

For long tasks, do not use process-level hard timeouts. Keep steps resumable, persist outputs, and define explicit fallback.

**Bash**
```bash
#!/usr/bin/env bash
set -euo pipefail

if ! ctenoctl run --kind browser -m "Collect pricing data from target pages" > /tmp/primary.json; then
  ctenoctl run --kind worker -m "Fallback: produce best-effort report using cached/internal knowledge and mark data as partial" > /tmp/fallback.json
fi
```

**Python**
```python
#!/usr/bin/env python3
import subprocess

primary = subprocess.run(
    ["ctenoctl", "run", "--kind", "browser", "--message", "Collect pricing data from target pages"],
    capture_output=True,
    text=True,
)

if primary.returncode != 0:
    subprocess.run(
        [
            "ctenoctl", "run", "--kind", "worker",
            "--message", "Fallback: produce best-effort report using cached/internal knowledge and mark data as partial",
        ],
        check=True,
    )
```

Avoid this anti-pattern for long tasks:
```bash
# BAD for long-running orchestration: hard-kills the process mid-step
timeout 300 ctenoctl run --kind worker -m "Long synthesis task"
```

## End-to-End Playbooks

### Playbook 1: Implement -> Test -> Fix

Recommended mode: **Persona Mode** (stateful iteration).

1. Create owner persona.
2. Ask for implementation.
3. Ask for test plan and failure analysis.
4. Run a bounded fix/retest loop (max N).
5. Ask for final summary and risk list.

Example skeleton:
```bash
#!/usr/bin/env bash
set -euo pipefail

PID=$(ctenoctl persona create --name "Feature Owner" | jq -r '.persona.id // .id // empty')
[ -n "$PID" ] || { echo "Failed to create persona"; exit 1; }

ctenoctl persona chat "$PID" -m "Implement feature X with clear patch notes" > /tmp/impl.json
ctenoctl persona chat "$PID" -m "Design tests for feature X and list likely failure points" > /tmp/test-plan.json

for i in 1 2 3; do
  ctenoctl persona chat "$PID" -m "Fix round $i based on latest issues and provide verification evidence" > "/tmp/fix-$i.json"
  ctenoctl persona chat "$PID" -m "Output PASS or FAIL for current state, plus one-line reason" > "/tmp/retest-$i.json"
  rg -q "PASS" "/tmp/retest-$i.json" && break
done

ctenoctl persona chat "$PID" -m "Provide final summary: done, open risks, and next actions" > /tmp/final.json
```

### Playbook 2: Research -> Draft -> Consolidate

Recommended mode: **Run Mode** for stateless parallel research, optional `agent run` for specialized consolidation.

1. Parallel research fan-out using `ctenoctl run`.
2. Consolidate outputs into one draft.
3. Optionally run a custom editor/reviewer agent for style/quality pass.

Example skeleton:
```bash
#!/usr/bin/env bash
set -euo pipefail

ctenoctl run --kind worker -m "Research topic A with citations" > /tmp/A.json &
ctenoctl run --kind worker -m "Research topic B with citations" > /tmp/B.json &
ctenoctl run --kind worker -m "Research topic C with citations" > /tmp/C.json &
wait

ctenoctl run --kind worker -m "Create one consolidated draft from A/B/C" > /tmp/draft.json
ctenoctl agent run editorial-reviewer -m "Polish /tmp/draft.json into final publish-ready copy" > /tmp/final-copy.json
```

## Anti-Patterns and Guardrails

Do not:
- Use Persona Mode for simple single-shot CI tasks.
- Use Run Mode when you need stable ownership memory across many turns.
- Run unbounded loops without a max retry count.
- Mix unrelated projects in the same persona thread.
- Ignore non-zero exit codes.
- Add aggressive process-level hard timeouts to long-running steps.

Always:
- Add `set -euo pipefail` in Bash scripts.
- Save each step output to a file.
- Keep prompts explicit about output format (`PASS/FAIL`, JSON fields, checklist format).
- Keep each step narrow and resumable; add retry/fallback instead of exposing tuning flags by default.
- Add a fallback path for critical workflows.
- For long tasks, prefer resumable step boundaries over hard-kill timeout wrappers.

## Quick Authoring Checklist

Before returning an orchestration script, verify:

1. Mode selection is explicit (`persona` / `run` / `agent run`).
2. Every step has a clear input and expected output.
3. Parallel steps are independent.
4. Retry loops have max attempts.
5. Failure path is explicit.
6. Script is directly runnable without hidden assumptions.

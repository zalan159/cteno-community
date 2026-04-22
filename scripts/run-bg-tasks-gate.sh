#!/bin/bash
# Run the auth task-gate workflow + orphan guard as a single, process-detached
# script. Start via `setsid nohup` so it survives terminal / Claude session exit:
#
#   cd /Users/zal/Cteno2.0
#   setsid nohup ./scripts/run-auth-gate.sh > /tmp/auth-gate.out 2>&1 &
#
# Stop with:  pkill -f run-auth-gate.sh

set -u
REPO=/Users/zal/Cteno2.0
cd "$REPO"

TASKS=00-management/tasks-bg-tasks.json
LOG=/tmp/bg-tasks-workflow.log
LOOPLOG=/tmp/bg-tasks-loop.log
GUARDLOG=/tmp/bg-tasks-guard.log
# WORKTREE resolved dynamically per-iteration from state file
WORKTREE=''

# ---- guard subshell (runs in parallel) ------------------------------------
(
  while true; do
    sleep 60
    cd "$REPO"
    python3 <<PY >> "$GUARDLOG" 2>&1
import json, os, time, subprocess, datetime
p = '$REPO/$TASKS'
try:
    with open('$REPO/00-management/.task-gate-state.bg-tasks.json') as wf:
        wt = json.load(wf).get('worktreeDir', '')
except Exception:
    wt = ''
with open(p) as f: d = json.load(f)
ts = datetime.datetime.now().strftime('%H:%M:%S')
changed = False

# Policy 1: QA-discarded → reset with +2 maxAttempts
for i in d['items']:
    if i.get('status') != 'discarded': continue
    tid = i['id']
    try:
        res = subprocess.run(['git','-C',wt,'log','--oneline','--grep',f'task({tid})','-n','1'],
                             capture_output=True, text=True, timeout=10)
        committed = bool(res.stdout.strip())
    except Exception:
        committed = False
    if not committed: continue
    old_max = i['maxAttempts']
    i['maxAttempts'] = old_max + 2
    i['attempts'] = 0
    i['status'] = 'pending'
    meta = i.setdefault('metadata', {})
    meta['qaRejects'] = meta.get('qaRejects', 0) + 1
    print(f"[{ts}] qa-retry: {tid} discarded→pending maxAttempts {old_max}→{i['maxAttempts']}")
    changed = True

# Policy 2: orphan running → reset. Only touch OUR runId's node (read pid
# from .task-gate-state.bg-tasks.json), so this guard doesn't kill the auth
# workflow's node that is sharing the same binary path.
running = [i for i in d['items'] if i.get('status')=='running']
if running:
    my_pid = None
    try:
        with open('$REPO/00-management/.task-gate-state.bg-tasks.json') as sf:
            my_pid = json.load(sf).get('pid')
    except Exception:
        my_pid = None
    if my_pid:
        try:
            os.kill(my_pid, 0)
            no_node = False
        except Exception:
            no_node = True
    else:
        no_node = True
    age = time.time() - os.path.getmtime(p)
    if no_node or age > 600:
        resets = []
        for i in running:
            i['status'] = 'pending'
            resets.append(i['id'])
        if not no_node and my_pid:
            try: os.kill(my_pid, 9)
            except Exception: pass
        print(f"[{ts}] orphan-reset: {','.join(resets)} (my_pid={my_pid}, no_node={no_node}, age={int(age)}s)")
        changed = True

if changed:
    with open(p,'w') as f:
        json.dump(d, f, indent=2, ensure_ascii=False); f.write('\n')
PY
  done
) &
GUARD_PID=$!
echo "[gate] guard pid=$GUARD_PID" >> "$LOOPLOG"

# ---- main loop (fg in this script) ----------------------------------------
trap "kill $GUARD_PID 2>/dev/null; pkill -9 -f rpc-parity-task-gate.mjs 2>/dev/null; exit" TERM INT

while true; do
  cd "$REPO"
  remaining=$(python3 -c "
import json
d=json.load(open('$TASKS'))
n=sum(1 for i in d['items'] if i['status'] in ('pending','running'))
print(n)" 2>/dev/null)
  if [ -z "$remaining" ] || [ "$remaining" = "0" ]; then
    echo "[loop $(date +%H:%M:%S)] all settled, exit" >> "$LOOPLOG"
    break
  fi
  echo "[loop $(date +%H:%M:%S)] $remaining remaining" >> "$LOOPLOG"
  # Only set RESUME if we already have a state file (subsequent runs);
  # first run must go fresh to actually create the state file.
  RESUME_ENV=""
  if [ -f "$REPO/00-management/.task-gate-state.bg-tasks.json" ]; then
    RESUME_ENV="TASK_GATE_RESUME=1"
  fi
  env TASK_GATE_TASKS_PATH="$TASKS" \
      TASK_GATE_RUN_ID=bg-tasks \
      TASK_GATE_ITEM_TIMEOUT_MS=3600000 \
      $RESUME_ENV \
      node "$REPO/packages/multi-agent-runtime/examples/rpc-parity-task-gate.mjs" >> "$LOG" 2>&1 || true
  sleep 5
done

# cleanup
kill $GUARD_PID 2>/dev/null
echo "[gate $(date +%H:%M:%S)] exit" >> "$LOOPLOG"

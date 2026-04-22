#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import shlex
import signal
import subprocess
import sys
import time
import uuid
from pathlib import Path


def log(message: str) -> None:
    print(f"[ctenoctl-e2e] {message}", flush=True)


class CommandError(RuntimeError):
    pass


def with_target(args: list[str], target: str | None) -> list[str]:
    if not target:
        return args
    return [args[0], "--target", target, *args[1:]]


def run_cmd(
    args: list[str],
    *,
    expect_json: bool = False,
    env: dict[str, str] | None = None,
    target: str | None = None,
) -> object:
    args = with_target(args, target)
    log("$ " + " ".join(shlex.quote(part) for part in args))
    completed = subprocess.run(
        args,
        check=False,
        text=True,
        capture_output=True,
        env=env,
    )
    if completed.returncode != 0:
        raise CommandError(
            f"command failed ({completed.returncode}): {' '.join(args)}\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    stdout = completed.stdout.strip()
    if expect_json:
        try:
            return json.loads(stdout)
        except json.JSONDecodeError as exc:
            raise CommandError(f"expected JSON from {' '.join(args)} but got:\n{stdout}") from exc
    return stdout


def wait_until(label: str, fn, *, timeout_s: int = 180, interval_s: float = 2.0):
    deadline = time.time() + timeout_s
    last_error = None
    while time.time() < deadline:
        try:
            value = fn()
            if value:
                return value
        except Exception as exc:  # noqa: BLE001
            last_error = exc
        time.sleep(interval_s)
    if last_error:
        raise CommandError(f"timed out waiting for {label}: {last_error}") from last_error
    raise CommandError(f"timed out waiting for {label}")


def as_rows(value: object, *keys: str) -> list[dict]:
    if isinstance(value, list):
        return [item for item in value if isinstance(item, dict)]
    if isinstance(value, dict):
        for key in keys:
            rows = value.get(key)
            if isinstance(rows, list):
                return [item for item in rows if isinstance(item, dict)]
    return []


def assert_contains(text: str, fragments: list[str], *, label: str) -> None:
    missing = [fragment for fragment in fragments if fragment not in text]
    if missing:
        raise CommandError(f"{label} missing expected fragments: {missing}\n{text}")


def assert_checklist(text: str) -> None:
    lines = [line.strip() for line in text.splitlines() if line.strip().startswith("- [ ]")]
    if not (12 <= len(lines) <= 30):
        raise CommandError(f"expected 12-30 checklist lines, got {len(lines)}")
    required_groups = [
        ["Cash", "现金", "银行", "现金流"],
        ["Invoices", "发票", "应收", "应付"],
        ["Subscriptions", "订阅", "SaaS", "renewal", "services"],
        ["Contractors", "承包商", "外包", "薪资", "Payroll"],
        ["Tax", "税", "个税", "增值税", "申报", "taxes", "tax liability", "estimated payment"],
        ["KPI", "毛利率", "净利率", "关键客户", "指标", "metrics", "burn rate", "customer acquisition cost"],
    ]
    missing = [group for group in required_groups if not any(token in text for token in group)]
    if missing:
        raise CommandError(f"opc checklist missing expected semantic groups: {missing}\n{text}")


def current_daemon_pid(ctenoctl: str, *, target: str | None = None) -> int | None:
    status = run_cmd([ctenoctl, "status"], target=target)
    if not isinstance(status, str):
        return None
    for line in status.splitlines():
        if line.startswith("daemon_pid:"):
            value = line.split(":", 1)[1].strip()
            if value.isdigit():
                return int(value)
    return None


def status_map(ctenoctl: str, *, target: str | None = None) -> dict[str, str]:
    raw = run_cmd([ctenoctl, "status"], target=target)
    if not isinstance(raw, str):
        return {}
    rows: dict[str, str] = {}
    for line in raw.splitlines():
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        rows[key.strip()] = value.strip()
    return rows


def daemon_services_ready(ctenoctl: str, *, target: str | None = None) -> bool:
    return status_map(ctenoctl, target=target).get("services_ready") == "yes"


def daemon_target_connected(ctenoctl: str, *, target: str | None = None) -> bool:
    rows = status_map(ctenoctl, target=target)
    if rows.get("daemon_target_connected") == "yes":
        return True
    active = rows.get("daemon_socket_active")
    return bool(active and rows.get("daemon_socket_active_connectable") == "yes")


def ensure_target_connected(ctenoctl: str, *, target: str | None = None) -> None:
    rows = status_map(ctenoctl, target=target)
    if rows.get("services_ready") != "yes":
        raise CommandError(f"target {target or 'auto'} services are not ready")
    if not daemon_target_connected(ctenoctl, target=target):
        raise CommandError(
            "target {target} is not connected to a live local RPC host "
            "(socket={socket}, connectable={connectable})".format(
                target=target or rows.get("daemon_target_selected", "auto"),
                socket=rows.get("daemon_socket_active", "unknown"),
                connectable=rows.get("daemon_socket_active_connectable", "no"),
            )
        )


def restart_daemon(ctenoctl: str, agentd_cmd: str, log_file: Path, *, target: str | None = None) -> None:
    pid = current_daemon_pid(ctenoctl, target=target)
    if pid:
        log(f"Stopping existing daemon pid={pid}")
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        wait_until(
            "daemon to stop",
            lambda: "daemon_running: no" in str(run_cmd([ctenoctl, "status"], target=target)),
            timeout_s=30,
            interval_s=1,
        )

    log(f"Starting daemon with: {agentd_cmd}")
    with log_file.open("a", encoding="utf-8") as handle:
        subprocess.Popen(  # noqa: S603
            ["/bin/zsh", "-lc", agentd_cmd],
            stdout=handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )

    wait_until(
        "daemon to start",
        lambda: "daemon_running: yes" in str(run_cmd([ctenoctl, "status"], target=target)),
        timeout_s=60,
        interval_s=2,
    )
    wait_until(
        "daemon services to be ready",
        lambda: daemon_services_ready(ctenoctl, target=target),
        timeout_s=90,
        interval_s=2,
    )
    ensure_target_connected(ctenoctl, target=target)


def bootstrap_workspace(ctenoctl: str, template: str, workdir: Path, model: str, *, target: str | None = None) -> dict:
    workdir.mkdir(parents=True, exist_ok=True)
    return run_cmd(
        [
            ctenoctl,
            "workspace",
            "bootstrap",
            "-t",
            template,
            "-n",
            f"{template}-{uuid.uuid4().hex[:8]}",
            "-w",
            str(workdir),
            "--model",
            model,
        ],
        expect_json=True,
        target=target,
    )


def send_workspace(ctenoctl: str, persona_id: str, message: str, *, target: str | None = None) -> dict:
    return run_cmd(
        [ctenoctl, "workspace", "send", persona_id, "-m", message],
        expect_json=True,
        target=target,
    )


def workspace_activity(ctenoctl: str, persona_id: str, limit: int = 30, *, target: str | None = None) -> dict:
    return run_cmd(
        [ctenoctl, "workspace", "activity", persona_id, "--limit", str(limit)],
        expect_json=True,
        target=target,
    )


def workspace_events(ctenoctl: str, persona_id: str, limit: int = 40, *, target: str | None = None) -> dict:
    return run_cmd(
        [ctenoctl, "workspace", "events", persona_id, "--limit", str(limit)],
        expect_json=True,
        target=target,
    )


def workspace_get(ctenoctl: str, persona_id: str, *, target: str | None = None) -> dict:
    return run_cmd([ctenoctl, "workspace", "get", persona_id], expect_json=True, target=target)


def workspace_delete(ctenoctl: str, persona_id: str, *, target: str | None = None) -> dict:
    return run_cmd([ctenoctl, "workspace", "delete", persona_id], expect_json=True, target=target)


def workspace_list(ctenoctl: str, *, target: str | None = None) -> dict:
    return run_cmd([ctenoctl, "workspace", "list"], expect_json=True, target=target)


def verify_coding(ctenoctl: str, workspace: dict, workdir: Path, *, target: str | None = None) -> None:
    persona_id = workspace["workspace"]["personaId"]
    send_workspace(
        ctenoctl,
        persona_id,
        "请直接编写 group mentions 的 PRD 初稿并保存到 10-prd/group-mentions.md。要求包含 Goal、User Story、Scope、Non-Goals、Acceptance Criteria 五个章节。",
        target=target,
    )
    artifact_path = workdir / "10-prd" / "group-mentions.md"
    wait_until("coding artifact", lambda: artifact_path.exists(), timeout_s=240, interval_s=3)
    text = artifact_path.read_text(encoding="utf-8")
    assert_contains(text, ["Goal", "User Story", "Scope", "Acceptance Criteria"], label="coding PRD")


def verify_opc(ctenoctl: str, workspace: dict, workdir: Path, *, target: str | None = None) -> None:
    persona_id = workspace["workspace"]["personaId"]
    send_workspace(
        ctenoctl,
        persona_id,
        "请直接编写一个非常简短的 Markdown checklist 文件，并保存到 company/10-finance/monthly-close-checklist.md。要求：1) 只输出 12-18 条待办；2) 每条都用 - [ ]；3) 覆盖 Cash Review、Invoices、Subscriptions、Contractors/Payroll、Tax Prep、KPI Review；4) 不要解释，不要写计划，直接完成文件。",
        target=target,
    )
    artifact_path = workdir / "company" / "10-finance" / "monthly-close-checklist.md"
    wait_until("opc artifact", lambda: artifact_path.exists(), timeout_s=240, interval_s=3)
    assert_checklist(artifact_path.read_text(encoding="utf-8"))


def verify_autoresearch(ctenoctl: str, workspace: dict, workdir: Path, *, target: str | None = None) -> None:
    persona_id = workspace["workspace"]["personaId"]
    send_workspace(
        ctenoctl,
        persona_id,
        "请研究 group mentions 的协作模式，进入 autoresearch workflow，先写一份假设与成功标准 brief 到 research/00-lead/hypothesis-brief.md。",
        target=target,
    )
    artifact_path = workdir / "research" / "00-lead" / "hypothesis-brief.md"
    wait_until("autoresearch artifact", lambda: artifact_path.exists(), timeout_s=300, interval_s=3)
    activities = workspace_activity(ctenoctl, persona_id, 40, target=target)
    activity_kinds = [item.get("kind") for item in as_rows(activities, "items", "activities")]
    if "workflow_started" not in activity_kinds:
        raise CommandError(f"autoresearch activity missing workflow_started: {activity_kinds}")
    events = workspace_events(ctenoctl, persona_id, 50, target=target)
    event_types = [item.get("type") for item in as_rows(events, "items", "events")]
    for expected in ("workflow_vote_opened", "workflow_vote_closed", "workflow_started"):
        if expected not in event_types:
            raise CommandError(f"autoresearch events missing {expected}: {event_types}")
    text = artifact_path.read_text(encoding="utf-8")
    required_heading_groups = [
        [
            "研究主题",
            "Research Topic",
            "Research Questions",
            "Research Question",
            "研究问题",
            "核心问题",
        ],
        ["核心假设", "Core Hypothesis", "假设", "Hypothesis", "Hypothesis Statements", "主要假设"],
        ["成功标准", "Success Criteria"],
    ]
    missing_groups = [
        group for group in required_heading_groups if not any(fragment in text for fragment in group)
    ]
    if missing_groups:
        raise CommandError(f"autoresearch brief missing expected semantic headings: {missing_groups}\n{text}")


def verify_restore(ctenoctl: str, coding_persona_id: str, coding_workdir: Path, *, target: str | None = None) -> None:
    workspace_get(ctenoctl, coding_persona_id, target=target)
    send_workspace(
        ctenoctl,
        coding_persona_id,
        "请再补一份一行状态说明到 10-prd/status-note.md，说明 PRD 已创建。",
        target=target,
    )
    artifact_path = coding_workdir / "10-prd" / "status-note.md"
    wait_until("restored coding follow-up artifact", lambda: artifact_path.exists(), timeout_s=180, interval_s=3)
    text = artifact_path.read_text(encoding="utf-8")
    assert_contains(text, ["PRD"], label="coding restore follow-up")


def verify_delete(
    ctenoctl: str,
    persona_id: str,
    workdir: Path,
    workspace_id: str,
    *,
    target: str | None = None,
) -> None:
    workspace_delete(ctenoctl, persona_id, target=target)
    runtime_root = workdir / ".multi-agent-runtime" / workspace_id

    def workspace_still_present() -> bool:
        listing = workspace_list(ctenoctl, target=target)
        rows = as_rows(listing, "items", "workspaces")
        return any(row.get("personaId") == persona_id for row in rows)

    wait_until(
        "workspace removal from list",
        lambda: not workspace_still_present(),
        timeout_s=60,
        interval_s=2,
    )
    if runtime_root.exists():
        raise CommandError(f"runtime directory still exists after delete: {runtime_root}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Run ctenoctl workspace e2e against local agentd.")
    parser.add_argument("--ctenoctl", default="/tmp/ctenoctl")
    parser.add_argument("--agentd-cmd", default="")
    parser.add_argument("--model", default="deepseek-reasoner")
    parser.add_argument("--target", choices=["agentd", "tauri-dev", "tauri"], default=None)
    parser.add_argument("--base-dir", default="/tmp/ctenoctl-workspace-e2e")
    args = parser.parse_args()

    ctenoctl = args.ctenoctl
    base_dir = Path(args.base_dir) / time.strftime("%Y%m%d-%H%M%S")
    base_dir.mkdir(parents=True, exist_ok=True)

    if args.agentd_cmd:
        restart_daemon(ctenoctl, args.agentd_cmd, base_dir / "agentd-bootstrap.log", target=args.target)
    else:
        ensure_target_connected(ctenoctl, target=args.target)

    templates = run_cmd([ctenoctl, "workspace", "templates"], expect_json=True, target=args.target)
    template_rows = as_rows(templates, "items", "templates")
    template_ids = {item["id"] for item in template_rows}
    required = {"coding-studio", "opc-solo-company", "autoresearch"}
    if not required.issubset(template_ids):
        raise CommandError(f"missing required templates: {required - template_ids}")

    coding_dir = base_dir / "coding"
    opc_dir = base_dir / "opc"
    autoresearch_dir = base_dir / "autoresearch"

    coding = bootstrap_workspace(ctenoctl, "coding-studio", coding_dir, args.model, target=args.target)
    opc = bootstrap_workspace(ctenoctl, "opc-solo-company", opc_dir, args.model, target=args.target)
    autoresearch = bootstrap_workspace(ctenoctl, "autoresearch", autoresearch_dir, args.model, target=args.target)

    verify_coding(ctenoctl, coding, coding_dir, target=args.target)
    verify_opc(ctenoctl, opc, opc_dir, target=args.target)
    verify_autoresearch(ctenoctl, autoresearch, autoresearch_dir, target=args.target)

    if args.agentd_cmd:
        restart_daemon(ctenoctl, args.agentd_cmd, base_dir / "agentd-restart.log", target=args.target)
        verify_restore(ctenoctl, coding["workspace"]["personaId"], coding_dir, target=args.target)

    verify_delete(ctenoctl, coding["workspace"]["personaId"], coding_dir, coding["workspace"]["id"], target=args.target)
    verify_delete(ctenoctl, opc["workspace"]["personaId"], opc_dir, opc["workspace"]["id"], target=args.target)
    verify_delete(
        ctenoctl,
        autoresearch["workspace"]["personaId"],
        autoresearch_dir,
        autoresearch["workspace"]["id"],
        target=args.target,
    )

    result = {
        "success": True,
        "model": args.model,
        "baseDir": str(base_dir),
        "codingPersonaId": coding["workspace"]["personaId"],
        "opcPersonaId": opc["workspace"]["personaId"],
        "autoresearchPersonaId": autoresearch["workspace"]["personaId"],
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CommandError as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(1)

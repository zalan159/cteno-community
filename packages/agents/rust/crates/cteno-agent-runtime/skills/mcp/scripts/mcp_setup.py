#!/usr/bin/env python3
"""Parse MCP docs, auto-install dependencies, and register MCP servers via local RPC."""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import socket
import subprocess
import sys
import urllib.request
import uuid
from pathlib import Path
from typing import Any


def sanitize_id(raw: str) -> str:
    cleaned = re.sub(r"[^a-zA-Z0-9_-]+", "-", raw.strip().lower()).strip("-_")
    if not cleaned:
        cleaned = "mcp-server"
    if cleaned[0].isdigit():
        cleaned = f"mcp-{cleaned}"
    return cleaned


def socket_path() -> Path:
    override = os.environ.get("CTENO_RPC_SOCKET")
    if override:
        return Path(override)

    home = Path.home()
    env_tag = os.environ.get("CTENO_ENV", "").strip()
    if env_tag and env_tag != "release":
        return home / ".agents" / f"daemon.{env_tag}.sock"

    dev_sock = home / ".agents" / "daemon.dev.sock"
    if dev_sock.exists():
        return dev_sock
    return home / ".agents" / "daemon.sock"


def rpc_call(method: str, params: dict[str, Any]) -> Any:
    sock = socket_path()
    if not sock.exists():
        raise RuntimeError(f"RPC socket not found: {sock}")

    req_id = str(uuid.uuid4())
    payload = {
        "id": req_id,
        "method": method,
        "params": params,
    }

    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(20)
        client.connect(str(sock))
        client.sendall((json.dumps(payload, ensure_ascii=False) + "\n").encode("utf-8"))

        chunks: list[bytes] = []
        while True:
            b = client.recv(4096)
            if not b:
                break
            chunks.append(b)
            if b"\n" in b:
                break

    if not chunks:
        raise RuntimeError(f"No RPC response for method {method}")

    line = b"".join(chunks).split(b"\n", 1)[0].decode("utf-8", errors="replace")
    response = json.loads(line)

    if response.get("id") != req_id:
        raise RuntimeError(f"RPC id mismatch for method {method}")
    if "error" in response and response["error"]:
        raise RuntimeError(f"RPC error ({method}): {response['error']}")

    return response.get("result")


def strip_json_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    text = re.sub(r"(^|\s)//.*$", "", text, flags=re.MULTILINE)
    text = re.sub(r",\s*([}\]])", r"\1", text)
    return text


def iter_fenced_blocks(text: str) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for m in re.finditer(r"```([a-zA-Z0-9_-]*)\n(.*?)```", text, flags=re.DOTALL):
        lang = (m.group(1) or "").strip().lower()
        body = m.group(2)
        out.append((lang, body))
    return out


def try_parse_json(text: str) -> Any | None:
    text = text.strip()
    if not text:
        return None

    for candidate in (text, strip_json_comments(text)):
        try:
            return json.loads(candidate)
        except Exception:
            pass

    return None


def is_url(value: str) -> bool:
    return value.startswith("http://") or value.startswith("https://")


def read_input_source(source: str) -> tuple[str, str]:
    p = Path(source)
    if p.exists() and p.is_file():
        return p.read_text(encoding="utf-8", errors="replace"), "file"

    if is_url(source):
        with urllib.request.urlopen(source, timeout=20) as resp:
            data = resp.read().decode("utf-8", errors="replace")
            return data, "url"

    return source, "text"


def first_non_flag(args: list[str]) -> str | None:
    skip_next = False
    for token in args:
        if skip_next:
            skip_next = False
            continue
        if token in {"-p", "--port", "-c", "--config", "--env-file"}:
            skip_next = True
            continue
        if token.startswith("-"):
            continue
        return token
    return None


def derive_install_commands(command: str, args: list[str]) -> list[list[str]]:
    cmd = Path(command).name.lower()
    pkg = first_non_flag(args)
    if not pkg:
        return []

    if cmd == "npx":
        return [["npm", "install", "-g", pkg]]
    if cmd == "bunx":
        return [["bun", "add", "-g", pkg]]
    if cmd == "uvx":
        return [["uv", "tool", "install", pkg]]
    if cmd in {"python", "python3"} and len(args) >= 2 and args[0] == "-m":
        return [["python3", "-m", "pip", "install", "-U", args[1]]]
    return []


def build_config_from_entry(server_key: str, entry: dict[str, Any]) -> dict[str, Any]:
    server_id = sanitize_id(server_key)
    name = str(entry.get("name") or server_key)

    if isinstance(entry.get("url"), str) and entry["url"].strip():
        return {
            "id": server_id,
            "name": name,
            "enabled": True,
            "transport": {
                "type": "http_sse",
                "url": entry["url"].strip(),
                "headers": entry.get("headers", {}) if isinstance(entry.get("headers"), dict) else {},
            },
        }

    if isinstance(entry.get("command"), str) and entry["command"].strip():
        args = entry.get("args", []) if isinstance(entry.get("args"), list) else []
        env = entry.get("env", {}) if isinstance(entry.get("env"), dict) else {}
        return {
            "id": server_id,
            "name": name,
            "enabled": True,
            "transport": {
                "type": "stdio",
                "command": entry["command"].strip(),
                "args": [str(x) for x in args],
                "env": {str(k): str(v) for k, v in env.items()},
            },
        }

    raise RuntimeError(f"Server entry '{server_key}' has neither 'command' nor 'url'")


def parse_from_json_obj(obj: Any, server_hint: str | None) -> tuple[dict[str, Any], str] | None:
    if not isinstance(obj, dict):
        return None

    mcp_servers = obj.get("mcpServers")
    if isinstance(mcp_servers, dict) and mcp_servers:
        if server_hint and server_hint in mcp_servers:
            key = server_hint
        else:
            key = next(iter(mcp_servers.keys()))

        entry = mcp_servers.get(key)
        if isinstance(entry, dict):
            return build_config_from_entry(str(key), entry), f"json:mcpServers:{key}"

    if isinstance(obj.get("command"), str) or isinstance(obj.get("url"), str):
        key = server_hint or str(obj.get("name") or "mcp-server")
        return build_config_from_entry(key, obj), "json:single"

    transport = obj.get("transport")
    if isinstance(transport, dict) and isinstance(obj.get("id"), str) and isinstance(obj.get("name"), str):
        t = transport.get("type")
        if t == "stdio" and isinstance(transport.get("command"), str):
            return {
                "id": sanitize_id(obj["id"]),
                "name": str(obj["name"]),
                "enabled": bool(obj.get("enabled", True)),
                "transport": {
                    "type": "stdio",
                    "command": transport["command"],
                    "args": transport.get("args", []) if isinstance(transport.get("args"), list) else [],
                    "env": transport.get("env", {}) if isinstance(transport.get("env"), dict) else {},
                },
            }, "json:transport"
        if t == "http_sse" and isinstance(transport.get("url"), str):
            return {
                "id": sanitize_id(obj["id"]),
                "name": str(obj["name"]),
                "enabled": bool(obj.get("enabled", True)),
                "transport": {
                    "type": "http_sse",
                    "url": transport["url"],
                    "headers": transport.get("headers", {}) if isinstance(transport.get("headers"), dict) else {},
                },
            }, "json:transport"

    return None


def parse_from_command_lines(text: str) -> tuple[dict[str, Any], str] | None:
    candidates: list[str] = []
    for lang, body in iter_fenced_blocks(text):
        if lang in {"bash", "sh", "zsh", "shell", ""}:
            candidates.extend(body.splitlines())

    if not candidates:
        candidates = text.splitlines()

    preferred = []
    fallback = []
    for raw in candidates:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if re.match(r"^(npx|bunx|uvx|python3?|node)\b", line):
            if "mcp" in line.lower():
                preferred.append(line)
            else:
                fallback.append(line)

    line = preferred[0] if preferred else (fallback[0] if fallback else None)
    if not line:
        inline = re.search(
            r"(npx|bunx|uvx|python3?|node)\s+[^\n`]+",
            text,
            flags=re.IGNORECASE,
        )
        if inline:
            line = inline.group(0).strip()
    if not line:
        return None

    parts = shlex.split(line)
    if not parts:
        return None

    command = parts[0]
    args = parts[1:]
    key = first_non_flag(args) or Path(command).name
    key = sanitize_id(key.replace("@", "").replace("/", "-"))

    config = {
        "id": key,
        "name": key,
        "enabled": True,
        "transport": {
            "type": "stdio",
            "command": command,
            "args": args,
            "env": {},
        },
    }
    return config, f"command:{line}"


def parse_mcp_doc(text: str, server_hint: str | None) -> tuple[dict[str, Any], str]:
    parsed_full = try_parse_json(text)
    if parsed_full is not None:
        out = parse_from_json_obj(parsed_full, server_hint)
        if out:
            return out

    for lang, body in iter_fenced_blocks(text):
        if lang in {"json", "jsonc", "javascript", "js", "ts", ""}:
            obj = try_parse_json(body)
            if obj is None:
                continue
            out = parse_from_json_obj(obj, server_hint)
            if out:
                return out

    cmd_out = parse_from_command_lines(text)
    if cmd_out:
        return cmd_out

    raise RuntimeError(
        "Could not extract MCP config from input. Provide mcpServers JSON or a runnable command."
    )


def maybe_install(config: dict[str, Any], dry_run: bool, continue_on_error: bool) -> list[dict[str, Any]]:
    transport = config.get("transport", {})
    if not isinstance(transport, dict):
        return []
    if transport.get("type") != "stdio":
        return []

    command = str(transport.get("command") or "")
    args = transport.get("args", []) if isinstance(transport.get("args"), list) else []
    install_cmds = derive_install_commands(command, [str(x) for x in args])

    results: list[dict[str, Any]] = []
    for cmd in install_cmds:
        item: dict[str, Any] = {"command": " ".join(shlex.quote(x) for x in cmd)}
        if dry_run:
            item["status"] = "skipped(dry-run)"
            results.append(item)
            continue

        proc = subprocess.run(cmd, text=True, capture_output=True)
        item["status"] = "ok" if proc.returncode == 0 else "failed"
        item["exit_code"] = proc.returncode
        item["stdout_tail"] = "\n".join(proc.stdout.splitlines()[-10:])
        item["stderr_tail"] = "\n".join(proc.stderr.splitlines()[-10:])
        results.append(item)

        if proc.returncode != 0 and not continue_on_error:
            raise RuntimeError(
                f"Install command failed: {item['command']}\n{item.get('stderr_tail', '')}".strip()
            )

    return results


def register_server(config: dict[str, Any], dry_run: bool) -> dict[str, Any]:
    server_id = config["id"]

    if dry_run:
        return {
            "status": "skipped(dry-run)",
            "server_id": server_id,
        }

    listed = rpc_call("list-mcp-servers", {})
    servers = listed.get("servers", []) if isinstance(listed, dict) else []
    exists = any(isinstance(s, dict) and s.get("id") == server_id for s in servers)

    if exists:
        remove_res = rpc_call("remove-mcp-server", {"serverId": server_id})
        if isinstance(remove_res, dict) and remove_res.get("success") is False:
            raise RuntimeError(remove_res.get("error") or f"Failed to remove existing server '{server_id}'")

    add_res = rpc_call("add-mcp-server", config)
    if isinstance(add_res, dict) and add_res.get("success") is False:
        raise RuntimeError(add_res.get("error") or f"Failed to add server '{server_id}'")

    listed_after = rpc_call("list-mcp-servers", {})
    servers_after = listed_after.get("servers", []) if isinstance(listed_after, dict) else []
    visible = any(isinstance(s, dict) and s.get("id") == server_id for s in servers_after)

    return {
        "status": "ok" if visible else "warning:not-visible",
        "server_id": server_id,
        "already_existed": exists,
        "add_result": add_res,
        "visible_in_list": visible,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Auto setup MCP server from docs.")
    parser.add_argument("--input", required=True, help="Path/URL/raw text containing MCP docs")
    parser.add_argument("--server", help="Server key when mcpServers has multiple entries")
    parser.add_argument("--id", dest="override_id", help="Override server id")
    parser.add_argument("--name", dest="override_name", help="Override server display name")
    parser.add_argument("--skip-install", action="store_true", help="Skip dependency installation")
    parser.add_argument("--skip-register", action="store_true", help="Skip RPC registration")
    parser.add_argument("--continue-on-install-error", action="store_true", help="Continue even when install fails")
    parser.add_argument("--dry-run", action="store_true", help="Parse only; do not install/register")
    args = parser.parse_args()

    result: dict[str, Any] = {
        "ok": False,
        "input": args.input,
    }

    try:
        text, input_kind = read_input_source(args.input)
        config, parsed_from = parse_mcp_doc(text, args.server)

        if args.override_id:
            config["id"] = sanitize_id(args.override_id)
        if args.override_name:
            config["name"] = args.override_name

        result["input_kind"] = input_kind
        result["parsed_from"] = parsed_from
        result["config"] = config

        if args.skip_install:
            result["install"] = {"status": "skipped"}
        else:
            installs = maybe_install(config, dry_run=args.dry_run, continue_on_error=args.continue_on_install_error)
            result["install"] = {
                "status": "ok",
                "steps": installs,
            }

        if args.skip_register:
            result["register"] = {"status": "skipped"}
        else:
            result["register"] = register_server(config, dry_run=args.dry_run)

        result["ok"] = True
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0

    except Exception as exc:
        result["error"] = str(exc)
        print(json.dumps(result, ensure_ascii=False, indent=2), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

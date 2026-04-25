//! ctenoctl — Cteno CLI (thin client connecting to daemon via Unix socket RPC).

pub use commercial_impl::run;

mod commercial_impl {

    use crate::headless_auth;
    use qrcode::{render::unicode, QrCode};
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::time::{sleep, Duration};

    use super::super::super::core;
    use super::super::super::daemon_runtime::{self, daemon_state_path, DaemonState};
    use super::super::super::local_rpc_server;

    pub fn run(argv0: Option<String>, args: Vec<String>) -> i32 {
        let argv0 = argv0.unwrap_or_else(|| "ctenoctl".to_string());

        let mut dev_mode = false;
        let mut target_override: Option<String> = None;
        let mut args = args;

        loop {
            match args.first().map(String::as_str) {
                Some("--dev") => {
                    dev_mode = true;
                    args = args[1..].to_vec();
                }
                Some("--target") => {
                    let Some(value) = args.get(1).cloned() else {
                        eprintln!("Missing value for --target. Use agentd, tauri-dev, or tauri.");
                        return 2;
                    };
                    let Some(normalized) = core::normalize_cli_target(&value) else {
                        eprintln!(
                            "Invalid --target '{}'. Use agentd, tauri-dev, or tauri.",
                            value
                        );
                        return 2;
                    };
                    target_override = Some(normalized.to_string());
                    args = args[2..].to_vec();
                }
                Some(flag) if flag.starts_with("--target=") => {
                    let value = flag.trim_start_matches("--target=");
                    let Some(normalized) = core::normalize_cli_target(value) else {
                        eprintln!(
                            "Invalid --target '{}'. Use agentd, tauri-dev, or tauri.",
                            value
                        );
                        return 2;
                    };
                    target_override = Some(normalized.to_string());
                    args = args[1..].to_vec();
                }
                _ => break,
            }
        }

        if let Some(target) = target_override {
            std::env::set_var("CTENO_ENV", target);
        }

        let command = args.first().map(String::as_str);

        // Initialize logger for all commands (safe for repeated calls)
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .format_timestamp_millis()
            .try_init();

        match command {
            None | Some("-h") | Some("--help") | Some("help") => {
                print_help(&argv0, dev_mode);
                0
            }
            Some("-v") | Some("--version") | Some("version") => {
                println!("ctenoctl {}", env!("CARGO_PKG_VERSION"));
                0
            }
            Some("status") => match print_status() {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("status failed: {}", e);
                    1
                }
            },
            Some("auth") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_auth(sub_args))
            }
            Some("connect") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_connect(sub_args))
            }
            Some("daemon") => {
                if args.get(1).map(String::as_str) == Some("status") {
                    match print_status() {
                        Ok(()) => 0,
                        Err(e) => {
                            eprintln!("daemon status failed: {}", e);
                            1
                        }
                    }
                } else {
                    eprintln!("Unsupported daemon command");
                    print_help(&argv0, dev_mode);
                    2
                }
            }
            // --- RPC-backed commands ---
            Some("run") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_run(sub_args))
            }
            Some("tool") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_tool(sub_args))
            }
            Some("persona") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_persona(sub_args))
            }
            Some("session") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_session(sub_args))
            }
            Some("agent") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_agent(sub_args))
            }
            Some("workspace") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_workspace(sub_args))
            }
            Some("memory") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_memory(sub_args))
            }
            Some("profile") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_profile(sub_args))
            }
            Some("mcp") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_mcp(sub_args))
            }
            // --- Direct dispatch is intentionally disabled for CLI parity with GUI ---
            Some("dispatch") => {
                eprintln!("`ctenoctl dispatch` is disabled.");
                eprintln!(
                    "Use `ctenoctl persona create` and interact through persona chat/GUI flows."
                );
                2
            }
            // --- Dev-only commands (requires --dev flag) ---
            Some("webview") if dev_mode => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_webview(sub_args))
            }
            Some("orchestration") if dev_mode => {
                let sub_args: Vec<String> = args[1..].to_vec();
                run_async(cmd_orchestration(sub_args))
            }
            // --- Local-only commands ---
            Some("prompt") => {
                let sub_args: Vec<String> = args[1..].to_vec();
                cmd_prompt(sub_args)
            }
            Some(other) => {
                eprintln!("Unknown command: {}", other);
                print_help(&argv0, dev_mode);
                2
            }
        }
    }

    /// Run an async function in a new tokio runtime, returning exit code.
    fn run_async(future: impl std::future::Future<Output = Result<(), String>>) -> i32 {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        match rt.block_on(future) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        }
    }

    // ============================================================================
    // RPC client — connects to daemon's Unix socket
    // ============================================================================

    /// Send a JSON-RPC request to the daemon and return the result.
    async fn rpc_call(method: &str, params: Value) -> Result<Value, String> {
        let sock_path = cteno_host_bridge_localrpc::socket_path();
        let target = std::env::var("CTENO_ENV").unwrap_or_else(|_| "auto".to_string());

        let stream = tokio::net::UnixStream::connect(&sock_path)
        .await
        .map_err(|e| {
            format!(
                "Cteno daemon not running. Start or install `cteno-agentd` first.\n  (target: {}, socket: {}, error: {})",
                target,
                sock_path.display(),
                e
            )
        })?;

        let (reader, mut writer) = stream.into_split();

        let request = json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize request: {}", e))?;
        line.push('\n');

        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;

        // Read response line
        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::new();
        buf_reader
            .read_line(&mut response_line)
            .await
            .map_err(|e| format!("Failed to read response: {}", e))?;

        if response_line.is_empty() {
            return Err("Daemon closed connection without response".to_string());
        }

        let response: Value = serde_json::from_str(&response_line)
            .map_err(|e| format!("Invalid response from daemon: {}", e))?;

        if let Some(error) = response.get("error").and_then(|v| v.as_str()) {
            return Err(format!("RPC error: {}", error));
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    fn happy_server_url() -> Result<String, String> {
        Ok(crate::resolved_happy_server_url())
    }

    fn render_ascii_qr(uri: &str) -> Result<String, String> {
        let qr = QrCode::new(uri.as_bytes())
            .map_err(|e| format!("Failed to generate QR code: {}", e))?;
        Ok(qr.render::<unicode::Dense1x2>().quiet_zone(false).build())
    }

    fn direct_auth_status() -> Result<Value, String> {
        let target = std::env::var("CTENO_ENV").ok();
        let identity = core::resolve_cli_target_identity_paths(target.as_deref())?;
        let app_data_dir = identity.app_data_dir.clone();
        std::fs::create_dir_all(&app_data_dir)
            .map_err(|e| format!("Failed to create {}: {}", app_data_dir.display(), e))?;
        let account_auth = headless_auth::load_account_auth(&app_data_dir)?;
        let machine_auth_store = daemon_runtime::machine_auth_cache_path(&app_data_dir);

        let daemon_state = read_daemon_state(&daemon_state_path()?)?;
        let daemon_running = daemon_state
            .as_ref()
            .map(|state| is_process_running(state.pid))
            .unwrap_or(false);
        let daemon_mode = daemon_state.map(|state| state.mode);
        let managed_mode = daemon_mode
            .as_deref()
            .map(|mode| mode == "agentd-managed")
            .unwrap_or(false);

        Ok(json!({
            "daemonRunning": daemon_running,
            "shellKind": identity.shell_kind.as_str(),
            "appDataDir": identity.app_data_dir.display().to_string(),
            "configPath": identity.config_path.display().to_string(),
            "profilesPath": identity.profiles_path.display().to_string(),
            "machineIdPath": identity.machine_id_path.display().to_string(),
            "localRpcEnvTag": identity.local_rpc_env_tag,
            "daemonMode": daemon_mode,
            "managedMode": managed_mode,
            "machineAuthenticated": machine_auth_store.exists(),
            "machinePending": false,
            "pendingMachinePublicKey": Value::Null,
            "pendingMachineUri": Value::Null,
            "machineAuthStorePath": machine_auth_store.display().to_string(),
            "accountAuthenticated": account_auth.is_some(),
            "accountAuthStorePath": headless_auth::account_auth_store_path(&app_data_dir).display().to_string(),
        }))
    }

    async fn auth_status() -> Result<Value, String> {
        match rpc_call("auth.status", json!({})).await {
            Ok(value) => Ok(value),
            Err(_) => direct_auth_status(),
        }
    }

    async fn pending_machine_status() -> Result<Value, String> {
        match rpc_call("auth.pending-machine", json!({})).await {
            Ok(value) => Ok(value),
            Err(_) => {
                let target = std::env::var("CTENO_ENV").ok();
                let identity = core::resolve_cli_target_identity_paths(target.as_deref())?;
                let app_data_dir = identity.app_data_dir.clone();
                std::fs::create_dir_all(&app_data_dir)
                    .map_err(|e| format!("Failed to create {}: {}", app_data_dir.display(), e))?;
                Ok(json!({
                    "publicKey": Value::Null,
                    "uri": Value::Null,
                    "pending": false,
                    "machineAuthenticated": daemon_runtime::machine_auth_cache_path(&app_data_dir).exists(),
                }))
            }
        }
    }

    async fn trigger_machine_reauth() -> Result<(), String> {
        rpc_call("auth.trigger-reauth", json!({})).await?;
        Ok(())
    }

    fn print_auth_status_value(value: &Value) {
        if let Some(kind) = value.get("shellKind").and_then(|v| v.as_str()) {
            println!("shell_kind: {}", kind);
        }
        if let Some(path) = value.get("appDataDir").and_then(|v| v.as_str()) {
            println!("app_data_dir: {}", path);
        }
        if let Some(path) = value.get("configPath").and_then(|v| v.as_str()) {
            println!("config_path: {}", path);
        }
        if let Some(path) = value.get("profilesPath").and_then(|v| v.as_str()) {
            println!("profiles_path: {}", path);
        }
        if let Some(path) = value.get("machineIdPath").and_then(|v| v.as_str()) {
            println!("machine_id_path: {}", path);
        }
        if let Some(env_tag) = value.get("localRpcEnvTag").and_then(|v| v.as_str()) {
            println!("local_rpc_env: {}", env_tag);
        }
        if let Some(mode) = value.get("daemonMode").and_then(|v| v.as_str()) {
            println!("daemon_mode: {}", mode);
        }
        if let Some(managed) = value.get("managedMode").and_then(|v| v.as_bool()) {
            println!("managed_mode: {}", if managed { "yes" } else { "no" });
        }
        println!(
            "account_authenticated: {}",
            if value
                .get("accountAuthenticated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "yes"
            } else {
                "no"
            }
        );
        if let Some(path) = value.get("accountAuthStorePath").and_then(|v| v.as_str()) {
            println!("account_auth_store: {}", path);
        }
        println!(
            "machine_authenticated: {}",
            if value
                .get("machineAuthenticated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "yes"
            } else {
                "no"
            }
        );
        println!(
            "machine_pending: {}",
            if value
                .get("machinePending")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "yes"
            } else {
                "no"
            }
        );
        if let Some(path) = value.get("machineAuthStorePath").and_then(|v| v.as_str()) {
            println!("machine_auth_store: {}", path);
        }
        if let Some(uri) = value.get("pendingMachineUri").and_then(|v| v.as_str()) {
            println!("pending_machine_uri: {}", uri);
        }
    }

    async fn cmd_auth(args: Vec<String>) -> Result<(), String> {
        match args.first().map(String::as_str) {
            Some("status") => {
                let status = auth_status().await?;
                print_auth_status_value(&status);
                Ok(())
            }
            Some("login") => {
                // Pre-2.0 ctenoctl ran the QR-over-encrypted-payload flow inline; in
                // 2.0 login is handled entirely by the GUI (browser OAuth / email
                // code). The daemon picks up `auth.json` on the next token-refresh
                // tick once the GUI has written it, so CLI users just need to log in
                // in the app and then re-run commands here.
                let _ = args; // silence unused binding when no flags parsed
                println!("ctenoctl auth login has been removed in Cteno 2.0.");
                println!("Please log in from the desktop app (browser OAuth or email code).");
                println!("The daemon will pick up the new credentials automatically.");
                Ok(())
            }
            Some("machine") => {
                println!("Machine approval QR flow has been removed. Use `ctenoctl auth login` to register this device.");
                Ok(())
            }
            Some("logout") => {
                let app_data_dir = headless_auth::ensure_app_data_dir()?;
                headless_auth::clear_account_auth(&app_data_dir)?;
                match trigger_machine_reauth().await {
                    Ok(()) => {
                        println!("Cleared headless account auth and triggered machine reauth.")
                    }
                    Err(_) => println!(
                    "Cleared headless account auth. Daemon reauth signal could not be delivered."
                ),
                }
                Ok(())
            }
            Some("reauth") => {
                trigger_machine_reauth().await?;
                println!("Triggered machine reauthentication.");
                Ok(())
            }
            _ => {
                println!("Usage: ctenoctl auth <status|login|machine|logout|reauth>");
                println!("  status                     Show headless account/machine auth status");
                println!(
                "  login [--timeout <sec>]    Authenticate this headless host with an account QR"
            );
                println!(
                    "  machine                    Explain the removed legacy machine approval flow"
                );
                println!("  logout                     Clear stored headless account auth and reauth machine");
                println!("  reauth                     Re-trigger machine auth without clearing account auth");
                Ok(())
            }
        }
    }

    async fn cmd_connect(args: Vec<String>) -> Result<(), String> {
        match args.first().map(String::as_str) {
            Some("github") | Some("wechat") | Some("claude") => Err(format!(
                "`ctenoctl connect {}` is not yet supported in headless mode.",
                args.first().unwrap()
            )),
            _ => {
                println!("Usage: ctenoctl connect <github|wechat|claude>");
                println!("These provider connect flows are not yet supported in headless mode.");
                Ok(())
            }
        }
    }

    // ============================================================================
    // ctenoctl run
    // ============================================================================

    /// `ctenoctl run --kind <kind> --message <msg> [--profile <id>] [--workdir <dir>] [--max-turns <n>]`
    /// Also supports subcommands for background run management:
    ///   `ctenoctl run list [--session <id>]`
    ///   `ctenoctl run get <run_id>`
    ///   `ctenoctl run logs <run_id> [--lines <n>]`
    ///   `ctenoctl run stop <run_id>`
    async fn cmd_run(args: Vec<String>) -> Result<(), String> {
        // Check if first arg is a management subcommand
        match args.first().map(String::as_str) {
            Some("list") => {
                let session_id = {
                    let mut sid: Option<String> = None;
                    let mut i = 1;
                    while i < args.len() {
                        if args[i] == "--session" || args[i] == "-s" {
                            i += 1;
                            sid = args.get(i).cloned();
                        }
                        i += 1;
                    }
                    sid
                };
                let mut params = json!({});
                if let Some(sid) = session_id {
                    params["sessionId"] = json!(sid);
                }
                let result = rpc_call("list-runs", params).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                return Ok(());
            }
            Some("get") => {
                let run_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl run get <run_id>")?;
                let result = rpc_call("get-run", json!({ "runId": run_id })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                return Ok(());
            }
            Some("logs") => {
                let run_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl run logs <run_id> [--lines <n>]")?;
                let mut lines: Option<u64> = None;
                let mut i = 2;
                while i < args.len() {
                    if args[i] == "--lines" || args[i] == "-n" {
                        i += 1;
                        lines = args.get(i).and_then(|s| s.parse().ok());
                    }
                    i += 1;
                }
                let mut params = json!({ "runId": run_id });
                if let Some(n) = lines {
                    params["lines"] = json!(n);
                }
                let result = rpc_call("get-run-logs", params).await?;
                // Print log data directly if available
                if let Some(data) = result.get("data").and_then(|v| v.as_str()) {
                    print!("{}", data);
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result).unwrap_or_default()
                    );
                }
                return Ok(());
            }
            Some("stop") => {
                let run_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl run stop <run_id>")?;
                let result = rpc_call("stop-run", json!({ "runId": run_id })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                return Ok(());
            }
            _ => {} // Fall through to existing agent-run behavior
        }

        let mut kind_str: Option<String> = None;
        let mut message: Option<String> = None;
        let mut profile: Option<String> = None;
        let mut workdir: Option<String> = None;
        let mut max_turns: Option<usize> = None;
        let mut show_trace = true;
        let mut trace_limit: u64 = 300;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--kind" | "-k" => {
                    i += 1;
                    kind_str = args.get(i).cloned();
                }
                "--message" | "-m" => {
                    i += 1;
                    message = args.get(i).cloned();
                }
                "--profile" | "-p" => {
                    i += 1;
                    profile = args.get(i).cloned();
                }
                "--workdir" | "-w" => {
                    i += 1;
                    workdir = args.get(i).cloned();
                }
                "--max-turns" => {
                    i += 1;
                    max_turns = args.get(i).and_then(|s| s.parse().ok());
                }
                "--no-trace" => {
                    show_trace = false;
                }
                "--trace-limit" => {
                    i += 1;
                    trace_limit = args
                        .get(i)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(trace_limit)
                        .clamp(1, 2000);
                }
                "--help" | "-h" => {
                    println!("Usage: ctenoctl run --kind <kind> --message <msg> [options]");
                    println!("       ctenoctl run <subcommand>");
                    println!();
                    println!("Run an agent task (blocks until done):");
                    println!("  --kind, -k <kind>       Agent kind (worker, browser, etc.)");
                    println!("  --message, -m <msg>     Task message");
                    println!("  --profile, -p <id>      LLM profile ID");
                    println!("  --workdir, -w <dir>     Working directory (default: current dir)");
                    println!("  --max-turns <n>         Max ReAct iterations (default: 20)");
                    println!("  --no-trace              Disable normalized session trace output");
                    println!("  --trace-limit <n>       Max trace events to print (default: 300)");
                    println!();
                    println!("Background run management:");
                    println!("  list [--session <id>]    List background runs");
                    println!("  get <run_id>             Get run details");
                    println!("  logs <run_id> [-n <N>]   Read run logs (default: 100 lines)");
                    println!("  stop <run_id>            Stop a background run");
                    return Ok(());
                }
                other => {
                    return Err(format!("Unknown option: {}. Use --help for usage.", other));
                }
            }
            i += 1;
        }

        let kind_str = kind_str.ok_or("--kind is required. Use --help for usage.")?;
        let message = message.ok_or("--message is required. Use --help for usage.")?;

        let wd = workdir.unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string()
        });

        let params = json!({
            "kind": kind_str,
            "message": message,
            "workdir": wd,
            "modelId": profile,
            "timeout": max_turns.map(|t| t as u64 * 30).unwrap_or(300), // rough: 30s per turn
        });

        eprintln!("Running agent ({})... (this may take a while)", kind_str);

        // cli-run-agent: spawns full session with tools, blocks until agent completes
        let result = rpc_call("cli-run-agent", params).await?;

        if show_trace {
            if let Some(session_id) = result.get("sessionId").and_then(|v| v.as_str()) {
                if let Err(e) = fetch_and_print_session_trace(session_id, trace_limit, true).await {
                    eprintln!("[trace] failed to load session trace: {}", e);
                }
            }
        }

        // Print result as JSON
        let json = serde_json::to_string_pretty(&result)
            .map_err(|e| format!("Failed to serialize result: {}", e))?;
        println!("{}", json);

        Ok(())
    }

    // ============================================================================
    // ctenoctl tool
    // ============================================================================

    /// `ctenoctl tool list` / `ctenoctl tool exec <id> --input <json>`
    async fn cmd_tool(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("list") => {
                let result = rpc_call("list-tools", json!({})).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("exec") => {
                let tool_id = args
                    .get(1)
                    .ok_or("Usage: ctenoctl tool exec <tool_id> --input <json>")?;

                let mut input_json: Option<String> = None;
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--input" | "-i" => {
                            i += 1;
                            input_json = args.get(i).cloned();
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let input: Value = match input_json {
                    Some(ref s) => {
                        serde_json::from_str(s).map_err(|e| format!("Invalid JSON input: {}", e))?
                    }
                    None => json!({}),
                };

                let result = rpc_call(
                    "exec-tool",
                    json!({
                        "tool_id": tool_id,
                        "input": input,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl tool <subcommand>");
                println!();
                println!("Subcommands:");
                println!("  list                       List all registered tools");
                println!("  exec <id> --input <json>   Execute a tool directly");
                Ok(())
            }
            Some(other) => Err(format!("Unknown tool subcommand: {}", other)),
        }
    }

    // ============================================================================
    // ctenoctl persona
    // ============================================================================

    async fn cmd_persona(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("list") => {
                let result = rpc_call("list-personas", json!({})).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("create") => {
                let mut name: Option<String> = None;
                let mut description: Option<String> = None;
                let mut model: Option<String> = None;
                let mut avatar_id: Option<String> = None;
                let mut profile_id: Option<String> = None;
                let mut workdir: Option<String> = None;

                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--name" | "-n" => {
                            i += 1;
                            name = args.get(i).cloned();
                        }
                        "--description" | "-d" => {
                            i += 1;
                            description = args.get(i).cloned();
                        }
                        "--model" => {
                            i += 1;
                            model = args.get(i).cloned();
                        }
                        "--avatar" => {
                            i += 1;
                            avatar_id = args.get(i).cloned();
                        }
                        "--profile" | "-p" => {
                            i += 1;
                            profile_id = args.get(i).cloned();
                        }
                        "--workdir" | "-w" => {
                            i += 1;
                            workdir = args.get(i).cloned();
                        }
                        "--help" | "-h" => {
                            println!("Usage: ctenoctl persona create [options]");
                            println!();
                            println!("Options:");
                            println!("  --name, -n <name>          Persona name");
                            println!("  --description, -d <desc>   Persona description");
                            println!(
                                "  --model <model>            Model ID (default: deepseek-chat)"
                            );
                            println!("  --avatar <id>              Avatar ID");
                            println!("  --profile, -p <id>         LLM profile ID");
                            println!("  --workdir, -w <dir>        Persona working directory");
                            return Ok(());
                        }
                        other => {
                            return Err(format!(
                                "Unknown option: {}. Use --help for usage.",
                                other
                            ));
                        }
                    }
                    i += 1;
                }

                let mut params = json!({});
                if let Some(v) = name {
                    params["name"] = json!(v);
                }
                if let Some(v) = description {
                    params["description"] = json!(v);
                }
                if let Some(v) = model {
                    params["model"] = json!(v);
                }
                if let Some(v) = avatar_id {
                    params["avatarId"] = json!(v);
                }
                if let Some(v) = profile_id {
                    params["modelId"] = json!(v);
                }
                if let Some(v) = workdir {
                    params["workdir"] = json!(v);
                }

                let result = rpc_call("create-persona", params).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("chat") => {
                let persona_id = args
                    .get(1)
                    .ok_or("Usage: ctenoctl persona chat <id> -m <message>")?;

                let mut message: Option<String> = None;
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--message" | "-m" => {
                            i += 1;
                            message = args.get(i).cloned();
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let message = message.ok_or("--message is required")?;

                let result = rpc_call(
                    "send-message",
                    json!({
                        "persona_id": persona_id,
                        "message": message,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("tasks") => {
                let persona_id = args.get(1).ok_or("Usage: ctenoctl persona tasks <id>")?;

                let result = rpc_call(
                    "get-persona-tasks",
                    json!({
                        "personaId": persona_id,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("delete") => {
                let persona_id = args.get(1).ok_or("Usage: ctenoctl persona delete <id>")?;

                let result = rpc_call(
                    "delete-persona",
                    json!({
                        "id": persona_id,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl persona <subcommand>");
                println!();
                println!("Subcommands:");
                println!("  list                               List all personas");
                println!("  create [options]                   Create a new persona");
                println!("  chat <id> -m <message>             Send a message to a persona");
                println!("  tasks <id>                         List tasks for a persona");
                println!("  delete <id>                        Delete a persona");
                Ok(())
            }
            Some(other) => Err(format!("Unknown persona subcommand: {}", other)),
        }
    }

    // ============================================================================
    // ctenoctl session
    // ============================================================================

    async fn cmd_session(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("list") => {
                let result = rpc_call("list-sessions", json!({})).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("get") => {
                let session_id = args
                    .get(1)
                    .ok_or("Usage: ctenoctl session get <session_id>")?;

                let result = rpc_call(
                    "get-session",
                    json!({
                        "id": session_id,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("stop") => {
                let session_id = args
                    .get(1)
                    .ok_or("Usage: ctenoctl session stop <session_id>")?;

                let result = rpc_call(
                    "stop-subagent",
                    json!({
                        "id": session_id,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("kill") => {
                let session_id = args
                    .get(1)
                    .ok_or("Usage: ctenoctl session kill <session_id>")?;

                let result = rpc_call(
                    "kill-session",
                    json!({
                        "sessionId": session_id,
                        "session_id": session_id,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("trace") => {
                let session_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl session trace <session_id> [--limit <n>]")?;
                let mut limit: u64 = 300;
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--limit" | "-n" => {
                            i += 1;
                            limit = args
                                .get(i)
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(limit)
                                .clamp(1, 2000);
                        }
                        _ => {}
                    }
                    i += 1;
                }
                fetch_and_print_session_trace(session_id, limit, false).await
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl session <subcommand>");
                println!();
                println!("Subcommands:");
                println!("  list                  List active sessions/subagents");
                println!("  get <session_id>      Get session details and status");
                println!("  stop <session_id>     Gracefully stop a session");
                println!("  kill <session_id>     Kill a session (force)");
                println!("  trace <session_id>    Print normalized tool/message trace");
                Ok(())
            }
            Some(other) => Err(format!("Unknown session subcommand: {}", other)),
        }
    }

    // ============================================================================
    // ctenoctl dispatch
    // ============================================================================

    /// `ctenoctl dispatch [persona_id] -m <task> [--type <agent_type>] [--profile <id>] [--workdir <dir>] [--wait] [--timeout <secs>]`
    ///
    /// When `persona_id` is omitted, automatically uses the default persona.
    async fn cmd_dispatch(args: Vec<String>) -> Result<(), String> {
        // If the first arg doesn't start with '-', treat it as persona_id; otherwise auto-resolve.
        let (explicit_persona_id, option_start) = match args.first().filter(|s| !s.starts_with('-'))
        {
            Some(id) => (Some(id.clone()), 1),
            None => (None, 0),
        };

        let mut message: Option<String> = None;
        let mut agent_type: Option<String> = None;
        let mut profile: Option<String> = None;
        let mut workdir: Option<String> = None;
        let mut skill_ids: Option<Vec<String>> = None;
        let mut label: Option<String> = None;
        let mut wait = false;
        let mut timeout_secs: u64 = 300;

        let mut i = option_start;
        while i < args.len() {
            match args[i].as_str() {
                "--message" | "-m" => {
                    i += 1;
                    message = args.get(i).cloned();
                }
                "--type" | "-t" => {
                    i += 1;
                    agent_type = args.get(i).cloned();
                }
                "--profile" | "-p" => {
                    i += 1;
                    profile = args.get(i).cloned();
                }
                "--workdir" | "-w" => {
                    i += 1;
                    workdir = args.get(i).cloned();
                }
                "--skill" => {
                    i += 1;
                    skill_ids = args
                        .get(i)
                        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
                }
                "--label" | "-l" => {
                    i += 1;
                    label = args.get(i).cloned();
                }
                "--wait" => {
                    wait = true;
                }
                "--timeout" => {
                    i += 1;
                    timeout_secs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(300);
                }
                "--help" | "-h" => {
                    println!("Usage: ctenoctl dispatch [persona_id] -m <task> [options]");
                    println!();
                    println!("When persona_id is omitted, the default persona is used.");
                    println!();
                    println!("Options:");
                    println!("  --message, -m <task>       Task description");
                    println!("  --type, -t <type>          Agent type (worker, browser, render, or custom agent ID)");
                    println!("  --profile, -p <id>         LLM profile ID");
                    println!("  --workdir, -w <dir>        Working directory");
                    println!("  --skill <id1,id2,...>       Pre-activate skills (comma-separated)");
                    println!("  --label, -l <label>        Orchestration flow node label");
                    println!("  --wait                     Block until worker completes");
                    println!("  --timeout <secs>           Timeout in seconds (default: 300, requires --wait)");
                    return Ok(());
                }
                _ => {}
            }
            i += 1;
        }

        // Dev mode: --persona is required (no implicit default resolution)
        let persona_id = explicit_persona_id
            .ok_or("--persona <id> is required. Use `ctenoctl persona list` to find IDs.")?;

        let message = message.ok_or("--message is required")?;

        let mut params = json!({
            "personaId": persona_id,
            "task": message,
        });
        if let Some(t) = agent_type {
            params["agentType"] = json!(t);
        }
        if let Some(p) = profile {
            params["modelId"] = json!(p);
        }
        if let Some(w) = workdir {
            params["workdir"] = json!(w);
        }
        if let Some(ref s) = skill_ids {
            params["skillIds"] = json!(s);
        }
        if let Some(l) = label {
            params["label"] = json!(l);
        }
        if wait {
            params["wait"] = json!(true);
            params["timeout"] = json!(timeout_secs);
        }

        if wait {
            eprintln!(
                "Dispatching and waiting for completion (timeout {}s)...",
                timeout_secs
            );
        }

        let result = rpc_call("dispatch-task", params).await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
        Ok(())
    }

    // ============================================================================
    // ctenoctl memory
    // ============================================================================

    async fn cmd_memory(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("schema") => print_json(&memory_cli_schema()),
            Some("list") => {
                let opts = parse_memory_options(&args[1..])?;
                let workspace = memory_workspace_dir()?;
                let persona_workdir = opts.persona_workdir()?;
                let files = cteno_community_core::memory::memory_list_core(
                    &workspace,
                    persona_workdir.as_deref(),
                )?;
                let data: Vec<String> = match opts.scope {
                    MemoryCliScope::Private => files
                        .into_iter()
                        .filter(|path| path.starts_with("[private] "))
                        .collect(),
                    MemoryCliScope::Global => files
                        .into_iter()
                        .filter(|path| path.starts_with("[global] "))
                        .collect(),
                    MemoryCliScope::Auto => files,
                };
                print_json(&json!({
                    "success": true,
                    "data": data,
                    "scope": opts.scope.as_str(),
                    "projectDir": opts.project_dir_display(),
                }))
            }
            Some("recall") | Some("search") => {
                let opts = parse_memory_options(&args[1..])?;
                let query = opts
                    .query
                    .as_deref()
                    .or_else(|| opts.positionals.first().map(String::as_str))
                    .ok_or("Usage: ctenoctl memory recall --query <text> [--project-dir <path>]")?;
                let workspace = memory_workspace_dir()?;
                let persona_workdir = opts.persona_workdir()?;
                let chunks = cteno_community_core::memory::memory_search_core(
                    &workspace,
                    query,
                    persona_workdir.as_deref(),
                    opts.limit.unwrap_or(10),
                    opts.memory_type.as_deref(),
                )?;
                print_json(&json!({
                    "success": true,
                    "data": chunks,
                    "scope": opts.scope.as_str(),
                    "projectDir": opts.project_dir_display(),
                }))
            }
            Some("read") => {
                let opts = parse_memory_options(&args[1..])?;
                let file_path = opts
                    .file_path
                    .as_deref()
                    .or_else(|| opts.positionals.first().map(String::as_str))
                    .ok_or("Usage: ctenoctl memory read <file_path> [--project-dir <path>]")?;
                let workspace = memory_workspace_dir()?;
                let persona_workdir = opts.persona_workdir()?;
                let content = cteno_community_core::memory::memory_read_core(
                    &workspace,
                    file_path,
                    persona_workdir.as_deref(),
                )?;
                let found = content.is_some();
                print_json(&json!({
                    "success": true,
                    "data": content,
                    "found": found,
                    "scope": opts.scope.as_str(),
                    "projectDir": opts.project_dir_display(),
                }))
            }
            Some("save") | Some("write") => {
                let opts = parse_memory_options(&args[1..])?;
                let file_path = opts
                    .file_path
                    .as_deref()
                    .or_else(|| opts.positionals.first().map(String::as_str))
                    .ok_or("Usage: ctenoctl memory save --file-path <path> --content <markdown>")?;
                let content = opts
                    .content
                    .as_deref()
                    .or_else(|| opts.positionals.get(1).map(String::as_str))
                    .ok_or("Usage: ctenoctl memory save --file-path <path> --content <markdown>")?;
                let workspace = memory_workspace_dir()?;
                let persona_workdir = opts.write_persona_workdir()?;
                cteno_community_core::memory::memory_write_core(
                    &workspace,
                    file_path,
                    content,
                    persona_workdir.as_deref(),
                )?;
                print_json(&json!({
                    "success": true,
                    "action": "save",
                    "filePath": file_path,
                    "scope": opts.scope.as_str(),
                    "projectDir": opts.project_dir_display(),
                }))
            }
            Some("append") => {
                let opts = parse_memory_options(&args[1..])?;
                let file_path = opts
                    .file_path
                    .as_deref()
                    .or_else(|| opts.positionals.first().map(String::as_str))
                    .ok_or(
                        "Usage: ctenoctl memory append --file-path <path> --content <markdown>",
                    )?;
                let content = opts
                    .content
                    .as_deref()
                    .or_else(|| opts.positionals.get(1).map(String::as_str))
                    .ok_or(
                        "Usage: ctenoctl memory append --file-path <path> --content <markdown>",
                    )?;
                let workspace = memory_workspace_dir()?;
                let persona_workdir = opts.write_persona_workdir()?;
                cteno_community_core::memory::memory_append_core(
                    &workspace,
                    file_path,
                    content,
                    persona_workdir.as_deref(),
                )?;
                print_json(&json!({
                    "success": true,
                    "action": "append",
                    "filePath": file_path,
                    "scope": opts.scope.as_str(),
                    "projectDir": opts.project_dir_display(),
                }))
            }
            Some("delete") | Some("rm") => {
                let opts = parse_memory_options(&args[1..])?;
                let file_path = opts
                    .file_path
                    .as_deref()
                    .or_else(|| opts.positionals.first().map(String::as_str))
                    .ok_or("Usage: ctenoctl memory delete <file_path> [--project-dir <path>]")?;
                let workspace = memory_workspace_dir()?;
                let persona_workdir = opts.write_persona_workdir()?;
                cteno_community_core::memory::memory_delete_core(
                    &workspace,
                    file_path,
                    persona_workdir.as_deref(),
                )?;
                print_json(&json!({
                    "success": true,
                    "action": "delete",
                    "filePath": file_path,
                    "scope": opts.scope.as_str(),
                    "projectDir": opts.project_dir_display(),
                }))
            }
            Some("--help") | Some("-h") | None => {
                print_memory_help();
                Ok(())
            }
            Some(other) => Err(format!("Unknown memory subcommand: {}", other)),
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum MemoryCliScope {
        Auto,
        Private,
        Global,
    }

    impl MemoryCliScope {
        fn as_str(self) -> &'static str {
            match self {
                MemoryCliScope::Auto => "auto",
                MemoryCliScope::Private => "private",
                MemoryCliScope::Global => "global",
            }
        }
    }

    #[derive(Debug)]
    struct MemoryCliOptions {
        project_dir: Option<PathBuf>,
        scope: MemoryCliScope,
        limit: Option<usize>,
        memory_type: Option<String>,
        query: Option<String>,
        file_path: Option<String>,
        content: Option<String>,
        positionals: Vec<String>,
    }

    impl MemoryCliOptions {
        fn project_dir_or_cwd(&self) -> Result<PathBuf, String> {
            match &self.project_dir {
                Some(path) => Ok(expand_path(path)),
                None => std::env::current_dir().map_err(|e| format!("resolve current_dir: {e}")),
            }
        }

        fn persona_workdir(&self) -> Result<Option<String>, String> {
            match self.scope {
                MemoryCliScope::Global => Ok(None),
                MemoryCliScope::Auto | MemoryCliScope::Private => {
                    Ok(Some(self.project_dir_or_cwd()?.display().to_string()))
                }
            }
        }

        fn write_persona_workdir(&self) -> Result<Option<String>, String> {
            match self.scope {
                MemoryCliScope::Global => Ok(None),
                MemoryCliScope::Auto | MemoryCliScope::Private => {
                    Ok(Some(self.project_dir_or_cwd()?.display().to_string()))
                }
            }
        }

        fn project_dir_display(&self) -> Option<String> {
            match self.scope {
                MemoryCliScope::Global => None,
                MemoryCliScope::Auto | MemoryCliScope::Private => self
                    .project_dir_or_cwd()
                    .ok()
                    .map(|path| path.display().to_string()),
            }
        }
    }

    fn parse_memory_options(args: &[String]) -> Result<MemoryCliOptions, String> {
        let mut opts = MemoryCliOptions {
            project_dir: None,
            scope: MemoryCliScope::Auto,
            limit: None,
            memory_type: None,
            query: None,
            file_path: None,
            content: None,
            positionals: Vec::new(),
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "--project-dir" | "--workdir" => {
                    i += 1;
                    let value = args.get(i).ok_or(format!("Missing value for {arg}"))?;
                    opts.project_dir = Some(PathBuf::from(value));
                }
                "--scope" => {
                    i += 1;
                    let value = args.get(i).ok_or("Missing value for --scope")?;
                    opts.scope = match value.as_str() {
                        "auto" => MemoryCliScope::Auto,
                        "private" | "project" => MemoryCliScope::Private,
                        "global" => MemoryCliScope::Global,
                        other => {
                            return Err(format!(
                                "Invalid --scope '{}'. Use auto, private, or global.",
                                other
                            ));
                        }
                    };
                }
                "--limit" => {
                    i += 1;
                    let value = args.get(i).ok_or("Missing value for --limit")?;
                    opts.limit = Some(
                        value
                            .parse::<usize>()
                            .map_err(|_| format!("Invalid --limit '{}'", value))?,
                    );
                }
                "--type" => {
                    i += 1;
                    opts.memory_type = Some(args.get(i).ok_or("Missing value for --type")?.clone());
                }
                "--query" | "-q" => {
                    i += 1;
                    opts.query = Some(
                        args.get(i)
                            .ok_or(format!("Missing value for {arg}"))?
                            .clone(),
                    );
                }
                "--file-path" | "--path" | "--key" => {
                    i += 1;
                    opts.file_path = Some(
                        args.get(i)
                            .ok_or(format!("Missing value for {arg}"))?
                            .clone(),
                    );
                }
                "--content" => {
                    i += 1;
                    opts.content = Some(args.get(i).ok_or("Missing value for --content")?.clone());
                }
                "--json" => {}
                "--help" | "-h" => {
                    print_memory_help();
                    return Err("".to_string());
                }
                flag if flag.starts_with("--") => {
                    return Err(format!("Unknown memory option: {}", flag));
                }
                value => opts.positionals.push(value.to_string()),
            }
            i += 1;
        }

        Ok(opts)
    }

    fn memory_workspace_dir() -> Result<PathBuf, String> {
        let target = std::env::var("CTENO_ENV").ok();
        let identity = core::resolve_cli_target_identity_paths(target.as_deref())?;
        let workspace = identity.app_data_dir.join("workspace");
        std::fs::create_dir_all(&workspace)
            .map_err(|e| format!("Failed to create {}: {}", workspace.display(), e))?;
        Ok(workspace)
    }

    fn expand_path(path: &Path) -> PathBuf {
        let s = path.to_string_lossy();
        let expanded = shellexpand::tilde(&s);
        PathBuf::from(expanded.as_ref())
    }

    fn memory_cli_schema() -> Value {
        json!({
            "name": "ctenoctl memory",
            "description": "Direct CLI bridge for Cteno markdown memory. No MCP server is required.",
            "commands": {
                "list": {
                    "args": {
                        "project_dir": { "type": "string", "required": false },
                        "scope": { "type": "string", "enum": ["auto", "private", "global"], "default": "auto" }
                    }
                },
                "recall": {
                    "args": {
                        "query": { "type": "string", "required": true },
                        "project_dir": { "type": "string", "required": false },
                        "limit": { "type": "integer", "default": 10 },
                        "type": { "type": "string", "required": false }
                    }
                },
                "read": {
                    "args": {
                        "file_path": { "type": "string", "required": true },
                        "project_dir": { "type": "string", "required": false },
                        "scope": { "type": "string", "enum": ["auto", "private", "global"], "default": "auto" }
                    }
                },
                "save": {
                    "args": {
                        "file_path": { "type": "string", "required": true },
                        "content": { "type": "string", "required": true },
                        "project_dir": { "type": "string", "required": false },
                        "scope": { "type": "string", "enum": ["private", "global"], "default": "private" }
                    }
                },
                "append": {
                    "args": {
                        "file_path": { "type": "string", "required": true },
                        "content": { "type": "string", "required": true },
                        "project_dir": { "type": "string", "required": false },
                        "scope": { "type": "string", "enum": ["private", "global"], "default": "private" }
                    }
                },
                "delete": {
                    "args": {
                        "file_path": { "type": "string", "required": true },
                        "project_dir": { "type": "string", "required": false },
                        "scope": { "type": "string", "enum": ["private", "global"], "default": "private" }
                    }
                }
            },
            "output": { "success": "boolean", "data": "command-specific JSON" }
        })
    }

    fn print_memory_help() {
        println!("Usage: ctenoctl memory <subcommand> [options]");
        println!();
        println!("Subcommands:");
        println!("  schema                         Print machine-readable command schema");
        println!("  list [--project-dir <path>]    List memory files");
        println!("  recall --query <text>          Search memory chunks");
        println!("  read <file_path>               Read a memory file");
        println!("  save --file-path <path> --content <markdown>");
        println!("  append --file-path <path> --content <markdown>");
        println!("  delete <file_path>");
        println!();
        println!("Options:");
        println!("  --project-dir, --workdir <path>  Project root for private memory");
        println!("  --scope auto|private|global      Memory scope (default: auto)");
        println!("  --limit <n>                      Recall result limit (default: 10)");
        println!("  --type <frontmatter-type>        Filter recall by YAML frontmatter type");
    }

    fn print_json(value: &Value) -> Result<(), String> {
        println!(
            "{}",
            serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
        );
        Ok(())
    }

    // ============================================================================
    // ctenoctl mcp
    // ============================================================================

    fn parse_key_value_pair(s: &str) -> Result<(String, String), String> {
        if let Some((k, v)) = s.split_once('=') {
            let key = k.trim().to_string();
            let value = v.trim().to_string();
            if key.is_empty() {
                return Err(format!("Invalid key/value pair '{}': empty key", s));
            }
            return Ok((key, value));
        }
        if let Some((k, v)) = s.split_once(':') {
            let key = k.trim().to_string();
            let value = v.trim().to_string();
            if key.is_empty() {
                return Err(format!("Invalid key/value pair '{}': empty key", s));
            }
            return Ok((key, value));
        }
        Err(format!(
            "Invalid key/value pair '{}': expected KEY=VALUE or KEY:VALUE",
            s
        ))
    }

    async fn add_mcp_server_and_verify(config: Value, server_id: &str) -> Result<(), String> {
        let result = rpc_call("add-mcp-server", config).await?;
        let success = result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !success {
            let err = result
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown add-mcp-server error");
            return Err(format!("Failed to add MCP server '{}': {}", server_id, err));
        }

        let list_result = rpc_call("list-mcp-servers", json!({})).await?;
        let maybe_server = list_result
            .get("servers")
            .and_then(|v| v.as_array())
            .and_then(|servers| {
                servers
                    .iter()
                    .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(server_id))
            })
            .cloned();

        if let Some(server) = maybe_server {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "success": true,
                    "server": server,
                }))
                .unwrap_or_default()
            );
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "success": true,
                    "serverId": server_id,
                    "warning": "Server added but not found in list result",
                }))
                .unwrap_or_default()
            );
        }

        Ok(())
    }

    /// `ctenoctl mcp ...`
    async fn cmd_mcp(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);
        match sub {
            Some("list") => {
                let result = rpc_call("list-mcp-servers", json!({})).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("remove") => {
                let server_id = args
                    .get(1)
                    .ok_or("Usage: ctenoctl mcp remove <server_id>")?;
                let result =
                    rpc_call("remove-mcp-server", json!({ "serverId": server_id })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("enable") | Some("disable") => {
                let server_id = args
                    .get(1)
                    .ok_or("Usage: ctenoctl mcp <enable|disable> <server_id>")?;
                let enabled = sub == Some("enable");
                let result = rpc_call(
                    "toggle-mcp-server",
                    json!({
                        "serverId": server_id,
                        "enabled": enabled,
                    }),
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("add-json") => {
                let mut config_json: Option<String> = None;
                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--config" | "-c" => {
                            i += 1;
                            config_json = args.get(i).cloned();
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let config_json = config_json
                    .or_else(|| args.get(1).cloned())
                    .ok_or("Usage: ctenoctl mcp add-json --config '<MCPServerConfig JSON>'")?;
                let config: Value = serde_json::from_str(&config_json)
                    .map_err(|e| format!("Invalid MCP config JSON: {}", e))?;
                let server_id = config
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("MCP config JSON must include string field 'id'")?
                    .to_string();

                add_mcp_server_and_verify(config, &server_id).await
            }
            Some("add-stdio") => {
                let mut id: Option<String> = None;
                let mut name: Option<String> = None;
                let mut command: Option<String> = None;
                let mut args_list: Vec<String> = Vec::new();
                let mut env_map: HashMap<String, String> = HashMap::new();
                let mut enabled = true;

                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--id" => {
                            i += 1;
                            id = args.get(i).cloned();
                        }
                        "--name" => {
                            i += 1;
                            name = args.get(i).cloned();
                        }
                        "--command" => {
                            i += 1;
                            command = args.get(i).cloned();
                        }
                        "--arg" => {
                            i += 1;
                            if let Some(v) = args.get(i) {
                                args_list.push(v.clone());
                            }
                        }
                        "--env" => {
                            i += 1;
                            if let Some(v) = args.get(i) {
                                let (k, val) = parse_key_value_pair(v)?;
                                env_map.insert(k, val);
                            }
                        }
                        "--disabled" => {
                            enabled = false;
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let id = id.ok_or("Usage: ctenoctl mcp add-stdio --id <id> --name <name> --command <cmd> [--arg <arg> ...] [--env KEY=VALUE ...] [--disabled]")?;
                let name = name.ok_or("Missing required --name")?;
                let command = command.ok_or("Missing required --command")?;

                let config = json!({
                    "id": id,
                    "name": name,
                    "enabled": enabled,
                    "transport": {
                        "type": "stdio",
                        "command": command,
                        "args": args_list,
                        "env": env_map
                    }
                });

                let server_id = config
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                add_mcp_server_and_verify(config, &server_id).await
            }
            Some("add-sse") => {
                let mut id: Option<String> = None;
                let mut name: Option<String> = None;
                let mut url: Option<String> = None;
                let mut headers: HashMap<String, String> = HashMap::new();
                let mut enabled = true;

                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--id" => {
                            i += 1;
                            id = args.get(i).cloned();
                        }
                        "--name" => {
                            i += 1;
                            name = args.get(i).cloned();
                        }
                        "--url" => {
                            i += 1;
                            url = args.get(i).cloned();
                        }
                        "--header" => {
                            i += 1;
                            if let Some(v) = args.get(i) {
                                let (k, val) = parse_key_value_pair(v)?;
                                headers.insert(k, val);
                            }
                        }
                        "--disabled" => {
                            enabled = false;
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let id = id.ok_or("Usage: ctenoctl mcp add-sse --id <id> --name <name> --url <url> [--header KEY=VALUE ...] [--disabled]")?;
                let name = name.ok_or("Missing required --name")?;
                let url = url.ok_or("Missing required --url")?;

                let config = json!({
                    "id": id,
                    "name": name,
                    "enabled": enabled,
                    "transport": {
                        "type": "http_sse",
                        "url": url,
                        "headers": headers
                    }
                });

                let server_id = config
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                add_mcp_server_and_verify(config, &server_id).await
            }
            Some("install-stdio") => {
                let mut id: Option<String> = None;
                let mut name: Option<String> = None;
                let mut install_cmd: Option<String> = None;
                let mut command: Option<String> = None;
                let mut args_list: Vec<String> = Vec::new();
                let mut env_map: HashMap<String, String> = HashMap::new();
                let mut cwd: Option<String> = None;
                let mut enabled = true;
                let mut continue_on_install_error = false;

                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--id" => {
                            i += 1;
                            id = args.get(i).cloned();
                        }
                        "--name" => {
                            i += 1;
                            name = args.get(i).cloned();
                        }
                        "--install" => {
                            i += 1;
                            install_cmd = args.get(i).cloned();
                        }
                        "--command" => {
                            i += 1;
                            command = args.get(i).cloned();
                        }
                        "--arg" => {
                            i += 1;
                            if let Some(v) = args.get(i) {
                                args_list.push(v.clone());
                            }
                        }
                        "--env" => {
                            i += 1;
                            if let Some(v) = args.get(i) {
                                let (k, val) = parse_key_value_pair(v)?;
                                env_map.insert(k, val);
                            }
                        }
                        "--cwd" => {
                            i += 1;
                            cwd = args.get(i).cloned();
                        }
                        "--disabled" => {
                            enabled = false;
                        }
                        "--continue-on-install-error" => {
                            continue_on_install_error = true;
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let id = id.ok_or("Usage: ctenoctl mcp install-stdio --id <id> --name <name> --install '<shell command>' --command <cmd> [--arg <arg> ...] [--env KEY=VALUE ...] [--cwd <dir>] [--disabled] [--continue-on-install-error]")?;
                let name = name.ok_or("Missing required --name")?;
                let install_cmd = install_cmd.ok_or("Missing required --install")?;
                let command = command.ok_or("Missing required --command")?;

                let install_cwd = if let Some(c) = cwd {
                    c
                } else {
                    std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| "/".to_string())
                };

                let install_result = rpc_call(
                    "bash",
                    json!({
                        "command": install_cmd,
                        "cwd": install_cwd,
                    }),
                )
                .await?;

                let exit_code = install_result
                    .get("exitCode")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                let install_success = install_result
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(exit_code == 0);

                if !install_success && !continue_on_install_error {
                    let stderr = install_result
                        .get("stderr")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown stderr");
                    return Err(format!(
                        "Install command failed (exitCode={}): {}",
                        exit_code, stderr
                    ));
                }

                if !install_success {
                    eprintln!(
                    "Warning: install command failed (exitCode={}), continuing due to --continue-on-install-error",
                    exit_code
                );
                }

                let config = json!({
                    "id": id,
                    "name": name,
                    "enabled": enabled,
                    "transport": {
                        "type": "stdio",
                        "command": command,
                        "args": args_list,
                        "env": env_map
                    }
                });

                let server_id = config
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                add_mcp_server_and_verify(config, &server_id).await
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl mcp <subcommand>");
                println!();
                println!("Subcommands:");
                println!("  list");
                println!("  add-json --config '<json>'");
                println!(
                "  add-stdio --id <id> --name <name> --command <cmd> [--arg <arg> ...] [--env KEY=VALUE ...] [--disabled]"
            );
                println!(
                "  add-sse --id <id> --name <name> --url <url> [--header KEY=VALUE ...] [--disabled]"
            );
                println!("  install-stdio --id <id> --name <name> --install '<shell command>' --command <cmd> [--arg <arg> ...]");
                println!("  remove <server_id>");
                println!("  enable <server_id>");
                println!("  disable <server_id>");
                println!();
                println!("Examples:");
                println!("  ctenoctl mcp list");
                println!("  ctenoctl mcp add-stdio --id filesystem --name filesystem --command npx --arg -y --arg @modelcontextprotocol/server-filesystem --arg /tmp");
                println!("  ctenoctl mcp install-stdio --id filesystem --name filesystem --install 'npm i -g @modelcontextprotocol/server-filesystem' --command npx --arg -y --arg @modelcontextprotocol/server-filesystem --arg /tmp");
                Ok(())
            }
            Some(other) => Err(format!("Unknown mcp subcommand: {}", other)),
        }
    }

    // ============================================================================
    // ctenoctl webview
    // ============================================================================

    /// `ctenoctl webview eval <js_expression>`   — Execute JS in the Tauri webview
    /// `ctenoctl webview screenshot [--output-dir <dir>]` — Capture the Cteno window
    async fn cmd_webview(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("eval") => {
                if args.len() < 2 {
                    return Err("Usage: ctenoctl webview eval <js_expression>".to_string());
                }
                let script = args[1..].join(" ");
                let result = rpc_call("webview-eval", json!({ "script": script })).await?;
                // Print value directly if it's a simple success result
                if let Some(value) = result.get("value") {
                    if let Some(s) = value.as_str() {
                        println!("{}", s);
                    } else {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(value).unwrap_or_default()
                        );
                    }
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result).unwrap_or_default()
                    );
                }
                Ok(())
            }
            Some("screenshot") => {
                let mut output_dir: Option<String> = None;
                let mut i = 1;
                while i < args.len() {
                    if args[i] == "--output-dir" || args[i] == "-o" {
                        i += 1;
                        output_dir = args.get(i).cloned();
                    }
                    i += 1;
                }
                let mut params = json!({});
                if let Some(dir) = output_dir {
                    params["output_dir"] = json!(dir);
                }
                let result = rpc_call("webview-screenshot", params).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl webview <subcommand>");
                println!();
                println!("Subcommands:");
                println!(
                "  eval <js>                    Execute JS in the Tauri webview and return result"
            );
                println!("  screenshot [--output-dir <dir>]  Capture the Cteno window as PNG");
                println!();
                println!("Examples:");
                println!("  ctenoctl webview eval \"document.title\"");
                println!("  ctenoctl webview eval \"document.querySelector('button').click()\"");
                println!("  ctenoctl webview screenshot");
                println!("  ctenoctl webview screenshot --output-dir /tmp");
                Ok(())
            }
            Some(other) => Err(format!("Unknown webview subcommand: {}", other)),
        }
    }

    // ============================================================================
    // ctenoctl agent
    // ============================================================================

    /// `ctenoctl agent list [--workdir <dir>]`
    /// `ctenoctl agent show <id> [--workdir <dir>]`
    /// `ctenoctl agent create <id> --name <name> [--scope global|workspace] [--workdir <dir>] [--model <model>] [--tools <t1,t2,...>] [--skills <s1,s2,...>] [--allowed-tools <t1,t2,...>]`
    /// `ctenoctl agent delete <id> [--workdir <dir>]`
    /// `ctenoctl agent run <id> -m <msg> [--workdir <dir>] [--profile <id>]`
    async fn cmd_agent(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("list") => {
                let mut workdir: Option<String> = None;
                let mut i = 1;
                while i < args.len() {
                    if args[i] == "--workdir" || args[i] == "-w" {
                        i += 1;
                        workdir = args.get(i).cloned();
                    }
                    i += 1;
                }
                let wd = workdir.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                });
                let result = rpc_call("list-agents", json!({ "workdir": wd })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("show") => {
                let agent_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl agent show <id>")?;
                let mut workdir: Option<String> = None;
                let mut i = 2;
                while i < args.len() {
                    if args[i] == "--workdir" || args[i] == "-w" {
                        i += 1;
                        workdir = args.get(i).cloned();
                    }
                    i += 1;
                }
                let wd = workdir.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                });
                let result =
                    rpc_call("get-agent", json!({ "id": agent_id, "workdir": wd })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("create") => {
                let agent_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl agent create <id> --name <name> [options]")?;
                let mut name: Option<String> = None;
                let mut description: Option<String> = None;
                let mut scope = "workspace".to_string();
                let mut workdir: Option<String> = None;
                let mut model: Option<String> = None;
                let mut tools: Option<Vec<String>> = None;
                let mut skills: Option<Vec<String>> = None;
                let mut allowed_tools: Option<Vec<String>> = None;

                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--name" | "-n" => {
                            i += 1;
                            name = args.get(i).cloned();
                        }
                        "--description" | "-d" => {
                            i += 1;
                            description = args.get(i).cloned();
                        }
                        "--scope" | "-s" => {
                            i += 1;
                            scope = args.get(i).cloned().unwrap_or(scope);
                        }
                        "--workdir" | "-w" => {
                            i += 1;
                            workdir = args.get(i).cloned();
                        }
                        "--model" => {
                            i += 1;
                            model = args.get(i).cloned();
                        }
                        "--tools" => {
                            i += 1;
                            tools = args
                                .get(i)
                                .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
                        }
                        "--skills" => {
                            i += 1;
                            skills = args
                                .get(i)
                                .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
                        }
                        "--allowed-tools" => {
                            i += 1;
                            allowed_tools = args
                                .get(i)
                                .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let wd = workdir.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                });
                let name = name.unwrap_or_else(|| agent_id.to_string());

                let mut params = json!({
                    "id": agent_id,
                    "name": name,
                    "scope": scope,
                    "workdir": wd,
                });
                if let Some(d) = description {
                    params["description"] = json!(d);
                }
                if let Some(m) = model {
                    params["model"] = json!(m);
                }
                if let Some(t) = tools {
                    params["tools"] = json!(t);
                }
                if let Some(s) = skills {
                    params["skills"] = json!(s);
                }
                if let Some(t) = allowed_tools {
                    params["allowed_tools"] = json!(t);
                }

                let result = rpc_call("create-agent", params).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("delete") => {
                let agent_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl agent delete <id>")?;
                let mut workdir: Option<String> = None;
                let mut i = 2;
                while i < args.len() {
                    if args[i] == "--workdir" || args[i] == "-w" {
                        i += 1;
                        workdir = args.get(i).cloned();
                    }
                    i += 1;
                }
                let wd = workdir.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                });
                let result =
                    rpc_call("delete-agent", json!({ "id": agent_id, "workdir": wd })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("run") => {
                let agent_id = args
                    .get(1)
                    .filter(|s| !s.starts_with('-'))
                    .ok_or("Usage: ctenoctl agent run <id> -m <message>")?;
                let mut message: Option<String> = None;
                let mut profile: Option<String> = None;
                let mut workdir: Option<String> = None;
                let mut show_trace = true;
                let mut trace_limit: u64 = 300;

                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--message" | "-m" => {
                            i += 1;
                            message = args.get(i).cloned();
                        }
                        "--profile" | "-p" => {
                            i += 1;
                            profile = args.get(i).cloned();
                        }
                        "--workdir" | "-w" => {
                            i += 1;
                            workdir = args.get(i).cloned();
                        }
                        "--no-trace" => {
                            show_trace = false;
                        }
                        "--trace-limit" => {
                            i += 1;
                            trace_limit = args
                                .get(i)
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(trace_limit)
                                .clamp(1, 2000);
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let message = message.ok_or("--message is required")?;
                let wd = workdir.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                });

                let params = json!({
                    "kind": agent_id,
                    "message": message,
                    "workdir": wd,
                    "modelId": profile,
                    "timeout": 300,
                });

                eprintln!("Running custom agent '{}'...", agent_id);
                let result = rpc_call("cli-run-agent", params).await?;

                if show_trace {
                    if let Some(session_id) = result.get("sessionId").and_then(|v| v.as_str()) {
                        if let Err(e) =
                            fetch_and_print_session_trace(session_id, trace_limit, true).await
                        {
                            eprintln!("[trace] failed to load session trace: {}", e);
                        }
                    }
                }

                let json = serde_json::to_string_pretty(&result)
                    .map_err(|e| format!("Failed to serialize result: {}", e))?;
                println!("{}", json);
                Ok(())
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl agent <subcommand>");
                println!();
                println!("Subcommands:");
                println!("  list [--workdir <dir>]                              List all agents (builtin + global + workspace)");
                println!(
                    "  show <id> [--workdir <dir>]                         Show agent details"
                );
                println!(
                    "  create <id> --name <name> [options]                 Create a new agent"
                );
                println!(
                    "  delete <id> [--workdir <dir>]                       Delete a custom agent"
                );
                println!("  run <id> -m <message> [--profile <id>] [--workdir]  Run a task with custom agent");
                println!("      Supports: --no-trace, --trace-limit <n>");
                println!();
                println!("Create options:");
                println!("  --name, -n <name>              Agent display name");
                println!("  --description, -d <desc>       Description");
                println!("  --scope, -s <global|workspace> Storage scope (default: workspace)");
                println!("  --workdir, -w <dir>            Working directory (default: cwd)");
                println!("  --model <model>                Default LLM model");
                println!("  --tools <t1,t2,...>            Exact tool IDs to expose in AGENT.md");
                println!("  --skills <s1,s2,...>           Predeclared skill IDs for the agent");
                println!("  --allowed-tools <t1,t2,...>     Comma-separated tool whitelist");
                Ok(())
            }
            Some(other) => Err(format!("Unknown agent subcommand: {}", other)),
        }
    }

    // ============================================================================
    // ctenoctl workspace
    // ============================================================================

    async fn cmd_workspace(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("templates") => {
                let result = rpc_call("list-agent-workspace-templates", json!({})).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("bootstrap") => {
                let mut template_id: Option<String> = None;
                let mut name: Option<String> = None;
                let mut workdir: Option<String> = None;
                let mut model: Option<String> = None;
                let mut id: Option<String> = None;

                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--template" | "-t" => {
                            i += 1;
                            template_id = args.get(i).cloned();
                        }
                        "--name" | "-n" => {
                            i += 1;
                            name = args.get(i).cloned();
                        }
                        "--workdir" | "-w" => {
                            i += 1;
                            workdir = args.get(i).cloned();
                        }
                        "--model" => {
                            i += 1;
                            model = args.get(i).cloned();
                        }
                        "--id" => {
                            i += 1;
                            id = args.get(i).cloned();
                        }
                        "--help" | "-h" => {
                            println!(
                                "Usage: ctenoctl workspace bootstrap --template <id> [options]"
                            );
                            println!();
                            println!("Templates:");
                            println!("  coding-studio");
                            println!("  opc-solo-company");
                            println!("  autoresearch");
                            println!("  edict-governance");
                            println!();
                            println!("Options:");
                            println!("  --template, -t <id>      Template ID");
                            println!("  --name, -n <name>        Workspace display name");
                            println!("  --workdir, -w <dir>      Workspace directory");
                            println!("  --model <model>          Workspace model label (default: deepseek-chat)");
                            println!("  --id <id>                Optional explicit workspace ID");
                            return Ok(());
                        }
                        other => {
                            return Err(format!(
                                "Unknown option: {}. Use --help for usage.",
                                other
                            ));
                        }
                    }
                    i += 1;
                }

                let template_id = template_id
                    .ok_or("Usage: ctenoctl workspace bootstrap --template <id> [options]")?;

                let mut params = json!({
                    "templateId": template_id,
                });
                if let Some(v) = name {
                    params["name"] = json!(v);
                }
                if let Some(v) = workdir {
                    params["workdir"] = json!(v);
                }
                if let Some(v) = model {
                    params["model"] = json!(v);
                }
                if let Some(v) = id {
                    params["id"] = json!(v);
                }

                let result = rpc_call("bootstrap-workspace", params).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("state") => {
                let persona_id = args
                    .get(1)
                    .cloned()
                    .ok_or("Usage: ctenoctl workspace state <persona_id>")?;
                let workspace = workspace_cli_get(&persona_id).await?;
                let state = workspace
                    .get("runtime")
                    .and_then(|runtime| runtime.get("state"))
                    .cloned()
                    .unwrap_or_else(|| json!(null));
                println!(
                    "{}",
                    serde_json::to_string_pretty(&state).unwrap_or_default()
                );
                Ok(())
            }
            Some("members") => {
                let persona_id = args
                    .get(1)
                    .cloned()
                    .ok_or("Usage: ctenoctl workspace members <persona_id>")?;
                let workspace = workspace_cli_get(&persona_id).await?;
                let members = workspace
                    .get("members")
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                println!(
                    "{}",
                    serde_json::to_string_pretty(&members).unwrap_or_default()
                );
                Ok(())
            }
            Some("activity") => {
                let persona_id = args
                    .get(1)
                    .cloned()
                    .ok_or("Usage: ctenoctl workspace activity <persona_id> [--limit <n>]")?;
                let limit = parse_workspace_limit(&args[2..], 20)?;
                let workspace = workspace_cli_get(&persona_id).await?;
                let activities = workspace
                    .get("runtime")
                    .and_then(|runtime| runtime.get("recentActivities"))
                    .and_then(|value| value.as_array())
                    .cloned()
                    .unwrap_or_default();
                let items = tail_json_array(&activities, limit);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Value::Array(items)).unwrap_or_default()
                );
                Ok(())
            }
            Some("events") => {
                let persona_id = args
                    .get(1)
                    .cloned()
                    .ok_or("Usage: ctenoctl workspace events <persona_id> [--limit <n>]")?;
                let limit = parse_workspace_limit(&args[2..], 20)?;
                let workspace = workspace_cli_get(&persona_id).await?;
                let events = workspace
                    .get("runtime")
                    .and_then(|runtime| runtime.get("recentEvents"))
                    .and_then(|value| value.as_array())
                    .cloned()
                    .unwrap_or_default();
                let items = tail_json_array(&events, limit);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Value::Array(items)).unwrap_or_default()
                );
                Ok(())
            }
            Some("watch") => {
                let persona_id = args.get(1).cloned().ok_or(
                "Usage: ctenoctl workspace watch <persona_id> [--interval <secs>] [--limit <n>]",
            )?;
                let (interval_secs, limit) = parse_workspace_watch_options(&args[2..])?;
                loop {
                    let workspace = workspace_cli_get(&persona_id).await?;
                    let state = workspace
                        .get("runtime")
                        .and_then(|runtime| runtime.get("state"))
                        .cloned()
                        .unwrap_or_else(|| json!(null));
                    let mode = state
                        .get("workflowRuntime")
                        .and_then(|workflow| workflow.get("mode"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown");
                    let status = state
                        .get("status")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown");
                    let activities = workspace
                        .get("runtime")
                        .and_then(|runtime| runtime.get("recentActivities"))
                        .and_then(|value| value.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let items = tail_json_array(&activities, limit);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "personaId": persona_id,
                            "status": status,
                            "mode": mode,
                            "activities": items,
                        }))
                        .unwrap_or_default()
                    );
                    println!();
                    sleep(Duration::from_secs(interval_secs)).await;
                }
            }
            Some("list") => {
                let result = rpc_call("list-agent-workspaces", json!({})).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("get") => {
                let persona_id = args
                    .get(1)
                    .cloned()
                    .ok_or("Usage: ctenoctl workspace get <persona_id>")?;

                let result =
                    rpc_call("get-agent-workspace", json!({ "personaId": persona_id })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("send") => {
                let persona_id = args.get(1).cloned().ok_or(
                    "Usage: ctenoctl workspace send <persona_id> -m <message> [--role <id>]",
                )?;
                let mut role_id: Option<String> = None;
                let mut message: Option<String> = None;

                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--role" | "-r" => {
                            i += 1;
                            role_id = args.get(i).cloned();
                        }
                        "--message" | "-m" => {
                            i += 1;
                            message = args.get(i).cloned();
                        }
                        "--help" | "-h" => {
                            println!("Usage: ctenoctl workspace send <persona_id> -m <message> [--role <id>]");
                            return Ok(());
                        }
                        other => {
                            return Err(format!(
                                "Unknown option: {}. Use --help for usage.",
                                other
                            ));
                        }
                    }
                    i += 1;
                }

                let message = message.ok_or("--message is required")?;
                let mut params = json!({
                    "personaId": persona_id,
                    "message": message,
                });
                if let Some(v) = role_id {
                    params["roleId"] = json!(v);
                }

                let result = rpc_call("workspace-send-message", params).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("delete") => {
                let persona_id = args
                    .get(1)
                    .cloned()
                    .ok_or("Usage: ctenoctl workspace delete <persona_id>")?;

                let result =
                    rpc_call("delete-agent-workspace", json!({ "personaId": persona_id })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl workspace <subcommand>");
                println!();
                println!("Subcommands:");
                println!(
                    "  templates                                List built-in workspace templates"
                );
                println!(
                    "  bootstrap --template <id> [options]      Create a multi-agent workspace"
                );
                println!("  list                                     List multi-agent workspaces");
                println!("  get <persona_id>                         Show one workspace");
                println!(
                "  state <persona_id>                       Show runtime state for one workspace"
            );
                println!("  members <persona_id>                     Show workspace members");
                println!(
                    "  activity <persona_id> [--limit <n>]      Show recent public activities"
                );
                println!("  events <persona_id> [--limit <n>]        Show recent runtime events");
                println!("  watch <persona_id> [--interval <secs>]   Poll state + activities");
                println!(
                "  send <persona_id> -m <message> [--role]  Send to orchestrator or a role member"
            );
                println!("  delete <persona_id>                      Delete a workspace and its runtime state");
                Ok(())
            }
            Some(other) => Err(format!("Unknown workspace subcommand: {}", other)),
        }
    }

    async fn workspace_cli_get(persona_id: &str) -> Result<Value, String> {
        let result = rpc_call("get-agent-workspace", json!({ "personaId": persona_id })).await?;
        if !result
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            let error = result
                .get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("Workspace not found");
            return Err(error.to_string());
        }
        result
            .get("workspace")
            .cloned()
            .ok_or_else(|| "Missing workspace payload".to_string())
    }

    fn parse_workspace_limit(args: &[String], default: usize) -> Result<usize, String> {
        let mut limit = default;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--limit" | "-n" => {
                    i += 1;
                    let value = args.get(i).ok_or("--limit requires a numeric value")?;
                    limit = value
                        .parse::<usize>()
                        .map_err(|_| format!("Invalid limit '{}'", value))?;
                }
                "--help" | "-h" => {}
                other => return Err(format!("Unknown option: {}. Use --help for usage.", other)),
            }
            i += 1;
        }
        Ok(limit)
    }

    fn parse_workspace_watch_options(args: &[String]) -> Result<(u64, usize), String> {
        let mut interval_secs = 3_u64;
        let mut limit = 10_usize;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--interval" | "-i" => {
                    i += 1;
                    let value = args.get(i).ok_or("--interval requires a numeric value")?;
                    interval_secs = value
                        .parse::<u64>()
                        .map_err(|_| format!("Invalid interval '{}'", value))?;
                }
                "--limit" | "-n" => {
                    i += 1;
                    let value = args.get(i).ok_or("--limit requires a numeric value")?;
                    limit = value
                        .parse::<usize>()
                        .map_err(|_| format!("Invalid limit '{}'", value))?;
                }
                "--help" | "-h" => {}
                other => return Err(format!("Unknown option: {}. Use --help for usage.", other)),
            }
            i += 1;
        }
        Ok((interval_secs, limit))
    }

    fn tail_json_array(items: &[Value], limit: usize) -> Vec<Value> {
        if limit == 0 || items.is_empty() {
            return Vec::new();
        }
        let start = items.len().saturating_sub(limit);
        items[start..].to_vec()
    }

    // ============================================================================
    // ctenoctl profile
    // ============================================================================

    /// `ctenoctl profile refresh-proxy`
    async fn cmd_profile(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);
        match sub {
            Some("refresh-proxy") => {
                let result = rpc_call("refresh-proxy-profiles", serde_json::json!({})).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            _ => {
                println!("Usage: ctenoctl profile <subcommand>");
                println!();
                println!("Subcommands:");
                println!(
                    "  refresh-proxy    Re-fetch proxy profiles from server (clears stale cache)"
                );
                Ok(())
            }
        }
    }

    // ============================================================================
    // ctenoctl prompt (local-only, no daemon needed)
    // ============================================================================

    /// `ctenoctl prompt render --kind <kind> [--workdir <dir>]`
    fn cmd_prompt(args: Vec<String>) -> i32 {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("render") => {
                let mut kind_str: Option<String> = None;
                let mut workdir: Option<String> = None;
                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--kind" | "-k" => {
                            i += 1;
                            kind_str = args.get(i).cloned();
                        }
                        "--workdir" | "-w" => {
                            i += 1;
                            workdir = args.get(i).cloned();
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let kind_str = match kind_str {
                    Some(k) => k,
                    None => {
                        eprintln!("--kind is required");
                        return 1;
                    }
                };
                let kind = match crate::agent_kind::parse_agent_kind(&kind_str) {
                    Ok(k) => k,
                    Err(e) => {
                        eprintln!("{}", e);
                        return 1;
                    }
                };
                let wd = workdir.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                });

                // Render system prompt for inspection (local-only, no daemon needed)
                let base = crate::system_prompt::build_system_prompt(
                    &crate::system_prompt::PromptOptions {
                        workspace_path: Some(std::path::PathBuf::from(&wd)),
                        agent_instructions: None,
                        include_tool_style: true,
                        current_datetime: Some(chrono::Utc::now().to_rfc3339()),
                        timezone: Some("Asia/Shanghai".to_string()),
                    },
                );
                // Build a synthetic resolution for prompt preview
                let resolution = crate::agent_kind::AgentKindResolution {
                    kind,
                    persona_link: None,
                    persona: None,
                };
                let (prompt, _, _) = crate::agent_kind::build_agent_prompt(&resolution, &base);
                println!("{}", prompt);
                0
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl prompt <subcommand>");
                println!();
                println!("Subcommands:");
                println!("  render --kind <kind> [--workdir <dir>]   Render system prompt");
                0
            }
            Some(other) => {
                eprintln!("Unknown prompt subcommand: {}", other);
                2
            }
        }
    }

    // ============================================================================
    // Helpers
    // ============================================================================

    fn truncate_trace_text(input: &str, max_chars: usize) -> String {
        if input.chars().count() <= max_chars {
            return input.to_string();
        }
        let mut out = String::new();
        for (idx, ch) in input.chars().enumerate() {
            if idx >= max_chars {
                break;
            }
            out.push(ch);
        }
        out.push_str("...");
        out
    }

    fn render_trace_line(session_id: &str, event: &Value) -> Option<String> {
        let timestamp = event
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let event_type = event
            .get("event_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let role = event.get("role").and_then(|v| v.as_str()).unwrap_or("-");
        let text = event.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let tool_name = event.get("tool_name").and_then(|v| v.as_str());
        let call_id = event.get("call_id").and_then(|v| v.as_str());
        let is_error = event
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let text = truncate_trace_text(text, 400);
        let line = match event_type {
            "tool_call" => format!(
                "[{}] session={} event=tool.call role={} tool={} call_id={} input={}",
                timestamp,
                session_id,
                role,
                tool_name.unwrap_or("-"),
                call_id.unwrap_or("-"),
                text
            ),
            "tool_result" => format!(
                "[{}] session={} event=tool.result role={} call_id={} status={} output={}",
                timestamp,
                session_id,
                role,
                call_id.unwrap_or("-"),
                if is_error { "error" } else { "ok" },
                text
            ),
            "assistant_message" => format!(
                "[{}] session={} event=assistant.message text={}",
                timestamp, session_id, text
            ),
            "user_message" => format!(
                "[{}] session={} event=user.message text={}",
                timestamp, session_id, text
            ),
            _ => return None,
        };
        Some(line)
    }

    async fn fetch_and_print_session_trace(
        session_id: &str,
        limit: u64,
        stderr: bool,
    ) -> Result<(), String> {
        let trace = rpc_call(
            "get-session-trace",
            json!({
                "sessionId": session_id,
                "limit": limit,
            }),
        )
        .await?;

        if !trace
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let err = trace
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown trace error");
            return Err(err.to_string());
        }

        let events = trace
            .get("events")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if stderr {
            eprintln!("[trace] session={} events={}", session_id, events.len());
        } else {
            println!("[trace] session={} events={}", session_id, events.len());
        }

        for event in events {
            if let Some(line) = render_trace_line(session_id, &event) {
                if stderr {
                    eprintln!("{}", line);
                } else {
                    println!("{}", line);
                }
            }
        }

        Ok(())
    }

    fn print_help(argv0: &str, dev_mode: bool) {
        println!("Usage: {} [--dev] <command> [options]", argv0);
        println!();
        println!("Commands:");
        println!("  persona                    Manage personas (list, create, delete)");
        println!("  session                    Manage sessions (list, get, stop, kill)");
        println!("  run                        Run an agent task / manage background runs");
        println!("  agent                      Manage custom agents (list, create, run)");
        println!("  workspace                  Bootstrap and message multi-agent workspaces");
        println!("  tool                       List tools");
        println!("  memory                     Read agent memory");
        println!("  profile                    Manage LLM profiles");
        println!("  mcp                        Manage MCP servers (register/connect/toggle)");
        println!("  auth                       Manage headless account/machine auth");
        println!("  connect                    Provider connect commands (headless placeholders)");
        println!("  status                     Show daemon state");
        println!("  version                    Show ctenoctl version");
        println!("  help                       Show this help");
        if dev_mode {
            println!();
            println!("Dev commands (--dev):");
            println!("  orchestration              Manage orchestration flows");
            println!(
            "  webview                    Interact with the Tauri webview (eval JS, screenshot)"
        );
        }
        println!();
        println!("Environment:");
        println!("  --target agentd|tauri-dev|tauri Connect to a specific host shell");
        println!("  CTENO_ENV=agentd|dev|release    Force connect to a specific daemon socket");
        println!();
        println!("Examples:");
        println!("  {} persona list", argv0);
        println!("  {} persona create --name \"New Persona\"", argv0);
        println!("  {} --target agentd status", argv0);
        println!("  {} --target tauri-dev status", argv0);
        println!("  {} auth login", argv0);
        println!("  {} auth machine", argv0);
        println!("  {} mcp list", argv0);
        println!(
            "  {} workspace bootstrap --template coding-studio -n \"Feature Squad\"",
            argv0
        );
        println!("  {} --target agentd workspace templates", argv0);
        println!(
            "  {} --target agentd workspace activity <persona_id> --limit 20",
            argv0
        );
        println!("  {} run --kind worker --message \"Write hello.py\"", argv0);
        println!("  {} session get <session_id>", argv0);
        println!("  {} session trace <session_id> -n 200", argv0);
        if dev_mode {
            println!(
                "  {} --dev orchestration create --persona <id> --flow flow.json",
                argv0
            );
        } else {
            println!();
            println!("Use --dev flag to access dev commands (orchestration, webview)");
        }
    }

    // ============================================================================
    // ctenoctl orchestration
    // ============================================================================

    /// `ctenoctl orchestration create --persona <id> --title <t> --flow <json_file>`
    /// `ctenoctl orchestration get --persona <id>`
    /// `ctenoctl orchestration delete --id <flow_id>`
    async fn cmd_orchestration(args: Vec<String>) -> Result<(), String> {
        let sub = args.first().map(String::as_str);

        match sub {
            Some("create") => {
                let mut persona_id: Option<String> = None;
                let mut session_id: Option<String> = None;
                let mut title: Option<String> = None;
                let mut flow_file: Option<String> = None;

                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--persona" => {
                            i += 1;
                            persona_id = args.get(i).cloned();
                        }
                        "--session" => {
                            i += 1;
                            session_id = args.get(i).cloned();
                        }
                        "--title" => {
                            i += 1;
                            title = args.get(i).cloned();
                        }
                        "--flow" => {
                            i += 1;
                            flow_file = args.get(i).cloned();
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let persona_id = persona_id.ok_or("--persona <id> is required")?;
                let title = title.unwrap_or_else(|| "Orchestration".to_string());
                let flow_file = flow_file.ok_or("--flow is required")?;

                let flow_json = std::fs::read_to_string(&flow_file)
                    .map_err(|e| format!("Failed to read flow file '{}': {}", flow_file, e))?;
                let flow: Value = serde_json::from_str(&flow_json)
                    .map_err(|e| format!("Invalid JSON in flow file: {}", e))?;

                let nodes = flow.get("nodes").cloned().unwrap_or(json!([]));
                let edges = flow.get("edges").cloned().unwrap_or(json!([]));

                let result = rpc_call(
                    "create-orchestration-flow",
                    json!({
                        "personaId": persona_id,
                        "sessionId": session_id.unwrap_or_default(),
                        "title": title,
                        "nodes": nodes,
                        "edges": edges,
                    }),
                )
                .await?;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("get") => {
                let mut persona_id: Option<String> = None;
                let mut flow_id: Option<String> = None;

                let mut i = 1;
                while i < args.len() {
                    match args[i].as_str() {
                        "--persona" => {
                            i += 1;
                            persona_id = args.get(i).cloned();
                        }
                        "--id" => {
                            i += 1;
                            flow_id = args.get(i).cloned();
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let mut params = json!({});
                if let Some(fid) = flow_id {
                    params["flowId"] = json!(fid);
                } else {
                    let pid = persona_id.ok_or("--persona <id> or --id <flow_id> is required")?;
                    params["personaId"] = json!(pid);
                }

                let result = rpc_call("get-orchestration-flow", params).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("delete") => {
                let flow_id = args
                    .get(1)
                    .and_then(|_| {
                        let mut i = 1;
                        while i < args.len() {
                            if args[i] == "--id" {
                                return args.get(i + 1).cloned();
                            }
                            i += 1;
                        }
                        None
                    })
                    .ok_or("--id is required")?;

                let result =
                    rpc_call("delete-orchestration-flow", json!({ "flowId": flow_id })).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
                Ok(())
            }
            Some("--help") | Some("-h") | None => {
                println!("Usage: ctenoctl orchestration <create|get|delete>");
                println!();
                println!("Subcommands:");
                println!("  create --flow <json_file> [--persona <id>] [--title <t>]  Create flow (persona auto-detected)");
                println!("  get [--persona <id>]                                     Get flow by persona (auto-detected)");
                println!(
                    "  get --id <flow_id>                                       Get flow by ID"
                );
                println!("  delete --id <flow_id>                                  Delete flow");
                Ok(())
            }
            Some(other) => Err(format!("Unknown orchestration subcommand: {}", other)),
        }
    }

    fn print_status() -> Result<(), String> {
        fn is_connectable(path: &std::path::Path) -> bool {
            std::os::unix::net::UnixStream::connect(path).is_ok()
        }

        let state_path = daemon_state_path()?;
        let state = read_daemon_state(&state_path)?;

        println!("ctenoctl {}", env!("CARGO_PKG_VERSION"));
        println!("daemon_state_file: {}", state_path.display());

        let selected_target = std::env::var("CTENO_ENV").unwrap_or_else(|_| "auto".to_string());
        // Show all known socket paths
        let agentd_sock = cteno_host_bridge_localrpc::socket_path_for_env("agentd");
        let dev_sock = cteno_host_bridge_localrpc::socket_path_for_env("dev");
        let release_sock = cteno_host_bridge_localrpc::socket_path_for_env("");
        let active_sock = cteno_host_bridge_localrpc::socket_path();
        println!(
            "daemon_socket_agentd: {} ({})",
            agentd_sock.display(),
            if agentd_sock.exists() {
                "exists"
            } else {
                "missing"
            }
        );
        println!(
            "daemon_socket_dev: {} ({})",
            dev_sock.display(),
            if dev_sock.exists() {
                "exists"
            } else {
                "missing"
            }
        );
        println!(
            "daemon_socket_release: {} ({})",
            release_sock.display(),
            if release_sock.exists() {
                "exists"
            } else {
                "missing"
            }
        );
        println!("daemon_target_selected: {}", selected_target);
        println!("daemon_socket_active: {}", active_sock.display());
        println!(
            "daemon_socket_active_connectable: {}",
            if is_connectable(&active_sock) {
                "yes"
            } else {
                "no"
            }
        );

        let active_connectable = is_connectable(&active_sock);

        match state {
            Some(state) => {
                let running = is_process_running(state.pid);
                let services_ready =
                    active_connectable || (running && daemon_runtime::is_daemon_ready());
                println!("daemon_mode: {}", state.mode);
                println!("daemon_pid: {}", state.pid);
                println!("daemon_started_at: {}", state.started_at);
                println!("daemon_version: {}", state.version);
                println!("daemon_running: {}", if running { "yes" } else { "no" });
                println!(
                    "services_ready: {}",
                    if services_ready { "yes" } else { "no" }
                );
                println!(
                    "daemon_target_connected: {}",
                    if active_connectable { "yes" } else { "no" }
                );
            }
            None => {
                println!("daemon_running: no");
                println!("services_ready: no");
                println!("daemon_target_connected: no");
                println!("daemon_state: missing");
            }
        }

        if let Ok(auth) = direct_auth_status() {
            if let Some(kind) = auth.get("shellKind").and_then(|v| v.as_str()) {
                println!("shell_kind: {}", kind);
            }
            if let Some(path) = auth.get("appDataDir").and_then(|v| v.as_str()) {
                println!("app_data_dir: {}", path);
            }
            if let Some(path) = auth.get("configPath").and_then(|v| v.as_str()) {
                println!("config_path: {}", path);
            }
            if let Some(path) = auth.get("profilesPath").and_then(|v| v.as_str()) {
                println!("profiles_path: {}", path);
            }
            if let Some(path) = auth.get("machineIdPath").and_then(|v| v.as_str()) {
                println!("machine_id_path: {}", path);
            }
            if let Some(env_tag) = auth.get("localRpcEnvTag").and_then(|v| v.as_str()) {
                println!("local_rpc_env: {}", env_tag);
            }
            println!(
                "account_authenticated: {}",
                if auth
                    .get("accountAuthenticated")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "yes"
                } else {
                    "no"
                }
            );
            println!(
                "machine_authenticated: {}",
                if auth
                    .get("machineAuthenticated")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "yes"
                } else {
                    "no"
                }
            );
            println!(
                "machine_pending: {}",
                if auth
                    .get("machinePending")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "yes"
                } else {
                    "no"
                }
            );
        }

        Ok(())
    }

    fn read_daemon_state(path: &PathBuf) -> Result<Option<DaemonState>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read daemon state: {}", e))?;
        let parsed: DaemonState = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse daemon state: {}", e))?;
        Ok(Some(parsed))
    }

    fn is_process_running(pid: u32) -> bool {
        #[cfg(unix)]
        {
            Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }

        #[cfg(windows)]
        {
            Command::new("cmd")
                .args([
                    "/C",
                    &format!(
                        "tasklist /FI \"PID eq {}\" | findstr /R /C:\"[ ]{}[ ]\" >NUL",
                        pid, pid
                    ),
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = pid;
            false
        }
    }
} // end mod commercial_impl

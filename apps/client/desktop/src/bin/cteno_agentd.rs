fn main() {
    if let Err(e) = configure_from_args(std::env::args().skip(1).collect()) {
        eprintln!("cteno-agentd argument error: {}", e);
        print_help();
        std::process::exit(2);
    }

    if let Err(e) = cteno_lib::run_agent_daemon() {
        eprintln!("cteno-agentd failed: {}", e);
        std::process::exit(1);
    }
}

fn configure_from_args(args: Vec<String>) -> Result<(), String> {
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--managed" => {
                std::env::set_var("CTENO_MANAGED_MODE", "1");
            }
            "--bootstrap-token" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--bootstrap-token requires a value".to_string())?;
                std::env::set_var("CTENO_MANAGED_BOOTSTRAP_TOKEN", value);
            }
            value if value.starts_with("--bootstrap-token=") => {
                let token = value.trim_start_matches("--bootstrap-token=");
                if token.is_empty() {
                    return Err("--bootstrap-token requires a non-empty value".to_string());
                }
                std::env::set_var("CTENO_MANAGED_BOOTSTRAP_TOKEN", token);
            }
            other => {
                return Err(format!("unknown argument '{}'", other));
            }
        }
        index += 1;
    }
    Ok(())
}

fn print_help() {
    eprintln!("Usage: cteno-agentd [--managed] [--bootstrap-token <token>]");
    eprintln!();
    eprintln!("Modes:");
    eprintln!("  default                     Interactive/local machine auth");
    eprintln!("  --managed                   Remote managed mode (requires bootstrap token)");
    eprintln!();
    eprintln!("Managed mode:");
    eprintln!("  --bootstrap-token <token>   Short-lived managed bootstrap token");
    eprintln!("  CTENO_MANAGED_MODE=1        Enable managed mode via environment");
    eprintln!("  CTENO_MANAGED_BOOTSTRAP_TOKEN=<token>");
}

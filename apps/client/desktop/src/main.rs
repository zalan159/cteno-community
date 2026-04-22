// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if should_run_ctenoctl() {
        let argv0 = std::env::args().next();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let exit_code = cteno_lib::run_ctenoctl(argv0, args);
        std::process::exit(exit_code);
    }

    cteno_lib::run()
}

fn should_run_ctenoctl() -> bool {
    let argv0 = std::env::args().next().unwrap_or_default();
    let executable = std::path::Path::new(&argv0)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    executable == "ctenoctl" || executable == "ctenoctl.exe"
}

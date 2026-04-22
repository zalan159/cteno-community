use env_logger::Env;

pub(crate) fn run() -> Result<(), String> {
    // Safe for repeated calls in tests/dev where logger may already be initialized.
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .try_init();

    // NOTE on signal handling: cteno-agentd does not install a custom
    // SIGTERM / SIGINT handler because (a) it would pull in an extra crate
    // for questionable benefit on the dev watch loop, and (b) the
    // `SubprocessSupervisor` performs an orphan sweep on the next daemon
    // boot which SIGTERMs any stale children left behind by a hard kill.
    // The watch loop below exits cleanly on dev frontend disappearance and
    // triggers the supervisor drain at the bottom of this function.

    let bootstrap =
        crate::host::shells::setup_headless_daemon(crate::commands::MachineAuthState::new())?;
    log::info!("cteno-agentd starting in {} mode...", bootstrap.daemon_mode);

    log::info!("cteno-agentd app data dir: {:?}", bootstrap.app_data_dir);
    log::info!(
        "cteno-agentd shell: {} (local rpc env: {})",
        bootstrap.paths.identity.shell_kind.as_str(),
        bootstrap.paths.identity.local_rpc_env_tag
    );
    log::info!(
        "cteno-agentd config path: {:?}",
        bootstrap.paths.identity.config_path
    );
    log::info!(
        "cteno-agentd profiles path: {:?}",
        bootstrap.paths.identity.profiles_path
    );
    log::info!(
        "cteno-agentd machine id path: {:?}",
        bootstrap.paths.identity.machine_id_path
    );
    log::info!(
        "cteno-agentd builtin skills dir: {:?} (exists: {})",
        bootstrap.paths.builtin_skills_dir,
        bootstrap.paths.builtin_skills_dir.exists()
    );
    log::info!(
        "cteno-agentd agents dir: {:?} (exists: {})",
        bootstrap.paths.builtin_agents_dir,
        bootstrap.paths.builtin_agents_dir.exists()
    );

    crate::host::shells::run_headless_dev_frontend_watch_loop();

    // If the watch loop returns (dev frontend missing → graceful exit),
    // drain the supervisor so the next daemon start doesn't need orphan
    // sweeping for our own children.
    if let Some(sup) = crate::local_services::subprocess_supervisor() {
        log::info!("cteno-agentd exiting normally; draining subprocess supervisor");
        sup.kill_all();
    }

    Ok(())
}

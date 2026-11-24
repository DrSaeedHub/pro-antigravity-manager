//! `AntigravityAccountSwitcher.exe`: the app (window and tray), `--tray` to start in the tray
//! (used when starting with Windows), or `--launch <name>` from an account shortcut.
#![windows_subsystem = "windows"]

use std::process::ExitCode;
use std::time::Duration;

use app::controller::Start;
use app::{bootstrap, cli, controller, window};

/// The name that marks the running copy for this Windows user.
const RUNNING_COPY: &str = "AntigravityAccountSwitcher";
/// How long a second start waits for the running copy to show its window.
const ANSWER_WITHIN: Duration = Duration::from_secs(3);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = window::select_backend() {
        tracing::error!(%error, "no window system");
        return ExitCode::FAILURE;
    }
    let services = match bootstrap::build(bootstrap::windows_parts()) {
        Ok(services) => services,
        Err(error) => {
            window::show_startup_problem(&error.to_string());
            return ExitCode::FAILURE;
        }
    };
    if let Ok(dir) = services.accounts.desktop().data_dir() {
        let _ = kernel::logging::init(&dir.join("logs"));
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "started");

    let result = if let Some(name) = cli::launch_target(&args) {
        cli::run(&services, name)
    } else {
        let start = if cli::tray_start(&args) { Start::Tray } else { Start::Window };
        match desktop::instance::claim(RUNNING_COPY) {
            Ok(desktop::instance::Claim::AlreadyRunning) if start == Start::Tray => {
                tracing::info!("already running");
                return ExitCode::SUCCESS;
            }
            Ok(desktop::instance::Claim::AlreadyRunning) => {
                if desktop::instance::wake_running_copy(RUNNING_COPY, ANSWER_WITHIN) {
                    tracing::info!("already running: showed its window");
                    return ExitCode::SUCCESS;
                }
                // The other copy does not answer (it hangs or is ending): run anyway.
                tracing::warn!("the running copy does not answer; starting another");
                controller::run(services.clone(), start, None)
            }
            Ok(desktop::instance::Claim::First(running)) => {
                let wake = move || running.wait_for_wake(Duration::from_secs(3600));
                controller::run(services.clone(), start, Some(Box::new(wake)))
            }
            Err(error) => {
                tracing::warn!(%error, "running without the single-copy guard");
                controller::run(services.clone(), start, None)
            }
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the window failed");
            ExitCode::FAILURE
        }
    }
}

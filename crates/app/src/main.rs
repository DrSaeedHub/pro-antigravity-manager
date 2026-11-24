//! `AntigravityAccountSwitcher.exe`: the app (window and tray), `--tray` to start in the tray
//! (used when starting with Windows), `--launch <name>` from an account shortcut, `--version` for
//! the updater, `--updated <old version>` after an update, and `--uninstall` for the uninstaller.
#![windows_subsystem = "windows"]

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use app::controller::{Outcome, Start};
use app::{bootstrap, cli, controller, window};
use desktop::instance::Claim;

/// The name that marks the running copy for this Windows user (the installer uses it too).
const RUNNING_COPY: &str = "AntigravityAccountSwitcher";
/// How long a second start waits for the running copy to show its window.
const ANSWER_WITHIN: Duration = Duration::from_secs(3);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if cli::version_request(&args) {
        // Asked by the updater of an older copy: answer and leave, touching nothing.
        let _ = writeln!(std::io::stdout(), "{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
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

    if cli::uninstall_request(&args) {
        let _ = cli::uninstall(&services);
        return ExitCode::SUCCESS;
    }
    if let Some(name) = cli::launch_target(&args) {
        return finish(cli::run(&services, name).map(|()| Outcome::Quit), None);
    }

    let start = if cli::tray_start(&args) { Start::Tray } else { Start::Window };
    let updated_from = cli::updated_from(&args);
    match desktop::instance::claim(RUNNING_COPY) {
        Ok(Claim::AlreadyRunning) if start == Start::Tray => {
            tracing::info!("already running");
            ExitCode::SUCCESS
        }
        Ok(Claim::AlreadyRunning) => {
            if desktop::instance::wake_running_copy(RUNNING_COPY, ANSWER_WITHIN) {
                tracing::info!("already running: showed its window");
                return ExitCode::SUCCESS;
            }
            // The other copy does not answer (it hangs or is ending): run anyway.
            tracing::warn!("the running copy does not answer; starting another");
            finish(controller::run(services, start, updated_from, None), None)
        }
        Ok(Claim::First(running)) => {
            let running = Arc::new(running);
            let listening = running.clone();
            let requests = move || listening.wait(Duration::from_secs(3600));
            let outcome = controller::run(services, start, updated_from, Some(Box::new(requests)));
            finish(outcome, Some(&running))
        }
        Err(error) => {
            tracing::warn!(%error, "running without the single-copy guard");
            finish(controller::run(services, start, updated_from, None), None)
        }
    }
}

/// Ends the process; after an update, gives up the single-copy lock and starts the new version.
fn finish(
    outcome: Result<Outcome, slint::PlatformError>,
    running: Option<&desktop::instance::RunningCopy>,
) -> ExitCode {
    match outcome {
        Ok(Outcome::Quit) => ExitCode::SUCCESS,
        Ok(Outcome::Restart(exe)) => {
            if let Some(running) = running {
                running.release();
            }
            match std::process::Command::new(&exe)
                .args([cli::UPDATED, env!("CARGO_PKG_VERSION")])
                .spawn()
            {
                Ok(_) => tracing::info!(exe = %exe.display(), "started the new version"),
                Err(error) => tracing::error!(%error, "the new version did not start"),
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "the window failed");
            ExitCode::FAILURE
        }
    }
}

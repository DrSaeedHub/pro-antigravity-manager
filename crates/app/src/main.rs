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
use desktop::instance::{Claim, Reached, RunningCopy};

/// The name that marks the running copy for this Windows user (the installer uses it too).
const RUNNING_COPY: &str = "AntigravityAccountSwitcher";
/// How long a second start waits for each answer of the running copy.
const ANSWER_WITHIN: Duration = Duration::from_secs(3);
/// How long a second start keeps asking a running copy that is alive but does not answer.
const PATIENCE: Duration = Duration::from_secs(10);
/// The installer runs without any window and opens the app again when it is done.
const SETUP_SWITCHES: [&str; 4] = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH"];

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
            window::show_problem("app.startup", &error.to_string());
            return ExitCode::FAILURE;
        }
    };
    if let Ok(dir) = services.accounts.desktop().data_dir() {
        let _ = kernel::logging::init(&dir.join("logs"));
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "started");

    if cli::uninstall_dialog_request(&args) {
        let answer = app::uninstall::ask(&services).unwrap_or_else(|error| {
            tracing::error!(%error, "the uninstall window failed");
            None
        });
        return ExitCode::from(app::uninstall::exit_code(answer));
    }
    if let Some(scope) = cli::uninstall_request(&args) {
        let _ = cli::uninstall(&services, scope);
        return ExitCode::SUCCESS;
    }
    if let Some(name) = cli::launch_target(&args) {
        return finish(cli::run(&services, name).map(|()| Outcome::Quit), None);
    }

    let start = if cli::tray_start(&args) { Start::Tray } else { Start::Window };
    let updated_from = cli::updated_from(&args);
    let running = match become_running_copy(start) {
        Ok(running) => running,
        Err(code) => return code,
    };
    match running {
        Some(running) => {
            let running = Arc::new(running);
            let listening = running.clone();
            let requests = move || listening.wait(Duration::from_secs(3600));
            let outcome = controller::run(services, start, updated_from, Some(Box::new(requests)));
            finish(outcome, Some(&running))
        }
        None => finish(controller::run(services, start, updated_from, None), None),
    }
}

/// One copy per Windows user. When one already runs, its window comes back (restored and in
/// front) and this start ends; a quiet start (`--tray`) just ends. The lock is taken over only
/// from a holder that is gone. `Err` is this start's exit code; `Ok(None)` runs unguarded
/// because Windows refused the lock itself.
fn become_running_copy(start: Start) -> Result<Option<RunningCopy>, ExitCode> {
    let quiet = start == Start::Tray;
    match desktop::instance::claim(RUNNING_COPY) {
        Ok(Claim::First(running)) => Ok(Some(running)),
        Ok(Claim::AlreadyRunning) => {
            match desktop::instance::reach(RUNNING_COPY, quiet, ANSWER_WITHIN, PATIENCE) {
                Reached::Running => {
                    tracing::info!(quiet, "already running: brought its window back");
                    Err(ExitCode::SUCCESS)
                }
                Reached::Unresponsive => {
                    tracing::warn!("a running copy does not answer; not starting another");
                    if !quiet {
                        window::show_problem("app.not_responding", "");
                    }
                    Err(ExitCode::FAILURE)
                }
                Reached::Gone => match desktop::instance::take_over(RUNNING_COPY) {
                    Ok(running) => {
                        tracing::warn!("the previous copy is gone; this one takes over");
                        Ok(Some(running))
                    }
                    Err(error) => {
                        tracing::warn!(%error, "running without the single-copy guard");
                        Ok(None)
                    }
                },
            }
        }
        Err(error) => {
            tracing::warn!(%error, "running without the single-copy guard");
            Ok(None)
        }
    }
}

/// Ends the process; after an update, gives up the single-copy lock and starts the new version.
fn finish(
    outcome: Result<Outcome, slint::PlatformError>,
    running: Option<&RunningCopy>,
) -> ExitCode {
    match outcome {
        Ok(Outcome::Quit) => ExitCode::SUCCESS,
        Ok(Outcome::RunSetup(setup)) => {
            if let Some(running) = running {
                running.release();
            }
            match std::process::Command::new(&setup).args(SETUP_SWITCHES).spawn() {
                Ok(_) => {
                    tracing::info!(setup = %setup.display(), "the installer finishes the update");
                }
                Err(error) => tracing::error!(%error, "the installer did not start"),
            }
            ExitCode::SUCCESS
        }
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

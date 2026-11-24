//! A stand-in executable for the update tests. With `--version` it behaves by its own file name:
//! a name with `hang` never answers, `fail` exits with an error, `quiet` prints nothing, anything
//! else prints `4.5.6`. Without arguments it waits a minute (a "running app" to replace).

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let name = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if std::env::args().nth(1).as_deref() != Some("--version") || name.contains("hang") {
        std::thread::sleep(Duration::from_secs(60));
        return ExitCode::SUCCESS;
    }
    if name.contains("fail") {
        return ExitCode::from(3);
    }
    if !name.contains("quiet") {
        let _ = writeln!(std::io::stdout(), "4.5.6");
    }
    ExitCode::SUCCESS
}

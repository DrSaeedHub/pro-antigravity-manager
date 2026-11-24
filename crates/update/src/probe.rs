//! Making sure a downloaded executable starts and is the version it claims to be.

use std::fmt::Debug;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::UpdateError;

/// The command-line switch that makes the app print its version and exit.
pub const VERSION_SWITCH: &str = "--version";

/// Asks an executable for its version.
pub trait VersionProbe: Send + Sync + Debug {
    /// The version `exe` reports.
    fn version(&self, exe: &Path) -> Result<String, UpdateError>;
}

/// Runs `exe --version` without a window and reads the first line it prints.
#[derive(Debug, Clone, Copy)]
pub struct RunProbe {
    /// How long the executable may take to answer.
    pub timeout: Duration,
}

impl Default for RunProbe {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(15) }
    }
}

fn failed(reason: impl std::fmt::Display) -> UpdateError {
    UpdateError::Probe(reason.to_string())
}

impl VersionProbe for RunProbe {
    fn version(&self, exe: &Path) -> Result<String, UpdateError> {
        let mut command = Command::new(exe);
        command
            .arg(VERSION_SWITCH)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// `CREATE_NO_WINDOW`: no console flashes up.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(failed)?;
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(failed)? {
                break status;
            }
            if started.elapsed() >= self.timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(format!("no answer within {} s", self.timeout.as_secs())));
            }
            std::thread::sleep(Duration::from_millis(40));
        };
        if !status.success() {
            return Err(failed(format!("it exited with {status}")));
        }
        let mut output = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout.read_to_string(&mut output).map_err(failed)?;
        }
        let version = output.lines().next().unwrap_or_default().trim().to_owned();
        if version.is_empty() {
            return Err(failed("it printed no version"));
        }
        Ok(version)
    }
}

/// A probe with a fixed answer, for tests.
#[derive(Debug, Clone)]
pub struct FakeProbe {
    answer: Result<String, UpdateError>,
}

impl FakeProbe {
    /// Every executable reports `version`.
    #[must_use]
    pub fn answering(version: &str) -> Self {
        Self { answer: Ok(version.to_owned()) }
    }

    /// Every executable fails to start.
    #[must_use]
    pub fn failing(reason: &str) -> Self {
        Self { answer: Err(failed(reason)) }
    }
}

impl VersionProbe for FakeProbe {
    fn version(&self, _exe: &Path) -> Result<String, UpdateError> {
        self.answer.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_answers_what_it_was_told() {
        assert_eq!(FakeProbe::answering("1.2.0").version(Path::new("x")).unwrap(), "1.2.0");
        let error = FakeProbe::failing("no").version(Path::new("x")).unwrap_err();
        assert_eq!(error, UpdateError::Probe("no".into()));
    }

    #[test]
    fn a_missing_file_does_not_start() {
        let error = RunProbe::default().version(Path::new("Z:/nothing/here.exe")).unwrap_err();
        assert_eq!(error.code(), "update.probe");
    }
}

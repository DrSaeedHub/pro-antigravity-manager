//! Desktop integration: per-account shortcuts, installing a local copy of the app, and the
//! folder where the app keeps its own files.

pub mod fake;
#[cfg(windows)]
pub mod instance;
pub mod preferences;
pub mod service;
pub mod shell;
#[cfg(windows)]
pub mod windows;

pub use fake::FakeShell;
pub use preferences::{PreferenceStore, Preferences};
pub use service::{Desktop, DesktopSettings};
pub use shell::{DesktopError, Shell, Shortcut};
#[cfg(windows)]
pub use windows::{WindowsShell, work_area};

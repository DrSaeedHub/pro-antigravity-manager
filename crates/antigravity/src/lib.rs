//! The Antigravity app on this PC: where it is installed, whether it runs, closing it (politely,
//! then by force), starting it, and finding its language-server files.

pub mod fake;
pub mod host;
pub mod install;
#[cfg(windows)]
pub mod windows;

pub use fake::FakeHost;
pub use host::{Antigravity, AntigravityError, AppHost, HostSettings};
pub use install::{language_server_files, locate};
#[cfg(windows)]
pub use windows::WindowsHost;

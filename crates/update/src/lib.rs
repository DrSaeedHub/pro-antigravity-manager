//! Self-update from the project's GitHub releases.
//!
//! Every release carries `latest.json`, a small manifest with the version and, per platform, the
//! package (the portable zip), its size and SHA-256. GitHub serves the newest stable release's copy
//! at `<repository>/releases/latest/download/latest.json`, which is not rate-limited the way the
//! REST API is, so the app can look every minute.
//!
//! Installing an update: download the package next to the running exe, check its size and hash,
//! take the exe out of it, make sure it starts and reports the offered version, then rename the
//! running exe aside and move the new one into its name. The app restarts on the new file; the
//! renamed old one is deleted on the next start.

pub mod download;
pub mod error;
pub mod feed;
pub mod package;
pub mod probe;
pub mod replace;
pub mod service;

pub use error::UpdateError;
pub use feed::{Package, Release, is_newer};
pub use probe::{FakeProbe, RunProbe, VersionProbe};
pub use service::{Check, UpdateSettings, Updater, platform};

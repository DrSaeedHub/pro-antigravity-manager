//! Shared kernel used by every module: time, Windows known folders, secret values, lenient JSON
//! reading and logging.
//!
//! It holds no business rules and depends on no other crate of the workspace.

pub mod clock;
pub mod folders;
pub mod json;
pub mod logging;
pub mod secret;

pub use clock::{Clock, ManualClock, SystemClock};
#[cfg(windows)]
pub use folders::SystemFolders;
pub use folders::{FixedFolders, Folder, FolderError, KnownFolders};
pub use secret::Secret;

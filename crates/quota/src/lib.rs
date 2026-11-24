//! Remaining quota of saved accounts.
//!
//! Each account's own refresh token is exchanged for an access token with the "installed app"
//! sign-in client that ships inside Antigravity (found by scanning its language server once and
//! cached encrypted for the Windows user). Then Google's Cloud Code API reports the plan, the
//! 5-hour and weekly limits, and every model's remaining fraction.

pub mod client;
pub mod error;
pub mod format;
pub mod fuel;
pub mod model;
pub mod parse;
pub mod proxy;
pub mod secret_box;
pub mod service;
#[cfg(windows)]
pub mod windows;

pub use error::QuotaError;
pub use fuel::{Decision, Fuel, decide};
pub use model::{AccountQuota, ModelQuota, QuotaBucket, QuotaGroup};
pub use proxy::{NoProxy, ProxyResolver};
pub use secret_box::{FakeBox, SecretBox};
pub use service::{QuotaService, QuotaSettings};
#[cfg(windows)]
pub use windows::{Dpapi, SystemProxy};

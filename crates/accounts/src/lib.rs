//! Saved Antigravity accounts.
//!
//! Antigravity keeps its sign-in in one Windows credential (`gemini:antigravity`). A saved account
//! is a copy of that credential under `antigravity-switcher:<name>`; switching copies it back and
//! starts Antigravity. Adding signs in fresh, then saves the copy, restoring the previous sign-in
//! if anything goes wrong.

pub mod add;
pub mod error;
pub mod name;
pub mod service;

pub use add::{AddOutcome, AddSession};
pub use error::AccountsError;
pub use name::{MAX_NAME_LENGTH, is_valid_name};
pub use service::{AccountSettings, Accounts, RemoveOutcome, SavedAccount};

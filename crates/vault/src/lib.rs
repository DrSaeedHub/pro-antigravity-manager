//! Credential access. The app copies whole Windows generic credentials between names and reads
//! the sign-in JSON inside them; it never changes their content.

pub mod blob;
pub mod memory;
pub mod store;
#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod contract;

pub use blob::{SignIn, SignInError, decode_text, read_email};
pub use memory::{MemoryStore, Operation};
pub use store::{CredentialStore, VaultError};
#[cfg(windows)]
pub use windows::WindowsStore;

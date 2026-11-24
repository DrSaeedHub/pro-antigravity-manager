//! The shared HTTP client.
//!
//! Every request the app makes (Google's quota service, GitHub's release feed) goes through one
//! client builder and the proxy the signed-in Windows user has set up: a static proxy, a PAC
//! script, or automatic detection.

pub mod client;
pub mod proxy;
#[cfg(windows)]
pub mod windows;

pub use client::{HttpOptions, client, describe};
pub use proxy::{NoProxy, ProxyResolver};
#[cfg(windows)]
pub use windows::SystemProxy;

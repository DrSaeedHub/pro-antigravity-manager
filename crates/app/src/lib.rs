//! The desktop app: Slint UI, window setup, command line and composition root.

pub mod bootstrap;
pub mod cli;
pub mod controller;
pub mod uninstall;
pub mod view;
pub mod window;

/// The generated Slint UI types.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::restriction,
    missing_debug_implementations,
    unused_qualifications,
    rust_2018_idioms,
    unsafe_code,
    reason = "generated code"
)]
pub mod ui {
    slint::include_modules!();
}

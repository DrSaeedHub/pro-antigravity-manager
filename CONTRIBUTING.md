# Contributing

1. Open an issue describing the bug or the change first.
2. Keep pull requests focused; no unrelated formatting.
3. Never include real credentials, account names, emails or logs with personal data. Tests use
   throw-away credential names (a scoped store refuses real ones) and pretend servers.
4. Run `cargo xtask check` and make it green: format, clippy (pedantic, warnings as errors),
   cargo-deny, cargo-machete, the environment rule, and every test with at least 95% line
   coverage outside the UI crate.
5. User-facing sentences live in `crates/app/ui/copy.slint` and other `.slint` files, wrapped in
   `@tr(...)`; Rust code sends stable codes, never sentences.
6. Every Windows call sits in a `windows.rs` adapter file behind a trait with a fake; `unsafe`
   blocks carry a `SAFETY:` comment.

By contributing you agree that your contribution is licensed under the MIT License.

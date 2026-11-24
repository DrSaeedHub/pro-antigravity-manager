//! Project tasks.
//!
//! - `cargo xtask check`: the gate. Format, lint, licenses and advisories, unused dependencies,
//!   the environment rule, then every test with the coverage gate. Stops at the first failure.
//! - `cargo xtask dist`: a release build packed as `dist/AntigravityAccountSwitcher-<version>.zip`
//!   with its SHA-256.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool reports to the console"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Minimum line coverage of the non-UI crates.
const COVERAGE_LINES: &str = "95";
/// Files not counted for coverage: the task runner itself and the UI crate (tested through the
/// Slint testing backend and screenshots instead).
const COVERAGE_IGNORE: &str = r"(xtask|crates[\\/]app[\\/])";

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "check" => check(),
        "dist" => dist(),
        _ => {
            eprintln!("usage: cargo xtask <check|dist>");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("\nxtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn run(title: &str, program: &str, args: &[&str]) -> Result<(), String> {
    println!("\n== {title}");
    let status = Command::new(program)
        .args(args)
        .current_dir(root())
        .status()
        .map_err(|error| format!("{title}: could not start `{program}`: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("{title} failed ({status})")) }
}

fn check() -> Result<(), String> {
    run("format", "cargo", &["fmt", "--all", "--check"])?;
    run(
        "lint",
        "cargo",
        &["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
    )?;
    run("licenses and advisories", "cargo", &["deny", "--workspace", "check"])?;
    run("unused dependencies", "cargo-machete", &["--skip-target-dir", "."])?;
    println!("\n== environment rule");
    env_rule()?;
    run(
        "tests and coverage",
        "cargo",
        &[
            "llvm-cov",
            "nextest",
            "--workspace",
            "--locked",
            "--ignore-filename-regex",
            COVERAGE_IGNORE,
            "--fail-under-lines",
            COVERAGE_LINES,
        ],
    )?;
    println!("\nThe gate is green.");
    Ok(())
}

/// The app reads no environment variables (see `.env.example`); only `xtask` may.
fn env_rule() -> Result<(), String> {
    let mut offenders = Vec::new();
    visit(&root().join("crates"), &mut |path, text| {
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if ["std::env::var", "env::var(", "env::var_os(", "option_env!(", "env::vars("]
                .iter()
                .any(|needle| code.contains(needle))
            {
                offenders.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    })
    .map_err(|error| format!("environment rule: {error}"))?;
    if offenders.is_empty() {
        println!("no environment reads in the app crates");
        Ok(())
    } else {
        Err(format!("environment variables read outside the rule: {}", offenders.join(", ")))
    }
}

fn visit(dir: &Path, on_file: &mut dyn FnMut(&Path, &str)) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            visit(&path, on_file)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            on_file(&path, &fs::read_to_string(&path)?);
        }
    }
    Ok(())
}

fn dist() -> Result<(), String> {
    run("release build", "cargo", &["build", "--release", "--locked", "--package", "app"])?;
    let root = root();
    let version = env!("CARGO_PKG_VERSION");
    let out = root.join("dist");
    fs::create_dir_all(&out).map_err(|error| format!("dist folder: {error}"))?;
    let zip = out.join(format!("AntigravityAccountSwitcher-{version}.zip"));
    let exe = root.join("target").join("release").join("AntigravityAccountSwitcher.exe");
    let script = format!(
        "$ErrorActionPreference='Stop'; Compress-Archive -Force -LiteralPath '{}','{}','{}' -DestinationPath '{}'; \
         (Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash.ToLower() + '  ' + (Split-Path -Leaf '{}') | \
         Set-Content -Encoding ascii -LiteralPath '{}.sha256'",
        exe.display(),
        root.join("LICENSE").display(),
        root.join("README.md").display(),
        zip.display(),
        zip.display(),
        zip.display(),
        zip.display()
    );
    run("package", "powershell", &["-NoProfile", "-Command", &script])?;
    println!("\n{}", zip.display());
    Ok(())
}

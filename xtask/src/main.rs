//! Project tasks.
//!
//! - `cargo xtask check`: the gate. Format, lint, licenses and advisories, unused dependencies,
//!   the environment rule, then every test with the coverage gate. Stops at the first failure.
//! - `cargo xtask dist [--tag vX.Y.Z] [--portable-only]`: a release build and every file a GitHub
//!   release carries, in `dist/`: the installer (Inno Setup), the portable zip, `latest.json` (the
//!   feed running copies update from), `SHA256SUMS.txt` and `release-notes.md` (the CHANGELOG
//!   section). With `--tag` it first checks that the tag is this version and, on GitHub Actions,
//!   that `repository` in Cargo.toml is the repository being built.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool reports to the console"
)]

mod release;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Minimum line coverage of the non-UI crates.
const COVERAGE_LINES: &str = "95";
/// Files not counted for coverage: the task runner itself and the UI crate (tested through the
/// Slint testing backend and screenshots instead).
const COVERAGE_IGNORE: &str = r"(xtask|crates[\\/]app[\\/])";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("check") => check(),
        Some("dist") => dist(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask <check | dist [--tag vX.Y.Z] [--portable-only]>");
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

/// Where Cargo writes builds: `CARGO_TARGET_DIR` when set, else `target` in the repository.
fn target_dir() -> PathBuf {
    let dir = match std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => root().join(dir),
        None => root().join("target"),
    };
    // `C:/x` from a POSIX shell becomes `C:\x`: PowerShell's Compress-Archive refuses mixed slashes.
    std::path::absolute(&dir).unwrap_or(dir)
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

/// Where Inno Setup's compiler may be.
fn inno_compiler() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for variable in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(variable) {
            let base = PathBuf::from(base);
            let base = if variable == "LOCALAPPDATA" { base.join("Programs") } else { base };
            for version in ["Inno Setup 7", "Inno Setup 6"] {
                candidates.push(base.join(version).join("ISCC.exe"));
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file()).or_else(|| {
        Command::new("where")
            .arg("iscc")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout).lines().next().map(PathBuf::from)
            })
    })
}

fn dist(args: &[String]) -> Result<(), String> {
    let options = release::Options::parse(args)?;
    let version = env!("CARGO_PKG_VERSION");
    let repository = env!("CARGO_PKG_REPOSITORY");
    let tag = release::tag_for(version, options.tag.as_deref())?;
    if let Some(building) = std::env::var_os("GITHUB_REPOSITORY") {
        release::check_repository(repository, &building.to_string_lossy())?;
    }
    let root = root();
    let changelog = fs::read_to_string(root.join("CHANGELOG.md"))
        .map_err(|error| format!("CHANGELOG.md: {error}"))?;
    let notes = release::notes(&changelog, version)
        .ok_or_else(|| format!("CHANGELOG.md has no section `## {version}`"))?;

    run("release build", "cargo", &["build", "--release", "--locked", "--package", "app"])?;
    let out = root.join("dist");
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|error| format!("emptying dist: {error}"))?;
    }
    fs::create_dir_all(&out).map_err(|error| format!("dist folder: {error}"))?;
    let names = release::Names::new(version);

    let built = target_dir().join("release");
    let exe = built.join(release::EXECUTABLE);
    if !exe.is_file() {
        return Err(format!("the release build is not at {}", exe.display()));
    }
    let zip = out.join(&names.portable);
    let script = format!(
        "$ErrorActionPreference='Stop'; Compress-Archive -Force -LiteralPath '{}','{}','{}' -DestinationPath '{}'",
        exe.display(),
        root.join("LICENSE").display(),
        root.join("README.md").display(),
        zip.display(),
    );
    run("portable zip", "powershell", &["-NoProfile", "-Command", &script])?;

    let mut files = vec![zip.clone()];
    if options.portable_only {
        println!("\n== installer: skipped (--portable-only)");
    } else {
        let compiler = inno_compiler().ok_or(
            "Inno Setup was not found: install it (winget install JRSoftware.InnoSetup) or pass --portable-only",
        )?;
        let defines = [
            format!("/DAppVersion={version}"),
            format!("/DNumericVersion={}", release::numeric(version)),
            format!("/DSourceDir={}", root.display()),
            format!("/DOutputDir={}", out.display()),
            format!("/DRepository={repository}"),
            format!("/DBuildDir={}", built.display()),
        ];
        let mut arguments: Vec<&str> = vec!["/Qp"];
        arguments.extend(defines.iter().map(String::as_str));
        let script = root.join("installer").join("AntigravityAccountSwitcher.iss");
        let script = script.display().to_string();
        arguments.push(&script);
        run("installer", &compiler.to_string_lossy(), &arguments)?;
        files.insert(0, out.join(&names.setup));
    }

    let zip_bytes = fs::read(&zip).map_err(|error| format!("{}: {error}", zip.display()))?;
    let published = jiff::Timestamp::now().strftime("%Y-%m-%dT%H:%M:%SZ").to_string();
    let manifest = release::manifest(version, &tag, repository, &published, &names, &zip_bytes);
    write(&out.join("latest.json"), &manifest)?;
    let mut sums = String::new();
    for file in &files {
        let bytes = fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = writeln!(sums, "{}  {name}", release::sha256(&bytes));
    }
    write(&out.join("SHA256SUMS.txt"), &sums)?;
    write(
        &out.join("release-notes.md"),
        &release::notes_page(&notes, &names, options.portable_only),
    )?;

    println!("\nRelease files for {tag} in {}:", out.display());
    for entry in fs::read_dir(&out).map_err(|error| error.to_string())?.flatten() {
        let size = entry.metadata().map(|m| m.len()).unwrap_or_default();
        println!("  {:>12}  {}", size, entry.file_name().to_string_lossy());
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

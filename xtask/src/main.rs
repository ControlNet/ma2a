//! Portable development and CI task runner for the MA2A workspace.

pub(crate) mod error;
pub(crate) mod fs;
pub(crate) mod loc;
pub(crate) mod pins;
pub(crate) mod process;
pub(crate) mod release;
pub(crate) mod support;

#[cfg(test)]
mod pins_tests;
#[cfg(test)]
mod release_tests;
#[cfg(test)]
mod support_tests;
#[cfg(test)]
mod workspace_tests;

use std::{env, path::Path};

use error::XtaskError;

fn main() -> Result<(), XtaskError> {
    let command = env::args().nth(1).ok_or_else(|| XtaskError::Usage {
        message: "usage: cargo xtask <check|test|web-build|dist|check-pins|check-loc|check-release|check-support> [path]".to_owned(),
    })?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| XtaskError::Usage {
            message: "xtask must be located directly below the workspace root".to_owned(),
        })?;
    match command.as_str() {
        "check" => check(root),
        "test" => test(root),
        "web-build" => web_build(root),
        "dist" => dist(root),
        "check-pins" => pins::check(root),
        "check-loc" => loc::check(root),
        "check-release" => release::check(root),
        "check-support" => {
            let path = env::args().nth(2).map_or_else(
                || root.join("docs/platform-support.json"),
                std::path::PathBuf::from,
            );
            support::check(&path)
        }
        _ => Err(XtaskError::Usage {
            message: format!("unknown xtask command: {command}"),
        }),
    }
}

fn check(root: &Path) -> Result<(), XtaskError> {
    process::run(root, "rustc", &["--version"])?;
    process::run(root, "cargo", &["--version"])?;
    process::run(root, "bun", &["--version"])?;
    process::run(
        root,
        "cargo",
        &["metadata", "--locked", "--no-deps", "--format-version", "1"],
    )?;
    pins::check(root)?;
    loc::check(root)?;
    support::check(&root.join("docs/platform-support.json"))?;
    release::check(root)?;
    process::run(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    process::run(
        root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    process::run(
        root,
        "cargo",
        &["nextest", "run", "--workspace", "--all-features"],
    )?;
    process::run(root, "cargo", &["deny", "check"])?;
    process::run(root, "cargo-machete", &[])?;
    process::run_web(root, &["install", "--frozen-lockfile"])?;
    process::run_web(root, &["x", "biome", "check", "."])?;
    process::run_web(root, &["x", "tsc", "--noEmit"])?;
    process::run_web(root, &["test"])
}

fn test(root: &Path) -> Result<(), XtaskError> {
    process::run(
        root,
        "cargo",
        &["nextest", "run", "--workspace", "--all-features"],
    )?;
    process::run_web(root, &["test"])
}

fn web_build(root: &Path) -> Result<(), XtaskError> {
    process::run_web(root, &["install", "--frozen-lockfile"])?;
    process::run_web(root, &["run", "build"])
}

fn dist(root: &Path) -> Result<(), XtaskError> {
    release::check(root)?;
    let target = host_target()?;
    release::check_target(root, target)?;
    process::run(
        root,
        "bun",
        &[
            "x",
            "@axodotdev/dist@0.32.0",
            "build",
            "--artifacts=local",
            "--target",
            target,
            "--allow-dirty",
        ],
    )
}

fn host_target() -> Result<&'static str, XtaskError> {
    if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
        Ok("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        Ok("x86_64-pc-windows-msvc")
    } else if cfg!(all(target_arch = "aarch64", target_os = "windows")) {
        Ok("aarch64-pc-windows-msvc")
    } else {
        Err(XtaskError::Usage {
            message: "cargo dist requires one of the six release target platforms".to_owned(),
        })
    }
}

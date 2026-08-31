use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::release;

type TestResult = Result<(), Box<dyn std::error::Error>>;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[test]
fn release_policy_accepts_the_exact_six_target_contract() -> TestResult {
    // Given
    let root = fixture_root()?;
    write_support_matrix(&root, false)?;
    write_dist_config(&root, false)?;

    // When
    let result = release::check(&root);

    // Then
    assert!(result.is_ok());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn release_policy_rejects_a_silently_removed_target() -> TestResult {
    // Given
    let root = fixture_root()?;
    write_support_matrix(&root, true)?;
    write_dist_config(&root, false)?;

    // When
    let error = release::check(&root)
        .err()
        .ok_or("release policy unexpectedly passed")?;

    // Then
    assert!(error.to_string().contains("aarch64-pc-windows-msvc"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn release_policy_rejects_installers() -> TestResult {
    // Given
    let root = fixture_root()?;
    write_support_matrix(&root, false)?;
    write_dist_config(&root, false)?;
    let path = root.join("dist-workspace.toml");
    let config = fs::read_to_string(&path)?.replace("installers = []", "installers = [\"shell\"]");
    fs::write(path, config)?;

    // When
    let error = release::check(&root)
        .err()
        .ok_or("release policy unexpectedly passed")?;

    // Then
    assert!(error.to_string().contains("installers"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn release_target_accepts_a_supported_host() -> TestResult {
    // Given
    let root = fixture_root()?;
    write_support_matrix(&root, false)?;

    // When
    let result = release::check_target(&root, "x86_64-unknown-linux-gnu");

    // Then
    assert!(result.is_ok());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn release_target_rejects_a_deferred_host() -> TestResult {
    // Given
    let root = fixture_root()?;
    write_support_matrix(&root, false)?;

    // When
    let error = release::check_target(&root, "aarch64-unknown-linux-gnu")
        .err()
        .ok_or("deferred release target unexpectedly passed")?;

    // Then
    assert!(error.to_string().contains("aarch64-unknown-linux-gnu"));
    assert!(error.to_string().contains("not supported"));
    fs::remove_dir_all(root)?;
    Ok(())
}

fn fixture_root() -> Result<std::path::PathBuf, std::io::Error> {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "ma2a-release-policy-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("docs"))?;
    Ok(root)
}

fn write_support_matrix(
    root: &std::path::Path,
    support_windows_arm64: bool,
) -> Result<(), std::io::Error> {
    let targets = targets()
        .into_iter()
        .map(|target| {
            let status = if target.starts_with("x86_64")
                || (support_windows_arm64 && target == "aarch64-pc-windows-msvc")
            {
                "supported"
            } else {
                "deferred"
            };
            format!(r#"{{"target":"{target}","status":"{status}"}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        root.join("docs/platform-support.json"),
        format!(r#"{{"schema_version":2,"targets":[{targets}]}}"#),
    )
}

fn write_dist_config(
    root: &std::path::Path,
    include_windows_arm64: bool,
) -> Result<(), std::io::Error> {
    let targets = targets()
        .into_iter()
        .filter(|target| target.starts_with("x86_64") || include_windows_arm64)
        .map(|target| format!("    \"{target}\""))
        .collect::<Vec<_>>()
        .join(",\n");
    fs::write(
        root.join("dist-workspace.toml"),
        format!(
            r#"[dist]
cargo-dist-version = "0.32.0"
packages = ["ma2a-app"]
targets = [
{targets}
]
installers = []
ci = []
hosting = []
checksum = "sha256"
unix-archive = ".tar.xz"
windows-archive = ".zip"
source-tarball = false
auto-includes = false
include = ["README.md", "docs/quickstart.md", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE"]
"#,
        ),
    )
}

fn targets() -> [&'static str; 6] {
    [
        "aarch64-apple-darwin",
        "aarch64-pc-windows-msvc",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
    ]
}

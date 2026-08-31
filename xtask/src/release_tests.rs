use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::json;

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

macro_rules! missing_target_test {
    ($name:ident, $target:literal) => {
        #[test]
        fn $name() -> TestResult {
            assert_missing_target_rejected($target)
        }
    };
}

missing_target_test!(
    release_policy_rejects_missing_arm64_macos_target,
    "aarch64-apple-darwin"
);
missing_target_test!(
    release_policy_rejects_missing_arm64_windows_target,
    "aarch64-pc-windows-msvc"
);
missing_target_test!(
    release_policy_rejects_missing_arm64_linux_target,
    "aarch64-unknown-linux-gnu"
);
missing_target_test!(
    release_policy_rejects_missing_x86_64_macos_target,
    "x86_64-apple-darwin"
);
missing_target_test!(
    release_policy_rejects_missing_x86_64_windows_target,
    "x86_64-pc-windows-msvc"
);
missing_target_test!(
    release_policy_rejects_missing_x86_64_linux_target,
    "x86_64-unknown-linux-gnu"
);

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

fn assert_missing_target_rejected(missing_target: &str) -> TestResult {
    // Given
    let root = fixture_root()?;
    write_support_matrix(&root, false)?;
    write_dist_config(&root, false)?;
    let path = root.join("docs/platform-support.json");
    let mut matrix = serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path)?)?;
    let entries = matrix
        .get_mut("targets")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("support matrix targets are not an array")?;
    entries.retain(|entry| {
        entry.get("target").and_then(serde_json::Value::as_str) != Some(missing_target)
    });
    fs::write(path, serde_json::to_vec(&matrix)?)?;

    // When
    let error = release::check(&root)
        .err()
        .ok_or("release policy unexpectedly passed")?;

    // Then
    assert!(error.to_string().contains(missing_target));
    fs::remove_dir_all(root)?;
    Ok(())
}

fn write_support_matrix(root: &std::path::Path, support_windows_arm64: bool) -> TestResult {
    let targets = targets()
        .into_iter()
        .map(|target| {
            let supported = target.starts_with("x86_64")
                || (support_windows_arm64 && target == "aarch64-pc-windows-msvc");
            let probe = if supported {
                json!({
                    "command": "cargo check --target example",
                    "exit_code": 0,
                    "outcome": "passed",
                    "output": "Finished successfully on the target runner"
                })
            } else {
                json!({
                    "command": "cargo check --target example",
                    "exit_code": 101,
                    "outcome": "host-limited",
                    "output": "Observed target toolchain is unavailable on this host"
                })
            };
            json!({
                "target": target,
                "status": if supported { "supported" } else { "deferred" },
                "evidence_host": "x86_64-unknown-linux-gnu",
                "compile": &probe,
                "smoke": &probe,
                "package": probe,
                "blocker": if supported { "none" } else { "Observed host toolchain limitation" },
                "re_evaluate": "Run on the matching native target runner"
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        root.join("docs/platform-support.json"),
        serde_json::to_vec(&json!({"schema_version": 2, "targets": targets}))?,
    )?;
    Ok(())
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

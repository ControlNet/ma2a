//! Whether a release workflow run publishes: only a tag named `v` and the version of `ma2a-app`.

use std::{env, path::Path, process::Command};

use crate::error::XtaskError;

/// Prints `true` when the run's `GITHUB_REF` is a tag that publishes and `false` for a branch;
/// fails for any other tag, so a release is never built under a name the build did not check.
pub(crate) fn check(root: &Path) -> Result<(), XtaskError> {
    let git_ref = env::var("GITHUB_REF").unwrap_or_default();
    println!("{}", publishes(&git_ref, &application_version(root)?)?);
    Ok(())
}

/// The comparison is case-sensitive, unlike `startsWith` in a workflow expression.
pub(crate) fn publishes(git_ref: &str, version: &str) -> Result<bool, XtaskError> {
    let Some(tag) = git_ref.strip_prefix("refs/tags/") else {
        return Ok(false);
    };
    if tag.strip_prefix('v') == Some(version) {
        Ok(true)
    } else {
        Err(XtaskError::Policy {
            name: "release tag policy",
            violations: vec![format!(
                "release tag `{tag}` does not name ma2a-app version `v{version}`"
            )],
        })
    }
}

fn application_version(root: &Path) -> Result<String, XtaskError> {
    let command = "cargo metadata --locked --no-deps --format-version 1";
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(XtaskError::Command {
            command: command.to_owned(),
            status: output.status.code(),
        });
    }
    let manifest = || XtaskError::Manifest {
        path: root.join("Cargo.toml"),
        message: "no ma2a-app package version in cargo metadata".to_owned(),
    };
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| manifest())?;
    document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|package| package.get("name").and_then(serde_json::Value::as_str) == Some("ma2a-app"))
        .and_then(|package| package.get("version").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .ok_or_else(manifest)
}

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use toml::Value;

use crate::{error::XtaskError, fs::collect_files, semver::is_version};

const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

pub(crate) fn check(root: &Path) -> Result<(), XtaskError> {
    let root = root.canonicalize()?;
    let workspace_members = workspace_members(&root)?;
    let policy = PinPolicy {
        root: &root,
        workspace_members: &workspace_members,
    };
    let lockfile = root.join("Cargo.lock");
    let mut violations = Vec::new();
    if !lockfile.is_file() {
        violations.push("Cargo.lock is missing".to_owned());
    }
    for manifest in collect_files(&root, Some("Cargo.toml"))? {
        inspect_manifest(&policy, &manifest, &mut violations)?;
    }
    inspect_web_manifest(&root, &mut violations)?;
    if violations.is_empty() {
        Ok(())
    } else {
        Err(XtaskError::Policy {
            name: "pin policy",
            violations,
        })
    }
}

struct PinPolicy<'a> {
    root: &'a Path,
    workspace_members: &'a BTreeSet<PathBuf>,
}

fn workspace_members(root: &Path) -> Result<BTreeSet<PathBuf>, XtaskError> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(XtaskError::Command {
            command: "cargo metadata --no-deps --format-version 1".to_owned(),
            status: output.status.code(),
        });
    }
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|error| XtaskError::Manifest {
            path: root.join("Cargo.toml"),
            message: error.to_string(),
        })?;
    let member_ids = document
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|package| {
            package
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| member_ids.contains(id))
        })
        .filter_map(|package| {
            package
                .get("manifest_path")
                .and_then(serde_json::Value::as_str)
        })
        .map(|manifest| {
            Path::new(manifest)
                .parent()
                .ok_or_else(|| XtaskError::Manifest {
                    path: PathBuf::from(manifest),
                    message: "workspace member manifest has no parent directory".to_owned(),
                })?
                .canonicalize()
                .map_err(XtaskError::from)
        })
        .collect()
}

fn inspect_web_manifest(root: &Path, violations: &mut Vec<String>) -> Result<(), XtaskError> {
    let path = root.join("web/package.json");
    if !path.is_file() {
        return Ok(());
    }
    if !root.join("web/bun.lock").is_file() {
        violations.push("web/bun.lock is missing".to_owned());
    }
    let text = fs::read_to_string(&path)?;
    let document: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| XtaskError::Manifest {
            path: path.clone(),
            message: error.to_string(),
        })?;
    for table_name in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let Some(section) = document.get(table_name) else {
            continue;
        };
        let Some(dependencies) = section.as_object() else {
            violations.push(format!("web/package.json: `{table_name}` is not an object"));
            continue;
        };
        for (name, version) in dependencies {
            if version
                .as_str()
                .is_none_or(|value| !is_npm_exact_version(value))
            {
                violations.push(format!(
                    "web/package.json: dependency `{name}` in `{table_name}` is not an exact pin"
                ));
            }
        }
    }
    Ok(())
}

fn inspect_manifest(
    policy: &PinPolicy<'_>,
    manifest: &Path,
    violations: &mut Vec<String>,
) -> Result<(), XtaskError> {
    let text = fs::read_to_string(manifest)?;
    let document = toml::from_str::<Value>(&text).map_err(|error| XtaskError::Manifest {
        path: manifest.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut inspection = ManifestInspection {
        policy,
        manifest,
        violations,
    };
    inspect_tables(&document, &mut inspection);
    if let Some(workspace) = document.get("workspace") {
        inspect_tables(workspace, &mut inspection);
    }
    if let Some(targets) = document.get("target").and_then(Value::as_table) {
        for target in targets.values() {
            inspect_tables(target, &mut inspection);
        }
    }
    // A patch replaces a dependency's source for the whole build, so it is held to the same rule.
    if let Some(patches) = document.get("patch").and_then(Value::as_table) {
        for (source, dependencies) in patches {
            if let Some(dependencies) = dependencies.as_table() {
                inspect_dependencies(dependencies, &format!("patch.{source}"), &mut inspection);
            }
        }
    }
    // Cargo deprecates [replace] in favour of [patch], so any entry is refused, wherever it points.
    if let Some(replacements) = document.get("replace").and_then(Value::as_table) {
        let relative = manifest.strip_prefix(policy.root).unwrap_or(manifest);
        for name in replacements.keys() {
            violations.push(format!(
                "{}: dependency `{name}` in [replace] is refused; use [patch]",
                relative.display()
            ));
        }
    }
    Ok(())
}

struct ManifestInspection<'a> {
    policy: &'a PinPolicy<'a>,
    manifest: &'a Path,
    violations: &'a mut Vec<String>,
}

fn inspect_tables(value: &Value, inspection: &mut ManifestInspection<'_>) {
    for table_name in DEPENDENCY_TABLES {
        if let Some(dependencies) = value.get(table_name).and_then(Value::as_table) {
            inspect_dependencies(dependencies, table_name, inspection);
        }
    }
}

fn inspect_dependencies(
    dependencies: &toml::Table,
    table_name: &str,
    inspection: &mut ManifestInspection<'_>,
) {
    for (name, specification) in dependencies {
        if !is_pinned(inspection.policy, inspection.manifest, specification) {
            let relative = inspection
                .manifest
                .strip_prefix(inspection.policy.root)
                .unwrap_or(inspection.manifest);
            inspection.violations.push(format!(
                "{}: dependency `{name}` in [{table_name}] violates exact-version/workspace-path policy",
                relative.display()
            ));
        }
    }
}

fn is_pinned(policy: &PinPolicy<'_>, manifest: &Path, specification: &Value) -> bool {
    if let Some(version) = specification.as_str() {
        return is_exact_version(version);
    }
    let Some(table) = specification.as_table() else {
        return false;
    };
    // A git source is neither a registry version nor a workspace path, whatever version it names.
    if table.contains_key("git") {
        return false;
    }
    if table.get("workspace").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    if let Some(path) = table.get("path").and_then(Value::as_str) {
        let parent = manifest.parent().unwrap_or(policy.root);
        let version_is_exact = table
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(is_exact_version);
        return version_is_exact
            && parent
                .join(path)
                .canonicalize()
                .is_ok_and(|canonical| policy.workspace_members.contains(&canonical));
    }
    table
        .get("version")
        .and_then(Value::as_str)
        .is_some_and(is_exact_version)
}

/// A Cargo requirement that admits one version: `=` and one complete version.
fn is_exact_version(version: &str) -> bool {
    version.strip_prefix('=').is_some_and(is_version)
}

/// An npm specification that admits one version: one complete version and nothing else.
fn is_npm_exact_version(version: &str) -> bool {
    is_version(version)
}

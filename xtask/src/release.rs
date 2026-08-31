use std::{collections::BTreeSet, fs, path::Path};

use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::error::XtaskError;

const REQUIRED_INCLUDES: [&str; 5] = [
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "NOTICE",
    "README.md",
    "docs/quickstart.md",
];

pub(crate) fn check(root: &Path) -> Result<(), XtaskError> {
    let matrix_path = root.join("docs/platform-support.json");
    let config_path = root.join("dist-workspace.toml");
    let matrix = parse_json(&matrix_path)?;
    let config = parse_toml(&config_path)?;
    let mut violations = Vec::new();

    let supported = supported_targets(&matrix, &matrix_path)?;
    let Some(dist) = config.get("dist").and_then(TomlValue::as_table) else {
        return Err(XtaskError::Manifest {
            path: config_path,
            message: "`dist` must be a table".to_owned(),
        });
    };
    let mut checks = PolicyChecks {
        dist,
        violations: &mut violations,
    };
    checks.string("cargo-dist-version", "0.32.0");
    checks.string("checksum", "sha256");
    checks.string("unix-archive", ".tar.xz");
    checks.string("windows-archive", ".zip");
    checks.boolean("source-tarball", false);
    checks.boolean("auto-includes", false);
    checks.exact_array("packages", ["ma2a-app"]);
    checks.exact_array("installers", []);
    checks.exact_array("ci", []);
    checks.exact_array("hosting", []);

    let configured = string_set(dist.get("targets"));
    if configured != supported {
        violations.push(format!(
            "dist targets must equal matrix-supported targets {supported:?}, observed {configured:?}"
        ));
    }
    let includes = string_set(dist.get("include"));
    let required_includes = REQUIRED_INCLUDES.into_iter().collect::<BTreeSet<_>>();
    if includes != required_includes {
        violations.push(format!(
            "archive includes must be exactly {required_includes:?}, observed {includes:?}"
        ));
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(XtaskError::Policy {
            name: "release policy",
            violations,
        })
    }
}

pub(crate) fn check_target(root: &Path, target: &str) -> Result<(), XtaskError> {
    let matrix_path = root.join("docs/platform-support.json");
    let matrix = parse_json(&matrix_path)?;
    let supported = supported_targets(&matrix, &matrix_path)?;
    if supported.contains(target) {
        Ok(())
    } else {
        Err(XtaskError::Policy {
            name: "release target policy",
            violations: vec![format!("release target `{target}` is not supported")],
        })
    }
}

fn parse_json(path: &Path) -> Result<JsonValue, XtaskError> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|error| XtaskError::Manifest {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn parse_toml(path: &Path) -> Result<TomlValue, XtaskError> {
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|error| XtaskError::Manifest {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn supported_targets<'a>(
    matrix: &'a JsonValue,
    path: &Path,
) -> Result<BTreeSet<&'a str>, XtaskError> {
    let targets = matrix
        .get("targets")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| XtaskError::Manifest {
            path: path.to_path_buf(),
            message: "`targets` must be an array".to_owned(),
        })?;
    targets
        .iter()
        .filter(|entry| entry.get("status").and_then(JsonValue::as_str) == Some("supported"))
        .map(|entry| {
            entry
                .get("target")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| XtaskError::Manifest {
                    path: path.to_path_buf(),
                    message: "each supported target requires string `target`".to_owned(),
                })
        })
        .collect()
}

struct PolicyChecks<'a> {
    dist: &'a toml::Table,
    violations: &'a mut Vec<String>,
}

impl PolicyChecks<'_> {
    fn string(&mut self, field: &str, expected: &str) {
        let observed = self.dist.get(field).and_then(TomlValue::as_str);
        if observed != Some(expected) {
            self.violations.push(format!(
                "`{field}` must be `{expected}`, observed {observed:?}"
            ));
        }
    }

    fn boolean(&mut self, field: &str, expected: bool) {
        let observed = self.dist.get(field).and_then(TomlValue::as_bool);
        if observed != Some(expected) {
            self.violations.push(format!(
                "`{field}` must be `{expected}`, observed {observed:?}"
            ));
        }
    }

    fn exact_array<const N: usize>(&mut self, field: &str, expected: [&str; N]) {
        let expected = expected.into_iter().collect::<BTreeSet<_>>();
        let observed = string_set(self.dist.get(field));
        if observed != expected {
            self.violations.push(format!(
                "`{field}` must be exactly {expected:?}, observed {observed:?}"
            ));
        }
    }
}

fn string_set(value: Option<&TomlValue>) -> BTreeSet<&str> {
    value
        .and_then(TomlValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(TomlValue::as_str)
        .collect()
}

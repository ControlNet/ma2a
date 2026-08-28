use std::{collections::BTreeSet, fs, path::Path};

use serde_json::Value;

use crate::error::XtaskError;

const REQUIRED_TARGETS: [&str; 6] = [
    "aarch64-apple-darwin",
    "aarch64-pc-windows-msvc",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

pub(crate) fn check(path: &Path) -> Result<(), XtaskError> {
    let text = fs::read_to_string(path)?;
    let document: Value = serde_json::from_str(&text).map_err(|error| XtaskError::Manifest {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if document.get("schema_version").and_then(Value::as_u64) != Some(2) {
        return Err(XtaskError::Manifest {
            path: path.to_path_buf(),
            message: "`schema_version` must be 2".to_owned(),
        });
    }
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| XtaskError::Manifest {
            path: path.to_path_buf(),
            message: "`targets` must be an array".to_owned(),
        })?;
    let mut observed = BTreeSet::new();
    let mut violations = Vec::new();
    for target in targets {
        inspect_target(target, &mut observed, &mut violations);
    }
    let required = REQUIRED_TARGETS.into_iter().collect::<BTreeSet<_>>();
    if observed != required {
        violations.push(format!(
            "target set must be exactly {required:?}, observed {observed:?}"
        ));
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(XtaskError::Policy {
            name: "architecture support matrix",
            violations,
        })
    }
}

fn inspect_target<'a>(
    target: &'a Value,
    observed: &mut BTreeSet<&'a str>,
    violations: &mut Vec<String>,
) {
    let Some(object) = target.as_object() else {
        violations.push("each target entry must be an object".to_owned());
        return;
    };
    let Some(triple) = object.get("target").and_then(Value::as_str) else {
        violations.push("target entry is missing string `target`".to_owned());
        return;
    };
    if !observed.insert(triple) {
        violations.push(format!("duplicate target entry `{triple}`"));
    }
    let status = object.get("status").and_then(Value::as_str);
    if !matches!(status, Some("supported" | "deferred")) {
        violations.push(format!(
            "{triple}: status must be `supported` or `deferred`"
        ));
    }
    if object
        .get("evidence_host")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        violations.push(format!(
            "{triple}: `evidence_host` must be a non-empty string"
        ));
    }
    for field in ["compile", "smoke", "package"] {
        let Some(probe) = object.get(field).and_then(Value::as_object) else {
            violations.push(format!("{triple}: `{field}` must be an object"));
            continue;
        };
        for required in ["command", "outcome", "output"] {
            if probe
                .get(required)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                violations.push(format!(
                    "{triple}: `{field}.{required}` must be a non-empty string"
                ));
            }
        }
        let output = probe
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_placeholder(output) {
            violations.push(format!(
                "{triple}: `{field}.output` must contain observed evidence, not a future intention"
            ));
        }
        let inspection = ProbeInspection {
            triple,
            field,
            status,
        };
        inspect_probe_status(&inspection, probe, violations);
    }
    if object
        .get("blocker")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        violations.push(format!("{triple}: `blocker` must be a non-empty string"));
    }
    if object
        .get("re_evaluate")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        violations.push(format!(
            "{triple}: `re_evaluate` must be a non-empty string"
        ));
    }
}

struct ProbeInspection<'a> {
    triple: &'a str,
    field: &'a str,
    status: Option<&'a str>,
}

fn inspect_probe_status(
    inspection: &ProbeInspection<'_>,
    probe: &serde_json::Map<String, Value>,
    violations: &mut Vec<String>,
) {
    let exit_code = probe.get("exit_code").and_then(Value::as_i64);
    let outcome = probe.get("outcome").and_then(Value::as_str);
    match inspection.status {
        Some("deferred") => {
            if exit_code.is_none() {
                violations.push(format!(
                    "{}: deferred `{}` requires an observed integer exit code",
                    inspection.triple, inspection.field
                ));
            }
            if !matches!(outcome, Some("failed" | "host-limited")) {
                violations.push(format!(
                    "{}: deferred `{}.outcome` must be `failed` or `host-limited`",
                    inspection.triple, inspection.field
                ));
            }
        }
        Some("supported") => match outcome {
            Some("passed") if exit_code == Some(0) => {}
            Some("required-ci") if probe.get("exit_code") == Some(&Value::Null) => {}
            _ => violations.push(format!(
                "{}: supported `{}` must be a passed exit 0 or an explicit required-ci probe",
                inspection.triple, inspection.field
            )),
        },
        Some(_) | None => {}
    }
}

fn is_placeholder(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    [
        "ci probe configured",
        "future run",
        "todo 22",
        "deferred until",
        "will be captured",
        "planned probe",
    ]
    .iter()
    .any(|placeholder| output.contains(placeholder))
}

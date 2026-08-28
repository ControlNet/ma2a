use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};

use crate::support;

static FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(mut transform: impl FnMut(&mut Value)) -> Result<Self, Box<dyn Error>> {
        let id = FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-support-policy-{}-{id}.json",
            std::process::id()
        ));
        let mut document = valid_document();
        transform(&mut document);
        fs::write(&path, serde_json::to_vec_pretty(&document)?)?;
        Ok(Self { path })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn valid_document() -> Value {
    let targets = [
        "aarch64-apple-darwin",
        "aarch64-pc-windows-msvc",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
    ]
    .map(|target| {
        json!({
            "target": target,
            "status": "deferred",
            "evidence_host": "x86_64-unknown-linux-gnu",
            "compile": probe(),
            "smoke": probe(),
            "package": probe(),
            "blocker": "Observed host toolchain limitation",
            "re_evaluate": "Run on a native target runner"
        })
    });
    json!({"schema_version": 2, "generated_at": "2026-08-29", "targets": targets})
}

fn probe() -> Value {
    json!({
        "command": "cargo check --target example",
        "exit_code": 101,
        "outcome": "host-limited",
        "output": "error: observed toolchain is unavailable on this host"
    })
}

fn first_compile(document: &mut Value) -> Option<&mut serde_json::Map<String, Value>> {
    document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| targets.first_mut())
        .and_then(|target| target.get_mut("compile"))
        .and_then(Value::as_object_mut)
}

#[test]
fn accepts_structured_observed_evidence() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::new(|_| {})?;

    // When
    let result = support::check(&fixture.path);

    // Then
    assert!(
        result.is_ok(),
        "structured evidence should pass: {result:?}"
    );
    Ok(())
}

#[test]
fn rejects_future_intention_as_probe_output() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::new(|document| {
        if let Some(probe) = first_compile(document) {
            probe.insert(
                "output".to_owned(),
                Value::String("CI probe configured for a future run".to_owned()),
            );
        }
    })?;

    // When
    let result = support::check(&fixture.path);

    // Then
    assert!(
        result.is_err(),
        "future intention must not count as evidence"
    );
    Ok(())
}

#[test]
fn rejects_deferred_probe_without_exit_code() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::new(|document| {
        if let Some(probe) = first_compile(document) {
            probe.insert("exit_code".to_owned(), Value::Null);
        }
    })?;

    // When
    let result = support::check(&fixture.path);

    // Then
    assert!(
        result.is_err(),
        "deferred evidence requires an observed exit code"
    );
    Ok(())
}

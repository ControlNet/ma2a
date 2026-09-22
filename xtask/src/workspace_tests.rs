use std::{collections::BTreeSet, error::Error, fs, path::Path, process::Command};

const EXPECTED_MEMBERS: [&str; 6] = [
    "ma2a-app",
    "ma2a-core",
    "ma2a-net",
    "ma2a-runtime",
    "ma2a-store",
    "xtask",
];
const AUDITED_UNSAFE_BOUNDARIES: [(&str, &str); 6] = [
    (
        "crates/ma2a-app/src/commands/workflows/space_windows.rs",
        concat!(
            "allow(unsafe",
            "_code, reason = \"audited Windows file-owner FFI boundary\")"
        ),
    ),
    (
        "crates/ma2a-app/src/commands/workflows/space_windows_sid.rs",
        concat!(
            "allow(unsafe",
            "_code, reason = \"audited Windows SID FFI boundary\")"
        ),
    ),
    (
        "crates/ma2a-runtime/src/ipc/lifecycle/process_windows.rs",
        concat!(
            "allow(unsafe",
            "_code, reason = \"audited Windows process-times FFI boundary\")"
        ),
    ),
    (
        "crates/ma2a-net/src/relay_tls_windows.rs",
        concat!(
            "allow(unsafe",
            "_code, reason = \"audited Windows file-security FFI boundary\")"
        ),
    ),
    (
        "crates/ma2a-net/src/relay_tls_windows_sid.rs",
        concat!(
            "allow(unsafe",
            "_code, reason = \"audited Windows SID FFI boundary\")"
        ),
    ),
    (
        "crates/ma2a-runtime/src/ipc/windows_security.rs",
        concat!(
            "allow(unsafe",
            "_code, reason = \"audited Windows token FFI boundary\")"
        ),
    ),
];

fn root() -> Result<&'static Path, Box<dyn Error>> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "xtask must be directly below the workspace root".into())
}

#[test]
fn repository_workspace_has_exact_accepted_members() -> Result<(), Box<dyn Error>> {
    // Given
    let root = root()?;

    // When
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()?;
    assert!(output.status.success(), "cargo metadata failed");
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let member_ids = document
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or("workspace_members missing")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    let actual = document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("packages missing")?
        .iter()
        .filter(|package| {
            package
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| member_ids.contains(id))
        })
        .filter_map(|package| package.get("name").and_then(serde_json::Value::as_str))
        .collect::<BTreeSet<_>>();

    // Then
    assert_eq!(actual, EXPECTED_MEMBERS.into_iter().collect());
    Ok(())
}

#[test]
fn unsafe_code_is_confined_to_audited_windows_boundary() -> Result<(), Box<dyn Error>> {
    // Given
    let root = root()?;
    let mut unsafe_files = BTreeSet::new();
    let mut allow_files = BTreeSet::new();

    // When
    for file in crate::fs::collect_files(root, None)?
        .into_iter()
        .filter(|file| file.extension().is_some_and(|extension| extension == "rs"))
    {
        let source = fs::read_to_string(&file)?;
        let relative = file
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if [
            concat!("unsafe", " {"),
            concat!("unsafe", " fn "),
            concat!("unsafe", " impl "),
        ]
        .into_iter()
        .any(|marker| source.contains(marker))
        {
            unsafe_files.insert(relative.clone());
        }
        if source.contains(concat!("allow(unsafe", "_code")) {
            allow_files.insert(relative);
        }
    }

    // Then
    let expected = AUDITED_UNSAFE_BOUNDARIES
        .iter()
        .map(|(path, _)| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(unsafe_files, expected);
    assert_eq!(allow_files, expected);
    for (path, audited_allow) in AUDITED_UNSAFE_BOUNDARIES {
        assert!(fs::read_to_string(root.join(path))?.contains(audited_allow));
    }
    Ok(())
}

#[test]
fn process_heavy_app_tests_have_a_bounded_nextest_group() -> Result<(), Box<dyn Error>> {
    // Given
    let root = root()?;
    let configuration = fs::read_to_string(root.join(".config/nextest.toml"))?;
    let document: toml::Value = toml::from_str(&configuration)?;

    // When
    let group = document
        .get("test-groups")
        .and_then(|groups| groups.get("ma2a-processes"))
        .ok_or("ma2a-processes test group missing")?;
    let max_threads = group.get("max-threads").and_then(toml::Value::as_integer);
    let default_overrides = document
        .get("profile")
        .and_then(|profile| profile.get("default"))
        .and_then(|default| default.get("overrides"))
        .and_then(toml::Value::as_array)
        .ok_or("default ma2a-processes override missing")?;
    let default_threads = document
        .get("profile")
        .and_then(|profile| profile.get("default"))
        .and_then(|default| default.get("test-threads"))
        .and_then(toml::Value::as_integer);
    let ci_threads = document
        .get("profile")
        .and_then(|profile| profile.get("ci"))
        .and_then(|ci| ci.get("test-threads"))
        .and_then(toml::Value::as_integer);

    // Then
    let max_threads = max_threads.ok_or("ma2a-processes max-threads missing")?;
    let default_threads = default_threads.ok_or("default test-threads missing")?;
    let ci_threads = ci_threads.ok_or("ci test-threads missing")?;
    assert_eq!(default_threads, 4);
    assert_eq!(ci_threads, 4);
    // A group equal to the global thread count bounds nothing, because every
    // slot may still hold a process-heavy test. That is how these tests came to
    // run four at a time, each starting whole Runtimes, until budgets that are
    // generous for one of them failed a different test on every run.
    assert!(max_threads >= 1, "the group must admit at least one test");
    assert!(
        max_threads < default_threads,
        "ma2a-processes must leave slots for the rest of the suite"
    );
    assert!(
        max_threads < ci_threads,
        "ma2a-processes must leave slots for the rest of the suite in CI"
    );
    assert!(default_overrides.iter().any(|override_value| {
        override_value.get("filter").and_then(toml::Value::as_str) == Some("package(ma2a-app)")
            && override_value
                .get("test-group")
                .and_then(toml::Value::as_str)
                == Some("ma2a-processes")
    }));
    Ok(())
}

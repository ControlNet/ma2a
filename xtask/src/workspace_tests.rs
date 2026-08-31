use std::{collections::BTreeSet, error::Error, fs, path::Path, process::Command};

const EXPECTED_MEMBERS: [&str; 6] = [
    "ma2a-app",
    "ma2a-core",
    "ma2a-net",
    "ma2a-runtime",
    "ma2a-store",
    "xtask",
];
const AUDITED_UNSAFE_BOUNDARIES: [(&str, &str); 4] = [
    (
        "crates/ma2a-app/src/commands/workflows/space_windows.rs",
        concat!(
            "allow(unsafe",
            "_code, reason = \"audited Windows file-owner FFI boundary\")"
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

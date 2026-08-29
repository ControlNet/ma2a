//! Architectural boundary regression for CLI credential administration.

use std::{error::Error, fs, path::Path};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn cli_credentials_use_only_the_local_control_boundary() -> TestResult {
    // Given
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(manifest_dir.join("Cargo.toml"))?;
    let source = rust_sources(&manifest_dir.join("src"))?;

    // When
    let forbidden_manifest_dependency = manifest.contains("ma2a-store");
    let forbidden_source = [
        "Repository::open",
        "StoreConfig",
        "MA2A_STATE_DIR",
        "ui_credentials",
        "UPDATE sessions",
    ]
    .into_iter()
    .find(|needle| source.contains(needle));

    // Then
    assert!(!forbidden_manifest_dependency);
    assert_eq!(forbidden_source, None);
    Ok(())
}

fn rust_sources(directory: &Path) -> Result<String, std::io::Error> {
    let mut source = String::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            source.push_str(&rust_sources(&path)?);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            source.push_str(&fs::read_to_string(path)?);
        }
    }
    Ok(source)
}

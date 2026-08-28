use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::pins;

static FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(dependency: &str) -> Result<Self, Box<dyn Error>> {
        let id = FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let parent =
            std::env::temp_dir().join(format!("ma2a-pin-policy-{}-{id}", std::process::id()));
        let root = parent.join("workspace");
        let member = root.join("member");
        let external = parent.join("external");
        fs::create_dir_all(&member)?;
        fs::create_dir_all(external.join("src"))?;
        fs::write(root.join("Cargo.lock"), "version = 4\n")?;
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n\n[workspace.dependencies]\nprobe = {dependency}\n"
            ),
        )?;
        write_package(&member, "member")?;
        write_package(&external, "external")?;
        Ok(Self { root })
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(parent) = self.root.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

fn write_package(path: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(path.join("src"))?;
    fs::write(
        path.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    )?;
    fs::write(path.join("src/lib.rs"), "pub fn marker() {}\n")
}

#[test]
fn accepts_exact_version_for_workspace_member_path() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::new("{ path = \"member\", version = \"=0.1.0\" }")?;

    // When
    let result = pins::check(fixture.root());

    // Then
    assert!(
        result.is_ok(),
        "valid workspace path should pass: {result:?}"
    );
    Ok(())
}

#[test]
fn rejects_canonical_path_outside_workspace_members() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::new("{ path = \"../external\", version = \"=0.1.0\" }")?;

    // When
    let result = pins::check(fixture.root());

    // Then
    assert!(result.is_err(), "external canonical path must fail");
    Ok(())
}

#[test]
fn rejects_workspace_path_without_exact_version() -> Result<(), Box<dyn Error>> {
    for dependency in [
        "{ path = \"member\" }",
        "{ path = \"member\", version = \"0.1.0\" }",
    ] {
        // Given
        let fixture = Fixture::new(dependency)?;

        // When
        let result = pins::check(fixture.root());

        // Then
        assert!(
            result.is_err(),
            "malformed path version must fail: {dependency}"
        );
    }
    Ok(())
}

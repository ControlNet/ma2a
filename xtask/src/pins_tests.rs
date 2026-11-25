use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{error::XtaskError, pins};

static FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(dependency: &str) -> Result<Self, Box<dyn Error>> {
        Self::with_manifest_tail(dependency, "")
    }

    /// A workspace whose root manifest ends with `tail`.
    fn with_manifest_tail(dependency: &str, tail: &str) -> Result<Self, Box<dyn Error>> {
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
                "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n\n[workspace.dependencies]\nprobe = {dependency}\n{tail}"
            ),
        )?;
        write_package(&member, "member")?;
        write_package(&external, "external")?;
        Ok(Self { root })
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn write_web_manifest(&self, manifest: &str) -> std::io::Result<()> {
        let web = self.root.join("web");
        fs::create_dir_all(&web)?;
        fs::write(web.join("bun.lock"), "{}\n")?;
        fs::write(web.join("package.json"), manifest)
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

/// The pin policy's own violations, so a check that failed for another reason is not mistaken
/// for one that refused the probe.
fn violations(fixture: &Fixture) -> Vec<String> {
    match pins::check(fixture.root()) {
        Err(XtaskError::Policy { violations, .. }) => violations,
        other => panic!("expected pin policy violations: {other:?}"),
    }
}

#[test]
fn rejects_git_dependency_with_exact_version() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::new(
        "{ git = \"https://example.com/probe.git\", branch = \"main\", version = \"=0.1.0\" }",
    )?;

    // When
    let violations = violations(&fixture);

    // Then
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("`probe`")),
        "a floating git source must fail: {violations:?}"
    );
    Ok(())
}

#[test]
fn rejects_patch_outside_workspace_members() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::with_manifest_tail(
        "\"=0.1.0\"",
        "\n[patch.crates-io]\nexternal = { path = \"../external\" }\n",
    )?;

    // When
    let violations = violations(&fixture);

    // Then
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("`external` in [patch.crates-io]")),
        "an unpinned patch must fail: {violations:?}"
    );
    Ok(())
}

#[test]
fn rejects_any_replace_entry() -> Result<(), Box<dyn Error>> {
    // Given
    let fixture = Fixture::with_manifest_tail(
        "\"=0.1.0\"",
        "\n[replace]\n\"external:0.1.0\" = { path = \"../external\" }\n",
    )?;

    // When
    let violations = violations(&fixture);

    // Then
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("`external:0.1.0` in [replace]")),
        "a replaced dependency must fail: {violations:?}"
    );
    Ok(())
}

#[test]
fn rejects_unpinned_or_malformed_web_dependency_sections() -> Result<(), Box<dyn Error>> {
    for (section, value) in [
        ("optionalDependencies", "{ \"probe\": \"^1.0.0\" }"),
        ("peerDependencies", "{ \"probe\": \"~1.0.0\" }"),
        ("dependencies", "[\"probe\"]"),
    ] {
        // Given
        let fixture = Fixture::new("\"=0.1.0\"")?;
        fixture.write_web_manifest(&format!("{{ \"{section}\": {value} }}"))?;

        // When
        let violations = violations(&fixture);

        // Then
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains(section)),
            "web `{section}` must fail: {violations:?}"
        );
    }
    Ok(())
}

#[test]
fn inspects_a_member_in_a_directory_the_manifest_walk_skips() -> Result<(), Box<dyn Error>> {
    // Given a member under `vendor`, which the walk for manifests skips, whose dependency on
    // another member names no version.
    let fixture = Fixture::new("\"=0.1.0\"")?;
    let tool = fixture.root().join("vendor/tool");
    write_package(&tool, "tool")?;
    fs::write(
        tool.join("Cargo.toml"),
        "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nmember = { path = \"../../member\" }\n",
    )?;
    let manifest = fixture.root().join("Cargo.toml");
    let text = fs::read_to_string(&manifest)?.replace(
        "members = [\"member\"]",
        "members = [\"member\", \"vendor/tool\"]",
    );
    fs::write(&manifest, text)?;

    // When
    let violations = violations(&fixture);

    // Then
    assert!(
        violations.iter().any(|violation| {
            violation.starts_with("vendor/tool/Cargo.toml") && violation.contains("`member`")
        }),
        "a member the walk skips must still be checked: {violations:?}"
    );
    Ok(())
}

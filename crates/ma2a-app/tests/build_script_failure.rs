//! Regression coverage for actionable frontend build failures.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> std::io::Result<Self> {
        let root =
            std::env::temp_dir().join(format!("ma2a-build-script-probe-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn executable(&self) -> PathBuf {
        self.root.join(if cfg!(windows) {
            "build-script-probe.exe"
        } else {
            "build-script-probe"
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn prints_actionable_message_when_bun_is_missing() -> std::io::Result<()> {
    // Given
    let fixture = Fixture::new()?;
    let compile_status = compile_build_script(&fixture.executable())?;
    assert!(compile_status.success());

    // When
    let output = Command::new(fixture.executable())
        .env_clear()
        .env("OUT_DIR", &fixture.root)
        .output()?;

    // Then
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("Bun 1.3.5"));
    assert!(stderr.contains("bun install --frozen-lockfile"));
    Ok(())
}

fn compile_build_script(executable: &Path) -> std::io::Result<std::process::ExitStatus> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    Command::new("rustc")
        .arg(manifest.join("build.rs"))
        .args(["--edition=2024", "-o"])
        .arg(executable)
        .env("CARGO_MANIFEST_DIR", manifest)
        .status()
}

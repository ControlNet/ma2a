//! Regression coverage for actionable frontend build failures.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> std::io::Result<Self> {
        let serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ma2a-build-script-probe-{}-{serial}",
            std::process::id()
        ));
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
    compile_build_script_at(Path::new(env!("CARGO_MANIFEST_DIR")), executable)
}

fn compile_build_script_at(
    manifest: &Path,
    executable: &Path,
) -> std::io::Result<std::process::ExitStatus> {
    Command::new("rustc")
        .arg(manifest.join("build.rs"))
        .args(["--edition=2024", "-o"])
        .arg(executable)
        .env("CARGO_MANIFEST_DIR", manifest)
        .status()
}

#[cfg(unix)]
#[test]
fn subsequent_frontend_rebuild_does_not_invalidate_prior_generated_module() -> std::io::Result<()> {
    let fixture = Fixture::new()?;
    let workspace = fixture.root.join("workspace");
    let app_dir = workspace.join("crates/ma2a-app");
    let web_dir = workspace.join("web");
    let out_a = fixture.root.join("out-a");
    let out_b = fixture.root.join("out-b");
    fs::create_dir_all(&app_dir)?;
    fs::create_dir_all(&web_dir)?;
    fs::create_dir_all(&out_a)?;
    fs::create_dir_all(&out_b)?;

    let real_manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let real_web = real_manifest.join("../../web");
    fs::copy(real_manifest.join("build.rs"), app_dir.join("build.rs"))?;
    copy_dir_all(&real_web.join("src"), &web_dir.join("src"))?;
    copy_dir_all(&real_web.join("public"), &web_dir.join("public"))?;
    for file in [
        "biome.jsonc",
        "bun.lock",
        "index.html",
        "package.json",
        "tsconfig.json",
        "vite.config.ts",
        "vitest.config.ts",
    ] {
        fs::copy(real_web.join(file), web_dir.join(file))?;
    }
    std::os::unix::fs::symlink(real_web.join("node_modules"), web_dir.join("node_modules"))?;

    let status = compile_build_script_at(&app_dir, &fixture.executable())?;
    assert!(status.success());

    let run_a = Command::new(fixture.executable())
        .env("CARGO_MANIFEST_DIR", &app_dir)
        .env("OUT_DIR", &out_a)
        .output()?;
    assert!(
        run_a.status.success(),
        "build A failed: {}",
        String::from_utf8_lossy(&run_a.stderr)
    );

    // Change CSS so Vite generates a different asset hash and empties its output dir.
    let styles = web_dir.join("src/styles.css");
    let mut css = fs::read_to_string(&styles)?;
    css.push_str("\n.probe-cache-bust { color: #123456; }\n");
    fs::write(&styles, css)?;

    let run_b = Command::new(fixture.executable())
        .env("CARGO_MANIFEST_DIR", &app_dir)
        .env("OUT_DIR", &out_b)
        .output()?;
    assert!(
        run_b.status.success(),
        "build B failed: {}",
        String::from_utf8_lossy(&run_b.stderr)
    );

    let probe_src = fixture.root.join("probe.rs");
    let probe_lib = fixture.root.join("libprobe.rlib");
    fs::write(
        &probe_src,
        "include!(concat!(env!(\"OUT_DIR\"), \"/embedded_web.rs\"));\n",
    )?;
    let compile_a = Command::new("rustc")
        .arg("--crate-type=lib")
        .arg("--edition=2024")
        .arg(&probe_src)
        .arg("-o")
        .arg(&probe_lib)
        .env("OUT_DIR", &out_a)
        .output()?;
    assert!(
        compile_a.status.success(),
        "compiling build A's generated module failed after build B: {}",
        String::from_utf8_lossy(&compile_a.stderr)
    );
    Ok(())
}

#[cfg(unix)]
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

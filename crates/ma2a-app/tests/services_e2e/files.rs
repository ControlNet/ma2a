//! Files and Transfer between real daemons: operations, integrity, cancellation, resume.

use std::{fs, path::Path, process::Child, time::Duration};

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{Lab, Root, content, refused, succeed},
};

/// Waits until `condition` holds, failing after `limit`.
fn eventually(limit: Duration, mut condition: impl FnMut() -> TestValue<bool>) -> TestResult {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if condition()? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("condition did not hold in time".into())
}

fn staging_files(root: &Path) -> TestValue<usize> {
    Ok(fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".ma2a-upload-")
        })
        .count())
}

fn kill(mut child: Child) -> TestResult {
    child.kill()?;
    child.wait()?;
    Ok(())
}

#[test]
fn files_operations_and_a_large_transfer_round_trip_with_integrity() -> TestResult {
    let lab = Lab::start("files", "1")?;
    let written = lab.member.piped(
        &[
            "file",
            "--endpoint",
            lab.owner.id(),
            "write",
            "docs:/hello.txt",
        ],
        b"hello ma2a",
    )?;
    succeed(&written)?;
    assert_eq!(fs::read(lab.root.path().join("hello.txt"))?, b"hello ma2a");
    let again = lab.member.piped(
        &[
            "file",
            "--endpoint",
            lab.owner.id(),
            "write",
            "docs:/hello.txt",
        ],
        b"clobber",
    )?;
    refused(&again, "already_exists")?;
    let cat = lab.remote(&["file"], &["cat", "docs:/hello.txt"])?;
    succeed(&cat)?;
    assert_eq!(cat.stdout, b"hello ma2a");
    succeed(&lab.remote(&["file"], &["mkdir", "-p", "docs:/a/b"])?)?;
    succeed(&lab.remote(&["file"], &["mv", "docs:/hello.txt", "docs:/a/b/hello.txt"])?)?;
    let listed = lab.remote(&["file"], &["ls", "docs:/a/b"])?;
    succeed(&listed)?;
    assert!(String::from_utf8(listed.stdout)?.contains("hello.txt"));
    let stat = lab.remote(&["file"], &["stat", "docs:/a/b/hello.txt"])?;
    succeed(&stat)?;
    assert!(String::from_utf8(stat.stdout)?.contains("Size: 10"));
    refused(&lab.remote(&["file"], &["rm", "docs:/a"])?, "not_empty")?;
    succeed(&lab.remote(&["file"], &["rm", "-r", "docs:/a"])?)?;
    assert!(!lab.root.path().join("a").exists());
    refused(
        &lab.remote(&["file"], &["stat", "docs:/../etc"])?,
        "invalid remote path",
    )?;

    let local = Root::new("files-local")?;
    let source = content(24 << 20);
    fs::write(local.path().join("up.bin"), &source)?;
    let up = local.path().join("up.bin");
    let up = up.to_str().ok_or("path")?;
    succeed(&lab.remote(&["file"], &["put", up, "docs:/big.bin"])?)?;
    assert!(fs::read(lab.root.path().join("big.bin"))? == source);
    let down = local.path().join("down.bin");
    let down = down.to_str().ok_or("path")?;
    succeed(&lab.remote(&["file"], &["get", "docs:/big.bin", down])?)?;
    assert!(fs::read(down)? == source);
    refused(
        &lab.remote(&["file"], &["get", "docs:/big.bin", down])?,
        "--force",
    )?;
    assert_eq!(staging_files(lab.root.path())?, 0);
    lab.shutdown()
}

#[test]
fn cancelled_transfers_leave_nothing_active_and_a_download_resumes_verified() -> TestResult {
    let lab = Lab::start("resume", "1")?;
    let source = content(160 << 20);
    fs::write(lab.root.path().join("huge.bin"), &source)?;
    let local = Root::new("resume-local")?;
    let target = local.path().join("huge.bin");
    let part = local.path().join("huge.bin.ma2a-part");
    let get = |resume: bool| {
        let mut arguments = vec![
            "file",
            "--endpoint",
            lab.owner.id(),
            "get",
            "docs:/huge.bin",
        ];
        arguments.push(target.to_str().unwrap_or_default());
        if resume {
            arguments.push("--resume");
        }
        lab.member.command(&arguments).spawn()
    };

    // Killing the CLI mid-download cancels the remote operation and keeps the partial file.
    let child = get(false)?;
    eventually(Duration::from_secs(60), || {
        Ok(fs::metadata(&part)
            .map(|metadata| metadata.len())
            .unwrap_or(0)
            > 1 << 20)
    })?;
    kill(child)?;
    let kept = fs::metadata(&part)?.len();
    assert!(
        kept < source.len() as u64,
        "the download finished before it was cancelled"
    );
    eventually(Duration::from_secs(20), || {
        Ok(lab.owner.active_operations()? == 0)
    })?;
    eventually(Duration::from_secs(20), || {
        Ok(lab.member.active_operations()? == 0)
    })?;

    // The resumed download continues from the partial file and verifies the whole content.
    let resumed = get(true)?.wait_with_output()?;
    succeed(&resumed)?;
    assert!(fs::read(&target)? == source);
    assert!(!part.exists());

    // A cancelled upload publishes nothing and removes its staging file.
    let upload = local.path().join("upload.bin");
    fs::write(&upload, &source)?;
    let put = lab
        .member
        .command(&[
            "file",
            "--endpoint",
            lab.owner.id(),
            "put",
            upload.to_str().ok_or("path")?,
            "docs:/uploaded.bin",
        ])
        .spawn()?;
    eventually(Duration::from_secs(60), || {
        Ok(staging_files(lab.root.path())? > 0)
    })?;
    kill(put)?;
    eventually(Duration::from_secs(20), || {
        Ok(staging_files(lab.root.path())? == 0)
    })?;
    assert!(!lab.root.path().join("uploaded.bin").exists());
    eventually(Duration::from_secs(20), || {
        Ok(lab.owner.active_operations()? == 0)
    })?;
    lab.shutdown()
}

//! Files host semantics, containment, and atomicity over an in-memory frame stream.

#[path = "support/host.rs"]
mod support;

use std::{fs, path::Path};

use ma2a_core::service::{
    Channel, FileKind, FilesRequest, FilesResult, FrameKind, MAX_LIST_PAGE, RemotePath,
    ServiceError, WriteMode,
};
use ma2a_services::{Frame, OperationEnd, serve_files};
use support::{TempRoot, TestResult, drain, pipe};

fn path(text: &str) -> TestResult<RemotePath> {
    Ok(RemotePath::parse(text)?)
}

/// Serves one request, sending `input` content first, and returns every reply frame.
async fn serve(
    root: &Path,
    request: FilesRequest,
    input: Option<&[u8]>,
) -> TestResult<(Vec<Frame>, ma2a_services::OperationReport)> {
    let ((mut client_reader, mut client_writer), (mut server_reader, mut server_writer)) = pipe();
    let root = root.to_path_buf();
    let server = tokio::spawn(async move {
        serve_files(root, request, (&mut server_reader, &mut server_writer)).await
    });
    if let Some(content) = input {
        let accept = client_reader.expect().await?;
        if accept.kind() != FrameKind::Accept {
            let mut frames = vec![accept];
            frames.extend(drain(&mut client_reader).await?);
            return Ok((frames, server.await?));
        }
        for chunk in content.chunks(1_000) {
            client_writer.data(Channel::Input, chunk).await?;
        }
        client_writer.end(Channel::Input).await?;
        let mut frames = vec![accept];
        frames.extend(drain(&mut client_reader).await?);
        return Ok((frames, server.await?));
    }
    let frames = drain(&mut client_reader).await?;
    Ok((frames, server.await?))
}

fn outcome(frames: &[Frame]) -> TestResult<Result<FilesResult, ServiceError>> {
    let last = frames.last().ok_or("no frames")?;
    assert_eq!(last.kind(), FrameKind::Outcome);
    Ok(FilesResult::decode_outcome(last.payload())?)
}

fn rejection(frames: &[Frame]) -> TestResult<ServiceError> {
    let [frame] = frames else {
        return Err(format!("expected one reject frame, got {}", frames.len()).into());
    };
    Ok(frame.rejection()?)
}

#[tokio::test]
async fn stat_list_mkdir_rename_and_remove_follow_the_contract() -> TestResult {
    let root = TempRoot::new("contract")?;
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Mkdir(path("docs:/a/b/c")?, true),
        None,
    )
    .await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Done));
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Mkdir(path("docs:/a")?, false),
        None,
    )
    .await?;
    assert_eq!(rejection(&frames)?, ServiceError::AlreadyExists);
    fs::write(root.path().join("a/file.txt"), b"hello")?;
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Stat(path("docs:/a/file.txt")?),
        None,
    )
    .await?;
    let Ok(FilesResult::Metadata(metadata)) = outcome(&frames)? else {
        return Err("metadata".into());
    };
    assert_eq!((metadata.kind, metadata.size), (FileKind::File, 5));
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Rename(path("docs:/a/file.txt")?, path("docs:/a/b/moved")?, false),
        None,
    )
    .await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Done));
    fs::write(root.path().join("a/other"), b"x")?;
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Rename(path("docs:/a/other")?, path("docs:/a/b/moved")?, false),
        None,
    )
    .await?;
    assert_eq!(rejection(&frames)?, ServiceError::AlreadyExists);
    assert_eq!(fs::read(root.path().join("a/b/moved"))?, b"hello");
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Rename(path("docs:/a/other")?, path("docs:/a/b/moved")?, true),
        None,
    )
    .await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Done));
    assert_eq!(fs::read(root.path().join("a/b/moved"))?, b"x");
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Remove(path("docs:/a")?, false),
        None,
    )
    .await?;
    assert_eq!(rejection(&frames)?, ServiceError::NotEmpty);
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Remove(path("docs:/")?, true),
        None,
    )
    .await?;
    assert_eq!(rejection(&frames)?, ServiceError::InvalidPath);
    let (frames, report) = serve(
        root.path(),
        FilesRequest::Remove(path("docs:/a")?, true),
        None,
    )
    .await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Done));
    assert_eq!(report.end, OperationEnd::Completed);
    assert!(!root.path().join("a").exists());
    let (frames, report) = serve(root.path(), FilesRequest::Stat(path("docs:/a")?), None).await?;
    assert_eq!(rejection(&frames)?, ServiceError::NotFound);
    assert_eq!(report.end, OperationEnd::Rejected(ServiceError::NotFound));
    Ok(())
}

#[tokio::test]
async fn listing_pages_are_sorted_bounded_and_resumable() -> TestResult {
    let root = TempRoot::new("paging")?;
    for index in 0..300 {
        fs::write(root.path().join(format!("f{index:03}")), b"")?;
    }
    let mut cursor = None;
    let mut seen = Vec::new();
    loop {
        let (frames, _) = serve(
            root.path(),
            FilesRequest::List(path("docs:/")?, cursor.clone()),
            None,
        )
        .await?;
        let Ok(FilesResult::Listing(entries, next)) = outcome(&frames)? else {
            return Err("listing".into());
        };
        assert!(entries.len() <= MAX_LIST_PAGE);
        seen.extend(entries.into_iter().map(|entry| entry.name));
        if next.is_none() {
            break;
        }
        cursor = next;
    }
    let expected = (0..300)
        .map(|index| format!("f{index:03}"))
        .collect::<Vec<_>>();
    assert_eq!(seen, expected);
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn a_name_too_long_to_address_is_left_out_of_a_listing() -> TestResult {
    // Given: a name NTFS takes but a remote path component cannot carry, beside an ordinary one.
    let root = TempRoot::new("long-name")?;
    fs::write(root.path().join("\u{6587}".repeat(100)), b"")?;
    fs::write(root.path().join("ok"), b"")?;

    // When
    let (frames, _) = serve(root.path(), FilesRequest::List(path("docs:/")?, None), None).await?;

    // Then: the page decodes and lists every addressable name.
    let Ok(FilesResult::Listing(entries, None)) = outcome(&frames)? else {
        return Err("listing".into());
    };
    let names = entries
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["ok"]);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn names_a_path_cannot_carry_are_left_out_and_the_rest_can_be_requested() -> TestResult {
    // Given: ordinary Unix names a remote path component cannot carry, and names the Runtime
    // reserves, around addressable ones.
    let root = TempRoot::new("unaddressable")?;
    for name in [
        "a",
        "phase:one.txt",
        "back\\slash",
        "tab\there",
        ".ma2a-upload-1.part",
        ".MA2A-write-2",
        ".ma2a-downloads",
        "z",
    ] {
        fs::write(root.path().join(name), b"")?;
    }

    // When
    let (frames, _) = serve(root.path(), FilesRequest::List(path("docs:/")?, None), None).await?;

    // Then: only addressable names are listed, and each one can be named in a request.
    let Ok(FilesResult::Listing(entries, None)) = outcome(&frames)? else {
        return Err("listing".into());
    };
    let names = entries
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["a", "z"]);
    for name in names {
        let (frames, _) = serve(
            root.path(),
            FilesRequest::Stat(path(&format!("docs:/{name}"))?),
            None,
        )
        .await?;
        assert!(matches!(outcome(&frames)?, Ok(FilesResult::Metadata(_))));
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_reported_but_never_followed() -> TestResult {
    let root = TempRoot::new("symlink")?;
    let outside = TempRoot::new("outside")?;
    fs::write(outside.path().join("secret"), b"secret")?;
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape"))?;
    std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("leaf"))?;
    let (frames, _) = serve(root.path(), FilesRequest::Stat(path("docs:/escape")?), None).await?;
    let Ok(FilesResult::Metadata(metadata)) = outcome(&frames)? else {
        return Err("metadata".into());
    };
    assert_eq!(metadata.kind, FileKind::Symlink);
    for request in [
        FilesRequest::Stat(path("docs:/escape/secret")?),
        FilesRequest::List(path("docs:/escape")?, None),
        FilesRequest::Read(path("docs:/leaf")?, 0, 10),
        FilesRequest::Read(path("docs:/escape/secret")?, 0, 10),
        FilesRequest::Mkdir(path("docs:/escape/new")?, true),
        FilesRequest::Remove(path("docs:/escape/secret")?, false),
    ] {
        let (frames, _) = serve(root.path(), request, None).await?;
        let error = rejection(&frames)?;
        assert!(
            matches!(error, ServiceError::SymlinkRefused),
            "unexpected {error:?}"
        );
    }
    let write = FilesRequest::Write(path("docs:/leaf")?, WriteMode::Replace, 3);
    let (frames, _) = serve(root.path(), write, Some(b"bad")).await?;
    assert_eq!(rejection(&frames)?, ServiceError::SymlinkRefused);
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Remove(path("docs:/escape")?, true),
        None,
    )
    .await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Done));
    assert_eq!(fs::read(outside.path().join("secret"))?, b"secret");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn special_files_are_never_opened() -> TestResult {
    use rustix::fs::{CWD, FileType, Mode, mknodat};

    let root = TempRoot::new("fifo")?;
    mknodat(CWD, root.path().join("pipe"), FileType::Fifo, Mode::RWXU, 0)?;
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Read(path("docs:/pipe")?, 0, 1),
        None,
    )
    .await?;
    assert_eq!(rejection(&frames)?, ServiceError::InvalidPath);
    Ok(())
}

#[tokio::test]
async fn small_reads_honour_offset_length_and_eof() -> TestResult {
    let root = TempRoot::new("read")?;
    fs::write(root.path().join("f"), b"0123456789")?;
    let (frames, report) = serve(
        root.path(),
        FilesRequest::Read(path("docs:/f")?, 2, 4),
        None,
    )
    .await?;
    let [accept, data, _outcome] = frames.as_slice() else {
        return Err("accept, data, outcome".into());
    };
    assert_eq!(accept.kind(), FrameKind::Accept);
    assert_eq!(data.data()?, (Channel::Output, &b"2345"[..]));
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Read(4, false)));
    assert_eq!(report.bytes_out, 4);
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Read(path("docs:/f")?, 8, 100),
        None,
    )
    .await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Read(2, true)));
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Read(path("docs:/f")?, 50, 1),
        None,
    )
    .await?;
    assert_eq!(frames.len(), 2);
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Read(0, true)));
    Ok(())
}

#[tokio::test]
async fn small_writes_are_atomic_and_leave_no_temporary_files() -> TestResult {
    let root = TempRoot::new("write")?;
    let create = || FilesRequest::Write(path("docs:/f").expect("path"), WriteMode::CreateNew, 5);
    let (frames, report) = serve(root.path(), create(), Some(b"first")).await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Written(5)));
    assert_eq!(report.bytes_in, 5);
    let (frames, _) = serve(root.path(), create(), Some(b"again")).await?;
    assert_eq!(rejection(&frames)?, ServiceError::AlreadyExists);
    let replace = FilesRequest::Write(path("docs:/f")?, WriteMode::Replace, 6);
    let (frames, _) = serve(root.path(), replace, Some(b"second")).await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Written(6)));
    let short = FilesRequest::Write(path("docs:/f")?, WriteMode::Replace, 9);
    let (frames, report) = serve(root.path(), short, Some(b"short")).await?;
    assert_eq!(outcome(&frames)?, Err(ServiceError::IntegrityMismatch));
    assert_eq!(
        report.end,
        OperationEnd::Failed(ServiceError::IntegrityMismatch)
    );
    let long = FilesRequest::Write(path("docs:/f")?, WriteMode::Replace, 2);
    let (frames, _) = serve(root.path(), long, Some(b"too long")).await?;
    assert_eq!(outcome(&frames)?, Err(ServiceError::TooLarge));
    assert_eq!(fs::read(root.path().join("f"))?, b"second");
    let names = fs::read_dir(root.path())?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    assert_eq!(names, ["f"]);
    let missing_parent = FilesRequest::Write(path("docs:/none/f")?, WriteMode::CreateNew, 1);
    let (frames, _) = serve(root.path(), missing_parent, Some(b"x")).await?;
    assert_eq!(rejection(&frames)?, ServiceError::NotFound);
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Mkdir(path("docs:/f/x")?, true),
        None,
    )
    .await?;
    assert_eq!(rejection(&frames)?, ServiceError::NotADirectory);
    Ok(())
}

#[tokio::test]
async fn a_recursive_removal_keeps_a_downloads_progress() -> TestResult {
    // Given: a directory holding an ordinary file, and a paused download's private directory.
    let root = TempRoot::new("files-keep-download")?;
    fs::create_dir_all(root.path().join("a/b"))?;
    fs::write(root.path().join("a/b/plain"), b"x")?;
    fs::write(root.path().join("a/top"), b"y")?;
    let helpers = root.path().join("a/b/.ma2a-downloads");
    fs::create_dir(&helpers)?;
    fs::write(helpers.join("0123.ma2a-part"), b"progress")?;

    // When
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Remove(path("docs:/a")?, true),
        None,
    )
    .await?;

    // Then: everything else is gone, the download keeps its progress, and the removal reports
    // that it could not finish.
    assert_eq!(outcome(&frames)?, Err(ServiceError::NotEmpty));
    assert!(!root.path().join("a/top").exists());
    assert!(!root.path().join("a/b/plain").exists());
    assert_eq!(fs::read(helpers.join("0123.ma2a-part"))?, b"progress");

    // And: a tree without one is still removed whole.
    fs::create_dir_all(root.path().join("c/d"))?;
    fs::write(root.path().join("c/d/plain"), b"z")?;
    let (frames, _) = serve(
        root.path(),
        FilesRequest::Remove(path("docs:/c")?, true),
        None,
    )
    .await?;
    assert_eq!(outcome(&frames)?, Ok(FilesResult::Done));
    assert!(!root.path().join("c").exists());
    Ok(())
}

#[tokio::test]
async fn a_listing_leaves_out_children_deeper_than_a_path_can_reach() -> TestResult {
    // Given: a directory at the deepest level a path names, holding one file.
    let root = TempRoot::new("deepest")?;
    let levels = (0..ma2a_core::service::MAX_PATH_COMPONENTS)
        .map(|level| level.to_string())
        .collect::<Vec<_>>();
    let deepest = levels
        .iter()
        .fold(root.path().to_path_buf(), |path, level| path.join(level));
    fs::create_dir_all(&deepest)?;
    fs::write(deepest.join("unreachable"), b"")?;

    // When
    let listed = path(&format!("docs:/{}", levels.join("/")))?;
    let (frames, _) = serve(root.path(), FilesRequest::List(listed, None), None).await?;

    // Then: its child would need one component more than a path carries.
    let Ok(FilesResult::Listing(entries, None)) = outcome(&frames)? else {
        return Err("listing".into());
    };
    assert!(entries.is_empty());
    Ok(())
}

/// Grants or withdraws, through `icacls`, a rule that denies everyone `rights` on `path`.
#[cfg(windows)]
fn deny_everyone(path: &Path, rights: Option<&str>) -> TestResult {
    let mut command = std::process::Command::new("icacls");
    command.arg(path);
    match rights {
        Some(rights) => command.args(["/deny", &format!("*S-1-1-0:({rights})")]),
        None => command.args(["/remove:d", "*S-1-1-0"]),
    };
    let status = command.stdout(std::process::Stdio::null()).status()?;
    if !status.success() {
        return Err(format!("icacls failed on {}: {status}", path.display()).into());
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn a_file_whose_acl_refuses_removal_reports_permission_denied() -> TestResult {
    // Given: a regular file that neither its own ACL nor its directory's lets anyone delete.
    let root = TempRoot::new("acl-refused")?;
    let file = root.path().join("kept");
    fs::write(&file, b"x")?;
    deny_everyone(&file, Some("D"))?;
    deny_everyone(root.path(), Some("DC"))?;
    let direct = fs::remove_file(&file);

    // When
    let served = serve(
        root.path(),
        FilesRequest::Remove(path("docs:/kept")?, false),
        None,
    )
    .await;
    deny_everyone(root.path(), None)?;
    deny_everyone(&file, None)?;

    // Then: the fixture really refuses deletion, and the service reports it as it is.
    assert_eq!(
        direct.err().map(|error| error.kind()),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    let (frames, _) = served?;
    assert_eq!(rejection(&frames)?, ServiceError::PermissionDenied);
    assert!(file.exists());
    Ok(())
}

//! Transfer host integrity, resume, cancellation, and staging lifecycle.

#[path = "support/host.rs"]
mod support;

use std::{fs, path::Path, sync::Mutex};

use ma2a_core::{
    RequestId,
    service::{
        Channel, ContentIdentity, FrameKind, MAX_DATA_CHUNK_LEN, RemotePath, ServiceError,
        TransferAccept, TransferCommit, TransferRequest, TransferResource, TransferResult,
        WriteMode,
    },
};
use ma2a_services::{Frame, OperationEnd, StagingJournal, serve_transfer};
use support::{ClientIo, TempRoot, TestResult, content, drain, pipe_with};

#[derive(Debug, Default)]
struct RecordingJournal {
    events: Mutex<Vec<(&'static str, String, bool)>>,
}

impl RecordingJournal {
    fn events(&self) -> Vec<(&'static str, String, bool)> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl StagingJournal for RecordingJournal {
    async fn record(&self, path: &str) -> Result<(), ServiceError> {
        let existed = Path::new(path).exists();
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(("record", path.to_owned(), existed));
        Ok(())
    }

    async fn forget(&self, path: &str) {
        let existed = Path::new(path).exists();
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(("forget", path.to_owned(), existed));
    }
}

fn file(text: &str) -> TestResult<TransferResource> {
    Ok(TransferResource::File(RemotePath::parse(text)?))
}

fn request_id() -> TestResult<RequestId> {
    Ok(RequestId::try_from([0xab; 16].as_slice())?)
}

type Served = tokio::task::JoinHandle<ma2a_services::OperationReport>;

fn start(
    (root, request): (&Path, TransferRequest),
    journal: std::sync::Arc<RecordingJournal>,
    capacity: usize,
) -> TestResult<(ClientIo, Served)> {
    let (client, (mut reader, mut writer)) = pipe_with(capacity);
    let root = root.to_path_buf();
    let request_id = request_id()?;
    let server = tokio::spawn(async move {
        serve_transfer(
            (root, request_id),
            (request, journal.as_ref()),
            (&mut reader, &mut writer),
        )
        .await
    });
    Ok((client, server))
}

fn data_bytes(frames: &[Frame]) -> TestResult<Vec<u8>> {
    let mut bytes = Vec::new();
    for frame in frames
        .iter()
        .filter(|frame| frame.kind() == FrameKind::Data)
    {
        let (channel, chunk) = frame.data()?;
        assert_eq!(channel, Channel::Output);
        assert!(chunk.len() <= MAX_DATA_CHUNK_LEN);
        bytes.extend_from_slice(chunk);
    }
    Ok(bytes)
}

fn transfer_outcome(frames: &[Frame]) -> TestResult<Result<TransferResult, ServiceError>> {
    let last = frames.last().ok_or("no frames")?;
    assert_eq!(last.kind(), FrameKind::Outcome);
    Ok(TransferResult::decode_outcome(last.payload())?)
}

#[tokio::test]
async fn downloads_verify_full_content_and_resume_from_an_identity() -> TestResult {
    let root = TempRoot::new("download")?;
    let source = content(300_000);
    fs::write(root.path().join("big"), &source)?;
    let journal = std::sync::Arc::default();
    let request = TransferRequest::Get(file("docs:/big")?, 0, None);
    let ((mut reader, _writer), server) = start((root.path(), request), journal, 1 << 20)?;
    let frames = drain(&mut reader).await?;
    assert_eq!(server.await?.bytes_out, 300_000);
    let accept = frames.first().ok_or("accept")?;
    let TransferAccept::Get(identity) = TransferAccept::decode(accept.payload())? else {
        return Err("get accept".into());
    };
    assert_eq!(identity.size(), 300_000);
    assert_eq!(data_bytes(&frames)?, source);
    let full = transfer_outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(full.hash, *blake3::hash(&source).as_bytes());

    let request = TransferRequest::Get(file("docs:/big")?, 250_001, Some(identity));
    let ((mut reader, _writer), server) =
        start((root.path(), request), std::sync::Arc::default(), 1 << 20)?;
    let frames = drain(&mut reader).await?;
    assert_eq!(server.await?.end, OperationEnd::Completed);
    assert_eq!(data_bytes(&frames)?, source.get(250_001..).ok_or("tail")?);
    let resumed = transfer_outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!((resumed.bytes, resumed.hash), (49_999, full.hash));

    let stale = ContentIdentity::new(identity.size(), identity.modified_ns() + 1);
    let request = TransferRequest::Get(file("docs:/big")?, 1, Some(stale));
    let ((mut reader, _writer), server) =
        start((root.path(), request), std::sync::Arc::default(), 1 << 20)?;
    let frames = drain(&mut reader).await?;
    assert_eq!(
        frames.first().ok_or("reject")?.rejection()?,
        ServiceError::PreconditionFailed
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Rejected(ServiceError::PreconditionFailed)
    );
    Ok(())
}

#[tokio::test]
async fn a_cancelled_download_stops_under_backpressure() -> TestResult {
    let root = TempRoot::new("cancel-download")?;
    fs::write(root.path().join("big"), content(4 << 20))?;
    let request = TransferRequest::Get(file("docs:/big")?, 0, None);
    let ((mut reader, mut writer), server) =
        start((root.path(), request), std::sync::Arc::default(), 128 << 10)?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    assert_eq!(reader.expect().await?.kind(), FrameKind::Data);
    writer.frame(FrameKind::Cancel, &[]).await?;
    let report = server.await?;
    assert_eq!(report.end, OperationEnd::Failed(ServiceError::Cancelled));
    assert!(report.bytes_out < 4 << 20);
    Ok(())
}

#[tokio::test]
async fn a_source_that_changes_during_download_fails_the_outcome() -> TestResult {
    let root = TempRoot::new("changed")?;
    let path = root.path().join("big");
    fs::write(&path, content(1 << 20))?;
    let request = TransferRequest::Get(file("docs:/big")?, 0, None);
    let ((mut reader, _writer), server) =
        start((root.path(), request), std::sync::Arc::default(), 128 << 10)?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let mut appended = fs::OpenOptions::new().append(true).open(&path)?;
    std::io::Write::write_all(&mut appended, b"grown")?;
    drop(appended);
    let frames = drain(&mut reader).await?;
    assert_eq!(
        transfer_outcome(&frames)?,
        Err(ServiceError::ContentChanged)
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::ContentChanged)
    );
    Ok(())
}

async fn upload(
    (client, content): (&mut ClientIo, &[u8]),
    hash: [u8; 32],
) -> TestResult<Vec<Frame>> {
    let (reader, writer) = client;
    let accept = reader.expect().await?;
    if accept.kind() != FrameKind::Accept {
        return Ok(vec![accept]);
    }
    for chunk in content.chunks(MAX_DATA_CHUNK_LEN) {
        writer.data(Channel::Input, chunk).await?;
    }
    writer.end(Channel::Input).await?;
    writer
        .frame(FrameKind::Control, &TransferCommit::new(hash).encode()?)
        .await?;
    drain(reader).await
}

fn staging_path(root: &Path) -> String {
    root.join(format!(".ma2a-upload-{}.part", "ab".repeat(16)))
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn uploads_are_journaled_before_staging_and_published_atomically() -> TestResult {
    let root = TempRoot::new("upload")?;
    let payload = content(200_000);
    let journal = std::sync::Arc::new(RecordingJournal::default());
    let request = TransferRequest::Put(file("docs:/new.bin")?, 200_000, WriteMode::CreateNew);
    let (mut client, server) = start(
        (root.path(), request),
        std::sync::Arc::clone(&journal),
        1 << 20,
    )?;
    let frames = upload((&mut client, &payload), *blake3::hash(&payload).as_bytes()).await?;
    let result = transfer_outcome(&frames)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result.size, 200_000);
    assert_eq!(server.await?.bytes_in, 200_000);
    assert_eq!(fs::read(root.path().join("new.bin"))?, payload);
    let staging = staging_path(root.path());
    assert_eq!(
        journal.events(),
        [
            ("record", staging.clone(), false),
            ("forget", staging, false)
        ]
    );

    let journal = std::sync::Arc::new(RecordingJournal::default());
    let request = TransferRequest::Put(file("docs:/new.bin")?, 1, WriteMode::CreateNew);
    let (mut client, server) = start(
        (root.path(), request),
        std::sync::Arc::clone(&journal),
        1 << 20,
    )?;
    let frames = upload((&mut client, b"x"), *blake3::hash(b"x").as_bytes()).await?;
    assert_eq!(
        frames.first().ok_or("reject")?.rejection()?,
        ServiceError::AlreadyExists
    );
    assert!(matches!(server.await?.end, OperationEnd::Rejected(_)));
    assert!(journal.events().is_empty());
    Ok(())
}

#[tokio::test]
async fn failed_uploads_leave_the_destination_and_no_staging_file() -> TestResult {
    let root = TempRoot::new("upload-fail")?;
    fs::write(root.path().join("keep"), b"original")?;
    let payload = content(70_000);
    let journal = std::sync::Arc::new(RecordingJournal::default());
    let request = TransferRequest::Put(file("docs:/keep")?, 70_000, WriteMode::Replace);
    let (mut client, server) = start(
        (root.path(), request),
        std::sync::Arc::clone(&journal),
        1 << 20,
    )?;
    let frames = upload((&mut client, &payload), [0; 32]).await?;
    assert_eq!(
        transfer_outcome(&frames)?,
        Err(ServiceError::IntegrityMismatch)
    );
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::IntegrityMismatch)
    );
    assert_eq!(fs::read(root.path().join("keep"))?, b"original");
    let events = journal.events();
    assert_eq!(
        events.last().map(|event| (event.0, event.2)),
        Some(("forget", false))
    );

    let request = TransferRequest::Put(file("docs:/keep")?, 1 << 20, WriteMode::Replace);
    let ((mut reader, mut writer), server) =
        start((root.path(), request), std::sync::Arc::default(), 1 << 20)?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let chunk = payload.get(..MAX_DATA_CHUNK_LEN).ok_or("chunk")?;
    writer.data(Channel::Input, chunk).await?;
    writer.frame(FrameKind::Cancel, &[]).await?;
    let report = server.await?;
    assert_eq!(report.end, OperationEnd::Failed(ServiceError::Cancelled));
    assert_eq!(report.bytes_in, 65_536);
    let names = fs::read_dir(root.path())?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    assert_eq!(names, ["keep"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stalled_upload_times_out_and_is_removed() -> TestResult {
    let root = TempRoot::new("stall")?;
    let request = TransferRequest::Put(file("docs:/slow")?, 10, WriteMode::CreateNew);
    let ((mut reader, _writer), server) =
        start((root.path(), request), std::sync::Arc::default(), 1 << 20)?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
    let frames = drain(&mut reader).await?;
    assert_eq!(transfer_outcome(&frames)?, Err(ServiceError::TimedOut));
    assert_eq!(
        server.await?.end,
        OperationEnd::Failed(ServiceError::TimedOut)
    );
    assert_eq!(fs::read_dir(root.path())?.count(), 0);
    Ok(())
}

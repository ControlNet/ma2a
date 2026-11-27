//! Caller-side Files and Transfer drivers against the real hosts, including resume.

#[path = "support/host.rs"]
mod support;

use ma2a_core::{
    RequestId,
    service::{
        FileKind, FilesRequest, FilesResult, RemotePath, ServiceError, TransferRequest,
        TransferResource, WriteMode,
    },
};
use ma2a_services::{
    StagingJournal,
    client::{
        self, ClientError,
        resume::{self, Partial},
    },
    serve_files, serve_transfer,
};
use support::{ClientIo, TempRoot, TestResult, content, pipe_with};
use tokio::io::AsyncWriteExt as _;

/// Records nothing: staging cleanup is covered by the host's own tests.
struct NoJournal;

impl StagingJournal for NoJournal {
    async fn record(&self, _path: &str) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn forget(&self, _path: &str) {}
}

fn path(text: &str) -> TestResult<RemotePath> {
    Ok(RemotePath::parse(text)?)
}

/// Runs one Files request through the caller driver against a host rooted at `root`.
async fn files(
    root: &TempRoot,
    request: FilesRequest,
    content: &[u8],
) -> Result<(FilesResult, Vec<u8>), ClientError> {
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 10);
    let host = serve_files(
        root.path().to_path_buf(),
        request.clone(),
        (&mut host_reader, &mut host_writer),
    );
    let (result, _report) = tokio::join!(
        client::files::run((&mut reader, &mut writer), &request, content),
        host
    );
    result
}

/// Starts a Transfer host for `request` and returns the caller's frame halves.
fn transfer(root: &TempRoot, request: TransferRequest) -> TestResult<ClientIo> {
    let (client, (mut reader, mut writer)) = pipe_with(1 << 12);
    let root = root.path().to_path_buf();
    let request_id = RequestId::try_from([0x5a; 16].as_slice())?;
    tokio::spawn(async move {
        serve_transfer(
            (root, request_id),
            (request, &NoJournal),
            (&mut reader, &mut writer),
        )
        .await
    });
    Ok(client)
}

#[tokio::test]
async fn files_operations_round_trip_through_the_caller_driver() -> TestResult {
    let root = TempRoot::new("client-files")?;
    let made = files(&root, FilesRequest::Mkdir(path("docs:/dir")?, false), &[]).await?;
    assert_eq!(made.0, FilesResult::Done);
    let write = FilesRequest::Write(path("docs:/dir/note")?, WriteMode::CreateNew, 5);
    assert_eq!(
        files(&root, write, b"hello").await?.0,
        FilesResult::Written(5)
    );
    let read = files(
        &root,
        FilesRequest::Read(path("docs:/dir/note")?, 1, 3),
        &[],
    )
    .await?;
    assert_eq!(read, (FilesResult::Read(3, false), b"ell".to_vec()));
    let FilesResult::Listing(entries, None) =
        files(&root, FilesRequest::List(path("docs:/dir")?, None), &[])
            .await?
            .0
    else {
        return Err("listing".into());
    };
    assert_eq!(entries.len(), 1);
    let FilesResult::Metadata(metadata) =
        files(&root, FilesRequest::Stat(path("docs:/dir/note")?), &[])
            .await?
            .0
    else {
        return Err("metadata".into());
    };
    assert_eq!((metadata.kind, metadata.size), (FileKind::File, 5));
    let missing = files(&root, FilesRequest::Stat(path("docs:/absent")?), &[]).await;
    assert!(matches!(
        missing,
        Err(ClientError::Rejected(ServiceError::NotFound))
    ));
    Ok(())
}

#[tokio::test]
async fn an_interrupted_download_resumes_to_the_verified_full_content() -> TestResult {
    let root = TempRoot::new("client-download")?;
    let local = TempRoot::new("client-download-local")?;
    let source = content(400_000);
    std::fs::write(root.path().join("big"), &source)?;
    let destination = local.path().join("big");
    let resource =
        || -> TestResult<TransferResource> { Ok(TransferResource::File(path("docs:/big")?)) };

    // A first attempt stores its identity, receives a prefix, and then "crashes".
    let mut partial = Partial::open(&destination, "peer docs:/big", true).await?;
    let (mut reader, _writer) = transfer(&root, TransferRequest::Get(resource()?, 0, None))?;
    let identity = client::transfer::download_accepted(&mut reader).await?;
    partial.begin(identity).await?;
    let prefix = source.get(..150_000).ok_or("prefix")?;
    let (file, hasher) = partial.sink();
    file.write_all(prefix).await?;
    hasher.update(prefix);
    file.flush().await?;
    drop((partial, reader));

    // The resumed attempt continues from the prefix and verifies the full hash.
    let mut partial = Partial::open(&destination, "peer docs:/big", true).await?;
    assert_eq!(
        (partial.offset(), partial.identity()),
        (150_000, Some(identity))
    );
    let request = TransferRequest::Get(resource()?, partial.offset(), partial.identity());
    let (mut reader, _writer) = transfer(&root, request)?;
    assert_eq!(
        client::transfer::download_accepted(&mut reader).await?,
        identity
    );
    let offset = partial.offset();
    let result =
        client::transfer::download_content(&mut reader, partial.sink(), (identity, offset)).await?;
    assert_eq!((result.bytes, result.size), (250_000, 400_000));
    partial.finish(false).await?;
    assert_eq!(std::fs::read(&destination)?, source);
    assert!(!local.path().join(resume::PRIVATE_DIRECTORY).exists());

    // A partial file of a different source never continues.
    let fresh = Partial::open(&local.path().join("other"), "peer docs:/big", true).await?;
    assert_eq!((fresh.offset(), fresh.identity()), (0, None));
    fresh.discard().await?;
    Ok(())
}

#[tokio::test]
async fn a_second_download_to_one_destination_is_refused_while_the_first_holds_it() -> TestResult {
    // Given: one download holds its destination, even across a restart from zero.
    let local = TempRoot::new("client-download-lease")?;
    let destination = local.path().join("file");
    let mut first = Partial::open(&destination, "peer docs:/file", true).await?;
    first.restart().await?;

    // When
    let second = Partial::open(&destination, "peer docs:/file", true).await;

    // Then: the second never touches the partial file the first is writing.
    assert_eq!(
        second.map(|_| ()).map_err(|error| error.kind()),
        Err(std::io::ErrorKind::WouldBlock)
    );
    first.discard().await?;
    let third = Partial::open(&destination, "peer docs:/file", true).await?;
    third.discard().await?;
    assert!(!local.path().join(resume::PRIVATE_DIRECTORY).exists());
    Ok(())
}

#[tokio::test]
async fn a_resumed_download_whose_source_changed_restarts_from_zero() -> TestResult {
    // Given: a resumed partial download holding a prefix of the old content.
    let local = TempRoot::new("client-download-restart")?;
    let destination = local.path().join("file");
    let mut first = Partial::open(&destination, "peer docs:/file", true).await?;
    first
        .begin(ma2a_core::service::ContentIdentity::new(100, 1))
        .await?;
    let (file, hasher) = first.sink();
    file.write_all(b"old prefix").await?;
    hasher.update(b"old prefix");
    file.flush().await?;
    drop(first);
    let mut resumed = Partial::open(&destination, "peer docs:/file", true).await?;
    assert_eq!(resumed.offset(), 10);

    // When: the source changed, so the download starts over under the same lease.
    resumed.restart().await?;
    assert_eq!((resumed.offset(), resumed.identity()), (0, None));
    let (file, hasher) = resumed.sink();
    file.write_all(b"new").await?;
    hasher.update(b"new");
    resumed.finish(false).await?;

    // Then: only the new content is published.
    assert_eq!(std::fs::read(&destination)?, b"new");
    assert!(!local.path().join(resume::PRIVATE_DIRECTORY).exists());
    Ok(())
}

#[tokio::test]
async fn a_download_never_replaces_a_destination_created_meanwhile_without_replace() -> TestResult {
    // Given: a complete partial download and a destination another writer created.
    let local = TempRoot::new("client-download-no-clobber")?;
    let destination = local.path().join("file");
    let mut partial = Partial::open(&destination, "peer docs:/file", false).await?;
    partial.sink().0.write_all(b"downloaded").await?;
    std::fs::write(&destination, b"theirs")?;

    // When
    let finished = partial.finish(false).await;

    // Then: their file stays and the downloaded content is still there to resume.
    assert_eq!(
        finished.map_err(|error| error.kind()),
        Err(std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(&destination)?, b"theirs");
    assert_eq!(
        std::fs::read(resume::partial_path(&destination)?)?,
        b"downloaded"
    );
    Ok(())
}

#[tokio::test]
async fn a_resume_against_changed_content_is_refused_before_any_byte() -> TestResult {
    let root = TempRoot::new("client-changed")?;
    std::fs::write(root.path().join("file"), content(10_000))?;
    let (mut reader, _writer) = transfer(
        &root,
        TransferRequest::Get(TransferResource::File(path("docs:/file")?), 0, None),
    )?;
    let identity = client::transfer::download_accepted(&mut reader).await?;
    std::fs::write(root.path().join("file"), content(20_000))?;
    let request = TransferRequest::Get(
        TransferResource::File(path("docs:/file")?),
        10,
        Some(identity),
    );
    let (mut reader, _writer) = transfer(&root, request)?;
    let refused = client::transfer::download_accepted(&mut reader).await;
    assert!(matches!(
        refused,
        Err(ClientError::Rejected(ServiceError::PreconditionFailed))
    ));
    Ok(())
}

#[tokio::test]
async fn an_upload_is_committed_and_verified_and_a_short_source_publishes_nothing() -> TestResult {
    let root = TempRoot::new("client-upload")?;
    let source = content(300_001);
    let request = TransferRequest::Put(
        TransferResource::File(path("docs:/up")?),
        300_001,
        WriteMode::CreateNew,
    );
    let (mut reader, mut writer) = transfer(&root, request)?;
    let result = client::transfer::upload(
        (&mut reader, &mut writer),
        (&mut source.as_slice(), 300_001),
    )
    .await?;
    assert_eq!(result.hash, *blake3::hash(&source).as_bytes());
    assert_eq!(std::fs::read(root.path().join("up"))?, source);

    let request = TransferRequest::Put(
        TransferResource::File(path("docs:/short")?),
        10,
        WriteMode::CreateNew,
    );
    let (mut reader, mut writer) = transfer(&root, request)?;
    let short = client::transfer::upload((&mut reader, &mut writer), (&mut &b"four"[..], 10)).await;
    assert!(matches!(short, Err(ClientError::Local(_))), "{short:?}");
    assert!(!root.path().join("short").exists());
    Ok(())
}

/// A reader that yields one slice, then fails with `BrokenPipe`.
struct FailsAfter(Option<&'static [u8]>);

impl tokio::io::AsyncRead for FailsAfter {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.0.take() {
            Some(chunk) => {
                let take = chunk.len().min(buffer.remaining());
                buffer.put_slice(chunk.get(..take).unwrap_or_default());
                if take < chunk.len() {
                    self.0 = chunk.get(take..);
                }
                std::task::Poll::Ready(Ok(()))
            }
            None => std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_local_read_bounds_upload_cancel_and_terminal_wait() -> TestResult {
    use ma2a_core::service::{FrameKind, TransferAccept};

    let grace = ma2a_services::reply::FRAME_GRACE;
    let put_accept = TransferAccept::Put.encode()?;

    // (a) Small full duplex (64 B = 5 B header + 1 B channel + 58 B data) whose peer never reads:
    // sending Cancel blocks on the full write buffer, so upload must return within FRAME_GRACE.
    let ((mut reader, mut writer), (_target_reader, mut target_writer)) = pipe_with(64);
    target_writer.frame(FrameKind::Accept, &put_accept).await?;
    target_writer.flush().await?;
    let mut source = FailsAfter(Some(&[b'x'; 58]));
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        grace * 3,
        client::transfer::upload((&mut reader, &mut writer), (&mut source, 128)),
    )
    .await?;
    assert_eq!(started.elapsed(), grace);
    assert!(matches!(result, Err(ClientError::Local(_))), "{result:?}");

    // (b) Peer reads Cancel and stays silent without closing or sending a terminal frame:
    // upload must still return within FRAME_GRACE.
    let ((mut reader, mut writer), (mut target_reader, mut target_writer)) = pipe_with(1 << 12);
    target_writer.frame(FrameKind::Accept, &put_accept).await?;
    target_writer.flush().await?;
    let peer = tokio::spawn(async move {
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Data);
        assert_eq!(target_reader.expect().await?.kind(), FrameKind::Cancel);
        std::future::pending::<TestResult>().await
    });
    let mut source = FailsAfter(Some(b"hello"));
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        grace * 3,
        client::transfer::upload((&mut reader, &mut writer), (&mut source, 128)),
    )
    .await?;
    assert!(started.elapsed() <= grace, "{:?}", started.elapsed());
    assert!(matches!(result, Err(ClientError::Local(_))), "{result:?}");
    peer.abort();
    Ok(())
}

//! Canonical service `open`, `accept`, `control`, and `outcome` codecs.

use iroh_base::SecretKey;
use ma2a_core::{
    EndpointId, ProtocolError, RequestId,
    service::{
        ContentIdentity, DirEntry, EndReason, ExecRequest, FileKind, FileMetadata, FilesRequest,
        FilesResult, MAX_HEADER_FRAME_LEN, OpenBody, OpenHeader, ProcessResult, ProcessSpec,
        ProxyHost, ProxyRequest, ProxyResult, RemotePath, ServiceError, ShellRequest, TerminalSize,
        Termination, TransferAccept, TransferCommit, TransferRequest, TransferResource,
        TransferResult, WriteMode,
    },
};
use proptest::prelude::*;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn target() -> EndpointId {
    SecretKey::from_bytes(&[0x51; 32]).public().into()
}

fn request_id() -> TestResult<RequestId> {
    Ok(RequestId::try_from([0x52; 16].as_slice())?)
}

fn path(text: &str) -> TestResult<RemotePath> {
    Ok(RemotePath::parse(text)?)
}

fn roundtrip(body: OpenBody) -> TestResult {
    let header = OpenHeader::new(request_id()?, target(), body);
    let encoded = header.encode()?;
    assert_eq!(OpenHeader::decode(&encoded)?, header);
    let mut trailing = encoded;
    trailing.push(0);
    assert!(OpenHeader::decode(&trailing).is_err());
    Ok(())
}

#[test]
fn every_open_body_roundtrips_canonically() -> TestResult {
    let file = path("docs:/a/b.txt")?;
    for request in [
        FilesRequest::Stat(path("docs:/")?),
        FilesRequest::List(file.clone(), Some("m".to_owned())),
        FilesRequest::Mkdir(file.clone(), true),
        FilesRequest::Read(file.clone(), 7, 65_536),
        FilesRequest::Write(file.clone(), WriteMode::CreateNew, 0),
        FilesRequest::Rename(file.clone(), path("docs:/c")?, false),
        FilesRequest::Remove(file.clone(), true),
    ] {
        roundtrip(OpenBody::Files(request))?;
    }
    let resource = TransferResource::File(file);
    let identity = ContentIdentity::new(10, 20);
    roundtrip(OpenBody::Transfer(TransferRequest::Get(
        resource.clone(),
        4,
        Some(identity),
    )))?;
    roundtrip(OpenBody::Transfer(TransferRequest::Put(
        resource,
        1 << 40,
        WriteMode::Replace,
    )))?;
    let spec = ProcessSpec::new(
        Some("printf".to_owned()),
        vec![String::new(), "a\tb\nc".to_owned()],
        Some("/tmp".to_owned()),
    )?
    .with_env("LANG".to_owned(), "C".to_owned())?;
    roundtrip(OpenBody::Exec(ExecRequest::new(spec, true, 1_000)?))?;
    let shell = ProcessSpec::new(None, Vec::new(), None)?;
    let terminal = ("xterm-256color".to_owned(), TerminalSize::new(80, 24)?);
    roundtrip(OpenBody::Shell(ShellRequest::new(shell, terminal, 60_000)?))?;
    let host = ProxyHost::new("db.internal")?;
    roundtrip(OpenBody::Proxy(ProxyRequest::new(host, 5432, 1_000)?))?;
    Ok(())
}

#[test]
fn malformed_headers_are_rejected_before_dispatch() -> TestResult {
    let header = OpenHeader::new(
        request_id()?,
        target(),
        OpenBody::Files(FilesRequest::Stat(path("docs:/x")?)),
    );
    let encoded = header.encode()?;
    let mut version = encoded.clone();
    let offset = version
        .windows(3)
        .position(|window| window == [0x00, 0x82, 0x01])
        .ok_or("version field")?;
    *version.get_mut(offset + 2).ok_or("major")? = 0x02;
    assert_eq!(
        OpenHeader::decode(&version),
        Err(ProtocolError::VERSION_MISMATCH)
    );
    assert!(OpenHeader::decode(&vec![0xff; MAX_HEADER_FRAME_LEN + 1]).is_err());
    let truncated = encoded.get(..encoded.len() - 1).ok_or("truncated")?;
    assert!(OpenHeader::decode(truncated).is_err());
    Ok(())
}

#[test]
fn process_and_proxy_bounds_are_enforced() -> TestResult {
    let spec = ProcessSpec::new(Some("true".to_owned()), Vec::new(), None)?;
    assert!(ExecRequest::new(spec.clone(), false, 0).is_err());
    assert!(ExecRequest::new(spec.clone(), false, 86_400_001).is_err());
    assert!(ExecRequest::new(ProcessSpec::new(None, Vec::new(), None)?, false, 1).is_err());
    assert!(
        spec.clone()
            .with_env("1BAD".to_owned(), String::new())
            .is_err()
    );
    let duplicate = spec.with_env("A".to_owned(), String::new())?;
    assert!(duplicate.with_env("A".to_owned(), String::new()).is_err());
    assert!(ProcessSpec::new(Some("a\0b".to_owned()), Vec::new(), None).is_err());
    assert!(TerminalSize::new(0, 24).is_err());
    assert!(ProxyRequest::new(ProxyHost::new("a.b")?, 0, 1).is_err());
    assert!(ProxyRequest::new(ProxyHost::new("a.b")?, 1, 3_600_001).is_err());
    Ok(())
}

#[test]
fn outcomes_accepts_and_controls_roundtrip() -> TestResult {
    let metadata = FileMetadata::new(FileKind::Symlink, 3)
        .with_modified_ms(Some(9))
        .with_read_only(true);
    for outcome in [
        Ok(FilesResult::Metadata(metadata)),
        Ok(FilesResult::Listing(
            vec![DirEntry::new("a".to_owned(), metadata)],
            None,
        )),
        Ok(FilesResult::Read(3, true)),
        Ok(FilesResult::Written(4)),
        Ok(FilesResult::Done),
        Err(ServiceError::SymlinkRefused),
    ] {
        let encoded = FilesResult::encode_outcome(&outcome)?;
        assert_eq!(FilesResult::decode_outcome(&encoded)?, outcome);
    }
    let transfer = Ok(TransferResult::new(1, 2, [7; 32]));
    let encoded = TransferResult::encode_outcome(&transfer)?;
    assert_eq!(TransferResult::decode_outcome(&encoded)?, transfer);
    let process =
        Ok(ProcessResult::new(Termination::Signaled(9), EndReason::TimedOut, 5).with_bytes(6, 7));
    let encoded = ProcessResult::encode_outcome(&process)?;
    assert_eq!(ProcessResult::decode_outcome(&encoded)?, process);
    let proxy = Err(ServiceError::ConnectionRefused);
    let encoded = ProxyResult::encode_outcome(&proxy)?;
    assert_eq!(ProxyResult::decode_outcome(&encoded)?, proxy);
    let accept = TransferAccept::Get(ContentIdentity::new(1, 2));
    assert_eq!(TransferAccept::decode(&accept.encode()?)?, accept);
    let commit = TransferCommit::new([3; 32]);
    assert_eq!(TransferCommit::decode(&commit.encode()?)?, commit);
    let size = TerminalSize::new(100, 40)?;
    assert_eq!(TerminalSize::decode_resize(&size.encode_resize()?)?, size);
    Ok(())
}

#[test]
fn resumed_download_must_name_its_identity_and_rename_stays_in_one_root() -> TestResult {
    let resource = TransferResource::File(path("docs:/f")?);
    let header = OpenHeader::new(
        request_id()?,
        target(),
        OpenBody::Transfer(TransferRequest::Get(resource, 5, None)),
    );
    assert!(OpenHeader::decode(&header.encode()?).is_err());
    let rename = OpenHeader::new(
        request_id()?,
        target(),
        OpenBody::Files(FilesRequest::Rename(
            path("docs:/a")?,
            path("other:/a")?,
            false,
        )),
    );
    assert!(OpenHeader::decode(&rename.encode()?).is_err());
    Ok(())
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic_the_decoders(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _open = OpenHeader::decode(&bytes);
        let _files = FilesResult::decode_outcome(&bytes);
        let _process = ProcessResult::decode_outcome(&bytes);
    }

    #[test]
    fn valid_paths_roundtrip_through_their_display_form(
        components in proptest::collection::vec("[a-zA-Z0-9 _.-]{1,12}", 0..6)
    ) {
        let components = components
            .into_iter()
            .filter(|component| component != "." && component != "..")
            .collect::<Vec<_>>();
        let text = format!("docs:/{}", components.join("/"));
        let parsed = RemotePath::parse(&text).expect("valid generated path");
        prop_assert_eq!(parsed.components(), components.as_slice());
        prop_assert_eq!(RemotePath::parse(&parsed.to_string()).expect("display"), parsed);
    }
}

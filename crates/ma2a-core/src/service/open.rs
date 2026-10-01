//! The canonical `open` header shared by every built-in streaming service.

use crate::{EndpointId, ProtocolError, RequestId, ServiceKind, space_codec::Decoder};

use super::{
    ExecRequest, FilesRequest, MAX_HEADER_FRAME_LEN, ProcessRequest, ProxyRequest, ServiceAccess,
    ShellRequest, TransferRequest, cbor::Writer,
};

/// `accept` payload of services whose admission carries no data: an empty CBOR array.
pub const EMPTY_ACCEPT: [u8; 1] = [0x80];

/// The typed request body of one stream, discriminated by service.
#[expect(
    clippy::exhaustive_enums,
    reason = "the streaming service set is closed and dispatched exhaustively"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenBody {
    /// A Files operation.
    Files(FilesRequest),
    /// A Transfer stream.
    Transfer(TransferRequest),
    /// A one-shot Exec.
    Exec(ExecRequest),
    /// An interactive Shell.
    Shell(ShellRequest),
    /// A TCP Proxy stream.
    Proxy(ProxyRequest),
    /// A durable Process operation.
    Process(ProcessRequest),
}

impl OpenBody {
    /// Returns the service this body belongs to.
    pub const fn kind(&self) -> ServiceKind {
        match self {
            Self::Files(_) => ServiceKind::Files,
            Self::Transfer(_) => ServiceKind::Transfer,
            Self::Exec(_) => ServiceKind::Exec,
            Self::Shell(_) => ServiceKind::Shell,
            Self::Proxy(_) => ServiceKind::Proxy,
            Self::Process(_) => ServiceKind::Process,
        }
    }

    /// Returns the complete resource access evaluated after the header is decoded.
    pub fn access(&self) -> ServiceAccess {
        match self {
            Self::Files(request) => request.access(),
            Self::Transfer(request) => request.access(),
            Self::Exec(_) => ServiceAccess::Exec,
            Self::Shell(_) => ServiceAccess::Shell,
            Self::Proxy(request) => request.access(),
            Self::Process(request) => request.access(),
        }
    }

    /// Returns whether this request can change target state and therefore uses durable replay.
    pub const fn is_mutation(&self) -> bool {
        match self {
            Self::Files(request) => request.is_mutation(),
            Self::Transfer(request) => request.is_mutation(),
            Self::Process(request) => request.is_mutation(),
            Self::Exec(_) | Self::Shell(_) | Self::Proxy(_) => true,
        }
    }
}

/// Returns the coarse admission evaluated before any byte of a stream is read.
pub const fn admission(kind: ServiceKind) -> Option<ServiceAccess> {
    match kind {
        ServiceKind::Echo => None,
        ServiceKind::Files | ServiceKind::Transfer => Some(ServiceAccess::FilesAdmission),
        ServiceKind::Exec => Some(ServiceAccess::Exec),
        ServiceKind::Shell => Some(ServiceAccess::Shell),
        ServiceKind::Proxy => Some(ServiceAccess::ProxyAdmission),
        ServiceKind::Process => Some(ServiceAccess::Process),
    }
}

/// One Endpoint-targeted stream request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenHeader {
    request_id: RequestId,
    target: EndpointId,
    body: OpenBody,
}

impl OpenHeader {
    /// Creates a header without any Space selector.
    pub const fn new(request_id: RequestId, target: EndpointId, body: OpenBody) -> Self {
        Self {
            request_id,
            target,
            body,
        }
    }

    /// Returns the request correlation and replay identifier.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Returns the exact target Endpoint.
    pub const fn target(&self) -> EndpointId {
        self.target
    }

    /// Returns the typed body.
    pub const fn body(&self) -> &OpenBody {
        &self.body
    }

    /// Consumes the header into its body.
    pub fn into_body(self) -> OpenBody {
        self.body
    }

    /// Encodes exact canonical bytes: `{0: [1, 0], 1: id, 2: target, 3: service, 4: body}`.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the header exceeds its frame bound.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        writer
            .map(5)?
            .uint(0)
            .array(2)?
            .uint(1)
            .uint(0)
            .uint(1)
            .bytes(self.request_id.as_bytes())?
            .uint(2)
            .bytes(self.target.as_bytes())?
            .uint(3)
            .uint(u64::from(self.body.kind().code()))
            .uint(4);
        match &self.body {
            OpenBody::Files(request) => request.encode(&mut writer)?,
            OpenBody::Transfer(request) => request.encode(&mut writer)?,
            OpenBody::Exec(request) => request.encode(&mut writer)?,
            OpenBody::Shell(request) => request.encode(&mut writer)?,
            OpenBody::Proxy(request) => request.encode(&mut writer)?,
            OpenBody::Process(request) => request.encode(&mut writer)?,
        }
        writer.finish(MAX_HEADER_FRAME_LEN)
    }

    /// Decodes exact canonical bytes.
    ///
    /// # Errors
    /// Returns [`ProtocolError::VERSION_MISMATCH`] for any version other than 1.0, otherwise
    /// [`ProtocolError::INVALID_INPUT`] for malformed, noncanonical, or out-of-bound input.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_HEADER_FRAME_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut decoder = Decoder::new(bytes);
        decoder.map(5)?;
        decoder.key(0)?;
        decoder.array_exact(2)?;
        if (decoder.uint()?, decoder.uint()?) != (1, 0) {
            return Err(ProtocolError::VERSION_MISMATCH);
        }
        decoder.key(1)?;
        let request_id = RequestId::try_from(decoder.bytes(16)?)?;
        decoder.key(2)?;
        let target = EndpointId::try_from(decoder.bytes(32)?)?;
        decoder.key(3)?;
        let code = u8::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        decoder.key(4)?;
        let body = match ServiceKind::from_code(code) {
            Some(ServiceKind::Files) => OpenBody::Files(FilesRequest::decode(&mut decoder)?),
            Some(ServiceKind::Transfer) => {
                OpenBody::Transfer(TransferRequest::decode(&mut decoder)?)
            }
            Some(ServiceKind::Exec) => OpenBody::Exec(ExecRequest::decode(&mut decoder)?),
            Some(ServiceKind::Shell) => OpenBody::Shell(ShellRequest::decode(&mut decoder)?),
            Some(ServiceKind::Proxy) => OpenBody::Proxy(ProxyRequest::decode(&mut decoder)?),
            Some(ServiceKind::Process) => OpenBody::Process(ProcessRequest::decode(&mut decoder)?),
            Some(ServiceKind::Echo) | None => return Err(ProtocolError::INVALID_INPUT),
        };
        decoder.finish()?;
        Ok(Self::new(request_id, target, body))
    }
}

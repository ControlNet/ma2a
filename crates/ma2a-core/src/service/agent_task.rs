//! The durable view of Agent tasks, their artifacts, and their events.

use crate::{EndpointId, ProtocolError, RequestId, space_codec::Decoder};

use super::{
    AgentId, AgentMessage, AgentTaskId, AgentTaskReason, AgentTaskState,
    cbor::{self, Writer},
};

/// Longest artifact name in bytes.
pub const MAX_ARTIFACT_NAME_LEN: usize = 128;
/// Longest artifact media type in bytes.
pub const MAX_MEDIA_TYPE_LEN: usize = 127;
/// Most artifacts one task may publish.
pub const MAX_TASK_ARTIFACTS: usize = 16;
/// Longest error or progress text in bytes.
pub const MAX_TASK_NOTE_LEN: usize = 1_024;

/// One immutable, content-addressed artifact a task published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRef {
    name: String,
    media_type: String,
    size: u64,
    blake3: [u8; 32],
}

impl ArtifactRef {
    /// Validates one artifact reference.
    ///
    /// The name is 1–128 bytes of `[A-Za-z0-9._-]` not starting with `.`; the media type
    /// is printable ASCII of the form `type/subtype`.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for any other name or media type.
    pub fn new(
        (name, media_type): (&str, &str),
        (size, blake3): (u64, [u8; 32]),
    ) -> Result<Self, ProtocolError> {
        let name_ok = !name.is_empty()
            && name.len() <= MAX_ARTIFACT_NAME_LEN
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        let media_ok = media_type.len() <= MAX_MEDIA_TYPE_LEN
            && media_type.bytes().all(|byte| byte.is_ascii_graphic())
            && media_type
                .split_once('/')
                .is_some_and(|(kind, sub)| !kind.is_empty() && !sub.is_empty());
        if name_ok && media_ok {
            Ok(Self {
                name: name.to_owned(),
                media_type: media_type.to_owned(),
                size,
                blake3,
            })
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    /// Returns the task-local name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the media type.
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Returns the content size in bytes.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns the BLAKE3 hash of the content.
    pub const fn blake3(&self) -> &[u8; 32] {
        &self.blake3
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(4)?
            .text(&self.name)?
            .text(&self.media_type)?
            .uint(self.size)
            .bytes(&self.blake3)?;
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(4)?;
        let name = decoder.text(MAX_ARTIFACT_NAME_LEN)?;
        let media_type = decoder.text(MAX_MEDIA_TYPE_LEN)?;
        let size = decoder.uint()?;
        Self::new((name, media_type), (size, cbor::hash(decoder)?))
    }
}

/// The content-free view of one task.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTaskSummary {
    /// Task identifier.
    pub id: AgentTaskId,
    /// The Agent the task was submitted to.
    pub agent: AgentId,
    /// The Endpoint that submitted it; the hosting Endpoint for local tasks.
    pub creator: EndpointId,
    /// The submitting Agent, recorded for attribution only.
    pub from_agent: Option<AgentId>,
    /// `RequestId` of the `submit` that created it.
    pub request_id: RequestId,
    /// Lifecycle state.
    pub state: AgentTaskState,
    /// Why it failed or was cancelled.
    pub reason: Option<AgentTaskReason>,
    /// Creation, start, and end times in Unix milliseconds.
    pub times: (u64, Option<u64>, Option<u64>),
    /// When the state last changed, in Unix milliseconds: the creation time while queued.
    pub changed_ms: u64,
    /// The sequence number of the newest event.
    pub last_seq: u32,
}

impl AgentTaskSummary {
    /// Creates a summary of a just-queued task.
    pub fn new(
        (id, agent): (AgentTaskId, AgentId),
        (creator, request_id): (EndpointId, RequestId),
        created_ms: u64,
    ) -> Self {
        Self {
            id,
            agent,
            creator,
            from_agent: None,
            request_id,
            state: AgentTaskState::Queued,
            reason: None,
            times: (created_ms, None, None),
            changed_ms: created_ms,
            last_seq: 0,
        }
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(12)?
            .bytes(self.id.as_bytes())?
            .text(self.agent.as_str())?
            .bytes(self.creator.as_bytes())?
            .optional_text(self.from_agent.as_ref().map(AgentId::as_str))?
            .bytes(self.request_id.as_bytes())?
            .uint(u64::from(self.state.code()))
            .optional_uint(self.reason.map(|reason| u64::from(reason.code())))
            .uint(self.times.0)
            .optional_uint(self.times.1)
            .optional_uint(self.times.2)
            .uint(self.changed_ms)
            .uint(u64::from(self.last_seq));
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(12)?;
        let id = AgentTaskId::decode_from(decoder)?;
        let agent = AgentId::decode_from(decoder)?;
        let creator = EndpointId::try_from(decoder.bytes(32)?)?;
        let from_agent = if decoder.null() {
            None
        } else {
            Some(AgentId::decode_from(decoder)?)
        };
        let request_id = RequestId::try_from(decoder.bytes(16)?)?;
        let state = AgentTaskState::from_code(decoder.uint()?)?;
        let reason = cbor::optional_uint(decoder)?
            .map(AgentTaskReason::from_code)
            .transpose()?;
        let times = (
            decoder.uint()?,
            cbor::optional_uint(decoder)?,
            cbor::optional_uint(decoder)?,
        );
        let changed_ms = decoder.uint()?;
        let last_seq = cbor::u32_value(decoder)?;
        Ok(Self {
            id,
            agent,
            creator,
            from_agent,
            request_id,
            state,
            reason,
            times,
            changed_ms,
            last_seq,
        })
    }
}

/// Everything one task holds: its input, its result, and its artifacts.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTaskDetails {
    /// The summary.
    pub summary: AgentTaskSummary,
    /// The submitted message.
    pub input: AgentMessage,
    /// The small inline result of a completed task.
    pub result: Option<AgentMessage>,
    /// The failure the Agent reported.
    pub error: Option<String>,
    /// The newest progress text.
    pub progress: Option<String>,
    /// Published artifacts in publication order.
    pub artifacts: Vec<ArtifactRef>,
}

impl AgentTaskDetails {
    /// Creates the details of a task that has no result yet.
    pub const fn new(summary: AgentTaskSummary, input: AgentMessage) -> Self {
        Self {
            summary,
            input,
            result: None,
            error: None,
            progress: None,
            artifacts: Vec::new(),
        }
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer.array(6)?;
        self.summary.encode(writer)?;
        self.input.encode(writer)?;
        match &self.result {
            Some(result) => result.encode(writer)?,
            None => {
                writer.null();
            }
        }
        writer
            .optional_text(self.error.as_deref())?
            .optional_text(self.progress.as_deref())?
            .array(self.artifacts.len())?;
        for artifact in &self.artifacts {
            artifact.encode(writer)?;
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(6)?;
        let summary = AgentTaskSummary::decode(decoder)?;
        let input = AgentMessage::decode(decoder)?;
        let result = if decoder.null() {
            None
        } else {
            Some(AgentMessage::decode(decoder)?)
        };
        let note = |decoder: &mut Decoder<'_>| {
            decoder
                .optional_text(MAX_TASK_NOTE_LEN)
                .map(|text| text.map(str::to_owned))
        };
        let (error, progress) = (note(decoder)?, note(decoder)?);
        let count = decoder.array(MAX_TASK_ARTIFACTS)?;
        let artifacts = (0..count)
            .map(|_| ArtifactRef::decode(decoder))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            summary,
            input,
            result,
            error,
            progress,
            artifacts,
        })
    }
}

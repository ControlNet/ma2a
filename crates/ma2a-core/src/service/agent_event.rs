//! Agent cards and the durable per-task event log.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    AgentId, AgentMessage, AgentTaskReason, AgentTaskState, ArtifactRef, MAX_DATA_CHUNK_LEN,
    MAX_TASK_NOTE_LEN,
    cbor::{self, Writer},
};

/// Longest Agent description in bytes.
pub const MAX_AGENT_DESCRIPTION_LEN: usize = 256;
/// Most concurrent tasks one Agent instance may hold.
pub const MAX_AGENT_TASKS: u8 = 16;

/// What a caller may learn about one Agent it is authorized to use.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCard {
    /// The Agent's name on its hosting Endpoint.
    pub id: AgentId,
    /// Operator-written description; may be empty.
    pub description: String,
    /// Whether the operator has the Agent started, so a `submit` is accepted.
    pub available: bool,
    /// Most tasks the Agent works on at once.
    pub max_tasks: u8,
}

impl AgentCard {
    /// Validates one card.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for a description over its bound or with a
    /// NUL, or `max_tasks` outside 1–16.
    pub fn new(
        id: AgentId,
        description: &str,
        (available, max_tasks): (bool, u8),
    ) -> Result<Self, ProtocolError> {
        if description.len() > MAX_AGENT_DESCRIPTION_LEN
            || description.contains('\0')
            || !(1..=MAX_AGENT_TASKS).contains(&max_tasks)
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            id,
            description: description.to_owned(),
            available,
            max_tasks,
        })
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(4)?
            .text(self.id.as_str())?
            .text(&self.description)?
            .boolean(self.available)
            .uint(u64::from(self.max_tasks));
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(4)?;
        let id = AgentId::decode_from(decoder)?;
        let description = decoder
            .text_without_nul(MAX_AGENT_DESCRIPTION_LEN)?
            .to_owned();
        let available = decoder.boolean()?;
        let max_tasks = u8::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Self::new(id, &description, (available, max_tasks))
    }
}

/// What one event records.
#[expect(
    clippy::exhaustive_enums,
    reason = "Agent task events are a closed, durable vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEventBody {
    /// The task moved to this state, with its end reason.
    State(AgentTaskState, Option<AgentTaskReason>),
    /// A message from the submitter or the Agent.
    Message(AgentMessage),
    /// Free-form progress text; only the newest progress events are retained.
    Progress(String),
    /// The task published an artifact.
    Artifact(ArtifactRef),
}

impl AgentEventBody {
    /// Returns whether the event log may prune this event.
    pub const fn is_progress(&self) -> bool {
        matches!(self, Self::Progress(_))
    }

    /// Validates progress text.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for text over its bound or with a NUL.
    pub fn progress(text: &str) -> Result<Self, ProtocolError> {
        if text.len() > MAX_TASK_NOTE_LEN || text.contains('\0') {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self::Progress(text.to_owned()))
    }
}

/// One event of a task's log; `seq` starts at 1 and increases by one per event.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEvent {
    /// Position in the task's log.
    pub seq: u32,
    /// When it was recorded, in Unix milliseconds.
    pub at_ms: u64,
    /// What happened.
    pub body: AgentEventBody,
}

impl AgentEvent {
    /// Creates one event.
    pub const fn new(seq: u32, at_ms: u64, body: AgentEventBody) -> Self {
        Self { seq, at_ms, body }
    }

    /// Encodes `[seq, at_ms, kind, …]` as one watch `data` payload or one Store row.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] only if the bounded encoder fails.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        match &self.body {
            AgentEventBody::State(state, reason) => {
                writer
                    .array(5)?
                    .uint(u64::from(self.seq))
                    .uint(self.at_ms)
                    .uint(0)
                    .uint(u64::from(state.code()))
                    .optional_uint(reason.map(|reason| u64::from(reason.code())));
            }
            AgentEventBody::Message(message) => {
                writer
                    .array(4)?
                    .uint(u64::from(self.seq))
                    .uint(self.at_ms)
                    .uint(1);
                message.encode(&mut writer)?;
            }
            AgentEventBody::Progress(text) => {
                writer
                    .array(4)?
                    .uint(u64::from(self.seq))
                    .uint(self.at_ms)
                    .uint(2)
                    .text(text)?;
            }
            AgentEventBody::Artifact(artifact) => {
                writer
                    .array(4)?
                    .uint(u64::from(self.seq))
                    .uint(self.at_ms)
                    .uint(3);
                artifact.encode(&mut writer)?;
            }
        }
        writer.finish(MAX_DATA_CHUNK_LEN)
    }

    /// Decodes exact canonical bytes written by [`Self::to_bytes`].
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        let length = decoder.array(5)?;
        let seq = cbor::u32_value(&mut decoder)?;
        let at_ms = decoder.uint()?;
        let body = match (length, decoder.uint()?) {
            (5, 0) => AgentEventBody::State(
                AgentTaskState::from_code(decoder.uint()?)?,
                cbor::optional_uint(&mut decoder)?
                    .map(AgentTaskReason::from_code)
                    .transpose()?,
            ),
            (4, 1) => AgentEventBody::Message(AgentMessage::decode(&mut decoder)?),
            (4, 2) => AgentEventBody::progress(decoder.text_without_nul(MAX_TASK_NOTE_LEN)?)?,
            (4, 3) => AgentEventBody::Artifact(ArtifactRef::decode(&mut decoder)?),
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        decoder.finish()?;
        if seq == 0 {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self::new(seq, at_ms, body))
    }
}

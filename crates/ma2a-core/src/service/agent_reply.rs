//! Agent v1 `accept` and terminal `outcome` payloads.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    AgentCard, AgentId, AgentTaskDetails, AgentTaskId, AgentTaskState, AgentTaskSummary,
    MAX_AGENT_PAGE, MAX_HEADER_FRAME_LEN, MAX_TASK_PAGE, ServiceError,
    cbor::{self, Writer},
};

/// What an Agent operation reports when it is accepted.
#[expect(
    clippy::exhaustive_enums,
    reason = "Agent accept forms are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentAccept {
    /// `submit` is bound to this task; `true` when an earlier attempt already created it.
    Submitted(AgentTaskId, bool),
    /// `watch` begins: the task state now and the sequence number of the next event.
    Watching(AgentTaskState, u32),
    /// Any other operation: nothing beyond admission to report.
    Admitted,
}

impl AgentAccept {
    /// Encodes one `accept` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
    pub fn encode(self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        match self {
            Self::Submitted(task, existing) => {
                writer
                    .array(3)?
                    .uint(0)
                    .bytes(task.as_bytes())?
                    .boolean(existing);
            }
            Self::Watching(state, next) => {
                writer
                    .array(3)?
                    .uint(1)
                    .uint(u64::from(state.code()))
                    .uint(u64::from(next));
            }
            Self::Admitted => {
                writer.array(1)?.uint(2);
            }
        }
        writer.finish(MAX_HEADER_FRAME_LEN)
    }

    /// Decodes one `accept` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        let accept = match (decoder.array(3)?, decoder.uint()?) {
            (3, 0) => Self::Submitted(AgentTaskId::decode_from(&mut decoder)?, decoder.boolean()?),
            (3, 1) => Self::Watching(
                AgentTaskState::from_code(decoder.uint()?)?,
                cbor::u32_value(&mut decoder)?,
            ),
            (1, 2) => Self::Admitted,
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        decoder.finish()?;
        Ok(accept)
    }
}

/// Successful terminal result of one Agent operation.
#[expect(
    clippy::exhaustive_enums,
    reason = "Agent results are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentReply {
    /// `agents`: one page and the name to continue after, if more exist.
    Cards(Vec<AgentCard>, Option<AgentId>),
    /// `submit`, `send`, `cancel`, or the end of `watch`: the task as it now is.
    Summary(AgentTaskSummary),
    /// `get`: the full view.
    Details(Box<AgentTaskDetails>),
    /// `tasks`: one page and the identifier to continue after, if more exist.
    Page(Vec<AgentTaskSummary>, Option<AgentTaskId>),
}

/// Terminal Agent outcome.
pub type AgentReplyOutcome = Result<AgentReply, ServiceError>;

impl AgentReply {
    /// Encodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the outcome exceeds its bounds.
    pub fn encode_outcome(outcome: &AgentReplyOutcome) -> Result<Vec<u8>, ProtocolError> {
        cbor::encode_outcome(outcome.as_ref().map_err(|error| *error), Self::encode)
    }

    /// Decodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_outcome(bytes: &[u8]) -> Result<AgentReplyOutcome, ProtocolError> {
        cbor::decode_outcome(bytes, Self::decode)
    }

    fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Cards(cards, next) => {
                writer.array(3)?.uint(0).array(cards.len())?;
                for card in cards {
                    card.encode(writer)?;
                }
                writer.optional_text(next.as_ref().map(AgentId::as_str))?;
            }
            Self::Summary(summary) => {
                writer.array(2)?.uint(1);
                summary.encode(writer)?;
            }
            Self::Details(details) => {
                writer.array(2)?.uint(2);
                details.encode(writer)?;
            }
            Self::Page(tasks, next) => {
                writer.array(3)?.uint(3).array(tasks.len())?;
                for task in tasks {
                    task.encode(writer)?;
                }
                match next {
                    Some(task) => writer.bytes(task.as_bytes())?,
                    None => writer.null(),
                };
            }
        }
        Ok(())
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let reply = match (decoder.array(3)?, decoder.uint()?) {
            (3, 0) => {
                let count = decoder.array(MAX_AGENT_PAGE)?;
                let cards = (0..count)
                    .map(|_| AgentCard::decode(decoder))
                    .collect::<Result<_, _>>()?;
                let next = if decoder.null() {
                    None
                } else {
                    Some(AgentId::decode_from(decoder)?)
                };
                Self::Cards(cards, next)
            }
            (2, 1) => Self::Summary(AgentTaskSummary::decode(decoder)?),
            (2, 2) => Self::Details(Box::new(AgentTaskDetails::decode(decoder)?)),
            (3, 3) => {
                let count = decoder.array(MAX_TASK_PAGE)?;
                let tasks = (0..count)
                    .map(|_| AgentTaskSummary::decode(decoder))
                    .collect::<Result<_, _>>()?;
                let next = if decoder.null() {
                    None
                } else {
                    Some(AgentTaskId::decode_from(decoder)?)
                };
                Self::Page(tasks, next)
            }
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        Ok(reply)
    }
}

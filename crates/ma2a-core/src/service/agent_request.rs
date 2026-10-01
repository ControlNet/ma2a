//! Agent v1 requests: tasks submitted to one Agent that a target Endpoint hosts.
//!
//! Every task operation names its Agent: routing is caller Endpoint → target Endpoint →
//! target Agent, and authorization is decided for that exact Agent.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    AgentId, AgentMessage, AgentTaskId, ContentIdentity, MAX_ARTIFACT_NAME_LEN, ServiceAccess,
    cbor::Writer,
};

/// Most Agent cards one `agents` page returns.
pub const MAX_AGENT_PAGE: usize = 16;
/// Most tasks one `tasks` page returns.
pub const MAX_TASK_PAGE: usize = 32;

/// One task submission; its canonical bytes are what a retry must repeat exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSubmit {
    agent: AgentId,
    message: AgentMessage,
    from_agent: Option<AgentId>,
}

impl AgentSubmit {
    /// Creates a submission; `from_agent` is attribution only and never authorizes.
    pub const fn new(agent: AgentId, message: AgentMessage, from_agent: Option<AgentId>) -> Self {
        Self {
            agent,
            message,
            from_agent,
        }
    }

    /// Returns the target Agent.
    pub const fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// Returns the task input.
    pub const fn message(&self) -> &AgentMessage {
        &self.message
    }

    /// Returns the submitting Agent the caller claims.
    pub const fn from_agent(&self) -> Option<&AgentId> {
        self.from_agent.as_ref()
    }

    fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer.array(3)?.text(self.agent.as_str())?;
        self.message.encode(writer)?;
        writer.optional_text(self.from_agent.as_ref().map(AgentId::as_str))?;
        Ok(())
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(3)?;
        let agent = AgentId::decode_from(decoder)?;
        let message = AgentMessage::decode(decoder)?;
        let from_agent = if decoder.null() {
            None
        } else {
            Some(AgentId::decode_from(decoder)?)
        };
        Ok(Self::new(agent, message, from_agent))
    }

    /// Encodes the exact canonical bytes the target fingerprints.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the submission exceeds its bound.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        self.encode(&mut writer)?;
        writer.finish(super::MAX_HEADER_FRAME_LEN)
    }
}

/// One Agent operation.
#[expect(
    clippy::exhaustive_enums,
    reason = "Agent operations are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRequest {
    /// One page of the Agents this caller may use, after this name.
    Agents(Option<AgentId>),
    /// Create one task; the `RequestId` binds exactly one task.
    Submit(AgentSubmit),
    /// Send one more message to a live task.
    Send(AgentId, AgentTaskId, AgentMessage),
    /// Return one task in full.
    Get(AgentId, AgentTaskId),
    /// One page of the caller's tasks on this Agent, after this identifier.
    Tasks(AgentId, Option<AgentTaskId>),
    /// Stream every retained event after this sequence number until the task ends.
    Watch(AgentId, AgentTaskId, u32),
    /// Cancel a task; answers once it ended.
    Cancel(AgentId, AgentTaskId),
    /// Download one artifact from an offset, as a Transfer v1 download.
    Artifact(AgentId, AgentTaskId, String, u64, Option<ContentIdentity>),
}

impl AgentRequest {
    /// Returns the complete access this operation needs.
    pub fn access(&self) -> ServiceAccess {
        self.agent().map_or(ServiceAccess::AgentAdmission, |agent| {
            ServiceAccess::Agent(agent.clone())
        })
    }

    /// Returns the Agent this operation addresses, if it addresses one.
    pub const fn agent(&self) -> Option<&AgentId> {
        match self {
            Self::Agents(_) => None,
            Self::Submit(submit) => Some(&submit.agent),
            Self::Send(agent, ..)
            | Self::Get(agent, _)
            | Self::Tasks(agent, _)
            | Self::Watch(agent, ..)
            | Self::Cancel(agent, _)
            | Self::Artifact(agent, ..) => Some(agent),
        }
    }

    /// Returns whether this operation can change target state.
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Submit(_) | Self::Send(..) | Self::Cancel(..))
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Agents(after) => {
                writer
                    .array(2)?
                    .uint(0)
                    .optional_text(after.as_ref().map(AgentId::as_str))?;
            }
            Self::Submit(submit) => {
                writer.array(2)?.uint(1);
                submit.encode(writer)?;
            }
            Self::Send(agent, task, message) => {
                task_then(writer, (4, 2), (agent, *task))?;
                message.encode(writer)?;
            }
            Self::Get(agent, task) => {
                task_then(writer, (3, 3), (agent, *task))?;
            }
            Self::Tasks(agent, after) => {
                writer.array(3)?.uint(4).text(agent.as_str())?;
                match after {
                    Some(task) => writer.bytes(task.as_bytes())?,
                    None => writer.null(),
                };
            }
            Self::Watch(agent, task, after) => {
                task_then(writer, (4, 5), (agent, *task))?.uint(u64::from(*after));
            }
            Self::Cancel(agent, task) => {
                task_then(writer, (3, 6), (agent, *task))?;
            }
            Self::Artifact(agent, task, name, offset, identity) => {
                task_then(writer, (6, 7), (agent, *task))?
                    .text(name)?
                    .uint(*offset);
                match identity {
                    Some(identity) => identity.encode(writer)?,
                    None => {
                        writer.null();
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let length = decoder.array(6)?;
        let tag = decoder.uint()?;
        let request = match (length, tag) {
            (2, 0) => Self::Agents(if decoder.null() {
                None
            } else {
                Some(AgentId::decode_from(decoder)?)
            }),
            (2, 1) => Self::Submit(AgentSubmit::decode(decoder)?),
            (4, 2) => {
                let (agent, task) = agent_task(decoder)?;
                Self::Send(agent, task, AgentMessage::decode(decoder)?)
            }
            (3, 3) => {
                let (agent, task) = agent_task(decoder)?;
                Self::Get(agent, task)
            }
            (3, 4) => {
                let agent = AgentId::decode_from(decoder)?;
                let after = if decoder.null() {
                    None
                } else {
                    Some(AgentTaskId::decode_from(decoder)?)
                };
                Self::Tasks(agent, after)
            }
            (4, 5) => {
                let (agent, task) = agent_task(decoder)?;
                Self::Watch(agent, task, super::cbor::u32_value(decoder)?)
            }
            (3, 6) => {
                let (agent, task) = agent_task(decoder)?;
                Self::Cancel(agent, task)
            }
            (6, 7) => {
                let (agent, task) = agent_task(decoder)?;
                let name = decoder.text(MAX_ARTIFACT_NAME_LEN)?.to_owned();
                let offset = decoder.uint()?;
                let identity = if decoder.null() {
                    None
                } else {
                    Some(ContentIdentity::decode(decoder)?)
                };
                Self::Artifact(agent, task, name, offset, identity)
            }
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        Ok(request)
    }
}

fn task_then<'a>(
    writer: &'a mut Writer,
    (length, tag): (usize, u64),
    (agent, task): (&AgentId, AgentTaskId),
) -> Result<&'a mut Writer, ProtocolError> {
    writer
        .array(length)?
        .uint(tag)
        .text(agent.as_str())?
        .bytes(task.as_bytes())
}

fn agent_task(decoder: &mut Decoder<'_>) -> Result<(AgentId, AgentTaskId), ProtocolError> {
    Ok((
        AgentId::decode_from(decoder)?,
        AgentTaskId::decode_from(decoder)?,
    ))
}

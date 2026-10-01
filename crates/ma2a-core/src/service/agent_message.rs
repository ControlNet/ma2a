//! Agent messages: small, typed, inline content. Large payloads are artifacts.

use crate::{ProtocolError, space_codec::Decoder};

use super::cbor::Writer;

/// Most parts in one message.
pub const MAX_MESSAGE_PARTS: usize = 16;
/// Most bytes of part content in one message, all parts together.
///
/// One message must fit inside a `submit` header frame with room to spare.
pub const MAX_MESSAGE_BYTES: usize = 12_288;

/// Who wrote a message.
#[expect(
    clippy::exhaustive_enums,
    reason = "message roles are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRole {
    /// The task's submitter.
    User,
    /// The Agent working on the task.
    Agent,
}

impl MessageRole {
    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }

    /// Returns the role with this name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// One part of a message.
#[expect(
    clippy::exhaustive_enums,
    reason = "message parts are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagePart {
    /// Plain UTF-8 text.
    Text(String),
    /// Structured data as JSON text; the hosting Runtime validates it before accepting it.
    Data(String),
}

impl MessagePart {
    /// Returns the content of this part.
    pub fn content(&self) -> &str {
        match self {
            Self::Text(text) | Self::Data(text) => text,
        }
    }
}

/// One bounded message: a role and 1–16 parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessage {
    role: MessageRole,
    parts: Vec<MessagePart>,
}

impl AgentMessage {
    /// Validates one message.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for no parts, too many parts, content over
    /// [`MAX_MESSAGE_BYTES`], or a NUL character.
    pub fn new(role: MessageRole, parts: Vec<MessagePart>) -> Result<Self, ProtocolError> {
        let total: usize = parts.iter().map(|part| part.content().len()).sum();
        let valid = !parts.is_empty()
            && parts.len() <= MAX_MESSAGE_PARTS
            && total <= MAX_MESSAGE_BYTES
            && parts.iter().all(|part| !part.content().contains('\0'));
        if valid {
            Ok(Self { role, parts })
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    /// Creates a message of one text part.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the text is out of bounds.
    pub fn text(role: MessageRole, text: &str) -> Result<Self, ProtocolError> {
        Self::new(role, vec![MessagePart::Text(text.to_owned())])
    }

    /// Returns who wrote it.
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// Returns its parts in order.
    pub fn parts(&self) -> &[MessagePart] {
        &self.parts
    }

    /// Returns every text part joined by newlines.
    pub fn joined_text(&self) -> String {
        let texts: Vec<&str> = self
            .parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Text(text) => Some(text.as_str()),
                MessagePart::Data(_) => None,
            })
            .collect();
        texts.join("\n")
    }

    /// Encodes `[role, [[kind, content]…]]`; kind 0 is text and 1 is data.
    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(2)?
            .text(self.role.name())?
            .array(self.parts.len())?;
        for part in &self.parts {
            let kind = match part {
                MessagePart::Text(_) => 0,
                MessagePart::Data(_) => 1,
            };
            writer.array(2)?.uint(kind).text(part.content())?;
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(2)?;
        let role = MessageRole::from_name(decoder.text(8)?).ok_or(ProtocolError::INVALID_INPUT)?;
        let count = decoder.array(MAX_MESSAGE_PARTS)?;
        let mut parts = Vec::with_capacity(count);
        for _ in 0..count {
            decoder.array_exact(2)?;
            let kind = decoder.uint()?;
            let content = decoder.text_without_nul(MAX_MESSAGE_BYTES)?.to_owned();
            parts.push(match kind {
                0 => MessagePart::Text(content),
                1 => MessagePart::Data(content),
                _ => return Err(ProtocolError::INVALID_INPUT),
            });
        }
        Self::new(role, parts)
    }

    /// Encodes this message alone as canonical bytes, as the Store keeps it.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] only if the bounded encoder fails.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        self.encode(&mut writer)?;
        writer.finish(super::MAX_HEADER_FRAME_LEN)
    }

    /// Decodes exact canonical bytes written by [`Self::to_bytes`].
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        let message = Self::decode(&mut decoder)?;
        decoder.finish()?;
        Ok(message)
    }
}

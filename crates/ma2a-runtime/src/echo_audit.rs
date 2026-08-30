//! Payload-free bounded Echo audit records.

use std::{collections::VecDeque, sync::Arc};

use ma2a_core::{
    EchoError, EchoResultClass, EndpointId, MAX_ECHO_DURATION_MS, MAX_ECHO_PAYLOAD_LEN, RequestId,
};

const AUDIT_CAPACITY: usize = 128;

#[derive(Clone, Debug, Default)]
pub(crate) struct EchoAuditLog(Arc<std::sync::Mutex<VecDeque<EchoAuditRecord>>>);

impl EchoAuditLog {
    pub(crate) fn record(&self, record: EchoAuditRecord) {
        let mut records = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if records.len() == AUDIT_CAPACITY {
            records.pop_front();
        }
        records.push_back(record);
    }

    pub(crate) fn snapshot(&self) -> Vec<EchoAuditRecord> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }
}

/// One bounded Echo audit record that cannot retain request or response payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EchoAuditRecord {
    request_id: RequestId,
    peer_endpoint_id: EndpointId,
    result_class: EchoResultClass,
    byte_count: u16,
    duration_ms: u16,
}

impl EchoAuditRecord {
    /// Creates one record after enforcing Echo payload and duration bounds.
    ///
    /// # Errors
    /// Returns [`EchoError::InvalidInput`] when byte-count or duration bounds are exceeded.
    #[expect(
        clippy::too_many_arguments,
        reason = "the audit record stores five independent bounded evidence fields"
    )]
    pub fn new(
        request_id: RequestId,
        peer_endpoint_id: EndpointId,
        result_class: EchoResultClass,
        byte_count: usize,
        duration_ms: u16,
    ) -> Result<Self, EchoError> {
        if byte_count > MAX_ECHO_PAYLOAD_LEN || duration_ms > MAX_ECHO_DURATION_MS {
            return Err(EchoError::InvalidInput);
        }
        Ok(Self {
            request_id,
            peer_endpoint_id,
            result_class,
            byte_count: u16::try_from(byte_count).map_err(|_| EchoError::InvalidInput)?,
            duration_ms,
        })
    }

    /// Returns the request correlation identifier.
    pub const fn request_id(self) -> RequestId {
        self.request_id
    }

    /// Returns the authenticated peer Endpoint identifier.
    pub const fn peer_endpoint_id(self) -> EndpointId {
        self.peer_endpoint_id
    }

    /// Returns the bounded result classification.
    pub const fn result_class(self) -> EchoResultClass {
        self.result_class
    }

    /// Returns the payload byte count without retaining payload bytes.
    pub const fn byte_count(self) -> u16 {
        self.byte_count
    }

    /// Returns the bounded duration in milliseconds.
    pub const fn duration_ms(self) -> u16 {
        self.duration_ms
    }
}

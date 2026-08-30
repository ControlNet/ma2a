use ma2a_core::{
    ControlArtifactKind, ControlArtifactV1, MAX_CONTROL_ARTIFACTS_PER_PAGE, MAX_CONTROL_BATCH_BYTES,
};
use ma2a_net::ControlRejection;

use crate::error::{RuntimeError, RuntimeErrorKind};

const ARTIFACT_OVERHEAD: usize = 5;

pub(super) struct PageBuilder {
    pub(super) artifacts: Vec<ControlArtifactV1>,
    bytes: usize,
}

impl PageBuilder {
    pub(super) const fn new() -> Self {
        Self {
            artifacts: Vec::new(),
            bytes: 0,
        }
    }

    pub(super) fn add(
        &mut self,
        kind: ControlArtifactKind,
        signed_bytes: &[u8],
    ) -> Result<bool, RuntimeError> {
        self.add_control(kind, signed_bytes)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
    }

    pub(super) fn add_control(
        &mut self,
        kind: ControlArtifactKind,
        signed_bytes: &[u8],
    ) -> Result<bool, ControlRejection> {
        let next = self
            .bytes
            .checked_add(signed_bytes.len())
            .and_then(|value| value.checked_add(ARTIFACT_OVERHEAD))
            .ok_or(ControlRejection::Invalid)?;
        if self.artifacts.len() == MAX_CONTROL_ARTIFACTS_PER_PAGE || next > MAX_CONTROL_BATCH_BYTES
        {
            return Ok(false);
        }
        self.artifacts.push(
            ControlArtifactV1::new(kind, signed_bytes.to_vec())
                .map_err(|_| ControlRejection::Invalid)?,
        );
        self.bytes = next;
        Ok(true)
    }
}

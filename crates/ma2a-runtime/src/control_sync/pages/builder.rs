use ma2a_core::{
    ControlArtifactKind, ControlArtifactV1, MAX_CONTROL_ARTIFACTS_PER_PAGE, MAX_CONTROL_BATCH_BYTES,
};
use ma2a_net::ControlRejection;

use crate::error::{RuntimeError, RuntimeErrorKind};

const ARTIFACT_OVERHEAD: usize = 5;
#[cfg(test)]
pub(super) const RESPONSE_HEADER_BYTES: usize = 6;
pub(super) const PAGE_HEADER_BYTES: usize = 35;

pub(super) struct PageBuilder {
    pub(super) artifacts: Vec<ControlArtifactV1>,
    bytes: usize,
}

impl PageBuilder {
    #[cfg(test)]
    pub(super) const fn new() -> Self {
        Self::with_budget(MAX_CONTROL_BATCH_BYTES - RESPONSE_HEADER_BYTES - PAGE_HEADER_BYTES)
    }

    pub(super) const fn with_budget(bytes: usize) -> Self {
        Self {
            artifacts: Vec::new(),
            bytes: MAX_CONTROL_BATCH_BYTES - bytes,
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

#[cfg(test)]
mod tests {
    use ma2a_core::{
        ControlArtifactKind, ControlPageV1, ControlResponseV1, MAX_CONTROL_ARTIFACTS_PER_PAGE,
        MAX_CONTROL_BATCH_BYTES, SpaceId,
    };

    use super::{ARTIFACT_OVERHEAD, PAGE_HEADER_BYTES, PageBuilder, RESPONSE_HEADER_BYTES};

    #[test]
    fn page_builder_omits_artifacts_after_the_count_limit() {
        // Given
        let mut builder = PageBuilder::new();

        // When
        for _ in 0..MAX_CONTROL_ARTIFACTS_PER_PAGE {
            assert!(
                builder
                    .add_control(ControlArtifactKind::MANIFEST, &[1])
                    .is_ok_and(|added| added)
            );
        }
        let overflow = builder.add_control(ControlArtifactKind::MANIFEST, &[1]);

        // Then
        assert!(overflow.is_ok_and(|added| !added));
        assert_eq!(builder.artifacts.len(), MAX_CONTROL_ARTIFACTS_PER_PAGE);
    }

    #[test]
    fn page_builder_uses_the_complete_byte_budget_without_exceeding_it() {
        // Given
        let mut builder = PageBuilder::new();
        let exact = vec![
            1_u8;
            MAX_CONTROL_BATCH_BYTES
                - RESPONSE_HEADER_BYTES
                - PAGE_HEADER_BYTES
                - ARTIFACT_OVERHEAD
        ];

        // When
        let accepted = builder.add_control(ControlArtifactKind::MANIFEST, &exact);
        let overflow = builder.add_control(ControlArtifactKind::MANIFEST, &[2]);

        // Then
        assert!(accepted.is_ok_and(|added| added));
        assert!(overflow.is_ok_and(|added| !added));
        assert_eq!(builder.artifacts.len(), 1);
    }

    #[test]
    fn accepted_page_always_fits_the_encoded_response_budget()
    -> Result<(), ma2a_core::ProtocolError> {
        // Given
        let mut builder = PageBuilder::new();
        let candidate = vec![
            1_u8;
            MAX_CONTROL_BATCH_BYTES
                - RESPONSE_HEADER_BYTES
                - PAGE_HEADER_BYTES
                - ARTIFACT_OVERHEAD
        ];
        assert!(
            builder
                .add_control(ControlArtifactKind::MANIFEST, &candidate)
                .is_ok_and(|added| added)
        );
        let space_id = SpaceId::try_from([0x71; 32].as_slice())?;
        let page = ControlPageV1::new(space_id, false, builder.artifacts)?;
        let response = ControlResponseV1::new(vec![page])?;

        // When
        let encoded = response.encode()?;

        // Then
        assert!(encoded.len() <= MAX_CONTROL_BATCH_BYTES);
        Ok(())
    }
}

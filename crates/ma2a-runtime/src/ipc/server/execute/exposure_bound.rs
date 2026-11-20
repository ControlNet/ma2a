//! Keeps the exposure list answerable in one bounded Local API response.
//!
//! Root names are checked one by one, so 256 roots that each name a long directory could add
//! up to a list that no longer fits one response frame. The list is the only way the CLI and
//! the Web console see those exposures, so a change that would make it unanswerable is refused
//! before it is committed.

use ma2a_core::{ProtocolError, RequestId};

use crate::api::{self, ApiError, ApiResponse, CommandResult, ServiceExposureView};

/// Whether the largest possible response listing `views` fits one frame.
fn fits(views: Vec<ServiceExposureView>) -> Result<bool, ApiError> {
    // The widest envelope: a request identifier and the largest revision.
    let request_id = RequestId::try_from([0xff; 16].as_slice())?;
    let response = ApiResponse::new(
        Some(request_id),
        u64::MAX,
        CommandResult::service_exposures(views),
    );
    Ok(api::encode_response(&response).is_ok())
}

/// Refuses an exposure list whose largest possible response would not fit one frame.
pub(super) fn ensure_listable(views: Vec<ServiceExposureView>) -> Result<(), ApiError> {
    if fits(views)? {
        return Ok(());
    }
    Err(ApiError::with_remediation(
        ProtocolError::INVALID_INPUT,
        "the exposure list would exceed one response; remove an exposure or use a shorter path",
    )
    .without_effects())
}

/// Answers a list too large for one response with how to shrink it, not a dropped connection.
///
/// Only a database written before [`ensure_listable`] guarded every change can hold one; each
/// exposure in it can still be removed by name.
pub(super) fn ensure_answerable(views: &[ServiceExposureView]) -> Result<(), ApiError> {
    if fits(views.to_vec())? {
        return Ok(());
    }
    Err(ApiError::with_remediation(
        ProtocolError::CONFLICT,
        "the exposures no longer fit one list response; remove some by name with `ma2a service unexpose`",
    )
    .without_effects())
}

#[cfg(test)]
mod tests {
    use ma2a_core::{ProtocolError, SpaceId, service::RootAccess};

    use super::{ensure_answerable, ensure_listable};
    use crate::api::ServiceExposureView;

    fn roots(count: usize, path_bytes: usize) -> Result<Vec<ServiceExposureView>, ProtocolError> {
        let space_id = SpaceId::try_from([0x11; 32].as_slice())?;
        let path = format!("/{}", "d".repeat(path_bytes - 1));
        (0..count)
            .map(|index| {
                ServiceExposureView::files_root(
                    space_id,
                    (format!("root{index}").as_str(), path.as_str()),
                    RootAccess::ReadOnly,
                )
                .map_err(crate::api::ApiError::code)
            })
            .collect()
    }

    #[test]
    fn a_list_that_fits_one_response_is_accepted() -> Result<(), ProtocolError> {
        assert!(ensure_listable(roots(256, 16)?).is_ok());
        Ok(())
    }

    #[test]
    fn a_list_too_large_for_one_response_is_refused() -> Result<(), ProtocolError> {
        // 128 roots naming a 512-byte directory carry 64 KiB of paths alone.
        let refused = ensure_listable(roots(128, 512)?).err();
        assert_eq!(
            refused.map(crate::api::ApiError::code),
            Some(ProtocolError::INVALID_INPUT)
        );
        Ok(())
    }

    #[test]
    fn a_stored_list_too_large_for_one_response_is_answered_with_a_remedy()
    -> Result<(), ProtocolError> {
        assert!(ensure_answerable(&roots(256, 16)?).is_ok());
        let refused = ensure_answerable(&roots(128, 512)?).err();
        assert_eq!(
            refused.as_ref().map(|error| error.code()),
            Some(ProtocolError::CONFLICT)
        );
        assert!(
            refused
                .and_then(|error| error.remediation().map(str::to_owned))
                .is_some_and(|remedy| remedy.contains("unexpose"))
        );
        Ok(())
    }
}

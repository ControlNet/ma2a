//! Echo runtime dispatch contract tests.

use ma2a_core::{EchoResultClass, RequestId};
use ma2a_net::EndpointSecret;
use ma2a_runtime::EchoAuditRecord;

#[test]
fn audit_record_exposes_only_bounded_non_payload_fields() -> Result<(), Box<dyn std::error::Error>>
{
    // Given
    let request_id = RequestId::try_from([0x21; 16].as_slice())?;
    let peer = EndpointSecret::generate().endpoint_id();

    // When
    let record = EchoAuditRecord::new(request_id, peer, EchoResultClass::Succeeded, 4096, 10_000)?;

    // Then
    assert_eq!(record.request_id(), request_id);
    assert_eq!(record.peer_endpoint_id(), peer);
    assert_eq!(record.result_class(), EchoResultClass::Succeeded);
    assert_eq!(record.byte_count(), 4096);
    assert_eq!(record.duration_ms(), 10_000);
    Ok(())
}

use ma2a_core::service::{FrameKind, MAX_HEADER_FRAME_LEN, ServiceError};
use ma2a_services::{FrameReader, FrameWriter};

use super::control;
use crate::Error;

#[tokio::test]
async fn a_caller_control_message_arrives_unchanged_up_to_its_bound() -> Result<(), Error> {
    let (near, far) = tokio::io::duplex(4 * MAX_HEADER_FRAME_LEN);
    let (mut writer, mut reader) = (FrameWriter::new(near), FrameReader::new(far));
    for message in [b"rate=10".to_vec(), vec![0x5a; MAX_HEADER_FRAME_LEN]] {
        control(&mut writer, &message).await?;
        let frame = reader.expect().await?;
        assert_eq!(frame.kind(), FrameKind::Control);
        assert_eq!(frame.into_payload(), message);
    }
    for refused in [Vec::new(), vec![0x5a; MAX_HEADER_FRAME_LEN + 1]] {
        assert!(matches!(
            control(&mut writer, &refused).await,
            Err(Error::Service(ServiceError::InvalidInput))
        ));
    }
    Ok(())
}

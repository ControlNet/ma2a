use ma2a_core::{
    ServiceKind,
    service::{DescribePage, FrameKind, MAX_HEADER_FRAME_LEN, ServiceError, builtin_descriptor},
};
use ma2a_services::{FrameReader, FrameWriter};

use super::{continuing, control};
use crate::Error;

#[test]
fn a_described_page_must_continue_from_the_cursor_sent() -> Result<(), Box<dyn std::error::Error>> {
    let page = |next: Option<&str>| -> Result<DescribePage, Box<dyn std::error::Error>> {
        let files = builtin_descriptor(ServiceKind::Files)?;
        Ok(DescribePage::new(vec![files], next.map(str::to_owned)))
    };
    assert!(continuing(page(Some("files"))?, Some("echo")).is_ok());
    // The cursor sent again, which would keep a caller paging forever.
    assert!(matches!(
        continuing(page(Some("files"))?, Some("files")),
        Err(Error::Protocol(_))
    ));
    // A cursor past the page's last service, which would hide the services between.
    assert!(matches!(
        continuing(page(Some("zulu"))?, None),
        Err(Error::Protocol(_))
    ));
    Ok(())
}

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

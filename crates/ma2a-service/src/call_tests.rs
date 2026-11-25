use ma2a_core::service::{Channel, FrameKind};
use ma2a_services::{FrameReader, FrameWriter};

use super::{Input, Party, caller_input, sends_on};
use crate::Error;

#[test]
fn each_party_sends_only_on_its_own_channels() {
    for channel in [Channel::Output, Channel::Error] {
        assert!(sends_on(channel, Party::Service).is_ok());
        assert!(matches!(
            sends_on(channel, Party::Caller),
            Err(Error::Protocol(_))
        ));
    }
    assert!(sends_on(Channel::Input, Party::Caller).is_ok());
    assert!(matches!(
        sends_on(Channel::Input, Party::Service),
        Err(Error::Protocol(_))
    ));
}

#[tokio::test]
async fn a_service_takes_data_and_end_only_on_the_callers_input() -> Result<(), Error> {
    let (near, far) = tokio::io::duplex(1 << 12);
    let (mut writer, mut reader) = (FrameWriter::new(near), FrameReader::new(far));
    writer.data(Channel::Input, b"in").await?;
    writer.end(Channel::Input).await?;
    writer.data(Channel::Output, b"out").await?;
    writer.end(Channel::Error).await?;
    writer.frame(FrameKind::Cancel, &[]).await?;
    writer.flush().await?;
    assert_eq!(
        caller_input(&mut reader).await?,
        Input::Data(Channel::Input, b"in".to_vec())
    );
    assert_eq!(caller_input(&mut reader).await?, Input::End(Channel::Input));
    for _ in 0..2 {
        assert!(matches!(
            caller_input(&mut reader).await,
            Err(Error::Protocol(_))
        ));
    }
    assert_eq!(caller_input(&mut reader).await?, Input::Cancelled);
    Ok(())
}

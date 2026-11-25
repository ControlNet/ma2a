use ma2a_core::service::{Channel, FrameKind};
use ma2a_services::{FrameReader, FrameWriter};

use super::{Channels, Input, Party, caller_input, sends_on};
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
    let mut ended = Channels::default();
    writer.data(Channel::Input, b"in").await?;
    writer.end(Channel::Input).await?;
    writer.data(Channel::Output, b"out").await?;
    writer.end(Channel::Error).await?;
    writer.frame(FrameKind::Cancel, &[]).await?;
    writer.flush().await?;
    assert_eq!(
        caller_input(&mut reader, &mut ended).await?,
        Input::Data(Channel::Input, b"in".to_vec())
    );
    assert_eq!(
        caller_input(&mut reader, &mut ended).await?,
        Input::End(Channel::Input)
    );
    for _ in 0..2 {
        assert!(matches!(
            caller_input(&mut reader, &mut ended).await,
            Err(Error::Protocol(_))
        ));
    }
    assert_eq!(
        caller_input(&mut reader, &mut ended).await?,
        Input::Cancelled
    );
    Ok(())
}

#[tokio::test]
async fn a_service_takes_nothing_on_the_callers_input_after_its_end() -> Result<(), Error> {
    for after in [Some(&b"late"[..]), None] {
        let (near, far) = tokio::io::duplex(1 << 12);
        let (mut writer, mut reader) = (FrameWriter::new(near), FrameReader::new(far));
        let mut ended = Channels::default();
        writer.end(Channel::Input).await?;
        match after {
            Some(bytes) => writer.data(Channel::Input, bytes).await?,
            None => writer.end(Channel::Input).await?,
        }
        writer.flush().await?;
        assert_eq!(
            caller_input(&mut reader, &mut ended).await?,
            Input::End(Channel::Input)
        );
        assert!(
            matches!(
                caller_input(&mut reader, &mut ended).await,
                Err(Error::Protocol(_))
            ),
            "{after:?}"
        );
    }
    Ok(())
}

#[test]
fn a_party_sends_nothing_on_a_channel_after_its_end() {
    let mut service = Channels::default();
    // The service ends its output and goes on reporting errors.
    assert!(
        service
            .take((Channel::Output, Party::Service), false)
            .is_ok()
    );
    assert!(
        service
            .take((Channel::Output, Party::Service), true)
            .is_ok()
    );
    assert!(
        service
            .take((Channel::Error, Party::Service), false)
            .is_ok()
    );
    assert!(service.take((Channel::Error, Party::Service), true).is_ok());
    // Only more output, or a second end, is refused.
    for (channel, ends) in [
        (Channel::Output, false),
        (Channel::Output, true),
        (Channel::Error, false),
        (Channel::Error, true),
    ] {
        assert!(matches!(
            service.take((channel, Party::Service), ends),
            Err(Error::Protocol(_))
        ));
    }
    let mut caller = Channels::default();
    assert!(caller.take((Channel::Input, Party::Caller), true).is_ok());
    for ends in [false, true] {
        assert!(matches!(
            caller.take((Channel::Input, Party::Caller), ends),
            Err(Error::Protocol(_))
        ));
    }
}

use ma2a_core::{
    ServiceKind,
    service::{
        Channel, DescribePage, FrameKind, MAX_HEADER_FRAME_LEN, ServiceError, builtin_descriptor,
        encode_call_accept, encode_call_outcome,
    },
};
use ma2a_services::{FrameReader, FrameWriter};

use super::{Received, Seen, continuing, control, received};
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

/// One frame the service sends.
#[derive(Clone, Copy, Debug)]
enum Sent {
    Accept,
    Data(Channel),
    End(Channel),
    Control,
    Reject,
    Outcome,
}

/// What the caller takes from the service's frames `sent`, in order, up to the first refused.
async fn taken(sent: &[Sent]) -> Result<Vec<Result<Received, Error>>, Box<dyn std::error::Error>> {
    let (near, far) = tokio::io::duplex(1 << 16);
    let (mut writer, mut reader) = (FrameWriter::new(near), FrameReader::new(far));
    for frame in sent {
        match frame {
            Sent::Accept => {
                writer
                    .frame(FrameKind::Accept, &encode_call_accept(b"h")?)
                    .await?;
            }
            Sent::Data(channel) => writer.data(*channel, b"x").await?,
            Sent::End(channel) => writer.end(*channel).await?,
            Sent::Control => writer.frame(FrameKind::Control, b"c").await?,
            Sent::Reject => writer.reject(ServiceError::Unavailable).await?,
            Sent::Outcome => {
                writer
                    .frame(FrameKind::Outcome, &encode_call_outcome(Ok(b"r"))?)
                    .await?;
            }
        }
    }
    writer.flush().await?;
    let mut seen = Seen::default();
    let mut taken = Vec::new();
    for _ in sent {
        let frame = reader.expect().await?;
        let one = received(frame, &mut seen);
        let refused = one.is_err();
        taken.push(one);
        if refused {
            break;
        }
    }
    Ok(taken)
}

#[tokio::test]
async fn a_call_takes_frames_only_in_the_order_and_direction_services_v1_allows()
-> Result<(), Box<dyn std::error::Error>> {
    let (output, error, input) = (Channel::Output, Channel::Error, Channel::Input);
    let allowed = [
        vec![
            Sent::Accept,
            Sent::Data(output),
            Sent::Data(error),
            Sent::Control,
            Sent::End(output),
            Sent::End(error),
            Sent::Outcome,
        ],
        vec![Sent::Reject],
        vec![Sent::Outcome],
    ];
    for sent in allowed {
        let taken = taken(&sent).await?;
        assert!(taken.iter().all(Result::is_ok), "{sent:?}: {taken:?}");
    }
    let refused = [
        vec![Sent::Data(output)],
        vec![Sent::End(output)],
        vec![Sent::Control],
        vec![Sent::Accept, Sent::Accept],
        vec![Sent::Accept, Sent::Reject],
        vec![Sent::Accept, Sent::Data(input)],
        vec![Sent::Accept, Sent::End(input)],
        vec![Sent::Accept, Sent::End(output), Sent::Data(output)],
        vec![Sent::Accept, Sent::End(error), Sent::End(error)],
        vec![Sent::Outcome, Sent::Outcome],
        vec![Sent::Reject, Sent::Outcome],
        vec![Sent::Accept, Sent::Outcome, Sent::Data(output)],
    ];
    for sent in refused {
        let taken = taken(&sent).await?;
        assert!(
            matches!(taken.last(), Some(Err(Error::Protocol(_)))),
            "{sent:?}: {taken:?}"
        );
        assert!(taken.iter().rev().skip(1).all(Result::is_ok), "{sent:?}");
    }
    Ok(())
}

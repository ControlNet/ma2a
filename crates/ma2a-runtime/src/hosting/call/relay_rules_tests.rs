//! The Services v1 order and direction rules the relay holds a call's two parties to.

use std::time::Duration;

use ma2a_core::service::{Channel, FrameKind, encode_call_accept, encode_call_outcome};
use ma2a_services::{FrameReader, FrameWriter};
use tokio_util::sync::CancellationToken;

use super::relay_call;
use crate::stream_plane::relay::{Relayed, Terminal};

/// Far past every grace; the clock is paused, so a hang fails at once instead of hanging.
const BOUND: Duration = Duration::from_secs(60);

/// One frame a party sends.
#[derive(Clone, Copy, Debug)]
enum Sent {
    Accept,
    Data(Channel),
    End(Channel),
    Outcome,
}

async fn send<W: tokio::io::AsyncWrite + Unpin>(writer: &mut FrameWriter<W>, sent: &[Sent]) {
    for frame in sent {
        let written = match frame {
            Sent::Accept => {
                let accept = encode_call_accept(b"").expect("accept");
                writer.frame(FrameKind::Accept, &accept).await
            }
            Sent::Data(channel) => writer.data(*channel, b"x").await,
            Sent::End(channel) => writer.end(*channel).await,
            Sent::Outcome => {
                let outcome = encode_call_outcome(Ok(b"r")).expect("outcome");
                writer.frame(FrameKind::Outcome, &outcome).await
            }
        };
        written.expect("frame");
    }
    writer.flush().await.expect("flush");
}

/// Relays one call whose caller sends `caller` and whose service sends `service`, the
/// service only after it read the caller's frames when `answers`; returns how the call ended
/// and the kinds of the frames the service read.
async fn relayed(
    caller: &[Sent],
    (service, answers): (&[Sent], bool),
) -> (Relayed, Vec<FrameKind>) {
    let (caller_end, host) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(host);
    let (mut reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    let (service_end, hosted) = tokio::io::duplex(1 << 16);
    let (caller_read, caller_write) = tokio::io::split(caller_end);
    let mut input = FrameWriter::new(caller_write);
    send(&mut input, caller).await;
    let (service_read, service_write) = tokio::io::split(service_end);
    let (mut taken, mut output) = (
        FrameReader::new(service_read),
        FrameWriter::new(service_write),
    );
    let (read, service) = (caller.len(), service.to_vec());
    let script = tokio::spawn(async move {
        let mut kinds = Vec::new();
        if !answers {
            send(&mut output, &service).await;
        }
        while kinds.len() < read {
            let Ok(Some(frame)) = taken.next().await else {
                break;
            };
            kinds.push(frame.kind());
        }
        if answers {
            send(&mut output, &service).await;
        }
        (kinds, taken, output)
    });
    let (stop, service_stop) = (CancellationToken::new(), CancellationToken::new());
    let calling = relay_call(
        (&mut reader, &mut writer),
        Box::new(hosted),
        (&stop, &service_stop),
    );
    let relayed = tokio::time::timeout(BOUND, calling)
        .await
        .expect("the call never ended");
    let (kinds, _taken, _output) = script.await.expect("service script");
    drop((caller_read, input));
    (relayed, kinds)
}

#[tokio::test(start_paused = true)]
async fn caller_data_on_a_service_channel_cancels_the_call_instead_of_reaching_it() {
    for wrong in [Sent::Data(Channel::Output), Sent::End(Channel::Error)] {
        let (relayed, kinds) = relayed(
            &[Sent::Data(Channel::Input), wrong],
            (&[Sent::Accept, Sent::Outcome], true),
        )
        .await;
        assert_eq!(kinds, [FrameKind::Data, FrameKind::Cancel], "{wrong:?}");
        assert!(
            matches!(relayed.terminal, Terminal::Outcome { .. }),
            "{wrong:?}: {relayed:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_service_frame_out_of_order_or_direction_loses_the_call() {
    for sent in [
        [Sent::Data(Channel::Output), Sent::Accept, Sent::Outcome],
        [Sent::Accept, Sent::Accept, Sent::Outcome],
        [Sent::Accept, Sent::Data(Channel::Input), Sent::Outcome],
        [Sent::Accept, Sent::End(Channel::Input), Sent::Outcome],
    ] {
        let (relayed, _) = relayed(&[], (&sent, false)).await;
        assert_eq!(relayed.terminal, Terminal::Lost, "{sent:?}");
    }
    let sent = [
        Sent::Accept,
        Sent::Data(Channel::Output),
        Sent::Data(Channel::Error),
        Sent::End(Channel::Output),
        Sent::Outcome,
    ];
    let (relayed, _) = relayed(&[], (&sent, false)).await;
    assert!(
        matches!(relayed.terminal, Terminal::Outcome { failure: None, .. }),
        "{relayed:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn caller_input_after_its_end_cancels_the_call_instead_of_reaching_it() {
    for after in [Sent::Data(Channel::Input), Sent::End(Channel::Input)] {
        let (relayed, kinds) = relayed(
            &[Sent::End(Channel::Input), after],
            (&[Sent::Accept, Sent::Outcome], true),
        )
        .await;
        assert_eq!(kinds, [FrameKind::End, FrameKind::Cancel], "{after:?}");
        assert!(
            matches!(relayed.terminal, Terminal::Outcome { .. }),
            "{after:?}: {relayed:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_service_sending_on_a_channel_it_ended_loses_the_call() {
    for (ended, after) in [
        (Channel::Output, Sent::Data(Channel::Output)),
        (Channel::Output, Sent::End(Channel::Output)),
        (Channel::Error, Sent::Data(Channel::Error)),
        (Channel::Error, Sent::End(Channel::Error)),
    ] {
        let sent = [Sent::Accept, Sent::End(ended), after, Sent::Outcome];
        let (relayed, _) = relayed(&[], (&sent, false)).await;
        assert_eq!(relayed.terminal, Terminal::Lost, "{sent:?}");
    }
    // The other channel stays open after one ended.
    let sent = [
        Sent::Accept,
        Sent::End(Channel::Output),
        Sent::Data(Channel::Error),
        Sent::End(Channel::Error),
        Sent::Outcome,
    ];
    let (relayed, _) = relayed(&[], (&sent, false)).await;
    assert!(
        matches!(relayed.terminal, Terminal::Outcome { failure: None, .. }),
        "{relayed:?}"
    );
}

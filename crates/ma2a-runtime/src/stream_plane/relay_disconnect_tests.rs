//! Out-of-band peer-disconnect bounds when `lookahead.ahead` holds a frame, under a paused clock.

use std::sync::Arc;

use ma2a_core::service::{Channel, EndReason, FrameKind, ProxyResult};
use tokio::sync::Notify;

use super::{ClientWrite, Terminal, relay_watching, tests::ends};

#[tokio::test(start_paused = true)]
async fn a_proxy_relay_with_lookahead_cancels_on_peer_disconnect_and_keeps_live_idle() {
    // 1. Disconnected client while lookahead.ahead holds a frame cancels within CANCEL_GRACE.
    let (client, plane_client) = tokio::io::duplex(1 << 16);
    let (plane_target, _target) = tokio::io::duplex(1);
    let notify = Arc::new(Notify::new());
    let watcher = Arc::clone(&notify);
    let task = tokio::spawn(async move {
        let ((mut cr, mut cw), (mut tr, mut tw)) = (ends(plane_client), ends(plane_target));
        relay_watching(
            (&mut cr, &mut cw),
            (&mut tr, &mut tw),
            (
                || (),
                &ClientWrite::default(),
                std::time::Duration::from_secs(605),
                watcher.notified(),
            ),
        )
        .await
    });
    let (_cr, mut cw) = ends(client);
    // Frame 1 blocks target_writer; frame 2 fills lookahead.ahead so client_reader is no longer
    // polled.
    cw.data(Channel::Input, b"blocked").await.expect("data 1");
    cw.data(Channel::Input, b"ahead").await.expect("data 2");
    cw.flush().await.expect("flush");
    tokio::task::yield_now().await;

    let started = tokio::time::Instant::now();
    notify.notify_one();
    let relayed = task.await.expect("relay");
    assert_eq!(started.elapsed(), super::CANCEL_GRACE);
    assert_eq!(relayed.terminal, Terminal::Lost);
    assert!(!relayed.client_present);

    // 2. Live client with lookahead.ahead held keeps its full Proxy idle past FORWARD_STALL.
    let (client, plane_client) = tokio::io::duplex(1 << 16);
    let (plane_target, target) = tokio::io::duplex(1);
    let task = tokio::spawn(async move {
        let ((mut cr, mut cw), (mut tr, mut tw)) = (ends(plane_client), ends(plane_target));
        relay_watching(
            (&mut cr, &mut cw),
            (&mut tr, &mut tw),
            (
                || (),
                &ClientWrite::default(),
                std::time::Duration::from_secs(45),
                std::future::pending(),
            ),
        )
        .await
    });
    let (mut cr, mut cw) = ends(client);
    cw.data(Channel::Input, b"blocked").await.expect("data 1");
    cw.data(Channel::Input, b"ahead").await.expect("data 2");
    cw.flush().await.expect("flush");
    tokio::task::yield_now().await;

    tokio::time::advance(std::time::Duration::from_secs(40)).await;
    assert!(!task.is_finished(), "live proxy client timed out early");
    let (_tr, mut tw) = ends(target);
    let timed_out =
        ProxyResult::encode_outcome(&Ok(ProxyResult::new(EndReason::TimedOut, (0, 0), 40_000)))
            .expect("outcome");
    let _sent = tw.frame(FrameKind::Outcome, &timed_out).await;
    let relayed = task.await.expect("relay");
    let _drained = cr.next().await;
    assert!(relayed.client_present);
    assert!(matches!(
        relayed.terminal,
        Terminal::Outcome { failure: None, .. }
    ));
}

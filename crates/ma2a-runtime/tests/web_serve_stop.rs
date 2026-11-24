//! Stopping `serve_connections` while a keep-alive connection sits idle.

use std::time::{Duration, Instant};

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The grace a stop gives open connections, far longer than an idle one should take.
const GRACE: Duration = Duration::from_secs(3);

#[tokio::test]
async fn a_stop_closes_an_idle_keep_alive_connection_at_once() -> TestResult {
    // Given: a connection that was answered once and stays open for more.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
    let stop = CancellationToken::new();
    let served = tokio::spawn(ma2a_runtime::web::serve_connections(
        listener,
        router,
        (stop.clone(), GRACE),
    ));
    let mut client = TcpStream::connect(address).await?;
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await?;
    let mut response = Vec::new();
    while !response.ends_with(b"ok") {
        let mut buffer = [0; 1024];
        let read = client.read(&mut buffer).await?;
        if read == 0 {
            return Err("the connection closed before its response".into());
        }
        response.extend_from_slice(buffer.get(..read).unwrap_or_default());
    }

    // When
    let started = Instant::now();
    stop.cancel();
    tokio::time::timeout(GRACE * 2, served).await??;

    // Then: the idle connection closed without waiting out the grace.
    let elapsed = started.elapsed();
    assert!(elapsed < GRACE / 3, "the stop took {elapsed:?}");
    let mut rest = [0; 64];
    assert_eq!(client.read(&mut rest).await?, 0);
    Ok(())
}

use std::error::Error;

use ma2a_core::{EchoError, EchoRequest, MAX_WIRE_LEN, RequestId};

use super::echo_e2e_adversarial::AuthorizedFixture;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_transport_rejects_seventeenth_stream_and_releases_capacity() -> TestResult {
    // Given
    let fixture = AuthorizedFixture::new("concurrency").await?;
    let connection = fixture.connect().await?;
    let held_body = vec![0_u8; MAX_WIRE_LEN];
    let mut held_streams = Vec::new();
    for _ in 0..16 {
        let (mut send, receive) = connection.open_bi().await?;
        send.write_all(&held_body).await?;
        held_streams.push((send, receive));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while fixture.handle().echo_metrics().active_streams != 16 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let request = EchoRequest::new(
        RequestId::try_from([0x75; 16].as_slice())?,
        fixture.owner_id(),
        b"replacement",
    )?
    .encode()?;

    // When
    let (mut excess_send, mut excess_receive) = connection.open_bi().await?;
    excess_send.write_all(&request).await?;
    excess_send.finish()?;
    let mut excess_header = [0_u8; 37];
    excess_receive.read_exact(&mut excess_header).await?;

    // Then
    assert_eq!(excess_header[0], EchoError::ConcurrencyExceeded.code());
    let (mut released_send, mut released_receive) =
        held_streams.pop().ok_or("held stream missing")?;
    released_send.write_all(&[0]).await?;
    released_send.finish()?;
    let mut released_header = [0_u8; 37];
    released_receive.read_exact(&mut released_header).await?;
    assert_eq!(released_header[0], EchoError::InvalidInput.code());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while fixture.handle().echo_metrics().active_streams != 15 {
            tokio::task::yield_now().await;
        }
    })
    .await?;

    let (mut replacement_send, mut replacement_receive) = connection.open_bi().await?;
    replacement_send.write_all(&request).await?;
    replacement_send.finish()?;
    let mut replacement_header = [0_u8; 37];
    replacement_receive
        .read_exact(&mut replacement_header)
        .await?;
    assert_eq!(replacement_header[0], 0);

    connection.close(0_u8.into(), b"");
    drop(held_streams);
    fixture.shutdown().await
}

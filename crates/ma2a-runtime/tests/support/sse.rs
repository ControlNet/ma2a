//! SSE consumers distinguish heartbeat comments from revisioned state events.
use axum::body::Bytes;
use tokio_stream::{Stream, StreamExt as _};

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) async fn next_state_event<S, E>(events: &mut S) -> TestResult<String>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    loop {
        let frame = events.next().await.ok_or("event stream closed")??;
        let event = std::str::from_utf8(&frame)?;
        if !event
            .lines()
            .all(|line| line.is_empty() || line.starts_with(':'))
        {
            return Ok(event.to_owned());
        }
    }
}

#[tokio::test]
async fn heartbeat_before_state_event_does_not_replace_the_revision_notification() -> TestResult<()>
{
    let state = "data: {\"type\":\"snapshot_invalidated\",\"revision\":7}\n\n";
    let mut events = tokio_stream::iter([
        Ok::<_, std::io::Error>(Bytes::from_static(b":\n\n")),
        Ok(Bytes::from_static(state.as_bytes())),
    ]);
    assert_eq!(next_state_event(&mut events).await?, state);
    Ok(())
}

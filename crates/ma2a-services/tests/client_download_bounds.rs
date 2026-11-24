//! The download receiver holds a stream to the size its `accept` declared.

#[path = "support/host.rs"]
mod support;

use ma2a_core::service::{Channel, ContentIdentity, FrameKind, ServiceError, TransferResult};
use ma2a_services::client::{self, ClientError};
use support::{TestResult, pipe};

/// What the target streams: content chunks, then a terminal claiming `(bytes, size, hash)`.
struct Stream<'a> {
    chunks: &'a [&'a [u8]],
    terminal: (u64, u64, [u8; 32]),
}

/// Receives `stream` as content accepted with `size`, starting at `prefix.len()`.
async fn receive(
    stream: Stream<'static>,
    (size, prefix): (u64, &[u8]),
) -> TestResult<(Result<TransferResult, ClientError>, Vec<u8>)> {
    let ((mut reader, _client_writer), (_host_reader, mut writer)) = pipe();
    let host = tokio::spawn(async move {
        for chunk in stream.chunks {
            writer.data(Channel::Output, chunk).await?;
        }
        let (bytes, size, hash) = stream.terminal;
        let outcome = TransferResult::encode_outcome(&Ok(TransferResult::new(bytes, size, hash)))
            .map_err(|_| ServiceError::Internal)?;
        writer.frame(FrameKind::Outcome, &outcome).await?;
        writer.flush().await
    });
    let mut sink = Vec::new();
    let mut hasher = blake3::Hasher::new();
    hasher.update(prefix);
    let identity = ContentIdentity::new(size, 7);
    let offset = u64::try_from(prefix.len())?;
    let result = client::transfer::download_content(
        &mut reader,
        (&mut sink, &mut hasher),
        (identity, offset),
    )
    .await;
    drop(reader);
    let _sent = host.await?;
    Ok((result, sink))
}

fn hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

const fn mismatched(result: &Result<TransferResult, ClientError>) -> bool {
    matches!(
        result,
        Err(ClientError::Failed(ServiceError::IntegrityMismatch))
    )
}

#[tokio::test]
async fn content_of_the_accepted_size_is_received() -> TestResult {
    let empty = Stream {
        chunks: &[],
        terminal: (0, 0, hash(&[])),
    };
    let (result, sink) = receive(empty, (0, b"")).await?;
    assert_eq!(result.map_err(|error| format!("{error:?}"))?.size, 0);
    assert!(sink.is_empty());

    let whole = Stream {
        chunks: &[b"ab", b"c"],
        terminal: (3, 3, hash(&[b"abc"])),
    };
    let (result, sink) = receive(whole, (3, b"")).await?;
    assert_eq!(result.map_err(|error| format!("{error:?}"))?.bytes, 3);
    assert_eq!(sink, b"abc");

    let resumed = Stream {
        chunks: &[b"cde"],
        terminal: (3, 5, hash(&[b"abcde"])),
    };
    let (result, sink) = receive(resumed, (5, b"ab")).await?;
    assert_eq!(result.map_err(|error| format!("{error:?}"))?.size, 5);
    assert_eq!(sink, b"cde");
    Ok(())
}

#[tokio::test]
async fn content_past_the_accepted_size_is_refused_before_it_is_written() -> TestResult {
    let long = Stream {
        chunks: &[b"ab", b"cd"],
        terminal: (4, 4, hash(&[b"abcd"])),
    };
    let (result, sink) = receive(long, (3, b"")).await?;
    assert!(mismatched(&result), "{result:?}");
    assert_eq!(sink, b"ab");
    Ok(())
}

#[tokio::test]
async fn a_terminal_that_disagrees_with_the_accepted_size_is_refused() -> TestResult {
    // Fewer bytes than accepted, reported consistently with what arrived.
    let short = Stream {
        chunks: &[b"ab"],
        terminal: (2, 2, hash(&[b"ab"])),
    };
    let (result, _) = receive(short, (3, b"")).await?;
    assert!(mismatched(&result), "{result:?}");

    // Every accepted byte arrived, but the terminal reports another full size.
    let resized = Stream {
        chunks: &[b"abc"],
        terminal: (3, 9, hash(&[b"abc"])),
    };
    let (result, _) = receive(resized, (3, b"")).await?;
    assert!(mismatched(&result), "{result:?}");

    // A resume offset past the accepted size can never be satisfied.
    let none = Stream {
        chunks: &[],
        terminal: (0, 2, hash(&[b"abc"])),
    };
    let (result, _) = receive(none, (2, b"abc")).await?;
    assert!(mismatched(&result), "{result:?}");
    Ok(())
}

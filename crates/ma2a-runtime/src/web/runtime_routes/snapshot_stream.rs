use axum::{
    Json,
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
};
use serde_json::Value;

use super::SnapshotStamp;
use crate::api::{MAX_LOCAL_RESPONSE_BYTES, fragments::SnapshotFragments};

pub(super) fn response(stamp: SnapshotStamp, payload: Value) -> Response {
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if encoded.len() <= MAX_LOCAL_RESPONSE_BYTES {
        return Json(payload).into_response();
    }
    let frames = SnapshotFragments::new(encoded, stamp.revision, stamp.boot_id).map(|frame| {
        frame.map(|mut bytes| {
            bytes.push(b'\n');
            bytes
        })
    });
    (
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(tokio_stream::iter(frames)),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn large_http_snapshot_uses_complete_bounded_ndjson_frames()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let payload = serde_json::json!({"revision": 9, "spaces": vec!["escaped \" 🦀"; 10_000]});
        let response = response(
            SnapshotStamp {
                revision: 9,
                boot_id: "ab".repeat(16),
            },
            payload.clone(),
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .ok_or("no content type")?,
            "application/x-ndjson"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000).await?;
        let mut assembly = crate::api::fragments::SnapshotAssembly::default();
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            assert!(line.len() < 33_000);
            assembly.push(line)?;
        }
        let (restored, revision, boot) = assembly.finish()?;
        assert_eq!(serde_json::from_slice::<Value>(&restored)?, payload);
        assert_eq!(revision, 9);
        assert_eq!(boot, "ab".repeat(16));
        Ok(())
    }
}

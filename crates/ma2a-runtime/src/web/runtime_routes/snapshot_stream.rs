use axum::{
    Json,
    body::Body,
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use serde_json::Value;

use super::SnapshotStamp;
use crate::api::{MAX_LOCAL_RESPONSE_BYTES, fragments::SnapshotFragments};

const RUNTIME_BOOT_ID_HEADER: HeaderName = HeaderName::from_static("x-ma2a-runtime-boot-id");

pub(super) fn response(stamp: SnapshotStamp, payload: Value) -> Response {
    let Ok(boot_id) = HeaderValue::from_str(&stamp.boot_id) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if encoded.len() <= MAX_LOCAL_RESPONSE_BYTES {
        return ([(RUNTIME_BOOT_ID_HEADER, boot_id)], Json(payload)).into_response();
    }
    let frames = SnapshotFragments::new(encoded, stamp.revision, stamp.boot_id).map(|frame| {
        frame.map(|mut bytes| {
            bytes.push(b'\n');
            bytes
        })
    });
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-ndjson"),
            ),
            (RUNTIME_BOOT_ID_HEADER, boot_id),
        ],
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
                view_sequence: 0,
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
        assert_eq!(
            response
                .headers()
                .get("x-ma2a-runtime-boot-id")
                .ok_or("missing runtime boot id header")?,
            "ab".repeat(16).as_str()
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

    #[tokio::test]
    async fn small_http_snapshot_carries_the_runtime_boot_id_header()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let payload = serde_json::json!({"revision": 5, "spaces": []});
        let boot = "cd".repeat(16);
        let response = response(
            SnapshotStamp {
                revision: 5,
                view_sequence: 0,
                boot_id: boot.clone(),
            },
            payload.clone(),
        );
        assert_eq!(
            response
                .headers()
                .get("x-ma2a-runtime-boot-id")
                .ok_or("missing runtime boot id header")?,
            boot.as_str()
        );
        let bytes = axum::body::to_bytes(response.into_body(), 65_536).await?;
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?, payload);
        Ok(())
    }
}

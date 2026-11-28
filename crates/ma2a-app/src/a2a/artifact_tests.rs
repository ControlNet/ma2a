use std::time::Duration;

use tokio::sync::oneshot;

use super::{Refusal, matches_reference, relay};

#[test]
fn content_must_be_what_the_artifact_reference_names() {
    let hash = *blake3::hash(b"result").as_bytes();
    let hex = blake3::hash(b"result").to_hex();
    assert!(matches_reference((6, &hex), (6, &hash)));
    // Another size, another hash, or a reference that names no hash.
    assert!(!matches_reference((7, &hex), (6, &hash)));
    let other = blake3::hash(b"forged").to_hex();
    assert!(!matches_reference((6, &other), (6, &hash)));
    assert!(!matches_reference((6, ""), (6, &hash)));
}

/// The status of an artifact request through a target answering every stream with `script`.
#[cfg(unix)]
async fn served(script: crate::test_daemon::Script) -> crate::test_daemon::TestResult<u16> {
    let daemon = crate::test_daemon::Daemon::start("artifact", script)?;
    let (backend, route) = super::Backend::standing_in(&daemon)?;
    let task = "01".repeat(16);
    let response = super::serve(backend, (route, task, "result".to_owned())).await;
    Ok(response.status().as_u16())
}

#[cfg(unix)]
#[tokio::test]
async fn only_a_missing_task_or_artifact_answers_not_found() -> crate::test_daemon::TestResult {
    use ma2a_core::service::{FrameKind, ServiceError};
    let refused = |error: ServiceError| vec![(FrameKind::Reject, vec![error.code()])];
    // A task the target does not have, or one this Endpoint may not see.
    assert_eq!(served(refused(ServiceError::UnknownTask)).await?, 404);
    assert_eq!(served(refused(ServiceError::Unauthorized)).await?, 404);
    // A target that cannot answer now may answer later.
    assert_eq!(served(refused(ServiceError::Unavailable)).await?, 503);
    assert_eq!(served(refused(ServiceError::Unreachable)).await?, 503);
    assert_eq!(
        served(refused(ServiceError::ConcurrencyExceeded)).await?,
        503
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_left_ends_a_stalled_download() {
    // Given: a target that never sends, and a client that left.
    let (outcome, verdict) = oneshot::channel();
    drop(verdict);

    // When
    let download = std::future::pending::<Result<(), Refusal>>();
    let relayed = tokio::time::timeout(Duration::from_secs(60), relay(outcome, download)).await;

    // Then
    assert!(relayed.is_ok());
}

#[cfg(unix)]
mod remote_artifact {
    use axum::http::StatusCode;
    use ma2a_core::service::{
        AgentAccept, AgentMessage, AgentReply, AgentTaskDetails, AgentTaskId, AgentTaskState,
        ArtifactRef, ContentIdentity, FrameKind, MessagePart, MessageRole, ServiceError,
        TransferAccept, TransferResult,
    };
    use tokio::time::Instant;

    use super::*;
    use crate::{
        a2a::remote::CALL_DEADLINE,
        test_daemon::{Daemon, Script, TestResult, answering, refusing, summary},
    };

    const TASK: AgentTaskId = AgentTaskId::from_bytes([1; 16]);

    fn details_with_artifact() -> TestResult<Script> {
        let mut task = summary(TASK, "worker")?;
        task.state = AgentTaskState::Completed;
        let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
        let mut details = AgentTaskDetails::new(task, input);
        let hash = *blake3::hash(b"result").as_bytes();
        details
            .artifacts
            .push(ArtifactRef::new(("result", "text/plain"), (6, hash))?);
        answering(
            AgentAccept::Admitted,
            &AgentReply::Details(Box::new(details)),
        )
    }

    async fn serve_two_step(
        artifact_script: Script,
    ) -> TestResult<(Daemon, axum::response::Response)> {
        let daemon = Daemon::paging(
            "artifact-two",
            vec![details_with_artifact()?, artifact_script],
        )?;
        let (backend, route) = super::super::Backend::standing_in(&daemon)?;
        let response =
            super::super::serve(backend, (route, TASK.to_string(), "result".to_owned())).await;
        Ok((daemon, response))
    }

    #[tokio::test]
    async fn artifact_refusal_after_get_answers_503_or_404_instead_of_200() -> TestResult {
        let outcome_err = |error: ServiceError| -> TestResult<Script> {
            Ok(vec![(
                FrameKind::Outcome,
                TransferResult::encode_outcome(&Err(error))?,
            )])
        };
        // Transient refusals on the Artifact stream itself (daemon or target) must answer 503.
        for busy in [
            vec![(
                FrameKind::Reject,
                vec![ServiceError::ConcurrencyExceeded.code()],
            )],
            vec![(FrameKind::Reject, vec![ServiceError::Unavailable.code()])],
            vec![(FrameKind::Reject, vec![ServiceError::Unreachable.code()])],
            outcome_err(ServiceError::ConcurrencyExceeded)?,
            refusing("concurrency_exceeded"),
            refusing("unavailable"),
        ] {
            let (_daemon, response) = serve_two_step(busy).await?;
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }

        // Missing, unauthorized, or contract-breaking initial answers must answer 404.
        let hash = *blake3::hash(b"result").as_bytes();
        let unexpected_ok = vec![(
            FrameKind::Outcome,
            TransferResult::encode_outcome(&Ok(TransferResult::new(6, 6, hash)))?,
        )];
        for missing in [
            vec![(
                FrameKind::Reject,
                vec![ServiceError::UnknownArtifact.code()],
            )],
            vec![(FrameKind::Reject, vec![ServiceError::UnknownTask.code()])],
            vec![(FrameKind::Reject, vec![ServiceError::Unauthorized.code()])],
            outcome_err(ServiceError::UnknownArtifact)?,
            unexpected_ok,
        ] {
            let (_daemon, response) = serve_two_step(missing).await?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_artifact_accept_times_out_with_503() -> TestResult {
        let daemon = Daemon::paging(
            "artifact-silent",
            vec![details_with_artifact()?, Vec::new()],
        )?;
        let (backend, route) = super::super::Backend::standing_in(&daemon)?;
        let started = Instant::now();

        let response = tokio::time::timeout(
            CALL_DEADLINE * 2,
            super::super::serve(backend, (route, TASK.to_string(), "result".to_owned())),
        )
        .await?;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(started.elapsed(), CALL_DEADLINE);
        let body = tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024),
        )
        .await??;
        assert!(body.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn an_accepted_artifact_streams_verified_content() -> TestResult {
        let hash = *blake3::hash(b"result").as_bytes();
        let mut data = vec![1];
        data.extend_from_slice(b"result");
        let outcome = TransferResult::encode_outcome(&Ok(TransferResult::new(6, 6, hash)))?;
        let accepted = vec![
            (
                FrameKind::Accept,
                TransferAccept::Get(ContentIdentity::new(6, 0)).encode()?,
            ),
            (FrameKind::Data, data),
            (FrameKind::Outcome, outcome),
        ];

        let (_daemon, response) = serve_two_step(accepted).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/plain")
        );
        let body = axum::body::to_bytes(response.into_body(), 1024).await?;
        assert_eq!(&body[..], b"result");
        Ok(())
    }

    #[tokio::test]
    async fn an_accepted_artifact_of_wrong_size_fails_before_sending_bytes() -> TestResult {
        let wrong_size = vec![(
            FrameKind::Accept,
            TransferAccept::Get(ContentIdentity::new(7, 0)).encode()?,
        )];

        let (_daemon, response) = serve_two_step(wrong_size).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024).await;
        assert!(body.is_err());
        Ok(())
    }
}

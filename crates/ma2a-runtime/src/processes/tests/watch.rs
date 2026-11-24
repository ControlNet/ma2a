//! Following a supervisor that proves itself and then never answers.

use std::{sync::Arc, time::Duration};

use interprocess::local_socket::tokio::prelude::*;
use ma2a_core::service::{ProcessId, ProcessState, ServiceError};
use ma2a_services::{
    FrameReader, FrameWriter,
    supervisor::{
        Credentials,
        proof::{prove_supervisor, read_challenge},
    },
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{TempState, TestResult};
use crate::{clock::SystemClock, processes::ProcessManager, store::StoreClient};

#[tokio::test(start_paused = true)]
async fn a_watch_on_a_silent_supervisor_is_unavailable() -> TestResult {
    // Given
    let state = TempState::new("silent")?;
    std::fs::create_dir_all(state.0.join("run-v1").join("p"))?;
    let (store, _requests) = mpsc::channel(1);
    let local = iroh::SecretKey::generate().public().into();
    let manager = ProcessManager::new(
        (local, StoreClient::new(store), Arc::new(SystemClock)),
        (&state.0, None),
        CancellationToken::new(),
    );
    let credentials = Credentials::new(ProcessId::random()?, [0x5a; 32]);
    let listener = manager.address(credentials.id()).bind()?;
    // Signalled once the proof completed, so the wait that follows is the watch itself.
    let (proven, mut proof) = tokio::sync::oneshot::channel();
    let supervisor = tokio::spawn(async move {
        let stream = listener.accept().await?;
        let (receive, send) = stream.split();
        let (mut reader, mut writer) = (FrameReader::new(receive), FrameWriter::new(send));
        let theirs = read_challenge(&mut reader).await?;
        prove_supervisor(theirs, (&mut reader, &mut writer), &credentials).await?;
        let _signalled = proven.send(());
        std::future::pending::<()>().await;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });

    // When
    let watched = tokio::time::timeout(
        Duration::from_secs(11),
        manager.watch(&credentials, ProcessState::Running),
    )
    .await;

    // Then
    assert!(
        matches!(watched, Ok(Err(ServiceError::Unavailable))),
        "a silent supervisor must not hold the watch: {:?}",
        watched.as_ref().map(|answer| answer.as_ref().err())
    );
    assert!(
        proof.try_recv().is_ok(),
        "the supervisor never proved itself"
    );
    supervisor.abort();
    Ok(())
}

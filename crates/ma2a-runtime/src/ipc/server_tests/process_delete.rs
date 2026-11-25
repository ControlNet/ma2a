//! A local Process deletion against a leftover directory it cannot remove.
#![cfg(unix)]

use std::{os::unix::fs::PermissionsExt as _, time::Duration};

use ma2a_core::{
    EndpointId, RequestId,
    service::{ProcessRequest, ProcessSpec, ProcessStart},
};
use ma2a_services::{FrameReader, FrameWriter};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    services::{call, text},
    *,
};
use crate::processes::ProcessHosting;

/// Starts one Process that exits at once, as a remote `start` would, and returns its ID.
async fn exited(runtime: &Runtime, client: &LocalApiClient) -> TestResult<String> {
    let manager = runtime.handle().processes().ok_or("no Processes")?.clone();
    let spec = ProcessSpec::new(
        Some("/bin/sh".to_owned()),
        vec!["-c".to_owned(), "true".to_owned()],
        Some("/".to_owned()),
    )?;
    let start = ProcessStart::new(spec, false, 1_048_576)?;
    let (host, _caller) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(host);
    let (mut reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    let creator: EndpointId = iroh::SecretKey::generate().public().into();
    let scope = (
        (creator, RequestId::try_from([9_u8; 16].as_slice())?),
        ProcessRequest::Start(start),
        &CancellationToken::new(),
    );
    let _report = crate::processes::serve(&manager, scope, (&mut reader, &mut writer)).await;
    let list = json!({"version": 1, "operation": "process_list", "after": null});
    for _ in 0..500 {
        let listed = call(client, list.clone()).await?;
        if text(&listed, "/result/payload/processes/0/state") == Some("exited") {
            let id = text(&listed, "/result/payload/processes/0/process_id");
            return Ok(id.ok_or("no Process ID")?.to_owned());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err("the Process did not exit".into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deletion_that_committed_is_answered_even_when_its_directory_stays() -> TestResult {
    // Given: an exited Process whose directory can no longer be removed, and whose output
    // log can no longer be read.
    let state = TempState::new_named("pdel")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_processes(config, ProcessHosting::in_process()).await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let live = LiveServer::spawn(server);
    let client = LocalApiClient::new(paths);
    let id = exited(&runtime, &client).await?;
    let directory = state.0.join("processes-v1").join(&id);
    std::fs::remove_dir_all(directory.join("log"))?;
    std::fs::write(directory.join("log"), b"not a log")?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500))?;

    // When
    let request = json!({
        "version": 1, "operation": "process_delete",
        "request_id": "2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c", "process_id": id,
    });
    let deleted = call(&client, request.clone()).await?;
    let replayed = call(&client, request).await?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;

    // Then: the archive committed, so the answer says so, and its replay answers the same.
    assert_eq!(
        text(&deleted, "/result/type"),
        Some("process_deleted"),
        "{deleted}"
    );
    assert_eq!(
        deleted.pointer("/result/payload/archived"),
        Some(&Value::Bool(true))
    );
    assert_eq!(text(&replayed, "/result/type"), Some("process_deleted"));
    assert_eq!(live.cancel().await?, ServerExit::Cancelled);
    runtime.shutdown().await?;
    Ok(())
}

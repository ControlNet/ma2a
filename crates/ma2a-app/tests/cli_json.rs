//! Stable machine-readable CLI output coverage.

use serde_json::Value;

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::{DaemonFixture, TestResult};

#[test]
fn status_json_uses_the_local_api_envelope() -> TestResult {
    // Given
    let fixture = DaemonFixture::running("cli-json-status")?;

    // When
    let output = fixture.run(&["status", "--json"])?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("snapshot")
    );

    fixture.shutdown()
}

#[test]
fn space_create_commits_membership_through_the_runtime() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::running("cli-json-space")?;

    // When
    let created = fixture.run(&["space", "create", "Personal", "--json"])?;

    // Then
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let response: Value = serde_json::from_slice(&created.stdout)?;
    assert_eq!(
        response.pointer("/result/type").and_then(Value::as_str),
        Some("space_created")
    );
    assert_eq!(
        response
            .pointer("/result/payload/name")
            .and_then(Value::as_str),
        Some("Personal")
    );
    let created_revision = response
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or("missing creation revision")?;
    let space_id = response
        .pointer("/result/payload/space_id")
        .and_then(Value::as_str)
        .ok_or("missing created Space ID")?;
    let listed = fixture.run(&["space", "list", "--json"])?;
    let list_response: Value = serde_json::from_slice(&listed.stdout)?;
    assert_eq!(
        list_response
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("spaces")
    );
    let shown = fixture.run(&["space", "show", space_id, "--json"])?;
    let show_response: Value = serde_json::from_slice(&shown.stdout)?;
    assert_eq!(
        show_response
            .pointer("/result/type")
            .and_then(Value::as_str),
        Some("space")
    );
    let status = fixture.run(&["status", "--json"])?;
    let snapshot: Value = serde_json::from_slice(&status.stdout)?;
    assert_creation_receipt(&fixture, &response, &snapshot)?;
    assert!(
        snapshot
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or("missing snapshot revision")?
            >= created_revision
    );
    assert_eq!(
        snapshot
            .pointer("/result/payload/spaces")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        snapshot
            .pointer("/result/payload/spaces/0/name")
            .and_then(Value::as_str),
        Some("Personal")
    );

    // A fresh Runtime boot over the same store is what proves the label is durable.
    fixture.stop_owned()?;
    fixture.start_owned()?;
    let restarted = fixture.run(&["space", "list", "--json"])?;
    let restarted_response: Value = serde_json::from_slice(&restarted.stdout)?;
    assert_eq!(
        restarted_response
            .pointer("/result/payload/0/name")
            .and_then(Value::as_str),
        Some("Personal")
    );
    fixture.shutdown()
}

// Address publication can advance Store after the creation transaction. The
// creation reply must remain the exact durable receipt, not that later revision.
fn assert_creation_receipt(
    fixture: &DaemonFixture,
    response: &Value,
    snapshot: &Value,
) -> TestResult {
    let config = ma2a_store::StoreConfig::new(fixture.state_dir());
    let sql = rusqlite::Connection::open(config.database_path())?;
    let request_id = response
        .get("request_id")
        .and_then(Value::as_str)
        .ok_or("missing creation request ID")?;
    let (revision, encoded): (u64, Vec<u8>) = sql.query_row(
        "SELECT revision, response FROM local_mutation_replay WHERE lower(hex(request_id)) = ?1",
        [request_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(
        response.get("revision").and_then(Value::as_u64),
        Some(revision)
    );
    assert_eq!(&serde_json::from_slice::<Value>(&encoded)?, response);
    assert_eq!(
        snapshot.pointer("/result/payload/spaces/0/space_id"),
        response.pointer("/result/payload/space_id")
    );
    Ok(())
}

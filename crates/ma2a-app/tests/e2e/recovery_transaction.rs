use std::{
    io::{BufRead as _, BufReader, Write as _},
    process::{Command, Stdio},
};

use ma2a_store::Repository;
use rusqlite::Connection;

use super::harness::{TempState, TestResult, emit};

const CHILD_ENV: &str = "MA2A_E2E_STORE_CRASH_CHILD";
const STATE_ENV: &str = "MA2A_E2E_STORE_CRASH_STATE";
const READY: &str = "MA2A_E2E_STORE_CRASH_READY";

#[test]
fn forced_process_kill_preserves_all_or_none_store_transaction_and_integrity() -> TestResult {
    if std::env::var_os(CHILD_ENV).is_some() {
        return crash_child();
    }
    let state = TempState::new("store-crash")?;
    let config = state.config();
    drop(Repository::open(&config)?);
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "recovery_transaction::forced_process_kill_preserves_all_or_none_store_transaction_and_integrity",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .env(STATE_ENV, config.database_path().parent().ok_or("state path missing")?)
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("crash child stdout missing")?;
    let mut lines = BufReader::new(stdout).lines();
    while lines
        .next()
        .ok_or("crash child exited before readiness")??
        != READY
    {}

    child.kill()?;
    assert!(!child.wait()?.success());
    let repository = Repository::open(&config)?;
    assert_eq!(repository.revision()?, 0);
    drop(repository);
    let connection = Connection::open(config.database_path())?;
    let spaces = connection.query_row("SELECT COUNT(*) FROM spaces", [], |row| {
        row.get::<_, u32>(0)
    })?;
    let integrity =
        connection.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
    assert_eq!(spaces, 0);
    assert_eq!(integrity, "ok");
    emit(&serde_json::json!({
        "scenario": "store-process-kill-recovery",
        "endpoint_ids": {"process": std::process::id().to_string()},
        "revision": 0,
        "space_rows": spaces,
        "integrity_check": integrity,
        "all_or_none": true
    }));
    Ok(())
}

fn crash_child() -> TestResult {
    let state = std::env::var_os(STATE_ENV).ok_or("crash child state path missing")?;
    let config = ma2a_store::StoreConfig::new(std::path::PathBuf::from(state));
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    connection.execute(
        "INSERT INTO spaces(space_id, genesis_cbor) VALUES (?1, ?2)",
        ([0x91_u8; 32].as_slice(), b"uncommitted".as_slice()),
    )?;
    connection.execute(
        "UPDATE runtime_metadata SET revision = revision + 1 WHERE singleton = 1",
        [],
    )?;
    println!("{READY}");
    std::io::stdout().flush()?;
    loop {
        std::thread::park();
    }
}

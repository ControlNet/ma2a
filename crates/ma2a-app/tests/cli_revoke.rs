//! Installed CLI coverage for explicit Space member revocation.

use std::error::Error;

use ma2a_core::{
    EndpointId, MemberCapabilities, SpaceId, SpaceManifestMembership, SpaceMemberV1,
    default_member_label,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, StoreConfig};
use serde_json::Value;

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::DaemonFixture;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

#[test]
fn space_member_revoke_requires_and_applies_explicit_ids() -> TestResult {
    // Given
    let mut fixture = DaemonFixture::running("cli-revoke")?;
    let created = fixture.run_ok(&["space", "create", "Revocable", "--json"])?;
    let created_response: Value = serde_json::from_slice(&created.stdout)?;
    let space_text = value(&created_response, "/result/payload/space_id")?;
    let endpoint = fixture.run_ok(&["endpoint", "show", "--json"])?;
    let endpoint_response: Value = serde_json::from_slice(&endpoint.stdout)?;
    let owner_text = value(&endpoint_response, "/result/payload/endpoint_id")?;
    // The store is opened directly below, so the daemon that owns it must be
    // proven gone first rather than merely asked to leave.
    fixture.stop_owned()?;
    let space_id = SpaceId::try_from(hex_bytes::<32>(space_text)?.as_slice())?;
    let owner_id = EndpointId::try_from(hex_bytes::<32>(owner_text)?.as_slice())?;
    let peer_id = EndpointId::try_from(
        [
            0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e,
            0xab, 0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06,
            0x98, 0x81, 0xdb, 0x12,
        ]
        .as_slice(),
    )?;
    add_peer(fixture.state_dir(), space_id, (owner_id, peer_id))?;
    fixture.start_owned()?;
    let peer_text = encode_hex(peer_id.as_bytes())?;

    let before = fixture.json(&["status", "--json"])?;
    reject_owner_removal(&fixture, (space_text, owner_text), &before)?;

    // When
    let revoked = fixture.run(&[
        "space", "member", "remove", space_text, &peer_text, "--json",
    ])?;

    // Then
    assert!(
        revoked.status.success(),
        "{}",
        String::from_utf8_lossy(&revoked.stderr)
    );
    let response: Value = serde_json::from_slice(&revoked.stdout)?;
    assert_eq!(value(&response, "/result/type")?, "space_revoked");
    assert_eq!(
        response
            .pointer("/result/payload/member_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    let status = fixture.json(&["status", "--json"])?;
    assert_eq!(
        status
            .pointer("/result/payload/spaces/0/member_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        response.get("revision").and_then(Value::as_u64),
        before
            .get("revision")
            .and_then(Value::as_u64)
            .map(|revision| revision + 1)
    );
    fixture.stop_owned()?;
    let repository = Repository::open(&StoreConfig::new(fixture.state_dir()))?;
    let chain = repository
        .load_space_chain(space_id)?
        .ok_or("missing Space")?;
    assert_eq!(chain.latest_generation(), 2);
    assert_eq!(chain.members().len(), 1);
    assert_eq!(
        chain
            .members()
            .first()
            .ok_or("missing owner")?
            .endpoint_id(),
        owner_id
    );
    assert_eq!(chain.revocations().len(), 1);
    assert_eq!(
        chain
            .revocations()
            .first()
            .ok_or("missing revocation")?
            .endpoint_id(),
        peer_id
    );
    drop(repository);
    fixture.start_owned()?;
    fixture.run_ok(&["space", "show", space_text])?;
    fixture.shutdown()
}

fn add_peer(
    state_dir: &std::path::Path,
    space_id: SpaceId,
    endpoints: (EndpointId, EndpointId),
) -> TestResult {
    let (owner_id, peer_id) = endpoints;
    let mut repository = Repository::open(&StoreConfig::new(state_dir))?;
    let mut members = vec![
        SpaceMemberV1::new(
            owner_id,
            default_member_label(owner_id),
            MemberCapabilities::new(true, true),
        )?,
        SpaceMemberV1::new(
            peer_id,
            default_member_label(peer_id),
            MemberCapabilities::new(true, false),
        )?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        1_800_000_000_000,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    Ok(())
}

fn value<'a>(document: &'a Value, pointer: &str) -> Result<&'a str, Box<dyn Error + Send + Sync>> {
    document
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string at {pointer}").into())
}

fn hex_bytes<const N: usize>(value: &str) -> Result<[u8; N], Box<dyn Error + Send + Sync>> {
    if value.len() != N * 2 {
        return Err("invalid hexadecimal length".into());
    }
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> Result<String, std::fmt::Error> {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}")?;
    }
    Ok(encoded)
}

fn reject_owner_removal(fixture: &DaemonFixture, ids: (&str, &str), before: &Value) -> TestResult {
    let (space_text, owner_text) = ids;
    // Owner protection is shared by CLI, IPC, and the Web command executor.
    let refused = fixture.run(&["space", "member", "remove", space_text, owner_text])?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8(refused.stderr)?.contains("Space owner cannot be removed in Phase 1")
    );
    let request = ma2a_runtime::api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1, "operation": "space_revoke",
        "request_id": "aabbccddeeff00112233445566778899",
        "space_id": space_text, "peer_endpoint_id": owner_text,
    }))?)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let client = ma2a_runtime::ipc::LocalApiClient::new(ma2a_runtime::ipc::IpcPaths::new(
        fixture.state_dir(),
    )?);
    for _ in 0..2 {
        let response: Value = serde_json::from_slice(&runtime.block_on(client.call(&request))?)?;
        assert_eq!(value(&response, "/error")?, "unauthorized");
        assert_eq!(
            value(&response, "/remediation")?,
            "Space owner cannot be removed in Phase 1"
        );
    }
    let after = fixture.json(&["status", "--json"])?;
    assert_eq!(after.get("revision"), before.get("revision"));
    assert_eq!(
        after.pointer("/result/payload/spaces"),
        before.pointer("/result/payload/spaces")
    );

    Ok(())
}

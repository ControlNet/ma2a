//! Installed CLI coverage for explicit Space member revocation.

use std::{error::Error, fs, path::PathBuf, process::Command};

use ma2a_core::{EndpointId, MemberCapabilities, SpaceId, SpaceManifestMembership, SpaceMemberV1};
use ma2a_store::{OwnedSpaceUpdate, Repository, StoreConfig};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn space_member_revoke_requires_and_applies_explicit_ids() -> TestResult {
    // Given
    let state_dir = state_dir()?;
    assert!(run(&state_dir, &["start"])?.status.success());
    let created = run(
        &state_dir,
        &["space", "create", "--name", "Revocable", "--json"],
    )?;
    let created_response: Value = serde_json::from_slice(&created.stdout)?;
    let space_text = value(&created_response, "/result/payload/space_id")?;
    let endpoint = run(&state_dir, &["endpoint", "show", "--json"])?;
    let endpoint_response: Value = serde_json::from_slice(&endpoint.stdout)?;
    let owner_text = value(&endpoint_response, "/result/payload/endpoint_id")?;
    let shutdown = run(&state_dir, &["stop"])?;
    assert!(shutdown.status.success());
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
    add_peer(&state_dir, space_id, (owner_id, peer_id))?;
    assert!(run(&state_dir, &["start"])?.status.success());
    let peer_text = encode_hex(peer_id.as_bytes())?;

    // When
    let revoked = run(
        &state_dir,
        &[
            "space",
            "member",
            "revoke",
            "--space",
            space_text,
            "--endpoint",
            &peer_text,
            "--json",
        ],
    )?;

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
    let status: Value = serde_json::from_slice(&run(&state_dir, &["status", "--json"])?.stdout)?;
    assert_eq!(
        status
            .pointer("/result/payload/spaces/0/member_count")
            .and_then(Value::as_u64),
        Some(1)
    );
    let _shutdown = run(&state_dir, &["stop"]);
    fs::remove_dir_all(state_dir)?;
    Ok(())
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
            "local-endpoint".to_owned(),
            MemberCapabilities::new(true, true),
        )?,
        SpaceMemberV1::new(
            peer_id,
            "revocable-peer".to_owned(),
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

fn run(
    state_dir: &std::path::Path,
    arguments: &[&str],
) -> Result<std::process::Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(state_dir)
        .args(arguments)
        .output()
}

fn value<'a>(document: &'a Value, pointer: &str) -> Result<&'a str, Box<dyn Error>> {
    document
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string at {pointer}").into())
}

fn hex_bytes<const N: usize>(value: &str) -> Result<[u8; N], Box<dyn Error>> {
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

fn state_dir() -> Result<PathBuf, std::io::Error> {
    let path = std::env::temp_dir().join(format!("ma2a-cli-revoke-{}", std::process::id()));
    fs::create_dir(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

//! Installed-CLI coverage for the piped invite/accept and leave workflows.

use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

use ma2a_core::SpaceId;
use ma2a_store::{Repository, StoreConfig};
use serde_json::Value;

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::{DaemonFixture, TestResult, TestValue};

/// One Endpoint, with the daemon serving it owned by this test.
struct Endpoint(DaemonFixture);

impl Endpoint {
    fn start(label: &str) -> TestValue<Self> {
        Ok(Self(DaemonFixture::running(&format!(
            "cli-membership-{label}"
        ))?))
    }

    fn path(&self) -> &Path {
        self.0.state_dir()
    }

    fn command(&self, arguments: &[&str]) -> Command {
        self.0.command(arguments)
    }

    fn run(&self, arguments: &[&str]) -> TestValue<Output> {
        self.0.run(arguments)
    }

    fn shutdown(self) -> TestResult {
        self.0.shutdown()
    }

    fn spaces(&self) -> TestValue<Vec<Value>> {
        let listed = self.run(&["space", "list", "--json"])?;
        succeed(&listed)?;
        Ok(serde_json::from_slice::<Value>(&listed.stdout)?
            .pointer("/result/payload")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
}

fn succeed(output: &Output) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Reads the signed member labels for one Space from a state directory.
fn member_labels(state_dir: &Path, space_id: SpaceId) -> TestValue<Vec<String>> {
    Ok(Repository::open(&StoreConfig::new(state_dir))?
        .load_space_chain(space_id)?
        .ok_or("Space chain is missing")?
        .members()
        .iter()
        .map(|member| member.label().to_owned())
        .collect())
}

fn hex_bytes(value: &str) -> TestValue<Vec<u8>> {
    if value.len() != 64 {
        return Err("Space identifier must be 64 hexadecimal characters".into());
    }
    (0..32)
        .map(|index| {
            let pair = value
                .get(index * 2..index * 2 + 2)
                .ok_or_else(|| "truncated Space identifier".into());
            pair.and_then(|pair| u8::from_str_radix(pair, 16).map_err(Into::into))
        })
        .collect()
}

fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_piped_invite_joins_a_named_space_and_a_member_can_leave_it() -> TestResult {
    // Given
    let owner = Endpoint::start("owner")?;
    let member = Endpoint::start("member")?;
    let created = owner.run(&["space", "create", "lab", "--json"])?;
    succeed(&created)?;
    let space_id = text(
        &serde_json::from_slice::<Value>(&created.stdout)?,
        "/result/payload/space_id",
    );

    // When: the documented pipeline joins without any flag or file.
    let mut invite = owner
        .command(&["space", "invite", "lab"])
        .stdout(Stdio::piped())
        .spawn()?;
    let ticket = invite.stdout.take().ok_or("missing invite stdout")?;
    let accepted = member
        .command(&["space", "accept"])
        .stdin(Stdio::from(ticket))
        .output()?;
    succeed(&invite.wait_with_output()?)?;

    // Then: the joining Endpoint knows the shared Space name it never configured.
    succeed(&accepted)?;
    let joined = member.spaces()?;
    let [joined] = joined.as_slice() else {
        return Err("the member must belong to exactly one Space".into());
    };
    assert_eq!(text(joined, "/name"), "lab");
    assert_eq!(text(joined, "/space_id"), space_id);
    let shown = member.run(&["space", "show", "lab"])?;
    succeed(&shown)?;
    assert!(String::from_utf8(shown.stdout)?.contains("Name: lab"));

    // The Space name and the member labels are separate signed facts, so no
    // member may be labelled "lab" and no placeholder may stand in for an
    // Endpoint. This is what the console reads, so it must be right in the data.
    let space = SpaceId::try_from(hex_bytes(&space_id)?.as_slice())?;
    let labels = member_labels(owner.path(), space)?;
    assert_eq!(labels.len(), 2, "{labels:?}");
    for label in &labels {
        assert_ne!(label, "lab");
        assert_ne!(label, "local-endpoint");
        assert!(label.starts_with("endpoint-"), "{labels:?}");
    }
    assert_eq!(member_labels(member.path(), space)?, labels);

    // The owner cannot leave, and a member's departure is authority-signed.
    let refused = owner.run(&["space", "leave", "lab"])?;
    assert!(!refused.status.success());
    assert!(String::from_utf8(refused.stderr)?.contains("Space owner cannot leave its own Space"));
    let left = member.run(&["space", "leave", "lab", "--json"])?;
    succeed(&left)?;
    let departure: Value = serde_json::from_slice(&left.stdout)?;
    assert_eq!(text(&departure, "/result/type"), "space_left");
    assert_eq!(text(&departure, "/result/payload/name"), "lab");
    assert_eq!(text(&departure, "/result/payload/space_id"), space_id);
    // Departure must not report pre-leave membership as post-leave state.
    assert!(
        departure.pointer("/result/payload/member_count").is_none(),
        "{departure}"
    );
    assert!(member.spaces()?.is_empty());
    let owner_spaces = owner.spaces()?;
    let [remaining] = owner_spaces.as_slice() else {
        return Err("the owner must still belong to exactly one Space".into());
    };
    assert_eq!(
        remaining.pointer("/member_count").and_then(Value::as_u64),
        Some(1),
        "the owner's signed membership must shrink when a member leaves"
    );

    // A departed Endpoint rejoins through a fresh invite.
    let mut rejoin = owner
        .command(&["space", "invite", "lab", "--ttl", "60s"])
        .stdout(Stdio::piped())
        .spawn()?;
    let ticket = rejoin.stdout.take().ok_or("missing invite stdout")?;
    let rejoined = member
        .command(&["space", "accept"])
        .stdin(Stdio::from(ticket))
        .output()?;
    succeed(&rejoin.wait_with_output()?)?;
    succeed(&rejoined)?;
    assert_eq!(member.spaces()?.len(), 1);

    // Human departure output names the Space it left and nothing it can no
    // longer speak for.
    let departed = member.run(&["space", "leave", "lab"])?;
    succeed(&departed)?;
    let departed = String::from_utf8(departed.stdout)?;
    assert!(departed.contains("Left Space"), "{departed}");
    assert!(departed.contains("Name: lab"), "{departed}");
    assert!(
        !departed.contains("Members:"),
        "post-departure output must not report membership: {departed}"
    );
    member.shutdown()?;
    owner.shutdown()
}

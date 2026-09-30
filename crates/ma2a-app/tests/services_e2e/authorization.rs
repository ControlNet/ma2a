//! Endpoint-centric authorization between real daemons: exactly one complete Space.

use super::{
    daemon_fixture::TestResult,
    harness::{Lab, Node, Root, join, refused, succeed, text},
};

#[test]
fn unrelated_revoked_and_zero_space_endpoints_get_nothing() -> TestResult {
    let lab = Lab::start("denied", "1")?;
    succeed(&lab.remote(&["file"], &["stat", "docs:/"])?)?;

    // An Endpoint sharing no Space with the target cannot even locate it: address lookup is
    // Space-scoped, so nothing about the target is reachable.
    let outsider = Node::start("denied-outsider")?;
    let unrelated = outsider.run(&["file", "--endpoint", lab.owner.id(), "stat", "docs:/"])?;
    refused(&unrelated, "unavailable")?;
    let exec = outsider.run(&["exec", "--endpoint", lab.owner.id(), "--", "true"])?;
    refused(&exec, "unavailable")?;

    // A zero-Space Endpoint cannot consent to serve anything, and serves nothing.
    let foreign = "ab".repeat(32);
    refused(
        &outsider.run(&["service", "expose", "exec", &foreign])?,
        "not_found",
    )?;
    let listed = outsider.json(&["service", "list", "--json"])?;
    assert_eq!(
        listed.pointer("/result/payload"),
        Some(&serde_json::json!([]))
    );

    // A revoked member loses every service at once.
    succeed(
        &lab.owner
            .run(&["space", "member", "remove", &lab.space, lab.member.id()])?,
    )?;
    refused(&lab.remote(&["file"], &["stat", "docs:/"])?, "unauthorized")?;
    refused(&lab.remote(&["exec"], &["--", "true"])?, "unauthorized")?;
    outsider.shutdown()?;
    lab.shutdown()
}

#[test]
fn two_incomplete_spaces_never_combine_and_one_complete_space_permits() -> TestResult {
    let owner = Node::start("split-owner")?;
    let member = Node::start("split-member")?;
    let root = Root::new("split")?;
    // `left` grants Files read but exposes no root; `right` exposes the root but grants
    // nothing. Together they would authorize; separately neither does.
    let left = join(&owner, &member, "left")?;
    let right = join(&owner, &member, "right")?;
    succeed(&owner.run(&["space", "grant", &left, member.id(), "--files-read"])?)?;
    succeed(&owner.run(&["service", "expose", "files", &right, "docs", root.as_arg()?])?)?;
    let shown = owner.json(&["space", "show", &right, "--json"])?;
    assert!(
        !shown.to_string().contains("\"files_read\":true"),
        "{shown}"
    );
    let stat = ["file", "--endpoint", owner.id(), "stat", "docs:/"];
    refused(&member.run(&stat)?, "unauthorized")?;

    // Completing either Space alone is enough.
    succeed(&owner.run(&["space", "grant", &right, member.id(), "--files-read"])?)?;
    succeed(&member.run(&stat)?)?;
    // Write needs its own grant and a read-write root in the same Space.
    let write = member.piped(
        &["file", "--endpoint", owner.id(), "write", "docs:/x"],
        b"x",
    )?;
    refused(&write, "unauthorized")?;
    let exposures = owner.json(&["service", "list", "--json"])?;
    assert_eq!(text(&exposures, "/result/payload/0/space_id"), right);
    member.shutdown()?;
    owner.shutdown()
}

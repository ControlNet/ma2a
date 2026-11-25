use crate::{error::XtaskError, release_tag};

#[test]
fn a_tag_naming_the_version_publishes() {
    // When
    let publishes = release_tag::publishes("refs/tags/v0.1.0", "0.1.0");

    // Then
    assert!(matches!(publishes, Ok(true)), "{publishes:?}");
}

#[test]
fn a_branch_builds_without_publishing() {
    // When
    let publishes = release_tag::publishes("refs/heads/master", "0.1.0");

    // Then
    assert!(matches!(publishes, Ok(false)), "{publishes:?}");
}

#[test]
fn any_other_tag_is_refused() {
    for tag in [
        "refs/tags/v0.2.0",
        "refs/tags/V0.1.0",
        "refs/tags/0.1.0",
        "refs/tags/v0.1.0-rc.1",
    ] {
        // When
        let publishes = release_tag::publishes(tag, "0.1.0");

        // Then
        assert!(
            matches!(publishes, Err(XtaskError::Policy { .. })),
            "{tag}: {publishes:?}"
        );
    }
}

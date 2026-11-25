//! Tests for a download's pinned directories on Windows.

use std::ffi::OsStr;

use super::Anchor;

#[test]
fn an_anchored_directory_and_its_parent_cannot_be_renamed() -> Result<(), Box<dyn std::error::Error>>
{
    let base = std::env::temp_dir().join(format!("ma2a-anchor-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    let (parent, child) = (base.join("parent"), base.join("parent").join("child"));
    std::fs::create_dir_all(&child)?;

    let anchor = Anchor::open(&parent)?;
    let moved = std::fs::rename(&parent, base.join("moved"));
    let pinned_child = anchor.child(OsStr::new("child"));
    let child_moved = std::fs::rename(&child, parent.join("moved"));

    let was_pinned = pinned_child.map(drop);
    drop(anchor);
    std::fs::remove_dir_all(&base)?;
    assert!(moved.is_err(), "{moved:?}");
    assert!(was_pinned.is_ok(), "{was_pinned:?}");
    assert!(child_moved.is_err(), "{child_moved:?}");
    Ok(())
}

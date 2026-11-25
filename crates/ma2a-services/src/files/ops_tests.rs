//! Listing pages, stopped operations, and addressable names.

use ma2a_core::service::{FilesResult, RemotePath, ServiceError};

use super::{Failure, Smallest, Stop, addressable, list, remove};

#[test]
fn a_stopped_listing_ends_without_a_page() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("ma2a-list-stop-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("dir"))?;
    std::fs::write(root.join("dir/a"), b"a")?;
    let path = RemotePath::parse("docs:/dir")?;

    let stopped = Stop::default();
    drop(stopped.on_drop());
    assert_eq!(
        list(&root, &path, (None, &stopped)),
        Err(ServiceError::Cancelled)
    );
    let listed = list(&root, &path, (None, &Stop::default()));
    assert!(matches!(listed, Ok(FilesResult::Listing(entries, None)) if entries.len() == 1));
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_stopped_recursive_removal_changes_nothing_more() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("ma2a-stop-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("tree/inner"))?;
    std::fs::write(root.join("tree/a"), b"a")?;
    std::fs::write(root.join("tree/inner/b"), b"b")?;
    let path = RemotePath::parse("docs:/tree")?;

    // Once its caller stopped waiting, the removal stops before its first change.
    let stopped = Stop::default();
    drop(stopped.on_drop());
    assert_eq!(
        remove(&root, &path, (true, &stopped)),
        Err(Failure {
            error: ServiceError::Cancelled,
            partial: true
        })
    );
    assert_eq!(std::fs::read(root.join("tree/a"))?, b"a");
    assert_eq!(std::fs::read(root.join("tree/inner/b"))?, b"b");

    // Otherwise the whole tree goes.
    assert_eq!(
        remove(&root, &path, (true, &Stop::default())),
        Ok(FilesResult::Done)
    );
    assert!(!root.join("tree").exists());
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_recursive_removal_keeps_what_lies_too_deep() -> Result<(), Box<dyn std::error::Error>> {
    // Given: a chain of directories deeper than a removal enters, with a file near the top
    // and one at the bottom.
    let root = std::env::temp_dir().join(format!("ma2a-deep-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&root);
    let deepest = (0..70).fold(root.join("tree"), |path, level| {
        path.join(level.to_string())
    });
    std::fs::create_dir_all(&deepest)?;
    std::fs::write(root.join("tree/0/near"), b"n")?;
    std::fs::write(deepest.join("far"), b"f")?;

    // When
    let removed = remove(
        &root,
        &RemotePath::parse("docs:/tree")?,
        (true, &Stop::default()),
    );

    // Then: what it reached is gone; the deep end stays, as a kept download would.
    assert_eq!(
        removed,
        Err(Failure {
            error: ServiceError::NotEmpty,
            partial: true
        })
    );
    assert!(!root.join("tree/0/near").exists());
    assert_eq!(std::fs::read(deepest.join("far"))?, b"f");
    std::fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn a_name_longer_than_a_path_component_is_not_addressable() {
    // 85 three-byte characters fill a component; 100 exceed it, though NTFS takes them.
    assert!(addressable(&"\u{6587}".repeat(85), (0, 0)));
    assert!(!addressable(&"\u{6587}".repeat(100), (0, 0)));
    assert!(addressable(&"n".repeat(255), (0, 0)));
    assert!(!addressable(&"n".repeat(256), (0, 0)));
    assert!(!addressable("", (0, 0)));
    assert!(!addressable("bad\nname", (0, 0)));
    assert!(!addressable("bad\tname", (0, 0)));
    assert!(!addressable("bad\x7fname", (0, 0)));
    assert!(!addressable("bad\u{0085}name", (0, 0)));
}

#[test]
fn a_name_seen_twice_is_listed_once() {
    // A directory read while it changes can yield one name twice.
    let mut smallest = Smallest::default();
    for name in ["b", "a", "b", "c", "a"] {
        smallest.offer(name);
    }
    assert_eq!(
        smallest.page(),
        (vec!["a".to_owned(), "b".to_owned(), "c".to_owned()], false)
    );

    // A full page whose last name repeats leaves no next page behind it.
    let mut smallest = Smallest::default();
    let names: Vec<String> = (0..super::MAX_LIST_PAGE)
        .map(|index| format!("{index:05}"))
        .collect();
    for name in names.iter().chain(names.last()) {
        smallest.offer(name);
    }
    assert_eq!(smallest.page(), (names, false));
}

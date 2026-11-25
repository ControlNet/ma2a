//! Tests for the Files service's directory handles on Windows.

#[test]
fn no_directory_an_operation_holds_can_be_renamed() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("ma2a-files-pin-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    std::fs::create_dir_all(root.join("a").join("b"))?;

    let held = crate::files::resolve::walk(&root, &["a".to_owned(), "b".to_owned()])?;
    let renamed = [
        std::fs::rename(&root, base.join("moved")),
        std::fs::rename(root.join("a"), root.join("moved")),
        std::fs::rename(root.join("a").join("b"), root.join("a").join("moved")),
    ];

    drop(held);
    std::fs::remove_dir_all(&base)?;
    assert!(renamed.iter().all(Result::is_err), "{renamed:?}");
    Ok(())
}

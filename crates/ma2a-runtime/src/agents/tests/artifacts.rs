use super::{hex, open_regular, place_blob, stage_blob};

#[test]
fn publishing_stored_content_again_keeps_the_blob_untouched()
-> Result<(), Box<dyn std::error::Error>> {
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-blobs-{}", hex(&nonce)));
    std::fs::create_dir_all(&dir)?;
    let (source, blobs) = (dir.join("report.txt"), dir.join("blobs"));
    std::fs::write(&source, b"the same content")?;

    let (size, hash, staging) = stage_blob(open_regular(&source)?, &blobs)?;
    place_blob(&staging, &blobs, (size, &hash))?;
    let blob = blobs.join(hex(&hash));
    let first = std::fs::metadata(&blob)?;
    std::thread::sleep(std::time::Duration::from_millis(20));
    let (_, again, staging) = stage_blob(open_regular(&source)?, &blobs)?;
    place_blob(&staging, &blobs, (size, &again))?;
    let second = std::fs::metadata(&blob)?;

    // A resumed download names the blob by size and modification time.
    assert_eq!((size, again), (16, hash));
    assert_eq!(first.modified()?, second.modified()?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(first.ino(), second.ino());
    }
    assert!(!staging.exists(), "the second copy is discarded");
    assert_eq!(std::fs::read_dir(&blobs)?.count(), 1);

    // A stored blob damaged in place is replaced by sound bytes published again.
    std::fs::write(&blob, b"the some content")?;
    let (_, _, staging) = stage_blob(open_regular(&source)?, &blobs)?;
    place_blob(&staging, &blobs, (size, &hash))?;
    assert_eq!(std::fs::read(&blob)?, b"the same content");
    assert_eq!(std::fs::read_dir(&blobs)?.count(), 1);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Downloads open a blob without following a link, so a link to the right bytes is no blob.
#[cfg(unix)]
#[test]
fn a_link_to_matching_content_is_replaced_by_the_published_copy()
-> Result<(), Box<dyn std::error::Error>> {
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-blob-link-{}", hex(&nonce)));
    std::fs::create_dir_all(&dir)?;
    let (source, blobs) = (dir.join("report.txt"), dir.join("blobs"));
    std::fs::write(&source, b"the same content")?;
    std::fs::create_dir(&blobs)?;
    let hash = *blake3::hash(b"the same content").as_bytes();
    let blob = blobs.join(hex(&hash));
    std::os::unix::fs::symlink(&source, &blob)?;

    let (size, staged_hash, staging) = stage_blob(open_regular(&source)?, &blobs)?;
    place_blob(&staging, &blobs, (size, &staged_hash))?;

    assert_eq!(staged_hash, hash);
    assert!(std::fs::symlink_metadata(&blob)?.file_type().is_file());
    assert_eq!(std::fs::read(&blob)?, b"the same content");
    assert_eq!(std::fs::read_dir(&blobs)?.count(), 1);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

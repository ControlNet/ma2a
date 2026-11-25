use std::sync::Arc;

use ma2a_core::service::{AgentTaskId, ServiceError};
use tokio_util::sync::CancellationToken;

use super::{AgentManager, hex, open_regular, place_blob, stage_blob};
use crate::{clock::SystemClock, processes::ProcessManager, store::StoreClient};

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

/// A stored blob that cannot be read is no sound blob: the copy replaces it, and the staging
/// file never outlives the publication.
#[cfg(unix)]
#[test]
fn an_unreadable_blob_is_replaced_by_the_published_copy() -> Result<(), Box<dyn std::error::Error>>
{
    use std::os::unix::fs::PermissionsExt as _;
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-blob-unreadable-{}", hex(&nonce)));
    std::fs::create_dir_all(&dir)?;
    let (source, blobs) = (dir.join("report.txt"), dir.join("blobs"));
    std::fs::write(&source, b"the same content")?;
    let (size, hash, staging) = stage_blob(open_regular(&source)?, &blobs)?;
    place_blob(&staging, &blobs, (size, &hash))?;
    let blob = blobs.join(hex(&hash));
    std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o000))?;

    let (_, _, staging) = stage_blob(open_regular(&source)?, &blobs)?;
    let placed = place_blob(&staging, &blobs, (size, &hash));

    let left = std::fs::read_dir(&blobs)?.count();
    let content = std::fs::read(&blob);
    std::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o600))?;
    std::fs::remove_dir_all(&dir)?;
    placed?;
    assert_eq!(content?, b"the same content");
    assert_eq!(left, 1, "the staging file is gone");
    Ok(())
}

/// An Agent manager over a state directory whose Store never answers.
fn storeless(dir: &std::path::Path) -> AgentManager {
    let (store, _requests) = tokio::sync::mpsc::channel(1);
    let store = StoreClient::new(store);
    let local = iroh::SecretKey::generate().public().into();
    let shutdown = CancellationToken::new();
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::new(SystemClock)),
        (dir, None),
        shutdown.clone(),
    );
    AgentManager::new(
        (local, store, Arc::new(SystemClock)),
        (dir, processes),
        shutdown,
    )
}

#[tokio::test]
async fn a_publication_with_an_invalid_name_copies_nothing()
-> Result<(), Box<dyn std::error::Error>> {
    // Given
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-publish-name-{}", hex(&nonce)));
    std::fs::create_dir_all(&dir)?;
    let source = dir.join("report.txt");
    std::fs::write(&source, b"some content")?;
    let manager = storeless(&dir);

    // When
    let file = open_regular(&source)?;
    let published = manager
        .publish(
            (AgentTaskId::random()?, "c1"),
            (".hidden", file, "text/plain"),
        )
        .await;

    // Then: refused before a byte was copied into the blob store.
    let created = manager.blobs().exists();
    std::fs::remove_dir_all(&dir)?;
    assert_eq!(published.err(), Some(ServiceError::InvalidInput));
    assert!(!created, "the blob store was touched");
    Ok(())
}

/// A blob store this Runtime cannot write is its own failure, not the Agent's input.
#[cfg(unix)]
#[tokio::test]
async fn a_blob_store_that_cannot_be_written_is_no_invalid_input()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    // Given
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-publish-io-{}", hex(&nonce)));
    std::fs::create_dir_all(&dir)?;
    let source = dir.join("report.txt");
    std::fs::write(&source, b"some content")?;
    let manager = storeless(&dir);
    let blobs = manager.blobs();
    std::fs::create_dir_all(&blobs)?;
    std::fs::set_permissions(&blobs, std::fs::Permissions::from_mode(0o500))?;

    // When
    let file = open_regular(&source)?;
    let published = manager
        .publish(
            (AgentTaskId::random()?, "c1"),
            ("report.txt", file, "text/plain"),
        )
        .await;

    // Then
    std::fs::set_permissions(&blobs, std::fs::Permissions::from_mode(0o700))?;
    std::fs::remove_dir_all(&dir)?;
    assert_eq!(published.err(), Some(ServiceError::Internal));
    Ok(())
}

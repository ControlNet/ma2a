use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use zeroize::Zeroizing;

use crate::{StoreError, permissions};

#[cfg(test)]
mod tests;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// The two private-key classes allowed in Phase 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "schema v1 defines exactly these two protected-key classes"
)]
pub enum KeyKind {
    /// The one persistent Iroh Endpoint key.
    Endpoint,
    /// One Space authority signing key.
    SpaceAuthority,
}

impl KeyKind {
    const fn directory(self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::SpaceAuthority => "space-authority",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Endpoint => "Endpoint",
            Self::SpaceAuthority => "Space authority",
        }
    }
}

/// An opaque, path-safe identifier stored in `SQLite` instead of private bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyReference(String);

impl KeyReference {
    /// Parses a path-safe opaque key reference.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidKeyReference`] when `value` is empty, too
    /// long, reserved, or contains path-unsafe bytes.
    pub fn parse(value: &str) -> Result<Self, StoreError> {
        let valid = !value.is_empty()
            && value.len() <= 96
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
        if !valid || value == "." || value == ".." {
            return Err(StoreError::InvalidKeyReference);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the opaque database value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Secret bytes that are securely zeroized when dropped.
pub struct ProtectedSecret(Zeroizing<Vec<u8>>);

impl ProtectedSecret {
    /// Borrows the loaded secret for its shortest necessary use.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl AsRef<[u8]> for ProtectedSecret {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Protected-key material and its immutable storage identity.
#[derive(Clone, Copy, Debug)]
pub struct KeyMaterial<'a> {
    kind: KeyKind,
    reference: &'a KeyReference,
    secret: &'a [u8],
}

impl<'a> KeyMaterial<'a> {
    /// Creates one protected-key write request.
    pub const fn new(kind: KeyKind, reference: &'a KeyReference, secret: &'a [u8]) -> Self {
        Self {
            kind,
            reference,
            secret,
        }
    }
}

impl fmt::Debug for ProtectedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProtectedSecret([REDACTED])")
    }
}

/// Synchronous owner-only filesystem storage for private-key bytes.
#[derive(Clone, Debug)]
pub struct KeyStore {
    root: PathBuf,
}

trait PublicationOperations {
    fn hard_link(&self, temporary: &Path, destination: &Path) -> std::io::Result<()>;
    fn remove_temporary(&self, temporary: &Path) -> std::io::Result<()>;
    fn sync_parent(&self, destination: &Path) -> Result<(), StoreError>;
    fn validate_destination(&self, destination: &Path) -> Result<(), StoreError>;
}

struct FilesystemPublication;

impl PublicationOperations for FilesystemPublication {
    fn hard_link(&self, temporary: &Path, destination: &Path) -> std::io::Result<()> {
        fs::hard_link(temporary, destination)
    }

    fn remove_temporary(&self, temporary: &Path) -> std::io::Result<()> {
        fs::remove_file(temporary)
    }

    fn sync_parent(&self, destination: &Path) -> Result<(), StoreError> {
        sync_parent(destination)
    }

    fn validate_destination(&self, destination: &Path) -> Result<(), StoreError> {
        permissions::validate_private_file(destination, "protected-key file")
    }
}

impl KeyStore {
    /// Opens and validates the protected-key directory hierarchy.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the directory cannot be created or fails
    /// current-user ownership and permission validation.
    pub fn open(state_dir: &Path) -> Result<Self, StoreError> {
        permissions::ensure_private_dir(state_dir, "state directory")?;
        let root = state_dir.join("keys");
        permissions::ensure_private_dir(&root, "protected-key directory")?;
        for kind in [KeyKind::Endpoint, KeyKind::SpaceAuthority] {
            permissions::ensure_private_dir(
                &root.join(kind.directory()),
                "protected-key directory",
            )?;
        }
        let store = Self { root };
        store.validate_all()?;
        Ok(store)
    }

    /// Atomically creates immutable protected-key material under an opaque reference.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the reference already exists or the atomic,
    /// owner-only filesystem write cannot be completed.
    pub fn write(&self, material: KeyMaterial<'_>) -> Result<(), StoreError> {
        self.write_with_operations(material, &FilesystemPublication)
    }

    fn write_with_operations(
        &self,
        material: KeyMaterial<'_>,
        operations: &impl PublicationOperations,
    ) -> Result<(), StoreError> {
        let destination = self.path_for(material.kind, material.reference);
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = destination.with_extension(format!("tmp-{}-{serial}", std::process::id()));
        let write_result = Self::write_temporary(&temporary, material.secret);
        if let Err(error) = write_result {
            let _cleanup_result = operations.remove_temporary(&temporary);
            return Err(error);
        }
        if let Err(error) = operations.hard_link(&temporary, &destination) {
            let _cleanup_result = operations.remove_temporary(&temporary);
            return if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err(StoreError::ProtectedKeyAlreadyExists)
            } else {
                Err(error.into())
            };
        }
        let _cleanup_result = operations.remove_temporary(&temporary);
        operations.sync_parent(&destination)?;
        operations.validate_destination(&destination)
    }

    /// Loads protected material into a buffer that zeroizes on drop.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the reference is absent, insecure, or cannot
    /// be read.
    pub fn read(
        &self,
        kind: KeyKind,
        reference: &KeyReference,
    ) -> Result<ProtectedSecret, StoreError> {
        let path = self.path_for(kind, reference);
        self.validate_reference(kind, reference)?;
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)?.read_to_end(&mut bytes)?;
        Ok(ProtectedSecret(bytes))
    }

    /// Returns the local path for permission inspection and controlled maintenance.
    pub fn path_for(&self, kind: KeyKind, reference: &KeyReference) -> PathBuf {
        self.root
            .join(kind.directory())
            .join(format!("{}.key", reference.as_str()))
    }

    pub(crate) fn validate_reference(
        &self,
        kind: KeyKind,
        reference: &KeyReference,
    ) -> Result<(), StoreError> {
        let path = self.path_for(kind, reference);
        if !path.exists() {
            return Err(StoreError::MissingProtectedKey {
                kind: kind.label(),
                reference: reference.as_str().to_owned(),
            });
        }
        permissions::validate_private_file(&path, "protected-key file")
    }

    fn write_temporary(path: &Path, secret: &[u8]) -> Result<(), StoreError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        permissions::protect_new_file(path, "protected-key temporary file")?;
        file.write_all(secret)?;
        file.sync_all()?;
        Ok(())
    }

    fn validate_all(&self) -> Result<(), StoreError> {
        for kind in [KeyKind::Endpoint, KeyKind::SpaceAuthority] {
            for entry in fs::read_dir(self.root.join(kind.directory()))? {
                permissions::validate_private_file(&entry?.path(), "protected-key file")?;
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), StoreError> {
    let parent = path.parent().ok_or(StoreError::InvalidStateDirectory)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(windows)]
const fn sync_parent(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

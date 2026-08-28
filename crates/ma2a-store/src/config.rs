use std::path::{Path, PathBuf};

const DATABASE_FILE: &str = "state-v1.sqlite3";

/// Paths and blocking `SQLite` settings for one current-user Runtime state store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreConfig {
    state_dir: PathBuf,
}

impl StoreConfig {
    /// Creates a configuration rooted at one current-user state directory.
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
        }
    }

    /// Returns the private state directory.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Returns the bundled `SQLite` database path.
    pub fn database_path(&self) -> PathBuf {
        self.state_dir.join(DATABASE_FILE)
    }
}

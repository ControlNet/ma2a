use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempState {
    path: PathBuf,
}

impl TempState {
    pub(crate) fn new(name: &str) -> TestResultValue<Self> {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-store-{name}-{}-{serial}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

pub(crate) type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

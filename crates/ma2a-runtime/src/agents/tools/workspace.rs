//! Workspace paths: relative, and never resolving outside the instance workspace.

use std::{
    io,
    path::{Component, Path, PathBuf},
};

use ma2a_agent::protocol::ToolError;

fn outside() -> ToolError {
    ToolError::new(
        "invalid_tool",
        "workspace paths are relative and stay inside the workspace",
    )
}

/// Returns the normal components of a relative path, refusing every other kind.
fn components(relative: &str) -> Result<Vec<&str>, ToolError> {
    let path = Path::new(relative);
    let parts: Vec<&str> = path
        .components()
        .map(|component| match component {
            Component::Normal(part) => part.to_str().ok_or_else(outside),
            _ => Err(outside()),
        })
        .collect::<Result<_, _>>()?;
    if parts.is_empty()
        || relative.contains('\\')
        || parts.iter().any(|part| {
            part.contains(':')
                || part.ends_with(['.', ' '])
                || is_windows_reserved(part)
                || ma2a_services::client::resume::is_private_directory(std::ffi::OsStr::new(part))
        })
    {
        return Err(outside());
    }
    Ok(parts)
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Resolves an existing regular file for reading; symlinks may be followed only within the
/// workspace.
pub(super) fn existing(workspace: &Path, relative: &str) -> Result<PathBuf, ToolError> {
    let path = components(relative)?
        .into_iter()
        .fold(workspace.to_path_buf(), |path, part| path.join(part));
    let root = std::fs::canonicalize(workspace).map_err(|error| local(&error))?;
    let resolved = std::fs::canonicalize(&path).map_err(|error| local(&error))?;
    if !resolved.starts_with(&root) {
        return Err(outside());
    }
    // Only a regular file is read: a FIFO or device could block its reader forever.
    if std::fs::metadata(&resolved)
        .map_err(|error| local(&error))?
        .is_file()
    {
        Ok(resolved)
    } else {
        Err(ToolError::new(
            "invalid_tool",
            "the path names no regular file",
        ))
    }
}

/// Resolves a destination, creating its parent directories; no directory on the way may be
/// a symlink, so nothing written can land outside the workspace.
pub(super) fn destination(workspace: &Path, relative: &str) -> Result<PathBuf, ToolError> {
    let parts = components(relative)?;
    let (name, parents) = parts.split_last().ok_or_else(outside)?;
    let mut path = workspace.to_path_buf();
    for part in parents {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(outside()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&path).map_err(|error| local(&error))?;
            }
            Err(error) => return Err(local(&error)),
        }
    }
    let target = path.join(name);
    if std::fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(ToolError::new(
            "invalid_tool",
            "the destination is a directory",
        ));
    }
    Ok(target)
}

/// Returns the error of one local file operation.
pub(super) fn local(error: &io::Error) -> ToolError {
    let code = match error.kind() {
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::PermissionDenied => "permission_denied",
        _ => "local_io",
    };
    ToolError::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{destination, existing};

    #[test]
    fn workspace_paths_never_leave_the_workspace() -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("ma2a-workspace-{}", std::process::id()));
        let workspace = base.join("workspace");
        std::fs::create_dir_all(&workspace)?;
        std::fs::write(base.join("secret"), b"x")?;
        std::fs::write(workspace.join("ok.txt"), b"y")?;
        for escaping in [
            "../secret",
            "/etc/passwd",
            "",
            ".",
            "a/../../secret",
            "a\\b",
            "report.txt:stream",
            "trailing.",
            "trailing ",
            ".ma2a-downloads/part",
            "sub/.MA2A-DOWNLOADS/part",
            "CON",
            "nul.txt",
            "sub/COM1.log",
        ] {
            assert!(existing(&workspace, escaping).is_err(), "{escaping}");
            assert!(destination(&workspace, escaping).is_err(), "{escaping}");
        }
        assert!(existing(&workspace, "ok.txt").is_ok());
        let nested = destination(&workspace, "out/deep/file.bin")?;
        assert!(nested.starts_with(&workspace) && workspace.join("out/deep").is_dir());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&base, workspace.join("link"))?;
            assert!(existing(&workspace, "link/secret").is_err());
            assert!(destination(&workspace, "link/new").is_err());
            // A FIFO would block the reader forever; directories are not files either.
            let made = std::process::Command::new("mkfifo")
                .arg(workspace.join("pipe"))
                .status()?;
            assert!(made.success());
            assert!(existing(&workspace, "pipe").is_err());
            assert!(existing(&workspace, "out").is_err());
        }
        std::fs::remove_dir_all(&base)?;
        Ok(())
    }
}

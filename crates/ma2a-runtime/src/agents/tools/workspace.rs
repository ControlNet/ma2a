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
                || ma2a_services::files::is_windows_reserved(part)
                || ma2a_services::client::resume::is_private_directory(std::ffi::OsStr::new(part))
        })
    {
        return Err(outside());
    }
    Ok(parts)
}

/// Opens an existing regular file for reading; links may be followed only within the workspace,
/// and never into a download's private directory.
///
/// The path is resolved again once the file is open, and the open file must be the one it then
/// names: a directory swapped on the way between the check and the open is refused.
pub(super) fn existing(workspace: &Path, relative: &str) -> Result<std::fs::File, ToolError> {
    let path = components(relative)?
        .into_iter()
        .fold(workspace.to_path_buf(), |path, part| path.join(part));
    let root = std::fs::canonicalize(workspace).map_err(|error| local(&error))?;
    let resolved = inside(&root, &path)?;
    // Only a regular file is read: a FIFO or device could block its reader forever.
    let file = crate::agents::artifacts::open_regular(&resolved).map_err(|error| {
        if error.kind() == io::ErrorKind::InvalidInput {
            ToolError::new("invalid_tool", "the path names no regular file")
        } else {
            local(&error)
        }
    })?;
    if !same_file(&file, &inside(&root, &path)?).map_err(|error| local(&error))? {
        return Err(outside());
    }
    Ok(file)
}

/// Resolves `path` and requires the result inside `root`, outside every private directory.
fn inside(root: &Path, path: &Path) -> Result<PathBuf, ToolError> {
    let resolved = std::fs::canonicalize(path).map_err(|error| local(&error))?;
    let relative = resolved.strip_prefix(root).map_err(|_| outside())?;
    if relative.components().any(|component| {
        matches!(component, Component::Normal(part)
            if ma2a_services::client::resume::is_private_directory(part))
    }) {
        return Err(outside());
    }
    Ok(resolved)
}

/// Whether the open `file` is the file `path` names now.
fn same_file(file: &std::fs::File, path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let (held, named) = (file.metadata()?, std::fs::metadata(path)?);
        Ok((held.dev(), held.ino()) == (named.dev(), named.ino()))
    }
    #[cfg(windows)]
    {
        let named = crate::agents::artifacts::open_regular(path)?;
        Ok(ma2a_services::windows_files::file_id(file)?
            == ma2a_services::windows_files::file_id(&named)?)
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
            "COM\u{b9}",
            "sub/lpt\u{b3}.txt",
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
            // A link inside the workspace may not lead into a download's private directory.
            std::fs::create_dir(workspace.join(".ma2a-downloads"))?;
            std::fs::write(workspace.join(".ma2a-downloads/held"), b"z")?;
            std::os::unix::fs::symlink(".ma2a-downloads", workspace.join("alias"))?;
            assert!(existing(&workspace, "alias/held").is_err());
            std::os::unix::fs::symlink("ok.txt", workspace.join("ok-link"))?;
            assert!(existing(&workspace, "ok-link").is_ok());
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

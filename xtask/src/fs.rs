use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::error::XtaskError;

const EXCLUDED_DIRECTORIES: [&str; 6] =
    [".git", ".omo", "dist", "node_modules", "target", "vendor"];

pub(crate) fn collect_files(root: &Path, name: Option<&str>) -> Result<Vec<PathBuf>, XtaskError> {
    let mut files = Vec::new();
    collect_from(root, name, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_from(
    directory: &Path,
    name: Option<&str>,
    files: &mut Vec<PathBuf>,
) -> Result<(), XtaskError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let directory_name = entry.file_name();
            let directory_name = directory_name.to_string_lossy();
            if !EXCLUDED_DIRECTORIES.contains(&directory_name.as_ref()) {
                collect_from(&path, name, files)?;
            }
        } else if name.is_none_or(|expected| entry.file_name() == expected) {
            files.push(path);
        }
    }
    Ok(())
}

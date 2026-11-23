use std::{fs, path::Path};

use crate::{error::XtaskError, fs::collect_files};

const SOURCE_EXTENSIONS: [&str; 9] = ["cts", "js", "jsx", "mts", "rs", "sh", "ts", "tsx", "css"];

pub(crate) fn check(root: &Path) -> Result<(), XtaskError> {
    let mut violations = Vec::new();
    for path in collect_files(root, None)? {
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if relative
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !SOURCE_EXTENSIONS.contains(&extension) {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        let pure_lines = count_pure_lines(&source, extension);
        if pure_lines > 250 {
            violations.push(format!(
                "{} has {pure_lines} pure LOC (maximum 250)",
                relative.display()
            ));
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(XtaskError::Policy {
            name: "LOC policy",
            violations,
        })
    }
}

fn count_pure_lines(source: &str, extension: &str) -> usize {
    let mut block_depth = 0_usize;
    source
        .lines()
        .filter(|line| is_pure_line(line, extension, &mut block_depth))
        .count()
}

fn is_pure_line(line: &str, extension: &str, block_depth: &mut usize) -> bool {
    let is_shell = extension == "sh";
    let block_comments = !is_shell;
    let line_slashes = !is_shell && extension != "css";
    // Shell comments and Rust attribute lines (`#[...]`, `#![...]`) start with `#`; CSS `#id`
    // selectors and JS/TS `#private` fields are code and remain counted.
    let hash_comments = is_shell || extension == "rs";
    let nested_blocks = extension == "rs";
    let mut remaining = line.trim();
    loop {
        if block_comments && *block_depth > 0 {
            if nested_blocks
                && let Some(open) = remaining.find("/*")
                && remaining.find("*/").is_none_or(|close| open < close)
            {
                *block_depth += 1;
                remaining = remaining.get(open + 2..).unwrap_or_default();
                continue;
            }
            let Some(end) = remaining.find("*/") else {
                return false;
            };
            *block_depth -= 1;
            remaining = remaining.get(end + 2..).unwrap_or_default().trim();
            continue;
        }
        if remaining.is_empty()
            || (line_slashes && remaining.starts_with("//"))
            || (hash_comments && remaining.starts_with('#'))
        {
            return false;
        }
        if block_comments && remaining.starts_with("/*") {
            *block_depth = 1;
            remaining = remaining.get(2..).unwrap_or_default();
            continue;
        }
        return true;
    }
}

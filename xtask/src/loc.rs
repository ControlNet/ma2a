use std::{fs, path::Path};

use crate::{error::XtaskError, fs::collect_files};

const SOURCE_EXTENSIONS: [&str; 9] = ["cts", "js", "jsx", "mts", "rs", "sh", "ts", "tsx", "css"];

pub(crate) fn check(root: &Path) -> Result<(), XtaskError> {
    let mut violations = Vec::new();
    for path in collect_files(root, None)? {
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !SOURCE_EXTENSIONS.contains(&extension) {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        let pure_lines = count_pure_lines(&source);
        if pure_lines > 250 {
            let relative = path.strip_prefix(root).unwrap_or(&path);
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

fn count_pure_lines(source: &str) -> usize {
    let mut in_block_comment = false;
    source
        .lines()
        .filter(|line| is_pure_line(line, &mut in_block_comment))
        .count()
}

fn is_pure_line(line: &str, in_block_comment: &mut bool) -> bool {
    let mut remaining = line.trim();
    loop {
        if *in_block_comment {
            let Some(end) = remaining.find("*/") else {
                return false;
            };
            *in_block_comment = false;
            remaining = remaining.get(end + 2..).unwrap_or_default().trim();
        }
        if remaining.is_empty() || remaining.starts_with("//") || remaining.starts_with('#') {
            return false;
        }
        if remaining.find("/*") == Some(0) {
            *in_block_comment = true;
            remaining = remaining.get(2..).unwrap_or_default();
            continue;
        }
        return true;
    }
}

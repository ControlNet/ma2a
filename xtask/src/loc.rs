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
        if block_comments {
            track_tail(remaining, extension, block_depth);
        }
        return true;
    }
}

/// Follows the rest of a line that holds code, so a block comment that opens after the code,
/// and any that closes again on the same line, leaves `block_depth` as the next line sees it.
///
/// Quoted strings on the line are skipped, so a delimiter inside one opens nothing.
fn track_tail(text: &str, extension: &str, block_depth: &mut usize) {
    let nested = extension == "rs";
    let line_slashes = extension != "css";
    let bytes = text.as_bytes();
    let starts = |at: usize, pair: &[u8]| bytes.get(at..at + 2) == Some(pair);
    let mut quote = None;
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if *block_depth > 0 {
            if nested && starts(at, b"/*") {
                *block_depth += 1;
                at += 2;
            } else if starts(at, b"*/") {
                *block_depth -= 1;
                at += 2;
            } else {
                at += 1;
            }
            continue;
        }
        if let Some(open) = quote {
            if byte == b'\\' {
                at += 2;
                continue;
            }
            if byte == open {
                quote = None;
            }
            at += 1;
            continue;
        }
        if byte == b'"' || (!nested && matches!(byte, b'\'' | b'`')) {
            quote = Some(byte);
        } else if line_slashes && starts(at, b"//") {
            return;
        } else if starts(at, b"/*") {
            *block_depth = 1;
            at += 2;
            continue;
        }
        at += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::count_pure_lines;

    #[test]
    fn a_block_comment_opened_after_code_is_not_counted() {
        let rust = "fn f() { /* starts here\n   still a comment\n*/\n    g();\n}\n";
        assert_eq!(count_pure_lines(rust, "rs"), 3);
        // Nested comments, code after the close, and delimiters inside strings.
        let nested = "a(); /* x /* y */\nz */ b();\nlet s = \"/*\";\nc();\n";
        assert_eq!(count_pure_lines(nested, "rs"), 4);
        let closed = "a(); /* x */ b();\nc();\n";
        assert_eq!(count_pure_lines(closed, "rs"), 2);
        let script = "a(); /* open\nb */\nconst s = '/*';\nc(); // /*\nd();\n";
        assert_eq!(count_pure_lines(script, "ts"), 4);
    }
}

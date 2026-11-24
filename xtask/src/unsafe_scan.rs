//! Lexical scanner for `unsafe` keywords and `allow`/`expect`/`warn(unsafe_code)` attributes.

/// Scans Rust source outside comments, literals, and raw identifiers for any `unsafe` keyword
/// token and any `#[allow|expect|warn(...)]` attribute mentioning `unsafe_code`.
pub(crate) fn scan_unsafe_tokens(source: &str) -> (bool, bool) {
    let stripped = strip_rust_comments_and_strings(source);
    let mut has_unsafe = false;
    let mut has_allow = false;
    let mut chars = stripped.char_indices().peekable();
    let mut after_hash = false;
    let mut attr_bracket_depth = 0_usize;
    let mut pending_lint_paren = false;
    let mut lint_paren_depth = 0_usize;
    while let Some((start, ch)) = chars.next() {
        if is_identifier_char(ch) && !ch.is_ascii_digit() {
            after_hash = false;
            let mut end = start + ch.len_utf8();
            while let Some(&(next_idx, next_ch)) = chars.peek() {
                if is_identifier_char(next_ch) {
                    end = next_idx + next_ch.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let ident = stripped.get(start..end).unwrap_or_default();
            if ident == "unsafe" {
                has_unsafe = true;
            } else if attr_bracket_depth > 0 && matches!(ident, "allow" | "expect" | "warn") {
                pending_lint_paren = true;
            } else if ident == "unsafe_code" && lint_paren_depth > 0 {
                has_allow = true;
                pending_lint_paren = false;
            } else {
                pending_lint_paren = false;
            }
            continue;
        }
        if ch.is_ascii_whitespace() {
            continue;
        }
        if pending_lint_paren {
            pending_lint_paren = false;
            if ch == '(' {
                lint_paren_depth = lint_paren_depth.saturating_add(1);
                continue;
            }
        }
        match ch {
            '#' if attr_bracket_depth == 0 => after_hash = true,
            '!' if after_hash => {}
            '[' if after_hash || attr_bracket_depth > 0 => {
                after_hash = false;
                attr_bracket_depth += 1;
            }
            ']' if attr_bracket_depth > 0 => {
                attr_bracket_depth -= 1;
                if attr_bracket_depth == 0 {
                    lint_paren_depth = 0;
                }
            }
            '(' if lint_paren_depth > 0 => lint_paren_depth += 1,
            ')' if lint_paren_depth > 0 => lint_paren_depth -= 1,
            _ => after_hash = false,
        }
    }
    (has_unsafe, has_allow)
}

/// Whether `ch` can continue a Rust identifier.
///
/// Outside comments and literals, a non-ASCII character is either part of an identifier or one
/// of the few non-ASCII whitespace characters Rust accepts, so every other non-ASCII character
/// is taken as an identifier character: a keyword is never read out of a longer identifier.
const fn is_identifier_char(ch: char) -> bool {
    ch == '_'
        || ch.is_ascii_alphanumeric()
        || (!ch.is_ascii()
            && !matches!(
                ch,
                '\u{85}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}'
            ))
}

fn strip_rust_comments_and_strings(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes.get(i).copied().unwrap_or_default();
        let next = bytes.get(i + 1).copied();
        if b == b'/' && next == Some(b'/') {
            i += 2;
            while bytes.get(i).is_some_and(|&c| c != b'\n') {
                i += 1;
            }
            out.push(' ');
        } else if b == b'/' && next == Some(b'*') {
            i = skip_block_comment(bytes, i + 2);
            out.push(' ');
        } else if matches!(b, b'r' | b'b' | b'c') && matches!(next, Some(b'"' | b'#' | b'r')) {
            if let Some(after) = skip_raw_string(bytes, i) {
                i = after;
                out.push(' ');
                continue;
            }
            if b == b'r'
                && next == Some(b'#')
                && let Some(after) = skip_raw_ident(bytes, i)
            {
                i = after;
                out.push(' ');
                continue;
            }
            out.push(char::from(b));
            i += 1;
        } else if b == b'b' && next == Some(b'\'') {
            if let Some(after) = skip_char_literal(bytes, i + 1) {
                i = after;
                out.push(' ');
                continue;
            }
            out.push('b');
            i += 1;
        } else if b == b'"' {
            i = skip_quoted(bytes, i + 1, b'"');
            out.push(' ');
        } else if b == b'\'' {
            if let Some(after) = skip_char_literal(bytes, i) {
                i = after;
                out.push(' ');
                continue;
            }
            out.push('\'');
            i += 1;
        } else if b.is_ascii() {
            out.push(char::from(b));
            i += 1;
        } else {
            // A non-ASCII character is kept whole, never read byte by byte.
            let ch = source
                .get(i..)
                .and_then(|rest| rest.chars().next())
                .unwrap_or(char::REPLACEMENT_CHARACTER);
            out.push(ch);
            i += ch.len_utf8().max(1);
        }
    }
    out
}

fn skip_block_comment(bytes: &[u8], mut i: usize) -> usize {
    let mut depth = 1_usize;
    while i < bytes.len() && depth > 0 {
        if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else if bytes.get(i) == Some(&b'*') && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    i
}

fn skip_raw_ident(bytes: &[u8], start: usize) -> Option<usize> {
    let first = *bytes.get(start + 2)?;
    if !(first.is_ascii_alphabetic() || first == b'_' || !first.is_ascii()) {
        return None;
    }
    let mut j = start + 3;
    while bytes
        .get(j)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || !c.is_ascii())
    {
        j += 1;
    }
    Some(j)
}

fn skip_raw_string(bytes: &[u8], start: usize) -> Option<usize> {
    let mut j = start;
    if matches!(bytes.get(j), Some(b'b' | b'c')) {
        j += 1;
    }
    if bytes.get(j) != Some(&b'r') {
        return None;
    }
    j += 1;
    let mut hashes = 0_usize;
    while bytes.get(j) == Some(&b'#') {
        hashes += 1;
        j += 1;
    }
    if bytes.get(j) != Some(&b'"') {
        return None;
    }
    j += 1;
    while j < bytes.len() {
        if bytes.get(j) == Some(&b'"') && (0..hashes).all(|k| bytes.get(j + 1 + k) == Some(&b'#')) {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(bytes.len())
}

fn skip_quoted(bytes: &[u8], mut i: usize, quote: u8) -> usize {
    while i < bytes.len() {
        let c = bytes.get(i).copied().unwrap_or_default();
        if c == b'\\' {
            i += 2;
        } else if c == quote {
            return i + 1;
        } else {
            i += 1;
        }
    }
    bytes.len()
}

fn skip_char_literal(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start + 1) == Some(&b'\\') {
        return match bytes.get(start + 2)? {
            b'\'' | b'"' | b'\\' | b'n' | b'r' | b't' | b'0' => {
                (bytes.get(start + 3) == Some(&b'\'')).then_some(start + 4)
            }
            b'x' => {
                let valid = bytes.get(start + 3).is_some_and(u8::is_ascii_hexdigit)
                    && bytes.get(start + 4).is_some_and(u8::is_ascii_hexdigit)
                    && bytes.get(start + 5) == Some(&b'\'');
                valid.then_some(start + 6)
            }
            b'u' if bytes.get(start + 3) == Some(&b'{') => {
                let mut j = start + 4;
                let mut digits = 0_usize;
                while let Some(&c) = bytes.get(j) {
                    if c.is_ascii_hexdigit() {
                        digits += 1;
                        j += 1;
                    } else if c == b'_' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                (digits > 0 && bytes.get(j) == Some(&b'}') && bytes.get(j + 1) == Some(&b'\''))
                    .then_some(j + 2)
            }
            _ => None,
        };
    }
    let ch = std::str::from_utf8(bytes.get(start + 1..)?)
        .ok()?
        .chars()
        .next()?;
    if ch == '\'' || ch == '\n' {
        return None;
    }
    let end = start + 1 + ch.len_utf8();
    (bytes.get(end) == Some(&b'\'')).then_some(end + 1)
}

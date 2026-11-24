//! Lexical cases for the unsafe keyword and lint attribute scanner.

use crate::unsafe_scan::scan_unsafe_tokens;

#[test]
fn unsafe_token_scanner_catches_rust_2024_and_multiline_forms() {
    assert_eq!(scan_unsafe_tokens("unsafe\n{}"), (true, false));
    assert_eq!(
        scan_unsafe_tokens("unsafe extern \"C\" { fn x(); }"),
        (true, false)
    );
    assert_eq!(
        scan_unsafe_tokens("#[unsafe(no_mangle)] fn x() {}"),
        (true, false)
    );
    assert_eq!(
        scan_unsafe_tokens("#[expect(unsafe_code, reason = \"x\")]"),
        (false, true)
    );
    assert_eq!(
        scan_unsafe_tokens("#![allow(\n    rust::unsafe_code,\n    reason = \"x\"\n)]"),
        (false, true)
    );
    assert_eq!(
        scan_unsafe_tokens("#[allow /* comment */ (unsafe_code)]"),
        (false, true)
    );
    assert_eq!(scan_unsafe_tokens("#[warn(unsafe_code)]"), (false, true));
    assert_eq!(
        scan_unsafe_tokens("let _ = ('\\'', '\"'); #[allow(unsafe_code)] unsafe {}"),
        (true, true)
    );
    assert_eq!(
        scan_unsafe_tokens("let _ = ('\\u{10FFFF}', '\"'); unsafe {}"),
        (true, false)
    );
    assert_eq!(scan_unsafe_tokens("let r#unsafe = 1;"), (false, false));
    // Unicode identifiers are whole tokens: no keyword is read out of their tail.
    assert_eq!(scan_unsafe_tokens("let \u{e9}unsafe = 3;"), (false, false));
    assert_eq!(
        scan_unsafe_tokens("let x\u{fc}unsafe_code = 3;"),
        (false, false)
    );
    assert_eq!(
        scan_unsafe_tokens("let \u{e9} = 1; unsafe {}"),
        (true, false)
    );
    assert_eq!(
        scan_unsafe_tokens("let a = 1;\u{2028}unsafe {}"),
        (true, false)
    );
    assert_eq!(
        scan_unsafe_tokens("let unsafe_code = \"m\"; opt.expect(unsafe_code);"),
        (false, false)
    );
    assert_eq!(
        scan_unsafe_tokens("// unsafe { }\nlet s = \"allow(unsafe_code)\";"),
        (false, false)
    );
}

#[test]
fn whitespace_rust_accepts_beyond_ascii_separates_tokens() {
    // Both compile: `\u{200E}` and `\u{0B}` are whitespace to Rust's lexer.
    assert_eq!(
        scan_unsafe_tokens("m!(r#x\u{200E}unsafe { () });"),
        (true, false)
    );
    assert_eq!(
        scan_unsafe_tokens("#[allow\u{200E}(unsafe_code)] fn g() {}"),
        (false, true)
    );
    assert_eq!(
        scan_unsafe_tokens("#[allow\u{0B}(unsafe_code)] fn g() {}"),
        (false, true)
    );
}

#[test]
fn raw_identifiers_still_name_lint_attributes() {
    // Rust 2024 accepts each of these, and each one lifts `#![deny(unsafe_code)]`.
    for source in [
        "#[r#allow(unsafe_code)] fn a() {}",
        "#[allow(r#unsafe_code)] fn b() {}",
        "#[r#expect(unsafe_code)] fn c() {}",
        "#[warn(rust::r#unsafe_code)] fn d() {}",
    ] {
        assert_eq!(scan_unsafe_tokens(source), (false, true), "{source}");
    }
    // `r#unsafe` is an identifier, never the keyword.
    assert_eq!(scan_unsafe_tokens("let r#unsafe = 1;"), (false, false));
}

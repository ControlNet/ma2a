//! A strict reading of one complete Semantic Versioning 2.0.0 version.

/// Whether `text` is exactly one version `MAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]` and nothing
/// else: a range, a union of ranges, or a version followed by more text is not one.
pub(crate) fn is_version(text: &str) -> bool {
    let (rest, build) = text
        .split_once('+')
        .map_or((text, None), |(rest, build)| (rest, Some(build)));
    let (core, prerelease) = rest
        .split_once('-')
        .map_or((rest, None), |(core, pre)| (core, Some(pre)));
    let mut numbers = core.split('.');
    let three = numbers.clone().count() == 3 && numbers.all(is_number);
    three
        && prerelease.is_none_or(|pre| {
            identifiers(pre, |id| {
                !id.bytes().all(|byte| byte.is_ascii_digit()) || is_number(id)
            })
        })
        && build.is_none_or(|build| identifiers(build, |_| true))
}

/// A numeric identifier: digits only, without a leading zero unless it is `0`.
fn is_number(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
}

/// Dot-separated non-empty identifiers of ASCII letters, digits, and hyphens, each also
/// passing `also`.
fn identifiers(text: &str, also: impl Fn(&str) -> bool) -> bool {
    text.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && also(identifier)
    })
}

#[cfg(test)]
mod tests {
    use super::is_version;

    #[test]
    fn only_one_complete_version_is_a_version() {
        for exact in [
            "1.2.3",
            "0.0.0",
            "1.2.3-beta",
            "1.2.3-beta.1",
            "1.2.3-0.x-y",
            "1.2.3+build.5",
            "1.2.3-rc.1+sha-abc",
            "10.20.30",
        ] {
            assert!(is_version(exact), "{exact} is one version");
        }
        for loose in [
            "1.2.3-beta || ^2.0.0",
            "1.2.3 || 2.0.0",
            "^1.2.3",
            "~1.2.3",
            ">=1.2.3",
            "1.2",
            "1.2.x",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-01",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-beta..1",
            "1.2.3 ",
            "latest",
            "",
        ] {
            assert!(!is_version(loose), "{loose:?} is not one version");
        }
    }
}

//! Validated names shared by service authorization and wire codecs.

use std::{fmt, net::IpAddr, str::FromStr};

use crate::ProtocolError;

/// Maximum byte length of an exported file-root name.
pub const MAX_ROOT_NAME_LEN: usize = 64;
/// Maximum byte length of one remote path component.
pub const MAX_PATH_COMPONENT_LEN: usize = 255;
/// Maximum number of components in one remote path.
pub const MAX_PATH_COMPONENTS: usize = 64;
/// Maximum total byte length of all components of one remote path.
pub const MAX_REMOTE_PATH_LEN: usize = 4_096;
/// Maximum byte length of a proxy destination host.
pub const MAX_PROXY_HOST_LEN: usize = 253;

/// The name of one exported file root: `[a-z0-9][a-z0-9._-]*`, 1–64 bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootName(String);

impl RootName {
    /// Validates an exported root name.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for empty, oversized, or non-canonical names.
    pub fn new(name: &str) -> Result<Self, ProtocolError> {
        let mut bytes = name.bytes();
        let valid_first = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        let valid_rest = bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
        if valid_first && valid_rest && name.len() <= MAX_ROOT_NAME_LEN {
            Ok(Self(name.to_owned()))
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    /// Returns the canonical name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RootName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One validated root-relative location: a root name plus ordered components.
///
/// Nothing is ever resolved lexically. `.` and `..` are rejected rather than
/// interpreted, so every accepted path names exactly one location below its root
/// independently of any process working directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemotePath {
    root: RootName,
    components: Vec<String>,
}

impl RemotePath {
    /// Validates a root-relative path. An empty component list names the root itself.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for invalid components or bounds.
    pub fn new(root: RootName, components: Vec<String>) -> Result<Self, ProtocolError> {
        let total = components.iter().map(String::len).sum::<usize>();
        if components.len() > MAX_PATH_COMPONENTS
            || total > MAX_REMOTE_PATH_LEN
            || !components
                .iter()
                .all(|component| valid_component(component))
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self { root, components })
    }

    /// Parses the CLI form `ROOT:/a/b`, collapsing repeated and trailing separators.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the root or any component is invalid.
    pub fn parse(text: &str) -> Result<Self, ProtocolError> {
        let (root, path) = text.split_once(':').ok_or(ProtocolError::INVALID_INPUT)?;
        let components = path
            .split('/')
            .filter(|component| !component.is_empty())
            .map(str::to_owned)
            .collect();
        Self::new(RootName::new(root)?, components)
    }

    /// Returns the exported root this path belongs to.
    pub const fn root(&self) -> &RootName {
        &self.root
    }

    /// Returns the ordered root-relative components.
    pub fn components(&self) -> &[String] {
        &self.components
    }

    /// Returns whether this path names the root directory itself.
    pub const fn is_root(&self) -> bool {
        self.components.is_empty()
    }

    /// Returns the final component, or `None` for the root itself.
    pub fn file_name(&self) -> Option<&str> {
        self.components.last().map(String::as_str)
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:/{}", self.root, self.components.join("/"))
    }
}

/// Whether `component` can be one component of a [`RemotePath`]: non-empty, at most
/// [`MAX_PATH_COMPONENT_LEN`] bytes, neither `.` nor `..`, and free of control characters and of
/// `/`, `\`, and `:`.
pub fn valid_component(component: &str) -> bool {
    !component.is_empty()
        && component.len() <= MAX_PATH_COMPONENT_LEN
        && component != "."
        && component != ".."
        && !component
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
}

/// A proxy destination host: a lowercase DNS name or a canonical IP literal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProxyHost(String);

impl ProxyHost {
    /// Validates a canonical destination host.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for non-canonical or invalid hosts.
    pub fn new(host: &str) -> Result<Self, ProtocolError> {
        if let Ok(address) = IpAddr::from_str(host) {
            return if address.to_string() == host {
                Ok(Self(host.to_owned()))
            } else {
                Err(ProtocolError::INVALID_INPUT)
            };
        }
        let valid = !host.is_empty()
            && host.len() <= MAX_PROXY_HOST_LEN
            && host.split('.').all(valid_dns_label);
        if valid {
            Ok(Self(host.to_owned()))
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    /// Returns the canonical host text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProxyHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn valid_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::{ProxyHost, RemotePath, RootName};

    #[test]
    fn parsing_collapses_separators_and_rejects_relative_components() {
        let path = RemotePath::parse("docs://a//b/").expect("valid path");
        assert_eq!(path.root().as_str(), "docs");
        assert_eq!(path.components(), ["a", "b"]);
        assert!(RemotePath::parse("docs:/").expect("root").is_root());
        for invalid in [
            "docs:/a/../b",
            "docs:/./a",
            "Docs:/a",
            "docs",
            "docs:/a\\b",
            ":/a",
        ] {
            assert!(RemotePath::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn root_names_and_hosts_are_canonical() {
        assert!(RootName::new("a.b_c-9").is_ok());
        assert!(RootName::new("-a").is_err());
        assert!(RootName::new(&"a".repeat(65)).is_err());
        assert!(ProxyHost::new("db.internal").is_ok());
        assert!(ProxyHost::new("127.0.0.1").is_ok());
        assert!(ProxyHost::new("::1").is_ok());
        assert!(ProxyHost::new("0:0::1").is_err());
        assert!(ProxyHost::new("DB.internal").is_err());
        assert!(ProxyHost::new("a..b").is_err());
    }
}

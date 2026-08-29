use std::fmt;

use serde_json::{Map, Value};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SchemaMismatch(String);

impl fmt::Display for SchemaMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for SchemaMismatch {}

pub(crate) type Check = Result<(), SchemaMismatch>;

#[derive(Debug, Clone, Copy)]
pub(super) struct Node<'a> {
    pub(super) value: &'a Value,
    pub(super) specification: &'a Value,
    pub(super) path: &'a str,
}

pub(super) fn enumeration(node: Node<'_>) -> Check {
    let values = node
        .specification
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| error(node.path, "enum values must exist"))?;
    if values.contains(node.value) {
        Ok(())
    } else {
        mismatch(node.path, "enum value mismatch")
    }
}

pub(super) fn integer(node: Node<'_>) -> Check {
    let number = node
        .value
        .as_u64()
        .ok_or_else(|| error(node.path, "expected unsigned integer"))?;
    if node
        .specification
        .get("minimum")
        .and_then(Value::as_u64)
        .is_some_and(|minimum| number < minimum)
    {
        return mismatch(node.path, "below minimum");
    }
    if node
        .specification
        .get("maximum")
        .and_then(Value::as_u64)
        .is_some_and(|maximum| number > maximum)
    {
        return mismatch(node.path, "above maximum");
    }
    Ok(())
}

pub(super) fn string(node: Node<'_>) -> Check {
    let text = node
        .value
        .as_str()
        .ok_or_else(|| error(node.path, "expected string"))?;
    check_text_bounds(text, node.specification, node.path)?;
    match node.specification.get("format").and_then(Value::as_str) {
        Some("lowercase_hex") => lowercase_hex(text, node.specification, node.path),
        Some("https_url_without_credentials") => https_url(text, node.path),
        Some(format) => mismatch(node.path, &format!("unsupported string format {format}")),
        None => Ok(()),
    }
}

fn check_text_bounds(text: &str, specification: &Value, path: &str) -> Check {
    if let Some(minimum) = specification.get("min_bytes").and_then(Value::as_u64) {
        let minimum = usize::try_from(minimum).map_err(|_| error(path, "invalid text bound"))?;
        if text.len() < minimum {
            return mismatch(path, "text below bound");
        }
    }
    if let Some(maximum) = specification.get("max_bytes").and_then(Value::as_u64) {
        let maximum = usize::try_from(maximum).map_err(|_| error(path, "invalid text bound"))?;
        if text.len() > maximum {
            return mismatch(path, "text above bound");
        }
    }
    Ok(())
}

fn lowercase_hex(text: &str, specification: &Value, path: &str) -> Check {
    let bytes = specification
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| error(path, "hex byte width must exist"))?;
    let width = usize::try_from(bytes * 2).map_err(|_| error(path, "invalid hex width"))?;
    if text.len() == width
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        mismatch(path, "lowercase hex mismatch")
    }
}

fn https_url(text: &str, path: &str) -> Check {
    let authority = text
        .strip_prefix("https://")
        .and_then(|remainder| remainder.split('/').next())
        .ok_or_else(|| error(path, "HTTPS URL must contain an authority"))?;
    if authority.is_empty() || authority.contains('@') {
        mismatch(path, "HTTPS URL authority mismatch")
    } else {
        Ok(())
    }
}

pub(super) fn text_field<'a>(
    value: &'a Value,
    name: &str,
    path: &str,
) -> Result<&'a str, SchemaMismatch> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| error(path, &format!("{name} must be text")))
}

pub(super) fn object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, SchemaMismatch> {
    value
        .as_object()
        .ok_or_else(|| error(path, "expected object"))
}

pub(super) fn field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    path: &str,
) -> Result<&'a Value, SchemaMismatch> {
    object
        .get(name)
        .ok_or_else(|| error(path, &format!("missing field {name}")))
}

pub(super) fn exact_fields(object: &Map<String, Value>, expected: &[&str], path: &str) -> Check {
    let mut actual = object.keys().map(String::as_str).collect::<Vec<_>>();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    if actual == expected {
        Ok(())
    } else {
        mismatch(path, "object fields mismatch")
    }
}

pub(super) fn equal(actual: Option<&Value>, expected: Option<&Value>, path: &str) -> Check {
    if actual == expected {
        Ok(())
    } else {
        mismatch(path, "literal mismatch")
    }
}

pub(super) fn mismatch(path: &str, message: &str) -> Check {
    Err(error(path, message))
}

pub(super) fn error(path: &str, message: &str) -> SchemaMismatch {
    SchemaMismatch(format!("{path}: {message}"))
}

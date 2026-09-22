//! Positional `SPACE_REF` resolution shared by every Space command.

use std::{fmt::Write as _, io, path::Path};

use ma2a_core::MAX_SPACE_NAME_LEN;
use ma2a_runtime::ipc::IpcPaths;
use serde_json::Value;

use crate::AppError;

/// Resolves a positional `SPACE_REF` to one canonical Space ID.
///
/// A complete Space ID always resolves to itself. Otherwise the reference is
/// matched exactly against the shared Space names this Runtime can see. Space
/// names are not unique, so an ambiguous reference fails and lists the
/// candidates rather than picking one.
pub(super) async fn resolve_space(
    state_dir: &Path,
    paths: &IpcPaths,
    reference: &str,
) -> Result<String, AppError> {
    if is_space_id(reference) {
        return Ok(reference.to_owned());
    }
    let response = crate::request(
        (state_dir, paths.clone()),
        crate::commands::workflows::unit_command("space_list")?,
    )
    .await?;
    let document: Value = serde_json::from_slice(&response).map_err(io::Error::other)?;
    crate::output::reject_runtime_error(&document)?;
    let spaces = document
        .pointer("/result/payload")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("Runtime did not return a Space list"))?;
    let matches = spaces
        .iter()
        .filter(|space| space.get("name").and_then(Value::as_str) == Some(reference))
        .filter_map(|space| space.get("space_id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let shown = quoted_reference(reference);
    match matches.as_slice() {
        [] => Err(AppError::NotFound(format!(
            "no Space named {shown}; pass a Space ID or run `ma2a space list`"
        ))),
        [only] => Ok((*only).to_owned()),
        several => {
            let mut message = format!("{} Spaces are named {shown}:", several.len());
            for space_id in several {
                let _written = write!(&mut message, "\n  {space_id}");
            }
            message.push_str("\nuse the Space ID instead.");
            Err(AppError::Invalid(message))
        }
    }
}

/// Echoes a reference only while it could be a Space name, so oversized input
/// is never reflected back into a message or a log.
fn quoted_reference(reference: &str) -> String {
    if reference.len() > MAX_SPACE_NAME_LEN {
        return "the supplied reference".to_owned();
    }
    format!("{reference:?}")
}

fn is_space_id(reference: &str) -> bool {
    reference.len() == 64
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::is_space_id;

    #[test]
    fn only_complete_lowercase_hexadecimal_references_are_space_identifiers() {
        // Given
        let identifier = "4b".repeat(32);

        // When / Then
        assert!(is_space_id(&identifier));
        assert!(!is_space_id(&identifier.to_uppercase()));
        assert!(!is_space_id(identifier.get(..63).unwrap_or_default()));
        assert!(!is_space_id("lab"));
    }
}

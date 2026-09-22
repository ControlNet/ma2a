use std::{borrow::Cow, ffi::OsString};

/// Prefix of every encoded MA2A invitation ticket.
const INVITE_TICKET_PREFIX: &str = "ma2ainvite";

pub(super) fn reject_secret_argv(arguments: &[OsString]) -> Result<(), clap::Error> {
    let values = arguments
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    let password_secret = values
        .iter()
        .any(|value| value == "--password" || value.starts_with("--password="));
    // An invitation is bearer material wherever it appears, including as the
    // operand of a command that has no invitation parameter at all.
    let invite_ticket = values
        .iter()
        .any(|value| value.trim().starts_with(INVITE_TICKET_PREFIX));
    let accept_index = command_tail_index(&values, ["space", "accept"]);
    let unsupported_accept_value = accept_index.is_some_and(|index| {
        values
            .get(index..)
            .is_some_and(state_dir_tail_has_unknown_value)
    });
    let password_index = values
        .windows(3)
        .position(|window| {
            matches!(
                window,
                [ui, password, action]
                    if ui == "ui"
                        && password == "password"
                        && matches!(action.as_ref(), "set" | "reset")
            )
        })
        .map(|index| index + 3)
        .or_else(|| {
            values
                .windows(2)
                .position(|window| window == ["ui", "init"])
                .map(|index| index + 2)
        });
    let unsupported_password_value = password_index.is_some_and(|index| {
        values
            .get(index..)
            .is_some_and(state_dir_tail_has_unknown_value)
    });
    if password_secret || invite_ticket || unsupported_password_value || unsupported_accept_value {
        Err(clap::Error::raw(
            clap::error::ErrorKind::InvalidValue,
            "secret values are not accepted in argv; use secure input",
        ))
    } else {
        Ok(())
    }
}

fn command_tail_index<const N: usize>(
    values: &[Cow<'_, str>],
    command: [&str; N],
) -> Option<usize> {
    values
        .windows(command.len())
        .position(|window| window == command)
        .map(|index| index + command.len())
}

/// Accepts only informational flags and the global state directory after a
/// command whose remaining operands could only ever be secret material.
fn state_dir_tail_has_unknown_value(values: &[Cow<'_, str>]) -> bool {
    let mut values = values.iter();
    while let Some(value) = values.next() {
        match value.as_ref() {
            "--help" | "-h" | "--version" | "-V" => {}
            "--state-dir" => {
                if values.next().is_none_or(|operand| operand.starts_with('-')) {
                    return true;
                }
            }
            value
                if value
                    .strip_prefix("--state-dir=")
                    .is_some_and(|path| !path.is_empty()) => {}
            _ => return true,
        }
    }
    false
}

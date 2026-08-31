use std::{borrow::Cow, ffi::OsString};

pub(super) fn reject_secret_argv(arguments: &[OsString]) -> Result<(), clap::Error> {
    let values = arguments
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    let password_secret = values
        .iter()
        .any(|value| value == "--password" || value.starts_with("--password="));
    let redeem_index = command_tail_index(&values, ["space", "invite", "redeem"]);
    let invite_redeem = redeem_index.is_some();
    let supported_redeem_input = values.iter().any(|value| {
        value == "--stdin"
            || value == "--file"
            || value
                .strip_prefix("--file=")
                .is_some_and(|path| !path.is_empty())
    });
    let unsupported_redeem_value = redeem_index.is_some_and(|index| {
        values
            .get(index..)
            .is_some_and(redeem_tail_has_unknown_value)
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
        .map(|index| index + 3);
    let unsupported_password_value = password_index.is_some_and(|index| {
        values
            .get(index..)
            .is_some_and(password_tail_has_unknown_value)
    });
    let clap_information = values
        .iter()
        .any(|value| matches!(value.as_ref(), "--help" | "-h" | "--version" | "-V"));
    if password_secret
        || unsupported_password_value
        || unsupported_redeem_value
        || (invite_redeem && !supported_redeem_input && !clap_information)
    {
        Err(clap::Error::raw(
            clap::error::ErrorKind::InvalidValue,
            "secret values are not accepted in argv; use secure input",
        ))
    } else {
        Ok(())
    }
}

fn command_tail_index(values: &[Cow<'_, str>], command: [&str; 3]) -> Option<usize> {
    values
        .windows(command.len())
        .position(|window| window == command)
        .map(|index| index + command.len())
}

fn redeem_tail_has_unknown_value(values: &[Cow<'_, str>]) -> bool {
    let mut values = values.iter();
    while let Some(value) = values.next() {
        match value.as_ref() {
            "--stdin" | "--help" | "-h" | "--version" | "-V" => {}
            "--file" | "--state-dir" => {
                if values.next().is_none_or(|operand| operand.starts_with('-')) {
                    return true;
                }
            }
            value
                if value
                    .strip_prefix("--file=")
                    .is_some_and(|path| !path.is_empty()) => {}
            value
                if value
                    .strip_prefix("--state-dir=")
                    .is_some_and(|path| !path.is_empty()) => {}
            _ => return true,
        }
    }
    false
}

fn password_tail_has_unknown_value(values: &[Cow<'_, str>]) -> bool {
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

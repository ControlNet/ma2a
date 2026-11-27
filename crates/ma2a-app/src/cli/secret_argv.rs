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
        .take_while(|value| *value != "--")
        .any(|value| value == "--password" || value.starts_with("--password="));
    // An invitation is bearer material wherever it appears, including as an inline `--flag=` value.
    let invite_ticket = values.iter().any(|value| {
        let trimmed = value.trim();
        trimmed.starts_with(INVITE_TICKET_PREFIX)
            || trimmed
                .split_once('=')
                .is_some_and(|(_, rhs)| rhs.trim().starts_with(INVITE_TICKET_PREFIX))
    });
    let stripped = strip_state_dir(&values);
    let accept_index = command_tail_index(&stripped, ["space", "accept"]);
    let unsupported_accept_value =
        accept_index.is_some_and(|index| stripped.get(index..).is_some_and(tail_has_unknown_value));
    let password_index = command_tail_index(&stripped, ["ui", "password", "set"])
        .or_else(|| command_tail_index(&stripped, ["ui", "password", "reset"]))
        .or_else(|| command_tail_index(&stripped, ["ui", "init"]));
    let unsupported_password_value = password_index
        .is_some_and(|index| stripped.get(index..).is_some_and(tail_has_unknown_value));
    if password_secret || invite_ticket || unsupported_password_value || unsupported_accept_value {
        Err(clap::Error::raw(
            clap::error::ErrorKind::InvalidValue,
            "secret values are not accepted in argv; use secure input",
        ))
    } else {
        Ok(())
    }
}

/// Removes valid global `--state-dir` options before `--` so subcommands cannot be split.
fn strip_state_dir<'a>(values: &[Cow<'a, str>]) -> Vec<Cow<'a, str>> {
    let mut kept = Vec::with_capacity(values.len());
    let mut iter = values.iter();
    while let Some(value) = iter.next() {
        if value == "--" {
            kept.push(value.clone());
            kept.extend(iter.cloned());
            break;
        }
        if value == "--state-dir" {
            if let Some(operand) = iter.next() {
                if operand.starts_with('-') {
                    kept.push(value.clone());
                    kept.push(operand.clone());
                }
            } else {
                kept.push(value.clone());
            }
        } else if value
            .strip_prefix("--state-dir=")
            .is_some_and(|path| !path.is_empty())
        {
        } else {
            kept.push(value.clone());
        }
    }
    kept
}

/// Anchors subcommand matching to index 0 (argv[0] is not present in `Cli::parse_safe`) so
/// passthrough commands after `exec --` or `process start --` are never misclassified.
fn command_tail_index<const N: usize>(
    values: &[Cow<'_, str>],
    command: [&str; N],
) -> Option<usize> {
    values
        .get(..command.len())
        .is_some_and(|prefix| prefix.iter().map(AsRef::as_ref).eq(command))
        .then_some(command.len())
}

/// Accepts only informational flags after a command whose remaining operands could only be secrets.
fn tail_has_unknown_value(values: &[Cow<'_, str>]) -> bool {
    values
        .iter()
        .any(|value| !matches!(value.as_ref(), "--help" | "-h" | "--version" | "-V"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::reject_secret_argv;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn rejects_secrets_even_when_global_state_dir_splits_subcommands_or_uses_inline_flag() {
        assert!(
            reject_secret_argv(&args(&[
                "ui",
                "--state-dir",
                "/tmp/s",
                "init",
                "SecretPass!"
            ]))
            .is_err()
        );
        assert!(
            reject_secret_argv(&args(&[
                "space",
                "--state-dir=/tmp/s",
                "accept",
                "raw-ticket"
            ]))
            .is_err()
        );
        assert!(reject_secret_argv(&args(&["echo", "--text=ma2ainvite1abcdef"])).is_err());
        assert!(reject_secret_argv(&args(&["ui", "--state-dir", "/tmp/s", "init"])).is_ok());
        assert!(
            reject_secret_argv(&args(&[
                "exec",
                "--endpoint",
                "peer",
                "--",
                "tool",
                "ui",
                "init",
                "--port",
                "80"
            ]))
            .is_ok()
        );
        assert!(
            reject_secret_argv(&args(&[
                "process",
                "start",
                "--endpoint",
                "peer",
                "--",
                "cli",
                "space",
                "accept",
                "foo"
            ]))
            .is_ok()
        );
        assert!(
            reject_secret_argv(&args(&[
                "exec",
                "--endpoint",
                "peer",
                "--",
                "psql",
                "--password",
            ]))
            .is_ok()
        );
        assert!(
            reject_secret_argv(&args(&[
                "process",
                "start",
                "--endpoint",
                "peer",
                "--",
                "tool",
                "--password=1",
            ]))
            .is_ok()
        );
        assert!(
            reject_secret_argv(&args(&[
                "agent",
                "define",
                "bot",
                "--",
                "bot",
                "--password",
            ]))
            .is_ok()
        );
        assert!(reject_secret_argv(&args(&["ui", "init", "--", "--password", "x"])).is_err());
        assert!(reject_secret_argv(&args(&["status", "--password=x"])).is_err());
    }
}

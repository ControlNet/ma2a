use std::path::Path;

use ma2a_runtime::ipc::IpcPaths;
use serde_json::Value;

use crate::{AppError, cli};

pub(crate) async fn run(
    state_dir: &Path,
    paths: IpcPaths,
    relay_command: cli::RelayCommand,
) -> Result<(), AppError> {
    match relay_command {
        cli::RelayCommand::Private { command } => match command {
            cli::PrivateRelayCommand::Configure(arguments) => {
                let mut fields = super::request_fields()?;
                fields.insert("listen".to_owned(), Value::String(arguments.listen));
                fields.insert("public_url".to_owned(), Value::String(arguments.public_url));
                fields.insert(
                    "served_space_ids".to_owned(),
                    Value::Array(
                        arguments
                            .serve_space
                            .into_iter()
                            .map(Value::String)
                            .collect(),
                    ),
                );
                let (mode, certificate, key) = if arguments.external_tls {
                    ("external_termination", Value::Null, Value::Null)
                } else {
                    (
                        "native_tls",
                        arguments.tls_cert.map_or(Value::Null, |path| {
                            Value::String(path.to_string_lossy().into_owned())
                        }),
                        arguments.tls_key.map_or(Value::Null, |path| {
                            Value::String(path.to_string_lossy().into_owned())
                        }),
                    )
                };
                fields.insert("mode".to_owned(), Value::String(mode.to_owned()));
                fields.insert("certificate_path".to_owned(), certificate);
                fields.insert("private_key_path".to_owned(), key);
                crate::call(
                    (state_dir, paths),
                    super::command("private_relay_configure", fields)?,
                    false,
                )
                .await
            }
            cli::PrivateRelayCommand::Disable => {
                crate::call(
                    (state_dir, paths),
                    super::command("private_relay_disable", super::request_fields()?)?,
                    false,
                )
                .await
            }
            cli::PrivateRelayCommand::Status { json } => {
                crate::call(
                    (state_dir, paths),
                    super::unit_command("private_relay_status")?,
                    json,
                )
                .await
            }
        },
        cli::RelayCommand::Public { command } => match command {
            cli::PublicRelayCommand::Configure { url } => {
                let mut fields = super::request_fields()?;
                fields.insert("url".to_owned(), Value::String(url));
                crate::call(
                    (state_dir, paths),
                    super::command("public_relay_configure", fields)?,
                    false,
                )
                .await
            }
            cli::PublicRelayCommand::Disable => {
                crate::call(
                    (state_dir, paths),
                    super::command("public_relay_disable", super::request_fields()?)?,
                    false,
                )
                .await
            }
            cli::PublicRelayCommand::Status { json } => {
                crate::call(
                    (state_dir, paths),
                    super::unit_command("public_relay_status")?,
                    json,
                )
                .await
            }
        },
    }
}

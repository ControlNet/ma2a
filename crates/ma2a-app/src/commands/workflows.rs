use std::{fmt::Write as _, io, path::Path};

use ma2a_core::RequestId;
use ma2a_runtime::{
    api,
    ipc::{IpcError, IpcPaths},
};
use serde_json::{Map, Value, json};

use crate::{AppError, cli};

mod relay;
mod space;

pub(crate) use relay::run as run_relay;
pub(crate) use space::run as run_space;

pub(crate) async fn run_echo(
    state_dir: &Path,
    paths: IpcPaths,
    arguments: cli::EchoArgs,
) -> Result<(), AppError> {
    let payload = match arguments.text {
        Some(text) => text,
        None if arguments.stdin => {
            let mut input = String::new();
            io::Read::read_to_string(&mut io::stdin().lock(), &mut input)?;
            input
        }
        None => return Err(AppError::Usage("echo requires --text or --stdin")),
    };
    let mut fields = request_fields()?;
    fields.insert(
        "target_endpoint_id".to_owned(),
        Value::String(arguments.endpoint),
    );
    fields.insert("payload".to_owned(), Value::String(payload));
    crate::call(
        (state_dir, paths),
        command("echo_call", fields)?,
        arguments.json,
    )
    .await
}

pub(crate) async fn run_ui(state_dir: &Path, command: cli::UiCommand) -> Result<(), AppError> {
    super::ui_lifecycle::run(state_dir, command).await
}

pub(crate) fn unit_command(operation: &str) -> Result<api::Command, AppError> {
    command(operation, Map::new())
}

pub(super) fn command(
    operation: &str,
    mut fields: Map<String, Value>,
) -> Result<api::Command, AppError> {
    fields.insert("version".to_owned(), json!(1));
    fields.insert("operation".to_owned(), Value::String(operation.to_owned()));
    let request = serde_json::to_vec(&Value::Object(fields)).map_err(io::Error::other)?;
    api::decode_command(&request)
        .map_err(IpcError::from)
        .map_err(Into::into)
}

pub(super) fn request_fields() -> Result<Map<String, Value>, AppError> {
    let request_id = RequestId::random()
        .map_err(|_| io::Error::other("operating-system random source failed"))?;
    let mut encoded = String::with_capacity(32);
    for byte in request_id.as_bytes() {
        write!(&mut encoded, "{byte:02x}")
            .map_err(|_| io::Error::other("request identifier encoding failed"))?;
    }
    let mut fields = Map::new();
    fields.insert("request_id".to_owned(), Value::String(encoded));
    Ok(fields)
}

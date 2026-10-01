//! `ma2a service register`, `start`, `stop`, `hosted`, and the remote `discover` and `call`.

use std::{io, path::Path};

use ma2a_runtime::ipc::IpcPaths;
use serde_json::{Value, json};

use crate::{
    AppError,
    cli::hosted::HostedServiceCommand,
    commands::{
        remote::service,
        workflows::{command, request_fields},
    },
};

pub(crate) async fn run(
    state_dir: &Path,
    paths: IpcPaths,
    hosted: HostedServiceCommand,
) -> Result<(), AppError> {
    let (operation, name, json) = match hosted {
        HostedServiceCommand::Register {
            service,
            manifest,
            json,
        } => {
            let text = std::fs::read(&manifest)?;
            let manifest: Value = serde_json::from_slice(&text)
                .map_err(|error| io::Error::other(format!("the manifest is not JSON: {error}")))?;
            let mut fields = request_fields()?;
            fields.insert("service".to_owned(), json!(service));
            fields.insert("manifest".to_owned(), manifest);
            let register = command("service_register", fields)?;
            return crate::call((state_dir, paths), register, json).await;
        }
        HostedServiceCommand::Hosted { after, json } => {
            let mut fields = serde_json::Map::new();
            fields.insert("after".to_owned(), json!(after));
            let list = command("service_hosted_list", fields)?;
            return crate::call((state_dir, paths), list, json).await;
        }
        HostedServiceCommand::Discover { endpoint, after } => {
            return service::discover(&paths, &endpoint, after).await;
        }
        HostedServiceCommand::Call(arguments) => return service::call(&paths, arguments).await,
        HostedServiceCommand::Unregister { service, json } => ("service_unregister", service, json),
        HostedServiceCommand::Start { service, json } => ("service_start", service, json),
        HostedServiceCommand::Stop { service, json } => ("service_stop", service, json),
    };
    let mut fields = request_fields()?;
    fields.insert("service".to_owned(), json!(name));
    crate::call((state_dir, paths), command(operation, fields)?, json).await
}

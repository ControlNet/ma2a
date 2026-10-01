//! Service grants, exposure, and operation status through the local API.

use std::{io, path::Path};

use ma2a_runtime::ipc::IpcPaths;
use serde_json::{Map, Value, json};

use crate::{
    AppError,
    cli::services::{ExposureArgs, GrantArgs, ServiceCommand},
    commands::workflows::{command, request_fields, resolve_space, unit_command},
};

pub(crate) async fn grant(
    state_dir: &Path,
    paths: IpcPaths,
    arguments: GrantArgs,
) -> Result<(), AppError> {
    let space_id = resolve_space(state_dir, &paths, &arguments.space).await?;
    let mut fields = request_fields()?;
    fields.insert("space_id".to_owned(), Value::String(space_id));
    fields.insert(
        "peer_endpoint_id".to_owned(),
        Value::String(arguments.endpoint),
    );
    for (name, granted) in [
        ("files_read", arguments.files_read),
        ("files_write", arguments.files_write),
        ("exec", arguments.exec),
        ("shell", arguments.shell),
        ("proxy", arguments.proxy),
        ("process", arguments.process),
        ("agent", arguments.agent),
    ] {
        fields.insert(name.to_owned(), Value::Bool(granted));
    }
    crate::call(
        (state_dir, paths),
        command("space_grant", fields)?,
        arguments.json,
    )
    .await
}

pub(crate) async fn run(
    state_dir: &Path,
    paths: IpcPaths,
    service_command: ServiceCommand,
) -> Result<(), AppError> {
    let (operation, exposure, json) = match service_command {
        ServiceCommand::List { json } => {
            return crate::call(
                (state_dir, paths),
                unit_command("service_exposure_list")?,
                json,
            )
            .await;
        }
        ServiceCommand::Expose { exposure, json } => ("service_expose", exposure, json),
        ServiceCommand::Unexpose { exposure, json } => ("service_unexpose", exposure, json),
    };
    let expose = operation == "service_expose";
    let mut fields = request_fields()?;
    let space = match &exposure {
        ExposureArgs::Files { space, .. }
        | ExposureArgs::Exec { space }
        | ExposureArgs::Shell { space }
        | ExposureArgs::Process { space }
        | ExposureArgs::Agent { space, .. }
        | ExposureArgs::Proxy { space, .. } => space,
    };
    fields.insert(
        "space_id".to_owned(),
        Value::String(resolve_space(state_dir, &paths, space).await?),
    );
    fields.extend(selector(exposure, expose)?);
    crate::call((state_dir, paths), command(operation, fields)?, json).await
}

pub(crate) async fn operations(
    state_dir: &Path,
    paths: IpcPaths,
    json: bool,
) -> Result<(), AppError> {
    crate::call((state_dir, paths), unit_command("operations_status")?, json).await
}

/// Flat nullable exposure fields; `unexpose` names the selector without path and access.
fn selector(exposure: ExposureArgs, expose: bool) -> Result<Map<String, Value>, AppError> {
    let agent = match &exposure {
        ExposureArgs::Agent { agent, .. } => Some(agent.clone()),
        _ => None,
    };
    let (kind, root, local, host, ports) = match exposure {
        ExposureArgs::Files {
            root,
            path,
            read_only,
            ..
        } => {
            let local = match path {
                Some(path) if expose => Some((local_path(path)?, read_only)),
                None if expose => return Err(AppError::Usage("exposing a root requires its PATH")),
                _ => None,
            };
            ("files_root", Some(root), local, None, None)
        }
        ExposureArgs::Exec { .. } => ("exec", None, None, None, None),
        ExposureArgs::Shell { .. } => ("shell", None, None, None, None),
        ExposureArgs::Process { .. } => ("process", None, None, None, None),
        ExposureArgs::Agent { .. } => ("agent", None, None, None, None),
        ExposureArgs::Proxy { host, ports, .. } => {
            ("proxy", None, None, Some(host), Some(port_range(&ports)?))
        }
    };
    let mut fields = Map::new();
    fields.insert("kind".to_owned(), json!(kind));
    fields.insert("root".to_owned(), json!(root));
    fields.insert("host".to_owned(), json!(host));
    fields.insert(
        "port_start".to_owned(),
        json!(ports.map(|(start, _)| start)),
    );
    fields.insert("port_end".to_owned(), json!(ports.map(|(_, end)| end)));
    fields.insert("agent".to_owned(), json!(agent));
    if expose {
        let access = local.as_ref().map(|(_, read_only)| {
            if *read_only {
                "read_only"
            } else {
                "read_write"
            }
        });
        fields.insert("path".to_owned(), json!(local.map(|(path, _)| path)));
        fields.insert("access".to_owned(), json!(access));
    }
    Ok(fields)
}

/// Makes a root path absolute for the daemon, which canonicalizes and validates it.
fn local_path(path: std::path::PathBuf) -> Result<String, AppError> {
    Ok(std::path::absolute(path)?
        .into_os_string()
        .into_string()
        .map_err(|_| io::Error::other("root path is not valid UTF-8"))?)
}

/// Parses `N` or an inclusive `N-M` port range.
fn port_range(ports: &str) -> Result<(u16, u16), AppError> {
    let (start, end) = ports.split_once('-').unwrap_or((ports, ports));
    match (start.parse::<u16>(), end.parse::<u16>()) {
        (Ok(start), Ok(end)) if 0 < start && start <= end => Ok((start, end)),
        _ => Err(AppError::Usage(
            "PORTS must be N or N-M with 1 <= N <= M <= 65535",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::port_range;

    #[test]
    fn port_ranges_are_single_ports_or_ordered_inclusive_ranges() {
        assert_eq!(port_range("8080").ok(), Some((8080, 8080)));
        assert_eq!(port_range("8000-8100").ok(), Some((8000, 8100)));
        assert!(port_range("0").is_err());
        assert!(port_range("9-8").is_err());
        assert!(port_range("80-").is_err());
        assert!(port_range("http").is_err());
    }
}

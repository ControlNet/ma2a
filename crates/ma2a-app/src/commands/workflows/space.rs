use std::{
    fmt::Write as _,
    io,
    io::IsTerminal as _,
    path::{Path, PathBuf},
};

use ma2a_core::RequestId;
use ma2a_runtime::ipc::IpcPaths;
use serde_json::{Map, Value, json};

use crate::{AppError, cli};

pub(crate) async fn run(
    state_dir: &Path,
    paths: IpcPaths,
    space_command: cli::SpaceCommand,
) -> Result<(), AppError> {
    match space_command {
        cli::SpaceCommand::List { json } => {
            crate::call((state_dir, paths), super::unit_command("space_list")?, json).await
        }
        cli::SpaceCommand::Show { space, json } => {
            let mut fields = Map::new();
            fields.insert("space_id".to_owned(), Value::String(space));
            crate::call(
                (state_dir, paths),
                super::command("space_show", fields)?,
                json,
            )
            .await
        }
        cli::SpaceCommand::Sync { command } => match command {
            cli::SyncCommand::Status { endpoint, json } => {
                sync((state_dir, paths), SyncRequest::status(endpoint, json)).await
            }
            cli::SyncCommand::Now { endpoint, json } => {
                sync((state_dir, paths), SyncRequest::trigger(endpoint, json)).await
            }
        },
        cli::SpaceCommand::Create { name, json } => {
            let mut fields = super::request_fields()?;
            fields.insert("name".to_owned(), Value::String(name));
            crate::call(
                (state_dir, paths),
                super::command("space_create", fields)?,
                json,
            )
            .await
        }
        cli::SpaceCommand::Invite { command } => invite(state_dir, paths, command).await,
        cli::SpaceCommand::Member {
            command:
                cli::MemberCommand::Revoke {
                    space,
                    endpoint,
                    json,
                },
        } => {
            let mut fields = super::request_fields()?;
            fields.insert("space_id".to_owned(), Value::String(space));
            fields.insert("peer_endpoint_id".to_owned(), Value::String(endpoint));
            crate::call(
                (state_dir, paths),
                super::command("space_revoke", fields)?,
                json,
            )
            .await
        }
    }
}

async fn invite(
    state_dir: &Path,
    paths: IpcPaths,
    invite_command: cli::InviteCommand,
) -> Result<(), AppError> {
    match invite_command {
        cli::InviteCommand::Create {
            space,
            ttl,
            file,
            stdout,
        } => {
            let output_path = invite_output_path(state_dir, file, stdout)?;
            let mut fields = super::request_fields()?;
            fields.insert("space_id".to_owned(), Value::String(space));
            fields.insert("ttl_ms".to_owned(), json!(parse_duration_ms(&ttl)?));
            fields.insert(
                "output_path".to_owned(),
                Value::String(output_path.to_string_lossy().into_owned()),
            );
            crate::call(
                (state_dir, paths),
                super::command("space_invite", fields)?,
                false,
            )
            .await?;
            if stdout {
                let invitation = std::fs::read_to_string(&output_path)?;
                std::fs::remove_file(&output_path)?;
                print!("{invitation}");
            }
            Ok(())
        }
        cli::InviteCommand::Redeem(arguments) => {
            let invitation = match arguments.file {
                Some(path) => std::fs::read_to_string(path)?,
                None if arguments.stdin => {
                    let mut input = String::new();
                    io::Read::read_to_string(&mut io::stdin().lock(), &mut input)?;
                    input
                }
                None => return Err(AppError::Usage("invite redeem requires --stdin or --file")),
            };
            let mut fields = super::request_fields()?;
            fields.insert(
                "invitation".to_owned(),
                Value::String(invitation.trim().to_owned()),
            );
            crate::call(
                (state_dir, paths),
                super::command("space_redeem", fields)?,
                false,
            )
            .await
        }
    }
}

struct SyncRequest {
    operation: &'static str,
    endpoint: String,
    json: bool,
    mutation: bool,
}

impl SyncRequest {
    const fn status(endpoint: String, json: bool) -> Self {
        Self {
            operation: "control_sync_status",
            endpoint,
            json,
            mutation: false,
        }
    }

    const fn trigger(endpoint: String, json: bool) -> Self {
        Self {
            operation: "control_sync_trigger",
            endpoint,
            json,
            mutation: true,
        }
    }
}

async fn sync(runtime: (&Path, IpcPaths), request: SyncRequest) -> Result<(), AppError> {
    let mut fields = if request.mutation {
        super::request_fields()?
    } else {
        Map::new()
    };
    fields.insert(
        "peer_endpoint_id".to_owned(),
        Value::String(request.endpoint),
    );
    crate::call(
        runtime,
        super::command(request.operation, fields)?,
        request.json,
    )
    .await
}

fn parse_duration_ms(value: &str) -> Result<u64, AppError> {
    let (amount, multiplier) = if let Some(amount) = value.strip_suffix("ms") {
        (amount, 1)
    } else if let Some(amount) = value.strip_suffix('s') {
        (amount, 1_000)
    } else if let Some(amount) = value.strip_suffix('m') {
        (amount, 60_000)
    } else {
        return Err(AppError::Usage("TTL must end in ms, s, or m"));
    };
    amount
        .parse::<u64>()
        .ok()
        .and_then(|amount| amount.checked_mul(multiplier))
        .filter(|ttl| (1..=300_000).contains(ttl))
        .ok_or(AppError::Usage("TTL must be between 1ms and 5m"))
}

fn invite_output_path(
    state_dir: &Path,
    file: Option<PathBuf>,
    stdout: bool,
) -> Result<PathBuf, AppError> {
    if let Some(path) = file {
        return Ok(path);
    }
    if !stdout || !io::stdout().is_terminal() {
        return Err(AppError::Usage(
            "invite create requires --file or interactive --stdout",
        ));
    }
    let request_id = RequestId::random()
        .map_err(|_| io::Error::other("operating-system random source failed"))?;
    Ok(state_dir.join(format!(".invite-{}", request_id_hex(request_id)?)))
}

fn request_id_hex(request_id: RequestId) -> Result<String, AppError> {
    let mut encoded = String::with_capacity(32);
    for byte in request_id.as_bytes() {
        write!(&mut encoded, "{byte:02x}")
            .map_err(|_| io::Error::other("request identifier encoding failed"))?;
    }
    Ok(encoded)
}

use std::path::Path;

use ma2a_runtime::ipc::IpcPaths;
use serde_json::{Map, Value};

use crate::{AppError, cli};

#[path = "space_invite.rs"]
mod space_invite;
#[path = "space_reference.rs"]
mod space_reference;

use space_invite::{InviteRequest, accept, invite};
use space_reference::resolve_space;

#[cfg(windows)]
#[path = "space_windows.rs"]
mod space_windows;

#[cfg(unix)]
#[path = "space_unix.rs"]
mod space_unix;

#[cfg(any(windows, test))]
#[path = "space_windows_policy.rs"]
mod space_windows_policy;

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
            let space_id = resolve_space(state_dir, &paths, &space).await?;
            let mut fields = Map::new();
            fields.insert("space_id".to_owned(), Value::String(space_id));
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
        cli::SpaceCommand::Invite { space, ttl } => {
            invite(
                (state_dir, paths),
                InviteRequest {
                    space: &space,
                    ttl: &ttl,
                },
            )
            .await
        }
        cli::SpaceCommand::Accept => accept(state_dir, paths).await,
        cli::SpaceCommand::Leave { space, json } => {
            let space_id = resolve_space(state_dir, &paths, &space).await?;
            let mut fields = super::request_fields()?;
            fields.insert("space_id".to_owned(), Value::String(space_id));
            crate::call(
                (state_dir, paths),
                super::command("space_leave", fields)?,
                json,
            )
            .await
        }
        cli::SpaceCommand::Member {
            command:
                cli::MemberCommand::Remove {
                    space,
                    endpoint,
                    json,
                },
        } => {
            let space_id = resolve_space(state_dir, &paths, &space).await?;
            let mut fields = super::request_fields()?;
            fields.insert("space_id".to_owned(), Value::String(space_id));
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

#[cfg(unix)]
use space_unix::read_owner_only_invitation;

#[cfg(windows)]
fn read_owner_only_invitation(path: &Path) -> Result<String, AppError> {
    space_windows::read_owner_only_invitation(path)
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

//! `ma2a service discover` and `call`: another Endpoint's services, used as this Endpoint.

use std::{io::Write as _, time::Duration};

use ma2a_core::service::{
    DescribePage, HostedCall, OpenBody, OperationName, ServiceDescriptor, ServiceError,
    ServiceName, ServiceOrigin, ServiceRequest, ServiceVersion,
};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client::{ClientError, process::ProcessIo, service};
use serde_json::{Value, json};

use super::{open, open_as, process::interrupted};
use crate::{AppError, cli::hosted::CallArgs};

fn version_value(version: &ServiceVersion) -> Value {
    let operations: Vec<Value> = version
        .operations()
        .iter()
        .map(
            |(name, interaction)| json!({"name": name.as_str(), "interaction": interaction.name()}),
        )
        .collect();
    json!({"version": version.version(), "operations": operations})
}

fn descriptor_value(descriptor: &ServiceDescriptor) -> Value {
    json!({
        "service": descriptor.origin().name(),
        "builtin": matches!(descriptor.origin(), ServiceOrigin::Builtin(_)),
        "description": descriptor.description(),
        "available": descriptor.available(),
        "versions": descriptor.versions().iter().map(version_value).collect::<Vec<_>>(),
    })
}

/// Longest wait for one discovery page, so a target that never answers fails the command.
const PAGE_DEADLINE: Duration = Duration::from_secs(30);

/// Runs `read` under `deadline`, failing as `timed_out` once it passes.
async fn bounded<T>(
    deadline: Duration,
    read: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    tokio::time::timeout(deadline, read)
        .await
        .unwrap_or_else(|_| Err(ClientError::Stream(ServiceError::TimedOut).into()))
}

async fn page(
    (paths, endpoint): (&IpcPaths, &str),
    after: Option<String>,
) -> Result<DescribePage, AppError> {
    bounded(PAGE_DEADLINE, async {
        let body = OpenBody::Service(ServiceRequest::Describe(after));
        let (mut reader, writer) = open(paths, endpoint, body).await?;
        let page = service::describe(&mut reader).await;
        drop(writer);
        Ok(page?)
    })
    .await
}

/// Prints one page of what `endpoint` serves this Endpoint.
pub(crate) async fn discover(
    paths: &IpcPaths,
    endpoint: &str,
    after: Option<String>,
) -> Result<(), AppError> {
    let page = page((paths, endpoint), after).await?;
    let services: Vec<Value> = page.descriptors.iter().map(descriptor_value).collect();
    writeln!(
        std::io::stdout().lock(),
        "{}",
        json!({"services": services, "next": page.next})
    )?;
    Ok(())
}

/// Most discovery pages read to find one service: every built-in family and the 64 hosted
/// services a Runtime registers fit in 5, so a target paging further is not believed.
const MAX_PAGES: usize = 8;

/// The highest version of `service` that lists `operation`, from discovery.
async fn newest(
    target: (&IpcPaths, &str),
    (service, operation): (&ServiceName, &OperationName),
) -> Result<u16, AppError> {
    let origin = ServiceOrigin::Hosted(service.clone());
    let mut after = None;
    for _ in 0..MAX_PAGES {
        let page = page(target, after).await?;
        if let Some(descriptor) = page
            .descriptors
            .iter()
            .find(|found| found.origin() == &origin)
        {
            return descriptor
                .versions()
                .iter()
                .filter(|version| version.operation(operation.as_str()).is_some())
                .map(ServiceVersion::version)
                .max()
                .ok_or_else(|| {
                    AppError::Invalid(format!(
                        "version_mismatch: no version of {} lists {}",
                        service.as_str(),
                        operation.as_str()
                    ))
                });
        }
        if page.next.is_none() {
            break;
        }
        after = page.next;
    }
    Err(AppError::NotFound(format!(
        "unknown_service: the target serves this Endpoint no service {}",
        service.as_str()
    )))
}

/// Calls one operation; output goes to stdout followed by the result, errors to stderr, and
/// Ctrl-C cancels the call.
pub(crate) async fn call(paths: &IpcPaths, arguments: CallArgs) -> Result<(), AppError> {
    let service = ServiceName::new(&arguments.service)
        .map_err(|_| AppError::Invalid(format!("invalid service name: {}", arguments.service)))?;
    let operation = OperationName::new(&arguments.operation).map_err(|_| {
        AppError::Invalid(format!("invalid operation name: {}", arguments.operation))
    })?;
    let target = (paths, arguments.endpoint.as_str());
    let version = match arguments.version {
        Some(version) => version,
        None => newest(target, (&service, &operation)).await?,
    };
    let call = HostedCall::new((service, version), operation, arguments.body.into_bytes())
        .map_err(|_| AppError::Usage("--version must be at least 1 and --body at most 8 KiB"))?;
    let body = OpenBody::Service(ServiceRequest::Call(call));
    let (mut reader, mut writer) =
        open_as(paths, (target.1, arguments.request_id.as_deref()), body).await?;
    let local = ProcessIo {
        input: arguments.stdin.then(tokio::io::stdin),
        output: tokio::io::stdout(),
        error: Some(tokio::io::stderr()),
    };
    let called = service::call((&mut reader, &mut writer), local, interrupted()).await;
    drop((reader, writer));
    let printed = called.map_err(AppError::from).and_then(|called| {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&called.result)?;
        Ok(stdout.flush()?)
    });
    if arguments.stdin {
        super::exit_with(printed);
    }
    printed
}

#[cfg(test)]
mod tests {
    use super::{AppError, PAGE_DEADLINE, bounded};

    #[tokio::test(start_paused = true)]
    async fn a_page_that_never_comes_fails_as_timed_out() {
        // Given: a target that never answers.
        let read = bounded(
            PAGE_DEADLINE,
            std::future::pending::<Result<(), AppError>>(),
        );

        // When
        let answered = tokio::time::timeout(PAGE_DEADLINE * 2, read).await;

        // Then
        assert!(matches!(
            answered,
            Ok(Err(AppError::Protocol(message))) if message.starts_with("timed_out")
        ));
    }
}

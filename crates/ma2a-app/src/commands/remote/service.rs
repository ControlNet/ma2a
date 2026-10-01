//! `ma2a service discover` and `call`: another Endpoint's services, used as this Endpoint.

use std::io::Write as _;

use ma2a_core::service::{
    DescribePage, HostedCall, OpenBody, OperationName, ServiceDescriptor, ServiceName,
    ServiceOrigin, ServiceRequest, ServiceVersion,
};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client::{process::ProcessIo, service};
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

async fn page(
    (paths, endpoint): (&IpcPaths, &str),
    after: Option<String>,
) -> Result<DescribePage, AppError> {
    let body = OpenBody::Service(ServiceRequest::Describe(after));
    let (mut reader, writer) = open(paths, endpoint, body).await?;
    let page = service::describe(&mut reader).await;
    drop(writer);
    Ok(page?)
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

/// The highest version of `service` that lists `operation`, from discovery.
async fn newest(
    target: (&IpcPaths, &str),
    (service, operation): (&ServiceName, &OperationName),
) -> Result<u16, AppError> {
    let origin = ServiceOrigin::Hosted(service.clone());
    let mut after = None;
    loop {
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
        after = match page.next {
            Some(next) => Some(next),
            None => {
                return Err(AppError::NotFound(format!(
                    "unknown_service: the target serves this Endpoint no service {}",
                    service.as_str()
                )));
            }
        };
    }
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
    let called = called?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&called.result)?;
    stdout.flush()?;
    drop(stdout);
    if arguments.stdin {
        // A pending read of stdin cannot be cancelled and would hold the runtime open.
        std::process::exit(0);
    }
    Ok(())
}

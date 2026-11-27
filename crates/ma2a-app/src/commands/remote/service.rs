//! `ma2a service discover` and `call`: another Endpoint's services, used as this Endpoint.

use std::{
    io::Write as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ma2a_core::service::{
    DescribePage, HostedCall, OpenBody, OperationName, ServiceDescriptor, ServiceError,
    ServiceName, ServiceOrigin, ServiceRequest, ServiceVersion,
};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client::{ClientError, process::ProcessIo, service};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;

use super::{bounded, open, open_as, process::interrupted, stdio::StdioGuard};
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

async fn page(
    (paths, endpoint): (&IpcPaths, &str),
    after: Option<String>,
) -> Result<DescribePage, AppError> {
    bounded(PAGE_DEADLINE, async {
        let body = OpenBody::Service(ServiceRequest::Describe(after.clone()));
        let (mut reader, writer) = open(paths, endpoint, body).await?;
        let page = service::describe(&mut reader, after.as_deref()).await;
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
        let page = page(target, after.clone()).await?;
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
            return Err(AppError::NotFound(format!(
                "unknown_service: the target serves this Endpoint no service {}",
                service.as_str()
            )));
        }
        after = page.next;
    }
    Err(ClientError::Stream(ServiceError::InvalidInput).into())
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
    let stdio = StdioGuard::default();
    let local = ProcessIo {
        input: arguments.stdin.then(tokio::io::stdin),
        output: stdio.stdout(),
        error: Some(stdio.stderr()),
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&cancelled);
    let mut interrupt = Box::pin(async move {
        interrupted().await;
        cancel_flag.store(true, Ordering::Release);
    });
    let called = service::call((&mut reader, &mut writer), local, interrupt.as_mut()).await;
    drop((reader, writer));
    match called {
        Ok(called) => {
            if stdio.stdout_stuck() {
                stdio.exit_error(interrupted_io().into()).await;
            }
            let was_cancelled = cancelled.load(Ordering::Acquire);
            let deliver = async {
                let mut stdout = stdio.stdout();
                stdout.write_all(&called.result).await?;
                stdout.flush().await
            };
            let outcome = if was_cancelled {
                tokio::time::timeout(super::stdio::DIAGNOSTIC_GRACE, deliver)
                    .await
                    .unwrap_or_else(|_| Err(interrupted_io()))
            } else {
                tokio::select! {
                    result = deliver => result,
                    () = interrupt.as_mut() => Err(interrupted_io()),
                }
            };
            match outcome {
                Ok(()) => stdio.exit_code(0, !was_cancelled).await,
                Err(error) => stdio.exit_error(error.into()).await,
            }
        }
        Err(error) => stdio.exit_error(error.into()).await,
    }
}

fn interrupted_io() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "standard output delivery was interrupted",
    )
}

#[cfg(test)]
mod tests {
    use super::{AppError, PAGE_DEADLINE, bounded};

    #[cfg(unix)]
    mod through_the_daemon {
        use ma2a_core::service::{
            DescribePage, FrameKind, Interaction, OperationName, ServiceDescriptor,
            ServiceManifest, ServiceName, ServiceVersion,
        };

        use crate::{
            AppError,
            test_daemon::{Daemon, Script, TestResult},
        };

        /// A hosted service `name` whose one version lists `run`.
        fn hosted(name: &str) -> TestResult<ServiceDescriptor> {
            let run = (OperationName::new("run")?, Interaction::Unary);
            let manifest = ServiceManifest::new("", vec![ServiceVersion::new(1, vec![run])?], 1)?;
            Ok(ServiceDescriptor::hosted(
                ServiceName::new(name)?,
                &manifest,
                true,
            ))
        }

        /// A target's discovery page listing `services`, built-in or hosted, and naming `next`.
        fn page(services: &[&str], next: Option<&str>) -> TestResult<Script> {
            let descriptors = services
                .iter()
                .map(|name| match ma2a_core::ServiceKind::from_name(name) {
                    Some(kind) => Ok(ma2a_core::service::builtin_descriptor(kind)?),
                    None => hosted(name),
                })
                .collect::<TestResult<_>>()?;
            let page = DescribePage::new(descriptors, next.map(str::to_owned));
            Ok(vec![(
                FrameKind::Outcome,
                DescribePage::encode_outcome(Ok(&page))?,
            )])
        }

        /// What the automatic lookup of `beta` makes of a target answering `pages` in turn.
        async fn newest_beta(pages: Vec<Script>) -> TestResult<Result<u16, AppError>> {
            let daemon = Daemon::paging("describe", pages)?;
            let (beta, run) = (ServiceName::new("beta")?, OperationName::new("run")?);
            let target = (&daemon.paths, daemon.endpoint.as_str());
            Ok(super::super::newest(target, (&beta, &run)).await)
        }

        #[tokio::test]
        async fn the_lookup_follows_only_pages_that_continue_the_listing() -> TestResult {
            let broken = |found: Result<u16, AppError>| matches!(found, Err(AppError::Protocol(_)));
            // Built-ins first, then hosted services by name, across the page boundary.
            let pages = vec![
                page(&["files", "alpha"], Some("alpha"))?,
                page(&["beta"], None)?,
            ];
            assert_eq!(newest_beta(pages).await?.ok(), Some(1));
            // A cursor past the page's last service would skip `beta`.
            let pages = vec![page(&["alpha"], Some("zulu"))?, page(&[], None)?];
            assert!(broken(newest_beta(pages).await?));
            // Out of order, or a cursor naming no listed service.
            assert!(broken(
                newest_beta(vec![page(&["gamma", "beta"], None)?]).await?
            ));
            let pages = vec![page(&[], Some("alpha"))?, page(&["beta"], None)?];
            assert!(broken(newest_beta(pages).await?));
            // A target still paging after the most pages read is not believed.
            let names: Vec<String> = (0..super::super::MAX_PAGES)
                .map(|page| format!("a{page:03}"))
                .collect();
            let pages = names
                .iter()
                .map(|name| page(&[name.as_str()], Some(name.as_str())))
                .collect::<TestResult<Vec<_>>>()?;
            assert!(broken(newest_beta(pages).await?));
            Ok(())
        }
    }

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

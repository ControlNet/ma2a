//! Discovery: what this Endpoint serves to one caller, never to anyone else.
//!
//! There is no registry. A caller asks one target, and the target answers from its own
//! durable authorization state: every built-in family whose admission the caller passes, and
//! every hosted service the caller may call. No Space, member, or path is disclosed.

use ma2a_core::{
    EchoError, EndpointId, ServiceKind,
    service::{
        DescribePage, MAX_DESCRIBE_PAGE, ServiceAccess, ServiceDescriptor, ServiceError, admission,
        builtin_descriptor, sorts_after,
    },
};
use ma2a_store::HostedServiceRecord;

use super::{HostedServices, MAX_HOSTED_SERVICES};
use crate::{api::HostedServiceState, store::authorize};

impl HostedServices {
    /// Returns one page of the services `peer` may use, after the `after` cursor.
    pub(crate) async fn describe(
        &self,
        peer: EndpointId,
        after: Option<String>,
    ) -> Result<DescribePage, ServiceError> {
        let local = self.0.local;
        let limit = u32::try_from(MAX_HOSTED_SERVICES).unwrap_or(u32::MAX);
        let (builtins, hosted) = self
            .store(move |repository| {
                let allowed = |access: &ServiceAccess| {
                    authorize(repository, (local, peer), access).map(|_| ())
                };
                let builtins = ServiceKind::ALL
                    .into_iter()
                    .filter_map(|kind| {
                        let decision = admission(kind).map_or_else(
                            || {
                                crate::services::echo::authorize(repository, local, peer).map_err(
                                    |error| match error {
                                        EchoError::Unauthorized => ServiceError::Unauthorized,
                                        _ => ServiceError::Unavailable,
                                    },
                                )
                            },
                            |access| allowed(&access),
                        );
                        shown(kind, decision)
                    })
                    .collect::<Result<Vec<_>, _>>();
                let hosted = repository
                    .hosted_services((None, limit))?
                    .into_value()
                    .into_iter()
                    .filter_map(|record| {
                        let decision = allowed(&ServiceAccess::Hosted(record.name.clone()));
                        shown(record, decision)
                    })
                    .collect::<Result<Vec<HostedServiceRecord>, _>>();
                Ok(builtins.and_then(|builtins| Ok((builtins, hosted?))))
            })
            .await??;
        let mut descriptors = builtins
            .into_iter()
            .map(builtin_descriptor)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ServiceError::Internal)?;
        descriptors.extend(hosted.into_iter().map(|record| {
            let view = self.observe(record);
            let ready = view.state() == HostedServiceState::Ready;
            let record = view.into_record();
            ServiceDescriptor::hosted(record.name, &record.manifest, ready)
        }));
        let mut page: Vec<ServiceDescriptor> = descriptors
            .into_iter()
            .filter(|descriptor| sorts_after(descriptor.origin(), after.as_deref()))
            .take(MAX_DESCRIBE_PAGE + 1)
            .collect();
        let next = (page.len() > MAX_DESCRIBE_PAGE)
            .then(|| {
                page.truncate(MAX_DESCRIBE_PAGE);
                page.last()
                    .map(|descriptor| descriptor.origin().name().to_owned())
            })
            .flatten();
        Ok(DescribePage::new(page, next))
    }
}

/// What one authorization decision makes of `item` in a page: listed when allowed, left out
/// when refused, and the page's failure otherwise, since leaving it out would tell the caller
/// a service it may use is not there.
fn shown<T>(item: T, decision: Result<(), ServiceError>) -> Option<Result<T, ServiceError>> {
    match decision {
        Ok(()) => Some(Ok(item)),
        Err(ServiceError::Unauthorized) => None,
        Err(error) => Some(Err(error)),
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::ServiceError;

    use super::shown;

    #[test]
    fn only_a_refusal_leaves_a_service_out() {
        assert_eq!(shown(1, Ok(())), Some(Ok(1)));
        assert_eq!(shown(1, Err(ServiceError::Unauthorized)), None);
        assert_eq!(
            shown(1, Err(ServiceError::Unavailable)),
            Some(Err(ServiceError::Unavailable))
        );
    }
}

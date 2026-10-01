//! Discovery: what this Endpoint serves to one caller, never to anyone else.
//!
//! There is no registry. A caller asks one target, and the target answers from its own
//! durable authorization state: every built-in family whose admission the caller passes, and
//! every hosted service the caller may call. No Space, member, or path is disclosed.

use ma2a_core::{
    EndpointId, ServiceKind,
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
                let builtins: Vec<ServiceKind> = ServiceKind::ALL
                    .into_iter()
                    .filter(|kind| {
                        admission(*kind).map_or_else(
                            || crate::services::echo::authorize(repository, local, peer).is_ok(),
                            |access| authorize(repository, (local, peer), &access).is_ok(),
                        )
                    })
                    .collect();
                let hosted: Vec<HostedServiceRecord> = repository
                    .hosted_services((None, limit))?
                    .into_value()
                    .into_iter()
                    .filter(|record| {
                        let access = ServiceAccess::Hosted(record.name.clone());
                        authorize(repository, (local, peer), &access).is_ok()
                    })
                    .collect();
                Ok((builtins, hosted))
            })
            .await?;
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

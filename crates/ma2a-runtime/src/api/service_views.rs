//! Local API views of service exposure and streaming operation status.
//!
//! Operation views are content-free by construction: they carry no path, argv,
//! environment, terminal bytes, or proxied bytes.

use ma2a_core::{
    EndpointId, RequestId, ServiceKind, SpaceId,
    service::{ProxyRule, RootAccess, RootName, ServiceError},
};
use serde_json::{Map, Value, json};

use super::{
    ApiError, CommandResult, SpaceMemberView,
    codec_fields::encode_hex,
    codec_services::{ExposureSpec, spec_fields},
    commands::BoundedText,
    responses::ResultKind,
};

/// Most active and most recent operations one `operations_status` result carries.
pub const MAX_LISTED_OPERATIONS: usize = 64;

/// One thing this Runtime serves inside one Space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceExposureView {
    space_id: SpaceId,
    spec: ExposureSpec,
}

impl ServiceExposureView {
    /// An exported root with its local directory and access in this Space.
    ///
    /// # Errors
    /// Returns invalid input for an invalid root name or an oversized path.
    pub fn files_root(
        space_id: SpaceId,
        (root, path): (&str, &str),
        access: RootAccess,
    ) -> Result<Self, ApiError> {
        let root = RootName::new(root).map_err(ApiError::new)?;
        let path = BoundedText::parse(path, 4_096)?;
        Ok(Self {
            space_id,
            spec: ExposureSpec::FilesRoot {
                root,
                local: Some((path, access)),
            },
        })
    }

    /// Exec served in this Space.
    pub const fn exec(space_id: SpaceId) -> Self {
        Self {
            space_id,
            spec: ExposureSpec::Exec,
        }
    }

    /// Shell served in this Space.
    pub const fn shell(space_id: SpaceId) -> Self {
        Self {
            space_id,
            spec: ExposureSpec::Shell,
        }
    }

    /// One allowed proxy destination rule in this Space.
    pub const fn proxy(space_id: SpaceId, rule: ProxyRule) -> Self {
        Self {
            space_id,
            spec: ExposureSpec::Proxy(rule),
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "space_id".to_owned(),
            json!(encode_hex(self.space_id.as_bytes())),
        );
        for (name, value) in spec_fields(&self.spec) {
            object.insert(name.to_owned(), value);
        }
        Value::Object(object)
    }
}

/// Who and what one operation is, without any of its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationIdentity {
    request_id: RequestId,
    peer: EndpointId,
    service: ServiceKind,
    operation: &'static str,
    outbound: bool,
}

impl OperationIdentity {
    /// Creates one identity; `operation` is a stable content-free name such as `upload`.
    pub const fn new(
        request_id: RequestId,
        peer: EndpointId,
        (service, operation, outbound): (ServiceKind, &'static str, bool),
    ) -> Self {
        Self {
            request_id,
            peer,
            service,
            operation,
            outbound,
        }
    }

    fn fields(&self) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert(
            "request_id".to_owned(),
            json!(encode_hex(self.request_id.as_bytes())),
        );
        object.insert(
            "peer_endpoint_id".to_owned(),
            json!(encode_hex(self.peer.as_bytes())),
        );
        object.insert("service".to_owned(), json!(self.service.name()));
        object.insert("operation".to_owned(), json!(self.operation));
        object.insert(
            "direction".to_owned(),
            json!(if self.outbound { "outbound" } else { "inbound" }),
        );
        object
    }
}

/// How one completed operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationEndView {
    result: &'static str,
    error: Option<ServiceError>,
}

impl OperationEndView {
    /// The operation completed.
    pub const fn completed() -> Self {
        Self {
            result: "completed",
            error: None,
        }
    }

    /// The operation was rejected before any effect.
    pub const fn rejected(error: ServiceError) -> Self {
        Self {
            result: "rejected",
            error: Some(error),
        }
    }

    /// The operation started and then failed.
    pub const fn failed(error: ServiceError) -> Self {
        Self {
            result: "failed",
            error: Some(error),
        }
    }
}

/// One completed operation with its byte counts and duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationRecordView {
    identity: OperationIdentity,
    end: OperationEndView,
    bytes: (u64, u64),
    duration_ms: u64,
}

impl OperationRecordView {
    /// Creates one audit record; `bytes` is `(in, out)` as seen by this Runtime.
    pub const fn new(
        identity: OperationIdentity,
        end: OperationEndView,
        bytes: (u64, u64),
    ) -> Self {
        Self {
            identity,
            end,
            bytes,
            duration_ms: 0,
        }
    }

    /// Returns this record with its duration.
    #[must_use]
    pub const fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }
}

/// Active operations and the most recent completed ones, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsView {
    active: Vec<(OperationIdentity, u64)>,
    recent: Vec<OperationRecordView>,
    totals: (usize, usize),
}

impl OperationsView {
    /// Keeps at most [`MAX_LISTED_OPERATIONS`] of each list; totals still count every one.
    pub fn new(active: Vec<(OperationIdentity, u64)>, recent: Vec<OperationRecordView>) -> Self {
        let totals = (active.len(), recent.len());
        let mut view = Self {
            active,
            recent,
            totals,
        };
        view.active.truncate(MAX_LISTED_OPERATIONS);
        view.recent.truncate(MAX_LISTED_OPERATIONS);
        view
    }

    pub(crate) fn to_value(&self) -> Value {
        let active = self
            .active
            .iter()
            .map(|(identity, elapsed_ms)| {
                let mut object = identity.fields();
                object.insert("elapsed_ms".to_owned(), json!(elapsed_ms));
                Value::Object(object)
            })
            .collect::<Vec<_>>();
        let recent = self
            .recent
            .iter()
            .map(|record| {
                let mut object = record.identity.fields();
                object.insert("result".to_owned(), json!(record.end.result));
                object.insert(
                    "error".to_owned(),
                    json!(record.end.error.map(ServiceError::name)),
                );
                object.insert("bytes_in".to_owned(), json!(record.bytes.0));
                object.insert("bytes_out".to_owned(), json!(record.bytes.1));
                object.insert("duration_ms".to_owned(), json!(record.duration_ms));
                Value::Object(object)
            })
            .collect::<Vec<_>>();
        json!({
            "active_count": self.totals.0,
            "active": active,
            "recent_count": self.totals.1,
            "recent": recent,
        })
    }
}

impl CommandResult {
    /// Creates the acknowledgement after which a stream-plane connection carries frames.
    pub const fn stream_opened() -> Self {
        Self(ResultKind::StreamOpened, None)
    }

    /// Creates the result of an owner-signed service grant change.
    pub const fn space_granted(value: SpaceMemberView) -> Self {
        Self(ResultKind::SpaceGranted(value), None)
    }
    /// Creates the result of adding or updating one local exposure.
    pub const fn service_exposed(value: ServiceExposureView) -> Self {
        Self(ResultKind::ServiceExposed(value), None)
    }
    /// Creates the result of removing one local exposure.
    pub const fn service_unexposed(value: ServiceExposureView) -> Self {
        Self(ResultKind::ServiceUnexposed(value), None)
    }
    /// Creates the list of every local exposure.
    pub const fn service_exposures(value: Vec<ServiceExposureView>) -> Self {
        Self(ResultKind::ServiceExposures(value), None)
    }
    /// Creates the streaming operation status result.
    pub const fn operations(value: OperationsView) -> Self {
        Self(ResultKind::Operations(value), None)
    }
}

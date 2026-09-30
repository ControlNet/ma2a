# `RuntimeHandle` must not own Endpoint-holding values

Symptom: four deterministic `Address already in use` failures when a test restarted a Runtime on
the same state directory (fixed port) after Phase 2 added outbound services.

Cause: `RuntimeHandle` stored an `ma2a_net::ServiceClient`, which clones the iroh Endpoint. Handles
are cheap clones held by IPC servers, Web routes and tests, and they can outlive `Runtime::shutdown`.
The surviving clone kept the old UDP sockets bound, so the restarted Runtime could not bind.

Rule: a handle carries only channels and plain metrics. Anything that owns the Endpoint is lent per
operation by the actor: `Command::ServiceClient(oneshot::Sender<ServiceClient>)`, answered with
`self.endpoint.service_client()`, and requested via `RuntimeHandle::service_client().await`. When
the actor is gone the request fails and the caller maps it to `ServiceError::Unavailable`.

When a restart test fails with address-in-use, look for any new field on `RuntimeHandle` (or on
values cached by long-lived servers) that clones the Endpoint.

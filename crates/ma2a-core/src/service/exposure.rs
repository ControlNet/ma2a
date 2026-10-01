//! Per-Space local target consent: what this Runtime serves in one Space.

use crate::{ProtocolError, SpaceId};

use super::{AgentId, ProxyHost, RootName, ServiceName};

/// Whether an exported root accepts mutation.
#[expect(
    clippy::exhaustive_enums,
    reason = "root access is a closed persisted vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RootAccess {
    /// Stat, list, read, and download only.
    ReadOnly,
    /// Every Files and upload operation.
    ReadWrite,
}

impl RootAccess {
    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
        }
    }

    /// Parses the stable lowercase name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "read_only" => Some(Self::ReadOnly),
            "read_write" => Some(Self::ReadWrite),
            _ => None,
        }
    }
}

/// One exported root as authorization sees it; the local directory never leaves the Store.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExportedRoot {
    name: RootName,
    access: RootAccess,
}

impl ExportedRoot {
    /// Creates an exported-root authorization fact.
    pub const fn new(name: RootName, access: RootAccess) -> Self {
        Self { name, access }
    }

    /// Returns the exported root name.
    pub const fn name(&self) -> &RootName {
        &self.name
    }

    /// Returns the exported access mode.
    pub const fn access(&self) -> RootAccess {
        self.access
    }
}

/// Destination host constraint of one proxy rule.
#[expect(
    clippy::exhaustive_enums,
    reason = "proxy host patterns are a closed persisted vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProxyHostPattern {
    /// Any destination host, an explicit operator opt-in.
    Any,
    /// Exactly this canonical host, matched as requested rather than as resolved.
    Exact(ProxyHost),
}

/// One allowed proxy destination: a host pattern and an inclusive port range.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProxyRule {
    host: ProxyHostPattern,
    port_start: u16,
    port_end: u16,
}

impl ProxyRule {
    /// Validates an inclusive, non-zero port range.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for port zero or a reversed range.
    pub fn new(host: ProxyHostPattern, ports: (u16, u16)) -> Result<Self, ProtocolError> {
        let (port_start, port_end) = ports;
        if port_start == 0 || port_start > port_end {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            host,
            port_start,
            port_end,
        })
    }

    /// Returns the host pattern.
    pub const fn host(&self) -> &ProxyHostPattern {
        &self.host
    }

    /// Returns the inclusive port range.
    pub const fn ports(&self) -> (u16, u16) {
        (self.port_start, self.port_end)
    }

    /// Returns whether this rule allows the exact requested destination.
    pub fn allows(&self, host: &ProxyHost, port: u16) -> bool {
        let host_matches = match &self.host {
            ProxyHostPattern::Any => true,
            ProxyHostPattern::Exact(allowed) => allowed == host,
        };
        host_matches && (self.port_start..=self.port_end).contains(&port)
    }
}

/// Everything this Runtime serves inside exactly one Space.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpaceExposureView {
    space_id: Option<SpaceId>,
    roots: Vec<ExportedRoot>,
    exec: bool,
    shell: bool,
    proxy_rules: Vec<ProxyRule>,
    process: bool,
    agents: Vec<AgentId>,
    services: Vec<ServiceName>,
}

impl SpaceExposureView {
    /// Creates an empty exposure for one Space.
    pub fn new(space_id: SpaceId) -> Self {
        Self {
            space_id: Some(space_id),
            ..Self::default()
        }
    }

    /// Adds an exported root.
    #[must_use]
    pub fn with_root(mut self, root: ExportedRoot) -> Self {
        self.roots.push(root);
        self
    }

    /// Enables Exec in this Space.
    #[must_use]
    pub const fn with_exec(mut self) -> Self {
        self.exec = true;
        self
    }

    /// Enables Shell in this Space.
    #[must_use]
    pub const fn with_shell(mut self) -> Self {
        self.shell = true;
        self
    }

    /// Enables durable Processes in this Space.
    #[must_use]
    pub const fn with_process(mut self) -> Self {
        self.process = true;
        self
    }

    /// Adds an allowed proxy destination rule.
    #[must_use]
    pub fn with_proxy_rule(mut self, rule: ProxyRule) -> Self {
        self.proxy_rules.push(rule);
        self
    }

    /// Exposes one Agent in this Space.
    #[must_use]
    pub fn with_agent(mut self, agent: AgentId) -> Self {
        if !self.agents.contains(&agent) {
            self.agents.push(agent);
        }
        self
    }

    /// Exposes one hosted service in this Space.
    #[must_use]
    pub fn with_service(mut self, service: ServiceName) -> Self {
        if !self.services.contains(&service) {
            self.services.push(service);
        }
        self
    }

    /// Returns the Space this exposure belongs to.
    pub const fn space_id(&self) -> Option<SpaceId> {
        self.space_id
    }

    /// Returns exported roots in this Space.
    pub fn roots(&self) -> &[ExportedRoot] {
        &self.roots
    }

    /// Returns whether Exec is exposed in this Space.
    pub const fn exec(&self) -> bool {
        self.exec
    }

    /// Returns whether Shell is exposed in this Space.
    pub const fn shell(&self) -> bool {
        self.shell
    }

    /// Returns whether durable Processes are exposed in this Space.
    pub const fn process(&self) -> bool {
        self.process
    }

    /// Returns allowed proxy destination rules in this Space.
    pub fn proxy_rules(&self) -> &[ProxyRule] {
        &self.proxy_rules
    }

    /// Returns the Agents exposed in this Space.
    pub fn agents(&self) -> &[AgentId] {
        &self.agents
    }

    /// Returns the hosted services exposed in this Space.
    pub fn services(&self) -> &[ServiceName] {
        &self.services
    }

    pub(crate) fn root(&self, name: &RootName) -> Option<&ExportedRoot> {
        self.roots.iter().find(|root| root.name() == name)
    }
}

//! What the local operator does with Agents: define, undefine, start, stop, list.

use ma2a_agent::tools::ToolFamily;
use ma2a_core::service::{
    AgentCard, AgentId, MAX_LOG_LIMIT, ProcessReason, ProcessSpec, ProcessStart, ServiceError,
};
use ma2a_store::{AgentDefine, AgentDefinition, AgentRecord};

use super::AgentManager;

/// Grace period of an instance whose Agent the operator stopped.
pub(super) const STOP_GRACE_MS: u32 = 5_000;

/// One Agent definition as the operator writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentSpec {
    /// The Agent's name.
    pub(crate) id: AgentId,
    /// What it does, for discovery; at most 256 bytes.
    pub(crate) description: String,
    /// Program and arguments of every instance.
    pub(crate) command: (String, Vec<String>),
    /// Absolute working directory; the Agent's workspace when absent.
    pub(crate) cwd: Option<String>,
    /// Extra environment of every instance.
    pub(crate) env: Vec<(String, String)>,
    /// Allowed tool families as a bit set.
    pub(crate) tools: u8,
    /// Most tasks one instance works on at once: 1–16.
    pub(crate) max_tasks: u8,
}

impl AgentSpec {
    /// Validates the spec into its durable definition.
    fn definition(self) -> Result<AgentDefinition, ServiceError> {
        let invalid = |_| ServiceError::InvalidInput;
        if reserved_name(self.id.as_str()) {
            return Err(ServiceError::InvalidInput);
        }
        AgentCard::new(self.id.clone(), &self.description, (true, self.max_tasks))
            .map_err(invalid)?;
        let all = ToolFamily::ALL
            .iter()
            .fold(0, |bits, family| bits | family.bit());
        let relative = |cwd: &String| !std::path::Path::new(cwd).is_absolute();
        if self.tools & !all != 0 || self.cwd.as_ref().is_some_and(relative) {
            return Err(ServiceError::InvalidInput);
        }
        let (program, args) = self.command;
        let mut spec = ProcessSpec::new(Some(program), args, self.cwd).map_err(invalid)?;
        for (name, value) in self.env {
            if name.starts_with("MA2A_AGENT_") {
                return Err(ServiceError::InvalidInput);
            }
            spec = spec.with_env(name, value).map_err(invalid)?;
        }
        let start = ProcessStart::new(spec, true, MAX_LOG_LIMIT)
            .and_then(|start| start.to_bytes())
            .map_err(invalid)?;
        Ok(AgentDefinition {
            id: self.id,
            description: self.description,
            start,
            tools: self.tools,
            max_tasks: self.max_tasks,
        })
    }
}

/// Whether an Agent name cannot name its own directory: `blobs` is the artifact store beside
/// the Agent directories, and a Windows device name opens a device there. Device names are
/// refused on every host, so a definition means the same wherever it is copied.
pub(super) fn reserved_name(name: &str) -> bool {
    name == "blobs" || ma2a_services::files::is_windows_reserved(name)
}

impl AgentManager {
    /// Stores a new definition, or replaces a stopped Agent's; `true` when it is new, with
    /// the revision of the commit.
    pub(crate) async fn define(
        &self,
        spec: AgentSpec,
    ) -> Result<((AgentRecord, bool), u64), ServiceError> {
        let definition = spec.definition()?;
        let at_ms = self.now_ms();
        let (defined, revision) = self
            .commit_at(move |repository| repository.define_agent(&definition, at_ms))
            .await?;
        match defined {
            AgentDefine::Defined(record, new) => Ok(((*record, new), revision)),
            AgentDefine::Active => Err(ServiceError::Conflict),
            AgentDefine::Missing => Err(ServiceError::UnknownAgent),
        }
    }

    /// Removes a stopped Agent's definition; the revision of the commit.
    pub(crate) async fn undefine(&self, agent: &AgentId) -> Result<u64, ServiceError> {
        let id = agent.clone();
        let (undefined, revision) = self
            .commit_at(move |repository| repository.undefine_agent(&id))
            .await?;
        match undefined {
            AgentDefine::Defined(..) => Ok(revision),
            AgentDefine::Active => Err(ServiceError::Conflict),
            AgentDefine::Missing => Err(ServiceError::UnknownAgent),
        }
    }

    /// Starts an Agent: its first instance starts at once. A reserved name, which only a
    /// definition stored before such names were refused can carry, is never started.
    pub(crate) async fn start(&self, agent: &AgentId) -> Result<AgentRecord, ServiceError> {
        if reserved_name(agent.as_str()) {
            return Err(ServiceError::InvalidInput);
        }
        let (id, at_ms) = (agent.clone(), self.now_ms());
        let record = self
            .commit(move |repository| repository.set_agent_enabled(&id, (true, at_ms)))
            .await?
            .ok_or(ServiceError::UnknownAgent)?;
        self.coordination().restarts.remove(agent);
        self.ensure_instance(agent).await;
        Ok(self.agent(agent).await?.unwrap_or(record))
    }

    /// Stops an Agent: its unfinished tasks fail and its instance is terminated.
    pub(crate) async fn stop(&self, agent: &AgentId) -> Result<AgentRecord, ServiceError> {
        let (id, at_ms) = (agent.clone(), self.now_ms());
        let record = self
            .commit(move |repository| repository.set_agent_enabled(&id, (false, at_ms)))
            .await?
            .ok_or(ServiceError::UnknownAgent)?;
        // The stop ended the instance; its Process still runs until terminated here.
        if let Some(process) = record.instance.and_then(|instance| instance.process_id) {
            let reason = (STOP_GRACE_MS, ProcessReason::Requested);
            let _ended = self.0.processes.terminate((None, process), reason).await;
        }
        Ok(self.agent(agent).await?.unwrap_or(record))
    }

    /// Returns every Agent with its number of unfinished tasks, and the read's revision.
    pub(crate) async fn list(&self) -> Result<(u64, Vec<(AgentRecord, usize)>), ServiceError> {
        let (agents, live) = self
            .store(|repository| {
                let agents = repository.agents()?;
                Ok((agents, repository.live_agent_tasks()?))
            })
            .await?;
        let revision = agents.revision();
        let listed = agents
            .into_value()
            .into_iter()
            .map(|record| {
                let id = &record.definition.id;
                let count = live.iter().filter(|task| task.summary.agent == *id).count();
                (record, count)
            })
            .collect();
        Ok((revision, listed))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ma2a_core::service::{AgentId, MAX_LOG_LIMIT, ProcessSpec, ProcessStart, ServiceError};
    use ma2a_store::{AgentDefinition, Repository, StoreConfig};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::AgentManager;
    use crate::{
        clock::SystemClock,
        processes::ProcessManager,
        store::{STORE_CAPACITY, StoreBackend, StoreClient},
    };

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[tokio::test]
    async fn a_reserved_agent_defined_before_the_refusal_is_never_started() -> TestResult {
        // Given: a definition stored directly, as a Runtime that did not refuse the name did.
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let dir = std::env::temp_dir().join(format!("ma2a-reserved-{}", u64::from_le_bytes(nonce)));
        std::fs::create_dir(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&dir, "test state directory")?;
        let config = StoreConfig::new(&dir);
        let backend = StoreBackend::open(&config)?;
        let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
        let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
        let store = StoreClient::new(sender);
        let local = store.initialize().await?.endpoint_id;
        let id = AgentId::new("blobs")?;
        let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
        let definition = AgentDefinition {
            id: id.clone(),
            description: String::new(),
            start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
            tools: 0,
            max_tasks: 1,
        };
        Repository::open(&config)?.define_agent(&definition, 1)?;
        let shutdown = CancellationToken::new();
        let processes = ProcessManager::new(
            (local, store.clone(), Arc::new(SystemClock)),
            (&dir, None),
            shutdown.clone(),
        );
        let manager = AgentManager::new(
            (local, store.clone(), Arc::new(SystemClock)),
            (&dir, processes),
            shutdown,
        );

        // When
        let started = manager.start(&id).await;

        // Then
        let enabled = manager.agent(&id).await?.map(|record| record.enabled);
        drop(manager);
        store.stop().await?;
        store_task.await?;
        let _cleanup = std::fs::remove_dir_all(&dir);
        assert_eq!(started.err(), Some(ServiceError::InvalidInput));
        assert_eq!(enabled, Some(false));
        Ok(())
    }

    #[test]
    fn names_the_runtime_or_windows_keeps_are_reserved() {
        for name in ["blobs", "con", "nul", "aux", "prn", "com1", "lpt9"] {
            assert!(super::reserved_name(name), "{name}");
        }
        for name in ["lead", "blob", "console", "com", "com10", "nul-agent"] {
            assert!(!super::reserved_name(name), "{name}");
        }
    }
}

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
        // Every instance adds the adapter environment; a definition no instance could start
        // with is refused here instead of failing every start.
        let workspace = self.workspace(&definition.id);
        super::instance::instance_start(&definition, &workspace.to_string_lossy())?;
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
    ///
    /// Answers the Agent and its number of unfinished tasks as the start committed them, with
    /// that commit's revision.
    pub(crate) async fn start(
        &self,
        agent: &AgentId,
    ) -> Result<((AgentRecord, usize), u64), ServiceError> {
        if reserved_name(agent.as_str()) {
            return Err(ServiceError::InvalidInput);
        }
        let (id, at_ms) = (agent.clone(), self.now_ms());
        let (record, revision) = self
            .commit_at(move |repository| repository.set_agent_enabled(&id, (true, at_ms)))
            .await?;
        let record = record.ok_or(ServiceError::UnknownAgent)?;
        self.coordination().restarts.remove(agent);
        self.ensure_instance(agent).await;
        Ok((record, revision))
    }

    /// Stops an Agent: its unfinished tasks fail and its instance is terminated.
    ///
    /// Answers the Agent and its number of unfinished tasks as the stop committed them, with
    /// that commit's revision.
    pub(crate) async fn stop(
        &self,
        agent: &AgentId,
    ) -> Result<((AgentRecord, usize), u64), ServiceError> {
        let (id, at_ms) = (agent.clone(), self.now_ms());
        let (record, revision) = self
            .commit_at(move |repository| repository.set_agent_enabled(&id, (false, at_ms)))
            .await?;
        let record = record.ok_or(ServiceError::UnknownAgent)?;
        // The stop ended the instance; its Process still runs until terminated here.
        if let Some(process) = record.0.instance.and_then(|instance| instance.process_id) {
            let reason = (STOP_GRACE_MS, ProcessReason::Requested);
            let _ended = self.0.processes.terminate((None, process), reason).await;
        }
        Ok((record, revision))
    }

    /// Returns every Agent with its number of unfinished tasks, and the read's revision.
    pub(crate) async fn list(&self) -> Result<(u64, Vec<(AgentRecord, usize)>), ServiceError> {
        let agents = self.store(|repository| repository.agents()).await?;
        Ok((agents.revision(), agents.into_value()))
    }
}

#[cfg(test)]
#[path = "tests/operator.rs"]
mod tests;

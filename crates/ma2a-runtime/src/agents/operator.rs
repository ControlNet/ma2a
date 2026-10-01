//! What the local operator does with Agents: define, undefine, start, stop, list.

use ma2a_agent::tools::ToolFamily;
use ma2a_core::service::{
    AgentCard, AgentId, MAX_LOG_LIMIT, ProcessReason, ProcessSpec, ProcessStart, ServiceError,
};
use ma2a_store::{AgentDefine, AgentDefinition, AgentRecord};

use super::AgentManager;

/// Grace period of an instance whose Agent the operator stopped.
const STOP_GRACE_MS: u32 = 5_000;

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

impl AgentManager {
    /// Stores a new definition, or replaces a stopped Agent's; `true` when it is new.
    pub(crate) async fn define(
        &self,
        spec: AgentSpec,
    ) -> Result<(AgentRecord, bool), ServiceError> {
        let definition = spec.definition()?;
        let at_ms = self.now_ms();
        match self
            .commit(move |repository| repository.define_agent(&definition, at_ms))
            .await?
        {
            AgentDefine::Defined(record, new) => Ok((*record, new)),
            AgentDefine::Active => Err(ServiceError::Conflict),
            AgentDefine::Missing => Err(ServiceError::UnknownAgent),
        }
    }

    /// Removes a stopped Agent's definition.
    pub(crate) async fn undefine(&self, agent: &AgentId) -> Result<(), ServiceError> {
        let id = agent.clone();
        match self
            .commit(move |repository| repository.undefine_agent(&id))
            .await?
        {
            AgentDefine::Defined(..) => Ok(()),
            AgentDefine::Active => Err(ServiceError::Conflict),
            AgentDefine::Missing => Err(ServiceError::UnknownAgent),
        }
    }

    /// Starts an Agent: its first instance starts at once.
    pub(crate) async fn start(&self, agent: &AgentId) -> Result<AgentRecord, ServiceError> {
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
        if let Some(process) = record
            .instance
            .filter(ma2a_store::AgentInstanceRecord::is_live)
            .and_then(|instance| instance.process_id)
        {
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

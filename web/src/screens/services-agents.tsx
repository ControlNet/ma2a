import type { ReactNode } from "react"

import type { HostedAgent, HostedAgentTask } from "../api/services"
import { shortId } from "./derive"

/** Whether an Agent runs, and which instance generation answers. */
export function agentStatus(agent: HostedAgent): string {
  if (!agent.enabled) return "stopped"
  if (agent.instance === null || agent.instance.ended_at_ms !== null) return "restarting"
  const readiness = agent.instance.ready ? "ready" : "starting"
  return `${readiness} (generation ${agent.instance.generation})`
}

/** Defined Agents: what each is for and does; programs and environment stay in the CLI. */
export function AgentTable({ agents }: { readonly agents: readonly HostedAgent[] }): ReactNode {
  return (
    <table className="ops-table">
      <caption className="visually-hidden">Agents this Runtime hosts</caption>
      <thead>
        <tr>
          <th scope="col">Agent</th>
          <th scope="col">Description</th>
          <th scope="col">State</th>
          <th scope="col">Tools</th>
          <th scope="col">Tasks</th>
        </tr>
      </thead>
      <tbody>
        {agents.map((agent) => (
          <tr key={agent.agent}>
            <th scope="row">{agent.agent}</th>
            <td>{agent.description}</td>
            <td>{agentStatus(agent)}</td>
            <td>{agent.tools.length === 0 ? "none" : agent.tools.join(", ")}</td>
            <td>{`${agent.live_tasks} of ${agent.max_tasks}`}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/** Tasks by creator; `from_agent` is attribution only. Messages are read through the CLI. */
export function AgentTaskTable({
  tasks,
}: {
  readonly tasks: readonly HostedAgentTask[]
}): ReactNode {
  return (
    <table className="ops-table">
      <caption className="visually-hidden">Agent tasks, by creator</caption>
      <thead>
        <tr>
          <th scope="col">Task</th>
          <th scope="col">Agent</th>
          <th scope="col">Creator</th>
          <th scope="col">State</th>
        </tr>
      </thead>
      <tbody>
        {tasks.map((task) => (
          <tr key={task.task_id}>
            <th className="ops-table__id" scope="row">
              {shortId(task.task_id)}
            </th>
            <td>{task.agent}</td>
            <td className="ops-table__id">
              {shortId(task.creator_endpoint_id)}
              {task.from_agent === null ? "" : ` (from ${task.from_agent})`}
            </td>
            <td>
              {task.reason === null
                ? task.state
                : `${task.state} (${task.reason.replaceAll("_", " ")})`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

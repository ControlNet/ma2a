INSERT INTO schema_migrations(version, name) VALUES (9, 'agents');

-- Kind 5 is consent for one Space's members to use one Agent, named by the selector.
-- SQLite cannot widen a CHECK in place, so the table is rebuilt with identical rows.
CREATE TABLE service_exposures_v9 (
    space_id BLOB NOT NULL CHECK (length(space_id) = 32),
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 5),
    exposure_selector TEXT NOT NULL CHECK (length(exposure_selector) <= 300),
    root_name TEXT REFERENCES file_roots(name),
    root_access INTEGER CHECK (root_access IS NULL OR root_access IN (0, 1)),
    proxy_host TEXT CHECK (proxy_host IS NULL OR length(proxy_host) BETWEEN 1 AND 253),
    port_start INTEGER CHECK (port_start IS NULL OR port_start BETWEEN 1 AND 65535),
    port_end INTEGER CHECK (port_end IS NULL OR port_end BETWEEN 1 AND 65535),
    PRIMARY KEY (space_id, kind, exposure_selector),
    CHECK ((kind = 0) = (root_name IS NOT NULL AND root_access IS NOT NULL)),
    CHECK ((kind = 3) = (proxy_host IS NOT NULL AND port_start IS NOT NULL AND port_end IS NOT NULL)),
    CHECK (kind != 5 OR length(exposure_selector) BETWEEN 1 AND 63),
    CHECK (port_start IS NULL OR port_start <= port_end)
) STRICT, WITHOUT ROWID;
INSERT INTO service_exposures_v9
    SELECT space_id, kind, exposure_selector, root_name, root_access, proxy_host, port_start, port_end
    FROM service_exposures;
DROP TABLE service_exposures;
ALTER TABLE service_exposures_v9 RENAME TO service_exposures;

-- Operator-defined Agents. `start` is the canonical Process start of every instance.
-- `generation` counts the instances ever started, so each instance has its own number.
CREATE TABLE agents (
    agent_id TEXT PRIMARY KEY CHECK (length(agent_id) BETWEEN 1 AND 63),
    description TEXT NOT NULL CHECK (length(description) <= 256),
    start BLOB NOT NULL CHECK (length(start) BETWEEN 1 AND 16384),
    tools INTEGER NOT NULL CHECK (tools BETWEEN 0 AND 63),
    max_tasks INTEGER NOT NULL CHECK (max_tasks BETWEEN 1 AND 16),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

-- The current instance of each Agent: its Process, how far its output was applied, and
-- which deliveries it acknowledged. A row whose `ended_at_ms` is set is a concluded instance.
CREATE TABLE agent_instances (
    agent_id TEXT PRIMARY KEY REFERENCES agents(agent_id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (generation >= 1),
    process_id BLOB CHECK (process_id IS NULL OR length(process_id) = 16),
    ready INTEGER NOT NULL CHECK (ready IN (0, 1)),
    output_cursor INTEGER NOT NULL CHECK (output_cursor >= 0),
    next_seq INTEGER NOT NULL CHECK (next_seq >= 1),
    acked_seq INTEGER NOT NULL CHECK (acked_seq >= 0 AND acked_seq < next_seq),
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    ended_at_ms INTEGER CHECK (ended_at_ms IS NULL OR ended_at_ms >= started_at_ms)
) STRICT, WITHOUT ROWID;

-- Deliveries to one instance not yet acknowledged. Each names durable rows it is rendered
-- from: kind 0 assigns a task, 1 delivers the message event `ref_seq`, 2 cancels, 3 is the
-- result of tool call `call_id`.
CREATE TABLE agent_outbox (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (generation >= 1),
    seq INTEGER NOT NULL CHECK (seq >= 1),
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 3),
    task_id BLOB NOT NULL CHECK (length(task_id) = 16),
    ref_seq INTEGER CHECK (ref_seq IS NULL OR ref_seq >= 1),
    call_id TEXT CHECK (call_id IS NULL OR length(call_id) BETWEEN 1 AND 64),
    PRIMARY KEY (agent_id, generation, seq),
    CHECK ((kind = 1) = (ref_seq IS NOT NULL)),
    CHECK ((kind = 3) = (call_id IS NOT NULL))
) STRICT, WITHOUT ROWID;

-- Agent tasks. The (creator, request_id) key binds one submission to exactly one task for as
-- long as the row exists; `fingerprint` covers the Agent and the canonical submission.
CREATE TABLE agent_tasks (
    task_id BLOB PRIMARY KEY CHECK (length(task_id) = 16),
    agent_id TEXT NOT NULL CHECK (length(agent_id) BETWEEN 1 AND 63),
    creator_endpoint_id BLOB NOT NULL CHECK (length(creator_endpoint_id) = 32),
    request_id BLOB NOT NULL CHECK (length(request_id) = 16),
    fingerprint BLOB NOT NULL CHECK (length(fingerprint) = 32),
    from_agent TEXT CHECK (from_agent IS NULL OR length(from_agent) BETWEEN 1 AND 63),
    state INTEGER NOT NULL CHECK (state BETWEEN 0 AND 5),
    reason INTEGER CHECK (reason IS NULL OR reason BETWEEN 0 AND 5),
    generation INTEGER CHECK (generation IS NULL OR generation >= 1),
    input BLOB NOT NULL CHECK (length(input) BETWEEN 1 AND 16384),
    result BLOB CHECK (result IS NULL OR length(result) BETWEEN 1 AND 16384),
    error TEXT CHECK (error IS NULL OR length(error) <= 1024),
    progress TEXT CHECK (progress IS NULL OR length(progress) <= 1024),
    cancel_reason INTEGER CHECK (cancel_reason IS NULL OR cancel_reason IN (0, 1)),
    cancel_requested_at_ms INTEGER CHECK (cancel_requested_at_ms IS NULL OR cancel_requested_at_ms >= 0),
    last_seq INTEGER NOT NULL CHECK (last_seq >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    started_at_ms INTEGER CHECK (started_at_ms IS NULL OR started_at_ms >= 0),
    ended_at_ms INTEGER CHECK (ended_at_ms IS NULL OR ended_at_ms >= 0),
    UNIQUE (creator_endpoint_id, request_id),
    CHECK ((state BETWEEN 3 AND 5) = (ended_at_ms IS NOT NULL)),
    CHECK (state NOT IN (1, 2) OR generation IS NOT NULL),
    CHECK ((cancel_reason IS NULL) = (cancel_requested_at_ms IS NULL))
) STRICT;
CREATE INDEX agent_tasks_by_agent ON agent_tasks(agent_id, state, created_at_ms);
CREATE INDEX agent_tasks_by_creator ON agent_tasks(creator_endpoint_id, task_id);
CREATE INDEX agent_tasks_by_end ON agent_tasks(state, ended_at_ms);

-- The durable event log of each task. Progress events are pruned to the newest few.
CREATE TABLE agent_task_events (
    task_id BLOB NOT NULL REFERENCES agent_tasks(task_id) ON DELETE CASCADE,
    seq INTEGER NOT NULL CHECK (seq >= 1),
    progress INTEGER NOT NULL CHECK (progress IN (0, 1)),
    event BLOB NOT NULL CHECK (length(event) BETWEEN 1 AND 65536),
    PRIMARY KEY (task_id, seq)
) STRICT, WITHOUT ROWID;

-- Artifacts each task published; content lives once per hash in the blob directory.
CREATE TABLE agent_artifacts (
    task_id BLOB NOT NULL REFERENCES agent_tasks(task_id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 15),
    media_type TEXT NOT NULL CHECK (length(media_type) BETWEEN 3 AND 127),
    size INTEGER NOT NULL CHECK (size >= 0),
    blake3 BLOB NOT NULL CHECK (length(blake3) = 32),
    PRIMARY KEY (task_id, name),
    UNIQUE (task_id, position)
) STRICT, WITHOUT ROWID;
CREATE INDEX agent_artifacts_by_hash ON agent_artifacts(blake3);

-- Tool calls an Agent made for a task: recorded before they run, completed exactly once.
CREATE TABLE agent_tool_calls (
    task_id BLOB NOT NULL REFERENCES agent_tasks(task_id) ON DELETE CASCADE,
    call_id TEXT NOT NULL CHECK (length(call_id) BETWEEN 1 AND 64),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    request BLOB NOT NULL CHECK (length(request) BETWEEN 1 AND 16384),
    result BLOB CHECK (result IS NULL OR length(result) BETWEEN 1 AND 65536),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    completed_at_ms INTEGER CHECK (completed_at_ms IS NULL OR completed_at_ms >= created_at_ms),
    PRIMARY KEY (task_id, call_id),
    CHECK ((result IS NULL) = (completed_at_ms IS NULL))
) STRICT, WITHOUT ROWID;

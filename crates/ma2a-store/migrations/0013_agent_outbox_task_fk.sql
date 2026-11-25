INSERT INTO schema_migrations(version, name) VALUES (13, 'agent_outbox_task_fk');

-- Outbox deliveries reference their task so evicting or expiring a finished task retires
-- its unacknowledged deliveries in the same transaction instead of wedging the instance.
CREATE TABLE agent_outbox_v13 (
    agent_id TEXT NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (generation >= 1),
    seq INTEGER NOT NULL CHECK (seq >= 1),
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 3),
    task_id BLOB NOT NULL REFERENCES agent_tasks(task_id) ON DELETE CASCADE
        CHECK (length(task_id) = 16),
    ref_seq INTEGER CHECK (ref_seq IS NULL OR ref_seq >= 1),
    call_id TEXT CHECK (call_id IS NULL OR length(call_id) BETWEEN 1 AND 64),
    PRIMARY KEY (agent_id, generation, seq),
    CHECK ((kind = 1) = (ref_seq IS NOT NULL)),
    CHECK ((kind = 3) = (call_id IS NOT NULL))
) STRICT, WITHOUT ROWID;
INSERT INTO agent_outbox_v13
    SELECT o.agent_id, o.generation, o.seq, o.kind, o.task_id, o.ref_seq, o.call_id
    FROM agent_outbox o
    WHERE EXISTS (SELECT 1 FROM agent_tasks t WHERE t.task_id = o.task_id);
DROP TABLE agent_outbox;
ALTER TABLE agent_outbox_v13 RENAME TO agent_outbox;
CREATE INDEX agent_outbox_by_task ON agent_outbox(task_id);

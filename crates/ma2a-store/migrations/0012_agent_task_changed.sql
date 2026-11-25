INSERT INTO schema_migrations(version, name) VALUES (12, 'agent_task_changed');

-- When each task's state last changed, between running and waiting too, so a reader knows when
-- its current status took effect. A task stored before this takes its newest recorded time.
ALTER TABLE agent_tasks ADD COLUMN changed_at_ms INTEGER NOT NULL DEFAULT 0
    CHECK (changed_at_ms >= 0);
UPDATE agent_tasks SET changed_at_ms = COALESCE(ended_at_ms, started_at_ms, created_at_ms);

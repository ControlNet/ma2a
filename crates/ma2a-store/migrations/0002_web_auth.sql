INSERT INTO schema_migrations(version, name) VALUES (2, 'web_auth');

ALTER TABLE ui_credentials
    ADD COLUMN auth_epoch INTEGER NOT NULL DEFAULT 0 CHECK (auth_epoch >= 0);

UPDATE ui_credentials SET auth_epoch = 1;

DROP INDEX sessions_active_by_expiry;
ALTER TABLE sessions RENAME TO sessions_v1;

CREATE TABLE sessions (
    session_id_hash BLOB PRIMARY KEY CHECK (length(session_id_hash) = 32),
    csrf_token_hash BLOB NOT NULL CHECK (length(csrf_token_hash) = 32),
    auth_epoch INTEGER NOT NULL CHECK (auth_epoch > 0),
    created_at_ms INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL,
    idle_expires_at_ms INTEGER NOT NULL CHECK (idle_expires_at_ms > last_seen_at_ms),
    absolute_expires_at_ms INTEGER NOT NULL CHECK (absolute_expires_at_ms > created_at_ms),
    revoked_at_ms INTEGER,
    CHECK (last_seen_at_ms >= created_at_ms),
    CHECK (idle_expires_at_ms <= absolute_expires_at_ms)
) STRICT, WITHOUT ROWID;

CREATE INDEX sessions_active_by_expiry
    ON sessions(idle_expires_at_ms, absolute_expires_at_ms)
    WHERE revoked_at_ms IS NULL;

DROP TABLE sessions_v1;

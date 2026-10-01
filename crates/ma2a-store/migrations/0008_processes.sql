INSERT INTO schema_migrations(version, name) VALUES (8, 'processes');

-- Kind 4 is consent to host durable Processes for one Space's members. SQLite cannot widen
-- a CHECK in place, so the table is rebuilt with identical rows.
CREATE TABLE service_exposures_v8 (
    space_id BLOB NOT NULL CHECK (length(space_id) = 32),
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 4),
    exposure_selector TEXT NOT NULL CHECK (length(exposure_selector) <= 300),
    root_name TEXT REFERENCES file_roots(name),
    root_access INTEGER CHECK (root_access IS NULL OR root_access IN (0, 1)),
    proxy_host TEXT CHECK (proxy_host IS NULL OR length(proxy_host) BETWEEN 1 AND 253),
    port_start INTEGER CHECK (port_start IS NULL OR port_start BETWEEN 1 AND 65535),
    port_end INTEGER CHECK (port_end IS NULL OR port_end BETWEEN 1 AND 65535),
    PRIMARY KEY (space_id, kind, exposure_selector),
    CHECK ((kind = 0) = (root_name IS NOT NULL AND root_access IS NOT NULL)),
    CHECK ((kind = 3) = (proxy_host IS NOT NULL AND port_start IS NOT NULL AND port_end IS NOT NULL)),
    CHECK (port_start IS NULL OR port_start <= port_end)
) STRICT, WITHOUT ROWID;
INSERT INTO service_exposures_v8
    SELECT space_id, kind, exposure_selector, root_name, root_access, proxy_host, port_start, port_end
    FROM service_exposures;
DROP TABLE service_exposures;
ALTER TABLE service_exposures_v8 RENAME TO service_exposures;

-- Durable Processes. The (creator, request_id) key binds one start request to exactly one
-- Process for as long as its row or tombstone exists. `start` and `secret` are cleared when
-- the Process is archived or ends, respectively; everything else stays as the tombstone.
CREATE TABLE processes (
    process_id BLOB PRIMARY KEY CHECK (length(process_id) = 16),
    creator_endpoint_id BLOB NOT NULL CHECK (length(creator_endpoint_id) = 32),
    request_id BLOB NOT NULL CHECK (length(request_id) = 16),
    fingerprint BLOB NOT NULL CHECK (length(fingerprint) = 32),
    start BLOB CHECK (start IS NULL OR length(start) BETWEEN 1 AND 16384),
    name TEXT CHECK (name IS NULL OR length(name) BETWEEN 1 AND 64),
    state INTEGER NOT NULL CHECK (state BETWEEN 0 AND 6),
    reason INTEGER CHECK (reason IS NULL OR reason BETWEEN 0 AND 4),
    termination_kind INTEGER CHECK (termination_kind IS NULL OR termination_kind BETWEEN 0 AND 2),
    termination_value INTEGER CHECK (termination_value IS NULL OR termination_value BETWEEN 0 AND 4294967295),
    secret BLOB CHECK (secret IS NULL OR length(secret) = 32),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    started_at_ms INTEGER CHECK (started_at_ms IS NULL OR started_at_ms >= 0),
    ended_at_ms INTEGER CHECK (ended_at_ms IS NULL OR ended_at_ms >= 0),
    archived_at_ms INTEGER CHECK (archived_at_ms IS NULL OR archived_at_ms >= 0),
    UNIQUE (creator_endpoint_id, request_id),
    CHECK ((termination_kind IS NULL OR termination_kind = 2) = (termination_value IS NULL)),
    CHECK ((state BETWEEN 3 AND 6) = (ended_at_ms IS NOT NULL)),
    CHECK (state BETWEEN 3 AND 6 OR secret IS NOT NULL),
    CHECK (archived_at_ms IS NULL OR (state BETWEEN 3 AND 6 AND start IS NULL))
) STRICT;
CREATE INDEX processes_by_creator ON processes(creator_endpoint_id, process_id);
CREATE INDEX processes_by_state ON processes(state, ended_at_ms);

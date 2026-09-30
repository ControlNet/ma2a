INSERT INTO schema_migrations(version, name) VALUES (7, 'service_exposures');

-- One root name names exactly one local directory for this Runtime, whichever Space
-- authorized a request, so path resolution never depends on the authorizing Space.
CREATE TABLE file_roots (
    name TEXT PRIMARY KEY CHECK (length(name) BETWEEN 1 AND 64),
    path TEXT NOT NULL CHECK (length(path) BETWEEN 1 AND 4096)
) STRICT, WITHOUT ROWID;

-- Local target consent: what this Runtime serves inside one Space. Signed member grants
-- decide what a caller may ask for; only these rows make this Runtime serve anything.
CREATE TABLE service_exposures (
    space_id BLOB NOT NULL CHECK (length(space_id) = 32),
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 3),
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

-- Upload staging files recorded before they are created, so a crash between creation and
-- finalization can always be cleaned up at the next start.
CREATE TABLE transfer_staging (
    staging_path TEXT PRIMARY KEY CHECK (length(staging_path) BETWEEN 1 AND 8192),
    request_id BLOB NOT NULL CHECK (length(request_id) = 16),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT, WITHOUT ROWID;

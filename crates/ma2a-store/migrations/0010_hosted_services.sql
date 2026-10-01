INSERT INTO schema_migrations(version, name) VALUES (10, 'hosted_services');

-- Kind 6 is consent for one Space's members to call one hosted service, named by the
-- selector. SQLite cannot widen a CHECK in place, so the table is rebuilt with identical rows.
CREATE TABLE service_exposures_v10 (
    space_id BLOB NOT NULL CHECK (length(space_id) = 32),
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 6),
    exposure_selector TEXT NOT NULL CHECK (length(exposure_selector) <= 300),
    root_name TEXT REFERENCES file_roots(name),
    root_access INTEGER CHECK (root_access IS NULL OR root_access IN (0, 1)),
    proxy_host TEXT CHECK (proxy_host IS NULL OR length(proxy_host) BETWEEN 1 AND 253),
    port_start INTEGER CHECK (port_start IS NULL OR port_start BETWEEN 1 AND 65535),
    port_end INTEGER CHECK (port_end IS NULL OR port_end BETWEEN 1 AND 65535),
    PRIMARY KEY (space_id, kind, exposure_selector),
    CHECK ((kind = 0) = (root_name IS NOT NULL AND root_access IS NOT NULL)),
    CHECK ((kind = 3) = (proxy_host IS NOT NULL AND port_start IS NOT NULL AND port_end IS NOT NULL)),
    CHECK (kind NOT IN (5, 6) OR length(exposure_selector) BETWEEN 1 AND 63),
    CHECK (port_start IS NULL OR port_start <= port_end)
) STRICT, WITHOUT ROWID;
INSERT INTO service_exposures_v10
    SELECT space_id, kind, exposure_selector, root_name, root_access, proxy_host, port_start, port_end
    FROM service_exposures;
DROP TABLE service_exposures;
ALTER TABLE service_exposures_v10 RENAME TO service_exposures;

-- Operator-registered hosted services. `manifest` is the canonical manifest encoding; its
-- `max_calls` is repeated so the bound is checked without decoding. Links and calls are
-- never durable: a process that serves the service attaches again after any restart.
CREATE TABLE hosted_services (
    service TEXT PRIMARY KEY CHECK (length(service) BETWEEN 1 AND 63),
    manifest BLOB NOT NULL CHECK (length(manifest) BETWEEN 1 AND 4096),
    max_calls INTEGER NOT NULL CHECK (max_calls BETWEEN 1 AND 64),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

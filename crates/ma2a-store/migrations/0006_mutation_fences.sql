INSERT INTO schema_migrations(version, name) VALUES (6, 'mutation_fences');

-- Response eviction must never make an already admitted RequestId executable.
-- Only fingerprints are retained here, never command bodies or invite secrets.
CREATE TABLE local_mutation_fences (
    request_id BLOB PRIMARY KEY CHECK (length(request_id) = 16),
    fingerprint BLOB NOT NULL CHECK (length(fingerprint) = 32)
) STRICT, WITHOUT ROWID;

INSERT INTO local_mutation_fences SELECT request_id, fingerprint FROM local_mutation_replay;

CREATE TRIGGER reserve_mutation_fence BEFORE INSERT ON local_mutation_replay
BEGIN
    INSERT INTO local_mutation_fences(request_id, fingerprint)
    VALUES (NEW.request_id, NEW.fingerprint);
END;

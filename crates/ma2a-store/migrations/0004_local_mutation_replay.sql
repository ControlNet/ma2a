INSERT INTO schema_migrations(version, name) VALUES (4, 'local_mutation_replay');

CREATE TABLE local_mutation_replay (
    request_id BLOB PRIMARY KEY CHECK (length(request_id) = 16),
    fingerprint BLOB NOT NULL CHECK (length(fingerprint) = 32),
    revision INTEGER CHECK (revision IS NULL OR revision >= 0),
    response BLOB CHECK (response IS NULL OR (length(response) > 0 AND length(response) <= 65536)),
    sequence INTEGER NOT NULL UNIQUE CHECK (sequence > 0),
    CHECK ((revision IS NULL) = (response IS NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX local_mutation_replay_eviction
ON local_mutation_replay(sequence, request_id);

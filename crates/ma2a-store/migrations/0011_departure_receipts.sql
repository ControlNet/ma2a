INSERT INTO schema_migrations(version, name) VALUES (11, 'departure_receipts');

-- One row per departure request this Runtime, as a Space authority, answered with a removal:
-- the generation that removed the requester. A retry of the same request is answered with that
-- generation, even after the requester joined again, so an old request never removes it twice.
CREATE TABLE departure_receipts (
    space_id BLOB NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE
        CHECK (length(space_id) = 32),
    endpoint_id BLOB NOT NULL CHECK (length(endpoint_id) = 32),
    request_id BLOB NOT NULL CHECK (length(request_id) = 16),
    generation INTEGER NOT NULL CHECK (generation BETWEEN 1 AND 255),
    PRIMARY KEY (space_id, endpoint_id, request_id)
) STRICT, WITHOUT ROWID;

INSERT INTO schema_migrations(version, name) VALUES (14, 'retryable_mutation_fences');

ALTER TABLE local_mutation_fences
    ADD COLUMN retryable INTEGER NOT NULL DEFAULT 0 CHECK (retryable IN (0, 1));

DROP TRIGGER reserve_mutation_fence;

CREATE TRIGGER reserve_mutation_fence BEFORE INSERT ON local_mutation_replay
BEGIN
    SELECT CASE
        WHEN EXISTS (
            SELECT 1 FROM local_mutation_fences
            WHERE request_id = NEW.request_id
              AND (fingerprint != NEW.fingerprint OR retryable = 0)
        )
        THEN RAISE(ABORT, 'mutation fence already bound')
    END;
    INSERT OR IGNORE INTO local_mutation_fences(request_id, fingerprint, retryable)
    VALUES (NEW.request_id, NEW.fingerprint, 0);
END;

INSERT INTO schema_migrations(version, name) VALUES (5, 'relay_advertisement_activity');

ALTER TABLE relay_advertisement_state
ADD COLUMN active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1));

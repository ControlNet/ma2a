INSERT INTO schema_migrations(version, name) VALUES (3, 'relay_persistence');

ALTER TABLE relay_configuration ADD COLUMN private_relay_url TEXT;
ALTER TABLE relay_configuration ADD COLUMN certificate_path TEXT;
ALTER TABLE relay_configuration ADD COLUMN private_key_path TEXT;
ALTER TABLE relay_advertisement_state ADD COLUMN issued_at_ms INTEGER NOT NULL DEFAULT 0;

CREATE TABLE relay_public_fallback_urls (
    position INTEGER PRIMARY KEY CHECK (position >= 0),
    relay_url TEXT NOT NULL UNIQUE
) STRICT;

INSERT INTO relay_public_fallback_urls(position, relay_url)
SELECT 0, public_relay_url FROM relay_configuration WHERE public_relay_url IS NOT NULL;

CREATE TABLE relay_served_spaces (
    space_id BLOB PRIMARY KEY CHECK (length(space_id) = 32)
) STRICT, WITHOUT ROWID;

CREATE TABLE relay_publication_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    sequence INTEGER NOT NULL DEFAULT 0 CHECK (sequence >= 0)
) STRICT;

INSERT INTO relay_publication_state(singleton) VALUES (1);

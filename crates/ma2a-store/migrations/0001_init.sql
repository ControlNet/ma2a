CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL UNIQUE
) STRICT;

INSERT INTO schema_migrations(version, name) VALUES (1, 'initial');

CREATE TABLE runtime_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    boot_id BLOB CHECK (boot_id IS NULL OR length(boot_id) = 16),
    last_shutdown_clean INTEGER CHECK (last_shutdown_clean IN (0, 1)),
    last_shutdown_at_ms INTEGER,
    endpoint_observed_at_ms INTEGER,
    endpoint_ready INTEGER CHECK (endpoint_ready IN (0, 1)),
    direct_address_count INTEGER CHECK (direct_address_count >= 0),
    relay_address_count INTEGER CHECK (relay_address_count >= 0),
    membership_count INTEGER CHECK (membership_count >= 0),
    endpoint_bind_port INTEGER CHECK (endpoint_bind_port BETWEEN 1 AND 65535),
    CHECK (
        (endpoint_observed_at_ms IS NULL AND endpoint_ready IS NULL
            AND direct_address_count IS NULL AND relay_address_count IS NULL
            AND membership_count IS NULL)
        OR
        (endpoint_observed_at_ms IS NOT NULL AND endpoint_ready IS NOT NULL
            AND direct_address_count IS NOT NULL AND relay_address_count IS NOT NULL
            AND membership_count IS NOT NULL)
    )
) STRICT;

INSERT INTO runtime_metadata(singleton) VALUES (1);

CREATE TABLE endpoints (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    endpoint_id BLOB NOT NULL UNIQUE CHECK (length(endpoint_id) = 32),
    endpoint_key_ref TEXT NOT NULL UNIQUE CHECK (length(endpoint_key_ref) BETWEEN 1 AND 96)
) STRICT;

CREATE TABLE spaces (
    space_id BLOB PRIMARY KEY CHECK (length(space_id) = 32),
    genesis_cbor BLOB NOT NULL,
    authority_key_ref TEXT UNIQUE CHECK (
        authority_key_ref IS NULL OR length(authority_key_ref) BETWEEN 1 AND 96
    ),
    latest_manifest_generation INTEGER CHECK (
        latest_manifest_generation IS NULL OR latest_manifest_generation >= 0
    ),
    latest_manifest_hash BLOB CHECK (
        latest_manifest_hash IS NULL OR length(latest_manifest_hash) = 32
    ),
    CHECK ((latest_manifest_generation IS NULL) = (latest_manifest_hash IS NULL))
) STRICT, WITHOUT ROWID;

CREATE TABLE manifests (
    space_id BLOB NOT NULL,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    previous_hash BLOB CHECK (previous_hash IS NULL OR length(previous_hash) = 32),
    manifest_hash BLOB NOT NULL CHECK (length(manifest_hash) = 32),
    signed_manifest BLOB NOT NULL,
    PRIMARY KEY (space_id, generation),
    UNIQUE (space_id, generation, manifest_hash),
    UNIQUE (space_id, manifest_hash),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    CHECK ((generation = 0 AND previous_hash IS NULL) OR (generation > 0 AND previous_hash IS NOT NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX manifests_by_space_hash ON manifests(space_id, manifest_hash);

CREATE TABLE members (
    space_id BLOB NOT NULL,
    endpoint_id BLOB NOT NULL CHECK (length(endpoint_id) = 32),
    role INTEGER NOT NULL CHECK (role BETWEEN 0 AND 2),
    accepted_generation INTEGER NOT NULL CHECK (accepted_generation >= 0),
    PRIMARY KEY (space_id, endpoint_id),
    FOREIGN KEY (space_id, accepted_generation)
        REFERENCES manifests(space_id, generation) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE member_revocations (
    space_id BLOB NOT NULL,
    endpoint_id BLOB NOT NULL CHECK (length(endpoint_id) = 32),
    revoked_generation INTEGER NOT NULL CHECK (revoked_generation >= 0),
    signed_revocation BLOB NOT NULL,
    PRIMARY KEY (space_id, endpoint_id),
    FOREIGN KEY (space_id, revoked_generation)
        REFERENCES manifests(space_id, generation) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE invitations (
    invitation_id BLOB PRIMARY KEY CHECK (length(invitation_id) = 16),
    space_id BLOB NOT NULL,
    token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    creator_endpoint_id BLOB NOT NULL CHECK (length(creator_endpoint_id) = 32),
    created_at_ms INTEGER NOT NULL,
    status INTEGER NOT NULL DEFAULT 0 CHECK (status BETWEEN 0 AND 3),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms),
    owner_bootstrap BLOB NOT NULL CHECK (length(owner_bootstrap) BETWEEN 1 AND 1024),
    consumed_at_ms INTEGER,
    consumed_by_endpoint_id BLOB CHECK (
        consumed_by_endpoint_id IS NULL OR length(consumed_by_endpoint_id) = 32
    ),
    consumed_request_id BLOB CHECK (consumed_request_id IS NULL OR length(consumed_request_id) = 16),
    response_chain BLOB,
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    CHECK (
        (status = 0 AND consumed_at_ms IS NULL AND consumed_by_endpoint_id IS NULL
            AND consumed_request_id IS NULL AND response_chain IS NULL)
        OR (status = 1 AND consumed_at_ms IS NOT NULL AND consumed_by_endpoint_id IS NOT NULL
            AND ((consumed_request_id IS NULL AND response_chain IS NULL)
                OR (consumed_request_id IS NOT NULL AND response_chain IS NOT NULL)))
        OR (status IN (2, 3) AND consumed_at_ms IS NULL AND consumed_by_endpoint_id IS NULL
            AND consumed_request_id IS NULL AND response_chain IS NULL)
    )
) STRICT, WITHOUT ROWID;

CREATE INDEX invitations_pending_by_expiry ON invitations(expires_at_ms) WHERE status = 0;

CREATE TABLE consumed_invitation_tokens (
    token_hash BLOB PRIMARY KEY CHECK (length(token_hash) = 32),
    invitation_id BLOB NOT NULL UNIQUE,
    consumed_at_ms INTEGER NOT NULL,
    FOREIGN KEY (invitation_id) REFERENCES invitations(invitation_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE address_state (
    space_id BLOB NOT NULL,
    endpoint_id BLOB NOT NULL CHECK (length(endpoint_id) = 32),
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    issued_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > issued_at_ms),
    record_hash BLOB NOT NULL CHECK (length(record_hash) = 32),
    signed_record BLOB NOT NULL,
    PRIMARY KEY (space_id, endpoint_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX address_state_by_expiry ON address_state(expires_at_ms);

CREATE TABLE relay_advertisement_state (
    space_id BLOB NOT NULL,
    relay_endpoint_id BLOB NOT NULL CHECK (length(relay_endpoint_id) = 32),
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    expires_at_ms INTEGER NOT NULL,
    advertisement_hash BLOB NOT NULL CHECK (length(advertisement_hash) = 32),
    signed_advertisement BLOB NOT NULL,
    PRIMARY KEY (space_id, relay_endpoint_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX relay_advertisement_by_expiry ON relay_advertisement_state(expires_at_ms);

CREATE TABLE relay_configuration (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    public_fallback_enabled INTEGER NOT NULL DEFAULT 0 CHECK (public_fallback_enabled IN (0, 1)),
    public_relay_url TEXT,
    private_provider_enabled INTEGER NOT NULL DEFAULT 0 CHECK (private_provider_enabled IN (0, 1)),
    listener_address TEXT,
    tls_mode INTEGER CHECK (tls_mode IS NULL OR tls_mode BETWEEN 0 AND 1)
) STRICT;

INSERT INTO relay_configuration(singleton) VALUES (1);

CREATE TABLE relay_observations (
    relay_url TEXT PRIMARY KEY,
    observed_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > observed_at_ms),
    reachable INTEGER NOT NULL CHECK (reachable IN (0, 1)),
    latency_ms INTEGER CHECK (latency_ms IS NULL OR latency_ms >= 0),
    observed_state BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX relay_observations_by_expiry ON relay_observations(expires_at_ms);

CREATE TABLE ui_credentials (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    password_verifier BLOB NOT NULL,
    verifier_version INTEGER NOT NULL CHECK (verifier_version > 0),
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE sessions (
    session_id_hash BLOB PRIMARY KEY CHECK (length(session_id_hash) = 32),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms),
    revoked_at_ms INTEGER
) STRICT, WITHOUT ROWID;

CREATE INDEX sessions_active_by_expiry ON sessions(expires_at_ms) WHERE revoked_at_ms IS NULL;

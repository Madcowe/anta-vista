pub const CREATE_APPLIED_MIGRATIONS: &str = "
CREATE TABLE IF NOT EXISTS applied_migrations (
    name        TEXT PRIMARY KEY,
    applied_at  INTEGER NOT NULL
);
";

pub const MIGRATION_001_INITIAL: &str = "
CREATE TABLE IF NOT EXISTS resources (
    id                  TEXT PRIMARY KEY,
    kind                TEXT NOT NULL,
    location            TEXT NOT NULL,
    location_scheme     TEXT,
    location_canonical  TEXT,
    mime_type           TEXT NOT NULL,
    filename            TEXT,
    metadata_json       TEXT NOT NULL DEFAULT '{}',
    description_text    TEXT NOT NULL DEFAULT '',
    created_at          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_resources_location_scheme ON resources(location_scheme);

CREATE TABLE IF NOT EXISTS embedding_profiles (
    profile_id      TEXT PRIMARY KEY,
    model_id        TEXT NOT NULL,
    model_version   TEXT NOT NULL,
    dim             INTEGER NOT NULL,
    normalized      INTEGER NOT NULL DEFAULT 1,
    preproc_version TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS embeddings (
    resource_id TEXT NOT NULL,
    profile_id  TEXT NOT NULL,
    vector_json TEXT NOT NULL,
    l2_norm     REAL NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (resource_id, profile_id),
    FOREIGN KEY (resource_id) REFERENCES resources(id) ON DELETE CASCADE,
    FOREIGN KEY (profile_id)  REFERENCES embedding_profiles(profile_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_embeddings_resource_id ON embeddings(resource_id);

CREATE TABLE IF NOT EXISTS claims (
    claim_id        TEXT PRIMARY KEY,
    schema_version  INTEGER NOT NULL DEFAULT 1,
    subject         TEXT NOT NULL,
    predicate       TEXT NOT NULL,
    object          TEXT NOT NULL,
    by_agent_id     TEXT NOT NULL,
    timestamp       INTEGER NOT NULL,
    signature       BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_claims_subject      ON claims(subject);
CREATE INDEX IF NOT EXISTS idx_claims_by_agent_id  ON claims(by_agent_id);

CREATE TABLE IF NOT EXISTS feedback_events (
    feedback_id     TEXT PRIMARY KEY,
    schema_version  INTEGER NOT NULL DEFAULT 1,
    query_text      TEXT NOT NULL,
    resource_id     TEXT NOT NULL,
    by_agent_id     TEXT NOT NULL,
    kind            TEXT NOT NULL,
    timestamp       INTEGER NOT NULL,
    signature       BLOB NOT NULL,
    FOREIGN KEY (resource_id) REFERENCES resources(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_feedback_resource_id  ON feedback_events(resource_id);
CREATE INDEX IF NOT EXISTS idx_feedback_by_agent_id  ON feedback_events(by_agent_id);

CREATE TABLE IF NOT EXISTS trust_state (
    subject_agent_id TEXT PRIMARY KEY,
    trust_score      REAL NOT NULL DEFAULT 0.0,
    evidence_count   INTEGER NOT NULL DEFAULT 0,
    last_updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS name_records (
    record_id        TEXT PRIMARY KEY,
    schema_version   INTEGER NOT NULL DEFAULT 1,
    normalized_name  TEXT NOT NULL,
    original_name    TEXT NOT NULL,
    record_type      TEXT NOT NULL,
    target           TEXT NOT NULL,
    target_scheme    TEXT,
    target_canonical TEXT,
    ttl_secs         INTEGER NOT NULL DEFAULT 3600,
    by_agent_id      TEXT NOT NULL,
    timestamp        INTEGER NOT NULL,
    signature        BLOB NOT NULL,
    UNIQUE (normalized_name, record_type, target_canonical, by_agent_id)
);
CREATE INDEX IF NOT EXISTS idx_name_records_normalized_name ON name_records(normalized_name);
CREATE INDEX IF NOT EXISTS idx_name_records_target_scheme   ON name_records(target_scheme);
CREATE INDEX IF NOT EXISTS idx_name_records_by_agent_id     ON name_records(by_agent_id);

CREATE TABLE IF NOT EXISTS peer_cache (
    peer_id       TEXT PRIMARY KEY,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    last_seen_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS query_cache (
    query_id     TEXT PRIMARY KEY,
    query_text   TEXT NOT NULL,
    result_json  TEXT NOT NULL DEFAULT '{}',
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL
);
";

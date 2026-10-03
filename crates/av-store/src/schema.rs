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

pub const MIGRATION_002_RELEVANCE: &str = "
CREATE TABLE IF NOT EXISTS relevance_judgments (
    normalized_query TEXT NOT NULL,
    resource_id      TEXT NOT NULL,
    score            REAL NOT NULL DEFAULT 1.0,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    PRIMARY KEY (normalized_query, resource_id),
    FOREIGN KEY (resource_id) REFERENCES resources(id) ON DELETE CASCADE
);
";

pub const MIGRATION_003_NAME_RELEVANCE: &str = "
CREATE TABLE IF NOT EXISTS name_relevance_judgments (
    normalized_query TEXT NOT NULL,
    record_id        TEXT NOT NULL,
    score            REAL NOT NULL DEFAULT 1.0,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    PRIMARY KEY (normalized_query, record_id)
);
";

pub const MIGRATION_004_CANONICAL_SCHEME: &str = "
UPDATE resources
SET location_canonical = 'autonomi://' || substr(location_canonical, 6)
WHERE location_canonical LIKE 'ant://%';

UPDATE name_records
SET target_canonical = 'autonomi://' || substr(target_canonical, 6)
WHERE target_canonical LIKE 'ant://%';
";

/// Local operator policy: subjects this node refuses to surface or serve.
///
/// One table rather than three because the three kinds are independent
/// subjects (a resource can be blocked without naming who sent it, and an
/// agent can be blocked without condemning anything they sent), they are
/// always queried together when building a filter, and `av block --list` is
/// then a single read.
///
/// The composite primary key makes re-blocking idempotent, so `blocked_at`
/// records when the block was *first* imposed rather than being churned by a
/// repeat `av block`. There is no expiry: blocks are permanent until
/// explicitly unblocked, because a silently lapsing block would let a
/// known-bad resource reappear without warning.
///
/// These rows are deliberately NOT cleared by `av purge --all`, which deletes
/// cached data but not policy. Use `av unblock --all` to reset.
pub const MIGRATION_005_BLOCKS: &str = "
CREATE TABLE IF NOT EXISTS blocks (
    subject_kind TEXT NOT NULL,
    subject_id   TEXT NOT NULL,
    reason       TEXT,
    blocked_at   INTEGER NOT NULL,
    source       TEXT NOT NULL DEFAULT 'manual',
    PRIMARY KEY (subject_kind, subject_id)
);
CREATE INDEX IF NOT EXISTS idx_blocks_kind ON blocks(subject_kind);
";

/// The transport origin a cached name record arrived from.
///
/// `name_records.by_agent_id` is whatever the publishing peer put in its own
/// payload, so it is self-reported and trivially forged — the envelope says who
/// the peer claims to be, not who the daemon actually delivered the message
/// from. That is good enough to *display* a publisher, and useless for blocking,
/// where a peer would simply lie about its own id to walk around the block.
///
/// This column records the id the transport authenticated (`DirectMessage::sender`
/// / `IncomingEvent::origin`) and is written only by the cache path, which is
/// the one place that has the real origin in hand. It is never read from the
/// wire, so no peer can populate it for itself.
///
/// NULL means the record was created on this machine (`av name`) or was cached
/// before this column existed. Either way there is no peer to block, so agent
/// blocks must leave those records alone.
pub const MIGRATION_006_NAME_ORIGIN: &str = "
ALTER TABLE name_records ADD COLUMN origin_agent_id TEXT;
CREATE INDEX IF NOT EXISTS idx_name_records_origin ON name_records(origin_agent_id);
";

/// Every migration, in the order they must be applied.
///
/// Single source of truth: `db::migrate` walks this, and the on-disk test counts
/// against it. Keeping the list here is what stops the migration count in that
/// test from silently going stale when a migration is added.
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("001_initial", MIGRATION_001_INITIAL),
    ("002_relevance", MIGRATION_002_RELEVANCE),
    ("003_name_relevance", MIGRATION_003_NAME_RELEVANCE),
    ("004_canonical_scheme", MIGRATION_004_CANONICAL_SCHEME),
    ("005_blocks", MIGRATION_005_BLOCKS),
    ("006_name_origin", MIGRATION_006_NAME_ORIGIN),
];

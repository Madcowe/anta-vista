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

/// The locations a blocked resource was held under.
///
/// A resource block is keyed by content hash, which is exactly what makes it
/// useless for the one question a *fetch* route has to answer: "is this location
/// blocked?" The hash is only known once the bytes are in hand, so it cannot be
/// derived from an `autonomi://` address — and the join that would resolve one to
/// the other stops working the moment the block lands, because `av block` deletes
/// the local copy by default. The row the guard needed is the row the block
/// removed.
///
/// So the location is recorded here at block time, before the delete, and this
/// table is what the fetch routes read. It is keyed by the location itself rather
/// than by `resources.id`, which is what lets the row outlive the resource it
/// names.
///
/// Both spellings are stored, because a record indexed under two paths is two rows
/// sharing one canonical address, and refusing one while allowing the other would
/// be a block with a hole in it exactly where a user would find it.
///
/// The resource id is kept alongside the location for the same reason the row
/// exists at all: the resource it names is deleted. Without it, `av unblock`
/// would have to re-read `resources` to find which locations to clear, and would
/// find nothing — leaving a block the user cannot lift through the API, which is a
/// worse failure than the one this table prevents. So the id is carried here
/// rather than looked up.
///
/// Note this is a *fetch-level* deny list, not a statement about the address. A
/// location stays refused until it is explicitly unblocked, even if different
/// content later turns up at that address, because a fetch by address has no
/// content to compare against and refusing is the safe direction to be wrong in.
/// Unblocking is the user's way to say the address is fine again.
///
/// Not cleared by `av purge --all`, for the same reason `blocks` is not: this is
/// policy, not cache. `av unblock` removes the rows alongside the block.
pub const MIGRATION_007_BLOCKED_LOCATIONS: &str = "
CREATE TABLE IF NOT EXISTS blocked_locations (
    resource_id TEXT NOT NULL,
    location    TEXT NOT NULL,
    PRIMARY KEY (resource_id, location)
);
CREATE INDEX IF NOT EXISTS idx_blocked_locations_location ON blocked_locations(location);
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
    ("007_blocked_locations", MIGRATION_007_BLOCKED_LOCATIONS),
];

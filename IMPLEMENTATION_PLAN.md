# Anta-vista Implementation Plan (Agent-Executable)

**Project:** Distributed Search + Naming System on x0x  
**Primary language:** Rust  
**Target runtime:** Linux first, then macOS + Windows  
**Primary objective (MVP):** Local-first semantic search + decentralized query/response over x0x, with emergent trust and feedback-based ranking.

---

## 0) Audience and Usage

This plan is written for an autonomous coding agent (Claude/Codex/similar) to execute in stages.

### Execution rules for agent
- Implement in small, testable commits (even if not actually committing).
- Keep code modular and crate-scoped.
- Do not over-scope: complete MVP first.
- For each phase:
  1. implement tasks,
  2. run validation,
  3. report what passed/failed,
  4. only then move to next phase.
- Prefer deterministic behavior over cleverness.
- Avoid adding non-essential dependencies.

---

## 1) Product Scope (MVP)

### In scope
- MIME-driven ingestion and metadata extraction.
- Text description synthesis for all resources (text-first strategy).
- Embedding generation using `sentence-transformers/all-MiniLM-L6-v2` equivalent runtime.
- Local vector search (cosine similarity).
- DNS-like naming mode with case-insensitive exact-name lookup.
- Protocol-agnostic URI targets/locations (e.g. `http://`, `https://`, `ant://`, `autonomi://`) with scheme-based filtering.
- x0x network query/response exchange.
- Claims and feedback propagation.
- Basic trust weighting (not hard filtering).
- Cold-start clustering strategy.
- SQLite-backed persistence.

### Out of scope (MVP)
- Multimodal embeddings (image/audio native models).
- Complex distributed consensus protocols.
- Full sybil-proof identity economics.
- Large-scale crawling.

---

## 2) Non-Negotiable Constraints

1. **Cross-platform:** must build and run on Linux/macOS/Windows.
2. **Low-spec friendly:** CPU-first; no GPU assumptions.
3. **Reproducibility:** same input + same model profile -> same normalized vector.
4. **Protocol versioning:** every network payload includes `schema_version`.
5. **Compatibility safety:** never compare embeddings across different model profiles.
6. **Trust as weight, not filter:** blocked is explicit exception.
7. **URI protocol agnostic by default:** do not hardcode HTTP-only assumptions.

---

## 3) Architecture (Crates)

Create a Rust workspace with these crates:

```text
anta-vista/
  Cargo.toml                 # workspace
  crates/
    av-core/                 # domain types, shared constants
    av-store/                # SQLite schema, migrations, repositories
    av-ingest/               # MIME detect, metadata, description synthesis
    av-embed/                # embedding runtime adapters + normalization
    av-index/                # local vector index + search APIs
    av-net-x0x/              # x0x transport adapter + wire protocols
    av-trust/                # trust updates, agreement scoring, decay
    av-query/                # query orchestration and ranking
  examples/
    local_search.rs
    p2p_two_nodes.rs
  docs/
    protocol.md
    ranking.md
    threat-model.md
```

### Suggested dependency baseline
- DB: `rusqlite` with `bundled` feature (cross-platform reliability).
- Serialization: `serde`, `serde_json`.
- Time/IDs: `time` or `chrono`, `uuid`.
- Hash/signing helpers: `sha2`, plus x0x-provided identity/signature utilities where possible.
- Paths/dirs: `directories` crate.
- Logging/metrics: `tracing`, `tracing-subscriber`.
- Tests: `rstest` optional, `tempfile` for integration tests.

> If async is needed broadly, standardize on `tokio`; otherwise keep sync where possible for simplicity.

---

## 4) Data Model (v1)

## 4.1 EmbeddingProfile (critical)
```rust
struct EmbeddingProfile {
    model_id: String,          // e.g., "all-MiniLM-L6-v2"
    model_version: String,     // exact pinned runtime/model version
    dim: u16,                  // 384
    normalized: bool,          // true
    preproc_version: String,   // tokenizer/preprocess contract hash or version
}
```

## 4.2 ResourceDescriptor
```rust
struct ResourceDescriptor {
    id: String,                // content hash or URI hash
    kind: ResourceKind,        // text|image|audio|file|pdf|other
    location: String,          // URI/path/reference (protocol-agnostic)
    location_scheme: Option<String>, // e.g. "https", "ant"
    location_canonical: Option<String>, // canonical URI form when applicable
    mime_type: String,
    filename: Option<String>,
    metadata_json: serde_json::Value,
    description_text: String,  // synthesized canonical description
    created_at: i64,
}
```

## 4.3 EmbeddingRecord
```rust
struct EmbeddingRecord {
    resource_id: String,
    profile_id: String,        // foreign key to embedding profile
    vector: Vec<f32>,          // len must match profile dim
    l2_norm: f32,              // ~1.0 expected
    created_at: i64,
}
```

## 4.4 Claim
```rust
struct Claim {
    schema_version: u16,
    claim_id: String,
    subject: String,
    predicate: String,         // about|tagged_as|useful_for|resolves_to
    object: String,
    by_agent_id: String,
    timestamp: i64,
    signature: Vec<u8>,
}
```

## 4.5 FeedbackEvent
```rust
enum FeedbackKind { Useful, NotUseful, Incorrect, HighConfidence }

struct FeedbackEvent {
    schema_version: u16,
    feedback_id: String,
    query_text: String,
    resource_id: String,
    by_agent_id: String,
    kind: FeedbackKind,
    timestamp: i64,
    signature: Vec<u8>,
}
```

## 4.6 TrustState
```rust
struct TrustState {
    subject_agent_id: String,
    trust_score: f32,          // bounded e.g. [-1.0, 1.0]
    evidence_count: u32,
    last_updated_at: i64,
}
```

## 4.7 NameRecord (DNS-like mode, v1)
```rust
enum NameRecordType { A, Txt, Uri, Service }

struct NameRecord {
    schema_version: u16,
    record_id: String,
    normalized_name: String,   // canonical key for exact match (case-insensitive)
    original_name: String,     // as published by agent
    record_type: NameRecordType,
    target: String,            // URI/address/content id/service payload
    target_scheme: Option<String>, // derived from target URI when parseable
    target_canonical: Option<String>, // canonical URI form (e.g. scheme aliases normalized)
    ttl_secs: u32,
    by_agent_id: String,
    timestamp: i64,
    signature: Vec<u8>,
}
```

### Name normalization contract (required)
- Normalize before storage and lookup.
- v1 minimum: Unicode NFC + lowercase (or full casefold where available).
- Never perform semantic/fuzzy matching in naming mode; exact match only on `normalized_name`.

### URI/scheme normalization contract (required)
- Treat URI schemes case-insensitively.
- Keep system protocol-agnostic: accept known and unknown schemes unless blocked by policy.
- Support alias mapping: `autonomi://` is an alias of `ant://`.
- Canonicalize alias schemes to `ant://` for storage/compare while preserving original input when needed.
- Extract and persist `target_scheme` / `location_scheme` for filterable queries.

### `ant://` / `autonomi://` canonical format (v1)
- Authority/address must be exactly 64 lowercase hex chars (`[0-9a-f]{64}`), representing 32 bytes.
- Optional path is allowed (e.g. filename), but does not change the base resource identity.
- Examples (both valid, same base resource):
  - `ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03`
  - `ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03/index.html`
- Canonical resource key for dedupe should be based on scheme-normalized base address (`ant://<64hex>`), not optional path.

---

## 5) SQLite Plan

SQLite is FOSS/public-domain and is acceptable.

### Library choice
- Use `rusqlite` with `features = ["bundled"]`.
- Rationale: easiest cross-platform build behavior, fewer system dependency surprises.

### Schema (minimum)
- `resources`
- `embedding_profiles`
- `embeddings`
- `claims`
- `feedback_events`
- `trust_state`
- `name_records`
- `peer_cache`
- `query_cache`
- `applied_migrations`

### DB requirements
- WAL mode enabled.
- Foreign keys enabled.
- Unique constraints on `claim_id`, `feedback_id`, and `(resource_id, profile_id)` for embeddings.
- Unique constraint for naming records on `(normalized_name, record_type, target_canonical, by_agent_id)`.
- Indexes on `resource_id`, `by_agent_id`, trust lookup keys, `name_records(normalized_name)`, and scheme indexes (`name_records(target_scheme)`, `resources(location_scheme)`).

---

## 6) Cross-Platform Plan (Must Implement)

## 6.1 Paths and data locations
- Never hardcode OS paths.
- Use `directories` for config/data/cache roots.
- Define app paths in one module (`av_core::paths`).

## 6.2 Build/linking
- Ensure CI validates:
  - `x86_64-unknown-linux-gnu`
  - `x86_64-apple-darwin`
  - `aarch64-apple-darwin`
  - `x86_64-pc-windows-msvc`
- SQLite via bundled build.

## 6.3 File system behavior
- Use `Path`/`PathBuf` consistently.
- Do not assume case sensitivity.
- Normalize line endings in fixtures.

## 6.4 Networking/runtime
- Make timeouts configurable.
- Handle firewall/connect failures with explicit error variants.

## 6.5 Model runtime portability
- Pin model artifact version and checksum.
- Validate model load in CI smoke test per OS.

---

## 7) Protocol and Network (x0x)

Define payloads in `docs/protocol.md` and mirror them in `av-core` types.

### Topic names
- `av.query.v1`
- `av.response.v1`
- `av.claim.v1`
- `av.feedback.v1`
- `av.name.query.v1`
- `av.name.response.v1`
- `av.name.claim.v1`
- `av.presence.v1` (optional)

### Required payload envelope
```json
{
  "schema_version": 1,
  "message_id": "uuid",
  "sent_at": 1716980000,
  "from_agent_id": "hex",
  "kind": "query|response|claim|feedback|name_query|name_response|name_claim",
  "payload": { }
}
```

### Security requirements
- Verify signatures for claim/feedback messages.
- Reject oversized payloads.
- Reject unsupported `schema_version`.
- Deduplicate by `message_id`.

---

## 8) Ranking + Trust (v1)

## 8.1 Ranking formula
Use bounded weighted sum:

```text
score =
  0.65 * semantic_similarity
+ 0.15 * agreement_score
+ 0.10 * feedback_score
+ 0.10 * trust_score
```

All components normalized to `[0,1]` before weighting.

## 8.2 Cold start behavior
When trust evidence is sparse:
1. collect multiple responses,
2. cluster near-duplicate resources/claims,
3. prefer largest coherent cluster,
4. apply semantic score within cluster.

## 8.3 DNS-like naming conflict ranking (exact match mode)
For naming lookups, first do exact match by `normalized_name` (optionally constrained by allowed schemes), then rank competing records:

```text
name_score =
  0.50 * trust_score
+ 0.30 * agreement_score
+ 0.10 * recency_score
+ 0.10 * ttl_validity_score
```

Rules:
- Matching is case-insensitive exact only (via canonical `normalized_name`).
- No embedding similarity in naming mode.
- Resolve URI scheme aliases before compare (e.g. `autonomi://` -> `ant://`).
- Return top record plus alternates (with scores) for transparency.
- Expired TTL records are excluded or heavily penalized by policy.

## 8.4 Trust update principles
- Start neutral (0.0).
- Increment for agreement/consistency.
- Decrement for high disagreement/spam/invalid signatures.
- Apply mild decay over time.
- Blocked agents => hard drop (policy exception).

---

## 9) Anti-Abuse (MVP)

- Per-agent message rate limit (configurable).
- Payload size caps.
- Invalid-signature strike counter.
- Duplicate flood detection (`message_id` + short-term cache).
- Trust penalty for repeated malformed/contradictory output.

---

## 10) Phase-by-Phase Execution

## Phase 1 — Workspace + Core Types
**Goal:** compilable workspace with shared domain types.

### Tasks
- Initialize workspace and crates listed above.
- Add `EmbeddingProfile`, `ResourceDescriptor`, `Claim`, `FeedbackEvent`, `TrustState`.
- Add shared error enum strategy (`thiserror` recommended).
- Add serialization tests for wire types.

### Acceptance
- `cargo check --workspace` passes.
- serde roundtrip tests pass.

---

## Phase 2 — Storage Layer (SQLite)
**Goal:** persistent local state with migrations.

### Tasks
- Implement migration runner.
- Create base schema + indexes.
- Repository methods for CRUD on resources/embeddings/claims/feedback/trust/name records.
- Add WAL + FK pragma setup.

### Acceptance
- Integration test creates DB, migrates, inserts and queries all entities.
- Works on Linux/macOS/Windows CI.

---

## Phase 3 — Ingestion Pipeline
**Goal:** deterministic resource description generation.

### Tasks
- MIME detection via content inspection.
- Filename tokenization.
- Metadata extraction (minimal useful subset).
- Canonical description synthesis templates.
- Unit tests with fixture files.

### Acceptance
- Given fixed fixture set, generated descriptions exactly match snapshots.

---

## Phase 4 — Embedding Adapter (MiniLM)
**Goal:** produce normalized 384-d vectors using pinned model profile.

### Tasks
- Add `EmbeddingProvider` trait.
- Implement MiniLM provider.
- Enforce profile + dimension checks.
- Enforce L2 normalization and deterministic preprocessing.

### Acceptance
- Embedding tests verify dimension=384 and norm≈1.0.
- Same input returns stable vectors within tolerance.

---

## Phase 5 — Local Index and Search
**Goal:** local semantic search works end-to-end.

### Tasks
- Start with brute-force cosine for correctness.
- Query API: top-k retrieval with score explanations.
- Add naming API: exact lookup by canonical `normalized_name`.
- Add scheme filters for search and naming (e.g. allow only `ant` + `https`).
- Optional metadata filter (`kind`, MIME prefix).

### Acceptance
- Local integration test: ingest -> embed -> search returns expected top result set.

---

## Phase 6 — x0x Transport Integration
**Goal:** peer query/response over x0x topics, via both gossip broadcast and direct agent-to-agent messaging.

x0x provides two communication modes that serve different purposes:

| Mode | Delivery | When to use |
|------|----------|-------------|
| **Gossip pub/sub** | Broadcast, epidemic, eventually consistent | Queries to unknown peers, broadcasting name claims, presence |
| **Direct messaging** | Private, immediate, reliable, ordered | Trusted/known agents, private responses, avoiding gossip fan-out |

### Tasks

#### Gossip (broadcast)
- Implement publish/subscribe wrapper in `av-net-x0x`.
- Add envelope validation and dedupe.
- Implement request-response flow with timeout.
- Implement name query/response flow (`name_query`, `name_response`, `name_claim`).
- Cache peer responses with TTL.

#### Direct messaging (established relationships)
- Implement `connect_agent()` to establish a direct relationship with a peer via `POST /agents/connect`.
- Implement `send_direct()` for private, reliable, ordered agent-to-agent messages via `POST /direct/send`.
- Implement `DirectListener` (SSE `GET /direct/events`) for receiving direct messages.
- Extend `MessageDispatcher` with direct variants: `send_direct_query()`, `send_direct_response()`, `send_direct_name_query()`, `send_direct_name_response()`.
- Extend `MockNetClient` to record direct connections and messages for test inspection.

### Acceptance
- Gossip: node A broadcasts a query and receives node B's gossip response.
- Direct: known agent A sends a private query directly to agent B and receives a direct response.
- Both modes share the same `MessageEnvelope` wire format and validation pipeline.

---

## Phase 7 — Ranking, Trust, Feedback Loop
**Goal:** ranking reflects semantic + social signals.

### Tasks
- Implement agreement scoring.
- Implement feedback aggregation.
- Implement trust update job + decay.
- Merge ranking components with weighted formulas (semantic mode + naming mode).

### Acceptance
- Test scenario shows ranking shift after simulated positive/negative feedback.

---

## Phase 8 — Cold Start + Anti-Abuse
**Goal:** robust behavior before trust matures and under noisy peers.

### Tasks
- Implement clustering for sparse trust conditions.
- Add rate limiter + payload caps.
- Add malformed message penalties.
- Add observability counters/log fields.

### Acceptance
- Cold-start tests pick coherent clusters.
- Abuse simulation does not crash node and reduces attacker influence.

---

## Phase 9 — Cross-Platform Hardening
**Goal:** portable and predictable across Linux/macOS/Windows.

### Tasks
- CI matrix across target triples.
- Path handling tests.
- DB opening/migration tests on each OS.
- Model load smoke tests on each OS.

### Acceptance
- Full CI matrix green for check/test/smoke.

---

## Phase 10 — Docs + Examples + MVP Release Candidate
**Goal:** usable by external developers.

### Tasks
- Write `README` quickstart.
- Provide `examples/local_search.rs` and `examples/p2p_two_nodes.rs`.
- Document config defaults and limits.
- Document known limitations.

### Acceptance
- Fresh user can run local example and 2-node p2p demo via documented steps.

---

## 11) Configuration Contract (v1)

Define TOML config:

```toml
[embedding]
model_id = "all-MiniLM-L6-v2"
model_version = "PINNED"
preproc_version = "v1"
normalized = true

[ranking]
semantic_weight = 0.65
agreement_weight = 0.15
feedback_weight = 0.10
trust_weight = 0.10

[network]
query_timeout_ms = 1200
max_payload_bytes = 65536
max_messages_per_minute_per_agent = 120

[uri]
allowed_schemes = []                    # empty => allow all valid schemes
blocked_schemes = []
scheme_aliases = { autonomi = "ant" }  # treat autonomi:// as ant://

[trust]
decay_per_day = 0.01
block_threshold = -0.8
```

---

## 12) Testing Strategy

## Unit tests
- Parsing, normalization, scoring helpers.
- Name normalization and exact-match helpers (case-insensitive canonicalization).
- Description synthesis deterministic snapshots.

## Integration tests
- SQLite migration and repository correctness.
- End-to-end local ingest->embed->search.
- End-to-end naming: publish name records -> exact resolve -> ranked conflict result.
- Simulated trust/feedback ranking updates.

## Network/integration tests
- Two-node x0x query/response.
- Two-node x0x name query/response.
- Deduplication and timeout behavior.

## Property/fuzz tests (optional but valuable)
- Ranking bounds stay in `[0,1]`.
- Malformed payload handling never panics.

---

## 13) MVP Definition of Done

MVP is complete when all are true:
1. Local file ingestion and semantic search works deterministically.
2. DNS-like mode resolves names by case-insensitive exact lookup and ranks conflicting responses.
3. x0x peers can exchange query/response/claim/feedback/name_query/name_response/name_claim payloads.
4. Trust and feedback alter ranking in tests.
5. Anti-abuse controls active and tested.
6. CI passes on Linux/macOS/Windows.
7. Docs and examples allow reproducible demo.

---

## 14) Immediate Task List for Next Agent (Start Here)

1. Scaffold workspace/crates and base `Cargo.toml` files.
2. Implement `av-core` domain structs + serde tests (including `NameRecord`).
3. Implement `av-store` with SQLite migrations + tests (including `name_records`).
4. Implement `av-ingest` deterministic description synthesis.
5. Implement `av-embed` MiniLM adapter with pinned profile.
6. Implement `av-index` brute-force cosine search + exact-name lookup index.
7. Wire `av-query` local orchestrator (semantic + naming mode).
8. Add `av-net-x0x` transport skeleton and protocol envelope validation (including name message kinds).
9. Add CI matrix for Linux/macOS/Windows.
10. Publish first runnable local example.

---

## 15) Known Risks and Mitigations

- **Model runtime complexity across OSes**  
  Mitigation: pin runtime + artifact checksums; add CI smoke model load.

- **Network spam / low-quality peers**  
  Mitigation: strict envelope validation, rate limits, trust penalties.

- **Early trust lock-in bias**  
  Mitigation: small trust weight initially + decay + cluster-based cold start.

- **Schema drift between nodes**  
  Mitigation: schema versioning + compatibility checks + reject unknown major versions.

---

## 16) Stretch Goals (Post-MVP)

- Multiple embedding profiles and per-profile indexes.
- Optional ANN index (HNSW) for larger corpora.
- CRDT-backed distributed feedback state.
- Plugin-based metadata extractors.
- Optional multimodal pipelines.

---

## 17) Appendix — Naming Message Examples (v1)

Use these as reference payloads for wire compatibility tests.

### 17.1 `name_query` (exact, case-insensitive key)
```json
{
  "schema_version": 1,
  "message_id": "3f8a3ad2-5c03-4f22-b24d-902f5f87d5f1",
  "sent_at": 1716980000,
  "from_agent_id": "a1b2c3...",
  "kind": "name_query",
  "payload": {
    "query_id": "d2f66b9b-706f-4c50-a029-76a9026e2bb9",
    "name": "Alice.App",
    "normalized_name": "alice.app",
    "record_type": "Uri",
    "max_results": 10,
    "timeout_ms": 1200
  }
}
```

### 17.2 `name_response` (multiple conflicting records allowed)
```json
{
  "schema_version": 1,
  "message_id": "8f9e9429-90fe-4f68-a1d7-2ac9f2a398e8",
  "sent_at": 1716980002,
  "from_agent_id": "f0e1d2...",
  "kind": "name_response",
  "payload": {
    "query_id": "d2f66b9b-706f-4c50-a029-76a9026e2bb9",
    "normalized_name": "alice.app",
    "results": [
      {
        "record_id": "rec_01",
        "original_name": "Alice.App",
        "normalized_name": "alice.app",
        "record_type": "Uri",
        "target": "autonomi://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03/index.html",
        "target_scheme": "ant",
        "target_canonical": "ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03/index.html",
        "ttl_secs": 3600,
        "timestamp": 1716979900,
        "by_agent_id": "f0e1d2...",
        "signature": "base64:MEQCIF..."
      },
      {
        "record_id": "rec_02",
        "original_name": "alice.app",
        "normalized_name": "alice.app",
        "record_type": "Uri",
        "target": "https://example.invalid/alt",
        "target_scheme": "https",
        "target_canonical": "https://example.invalid/alt",
        "ttl_secs": 1800,
        "timestamp": 1716979800,
        "by_agent_id": "aa55bb...",
        "signature": "base64:MEUCIG..."
      }
    ]
  }
}
```

### 17.3 `name_claim` (gossip publication)
```json
{
  "schema_version": 1,
  "message_id": "eab9a8df-9cd0-4470-a92b-3f8474e40724",
  "sent_at": 1716980005,
  "from_agent_id": "f0e1d2...",
  "kind": "name_claim",
  "payload": {
    "record": {
      "schema_version": 1,
      "record_id": "rec_01",
      "normalized_name": "alice.app",
      "original_name": "Alice.App",
      "record_type": "Uri",
      "target": "autonomi://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03",
      "target_scheme": "ant",
      "target_canonical": "ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03",
      "ttl_secs": 3600,
      "by_agent_id": "f0e1d2...", 
      "timestamp": 1716979900,
      "signature": "base64:MEQCIF..."
    }
  }
}
```

### 17.4 Resolver output contract (local API suggestion)
Return winner + alternates so clients can inspect conflicts.

```json
{
  "name": "Alice.App",
  "normalized_name": "alice.app",
  "winner": {
    "record_id": "rec_01",
    "target": "ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03",
    "record_type": "Uri",
    "score": 0.83
  },
  "alternates": [
    {
      "record_id": "rec_02",
      "target": "https://example.invalid/alt",
      "record_type": "Uri",
      "score": 0.52
    }
  ],
  "scoring": {
    "mode": "name_v1",
    "weights": {
      "trust": 0.50,
      "agreement": 0.30,
      "recency": 0.10,
      "ttl_validity": 0.10
    }
  }
}
```

### 17.5 Validation checklist for naming payloads
- `normalized_name` must be canonicalized before comparison.
- `name_query` lookup is exact match on `normalized_name` only.
- `record_type` must match exactly when specified.
- `target` must be a syntactically valid URI when `record_type = Uri` (custom schemes allowed).
- `autonomi://` must be treated as alias of `ant://` for matching and canonical storage.
- For `ant://` (or `autonomi://`) targets, authority must be exactly 64 hex chars.
- Optional path after `ant://<64hex>/...` is allowed; base address identity remains `ant://<64hex>`.
- `ttl_secs` must be non-zero and bounded by policy.
- Signature verification must pass for accepted records.
- Expired records must be excluded or strongly penalized.

---

**End of plan.**

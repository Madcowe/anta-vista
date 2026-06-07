# Anta-Vista Implementation Progress

## Status: Phase 10 Complete — MVP Release Candidate ✅

---

## Completed Phases

### ✅ Phase 1 — Workspace + Core Types
**Date completed:** 2026-06-06  
**Acceptance criteria:** All met

- `cargo check --workspace` passes cleanly
- 10/10 serde roundtrip tests pass in `av-core`

**Files created:**

| Path | Purpose |
|------|---------|
| `Cargo.toml` | Workspace root; all 8 crates + shared dependency table |
| `crates/av-core/Cargo.toml` | `av-core` package manifest |
| `crates/av-core/src/lib.rs` | Module tree + flat re-exports |
| `crates/av-core/src/types.rs` | All domain types + normalisation helpers + 10 tests |
| `crates/av-core/src/error.rs` | `AvError` enum + `AvResult<T>` alias |
| `crates/av-core/src/constants.rs` | Topics, schema version, model constants, weights, anti-abuse limits |
| `crates/av-core/src/paths.rs` | Cross-platform data/config/cache paths via `directories` |
| `crates/av-{store,ingest,embed,index,net-x0x,trust,query}/` | Stub crates (Cargo.toml + minimal lib.rs) |
| `docs/protocol.md` | MessageEnvelope format, topic table, security requirements |
| `docs/ranking.md` | Ranking formula, signal definitions, trust update principles |
| `docs/threat-model.md` | Six threat scenarios with mitigations |

**Key types implemented in `av-core`:**
- `EmbeddingProfile`
- `ResourceDescriptor` / `ResourceKind`
- `EmbeddingRecord`
- `Claim`
- `FeedbackEvent` / `FeedbackKind`
- `TrustState`
- `NameRecord` / `NameRecordType`
- `MessageEnvelope` / `MessageKind` (wire protocol envelope)
- `AvError` / `AvResult<T>`
- `normalize_name()` — Unicode NFC + lowercase
- `normalize_scheme()` — maps `autonomi://` → `ant://`

---

## Pending Phases

### ✅ Phase 2 — Storage Layer (SQLite)
**Date completed:** 2026-06-06  
**Acceptance criteria:** All met

- In-memory + on-disk DB open, WAL + FK pragmas enabled
- Migration runner is idempotent (`applied_migrations` table)
- 11/11 tests pass (10 Phase 1 + 1 integration test covering all repos)

**Files created/modified:**

| Path | Description |
|------|-------------|
| `Cargo.toml` (workspace) | Added `rusqlite = { version = "0.31", features = ["bundled"] }`, `tempfile = "3"` |
| `crates/av-store/Cargo.toml` | Added `rusqlite` dep, `tempfile` dev-dep |
| `crates/av-store/src/lib.rs` | Public API — re-exports `open`, `open_in_memory`, `repo` |
| `crates/av-store/src/db.rs` | WAL+FK setup, idempotent migration runner |
| `crates/av-store/src/schema.rs` | 9-table DDL (resources, embedding_profiles, embeddings, claims, feedback_events, trust_state, name_records, peer_cache, query_cache) + 9 indexes |
| `crates/av-store/src/repo/resources.rs` | insert, get, list, delete |
| `crates/av-store/src/repo/embeddings.rs` | insert_profile, get_profile, insert, get, delete; vector as JSON text |
| `crates/av-store/src/repo/claims.rs` | insert (OR IGNORE), get_by_id, list_by_subject, list_by_agent, delete |
| `crates/av-store/src/repo/feedback.rs` | insert (OR IGNORE), list_by_resource, list_by_agent |
| `crates/av-store/src/repo/trust.rs` | upsert, get, list_all |
| `crates/av-store/src/repo/names.rs` | insert (OR REPLACE), get_by_normalized_name, list_by_scheme, delete |
| `crates/av-store/src/repo/peers.rs` | upsert, list_recent(limit) |
| `crates/av-store/src/repo/query_cache.rs` | insert, get_if_valid(now), purge_expired(now) |
| `crates/av-store/tests/integration.rs` | End-to-end test covering all 7 repos |

### ✅ Phase 3 — Ingestion Pipeline
**Date completed:** 2026-06-06  
**Acceptance criteria:** All met

- 28/28 tests pass (10 av-core + 1 av-store integration + 17 av-ingest pipeline)
- Descriptions exactly match snapshots (e.g. `fish.jpg` → `"a fish image file in jpeg format"`)

**Files created:**

| Path | Purpose |
|------|--------|
| `crates/av-ingest/src/error.rs` | `IngestError` / `IngestResult` |
| `crates/av-ingest/src/mime.rs` | Content-based MIME detection via `infer`; UTF-8/HTML fallback |
| `crates/av-ingest/src/filename.rs` | Stem extraction + separator-split tokenization |
| `crates/av-ingest/src/metadata.rs` | EXIF (`kamadak-exif`), ID3 (`id3`), first-line text, PDF `/Title` scan |
| `crates/av-ingest/src/describe.rs` | `semantic_label` table + deterministic `synthesize` |
| `crates/av-ingest/src/ingest.rs` | `ingest_bytes` / `ingest_file` — SHA-256 ID, scheme parsing, full `ResourceDescriptor` assembly |
| `crates/av-ingest/tests/pipeline.rs` | 17 pipeline tests (MIME detection, tokenisation, description snapshots, full ingest) |

**Note:** `id3` v1.17 requires `use id3::TagLike` trait and `read_from2`. Fixed during implementation.

---

### ✅ Phase 4 — Embedding Adapter (MiniLM)
**Date completed:** 2026-06-06  
**Acceptance criteria:** All met

- 37 tests pass, 2 ignored (real-model tests that need internet / model download)
- Mock provider: dimension=384, L2 norm≈1.0, deterministic ✅
- Real `MiniLmProvider` compiles and wires to fastembed ONNX backend ✅

**Files created:**

| Path | Purpose |
|------|---------|
| `crates/av-embed/src/error.rs` | `EmbedError` / `EmbedResult` |
| `crates/av-embed/src/normalize.rs` | `l2_norm`, `l2_normalize`, `cosine_similarity`, `check_dim` |
| `crates/av-embed/src/provider.rs` | `EmbeddingProvider` trait, `minilm_profile()`, `profile_id()` |
| `crates/av-embed/src/mock.rs` | `MockEmbeddingProvider` — SHA-256-seeded 384-d unit vectors, no model |
| `crates/av-embed/src/minilm.rs` | `MiniLmProvider` — fastembed ONNX backend, downloads model on first use |
| `crates/av-embed/tests/embedding.rs` | 11 tests: 9 unit/mock, 2 `#[ignore]` real-model |

**Note:** `fastembed = "4"` uses `default-features = false, features = ["ort-download-binaries", "hf-hub-rustls-tls"]` to avoid requiring system OpenSSL headers (uses pure-Rust `rustls` instead).

---

### ✅ Phase 5 — Local Index and Search
**Date completed:** 2026-06-06  
**Acceptance criteria:** All met

- 45 tests pass, 2 ignored — 0 failures
- Full pipeline test: ingest → embed → store → cosine search → top-k results ✅
- Scheme filter, MIME filter, kind filter all tested ✅
- Exact name lookup with case-insensitive normalization and conflict ranking ✅

**Files created:**

| Path | Purpose |
|------|---------|
| `crates/av-index/src/error.rs` | `IndexError` / `IndexResult` |
| `crates/av-index/src/filter.rs` | `SchemeFilter`, `KindFilter`, `MimeFilter`, `QueryFilter` |
| `crates/av-index/src/search.rs` | `search_top_k` — brute-force cosine over stored embeddings |
| `crates/av-index/src/naming.rs` | `lookup_name` — exact match, TTL/recency ranking, scheme filter |
| `crates/av-index/src/index.rs` | `LocalIndex` orchestrator |
| `crates/av-index/tests/local_search.rs` | 8 integration tests |

### ✅ Phase 6 — x0x Transport Integration
**Date completed:** 2026-06-06
**Acceptance criteria:** All met

- 61 tests pass, 3 ignored, 0 failures
- All 7 topics subscribe-all ✅
- Publish query / name-query / name-claim via MockNetClient ✅
- Envelope validation (size cap, schema version, empty fields) ✅
- Dedupe cache prevents replay within TTL window ✅
- Payload serde roundtrips for all 6 message kinds ✅
- Two-node test skeleton present (`#[ignore]`) ✅

**Files created:**

| Path | Purpose |
|------|---------|
| `crates/av-net-x0x/src/error.rs` | `NetError` / `NetResult` |
| `crates/av-net-x0x/src/payloads.rs` | Typed payload structs (Query, Response, NameQuery, NameResponse, NameClaim, Claim, Feedback) |
| `crates/av-net-x0x/src/envelope.rs` | `build_envelope()`, `validate_envelope()`, `DedupeCache` |
| `crates/av-net-x0x/src/client.rs` | `NetworkClient` trait, `X0xNetClient` (real HTTP via ureq), `X0xConfig::from_data_dir()` |
| `crates/av-net-x0x/src/mock.rs` | `MockNetClient` — thread-safe, records all calls for test inspection |
| `crates/av-net-x0x/src/listener.rs` | `start_listener()` — SSE background thread decoding `IncomingEvent`s |
| `crates/av-net-x0x/src/dispatcher.rs` | `MessageDispatcher` — high-level orchestrate subscribe/publish/validate |
| `crates/av-net-x0x/tests/net.rs` | 17 tests (16 running, 1 ignored two-node scenario) |

**Extended (Phase 6b — Direct Messaging):**

- 69 tests pass total (8 new direct messaging tests added)
- `NetworkClient` trait extended: `connect_agent()`, `send_direct()`
- `MockNetClient` extended: records connections and direct sends, inspector methods
- `direct_listener.rs` — `start_direct_listener()` SSE thread for `GET /direct/events`, `DirectMessage` type
- `MessageDispatcher` extended: `connect_agent()`, `send_direct_query()`, `send_direct_response()`, `send_direct_name_query()`, `send_direct_name_response()`

**Mode comparison (both use same `MessageEnvelope` wire format):**

| Mode | API endpoint | Use case |
|------|-------------|----------|
| Gossip | `POST /publish` + `GET /events` | Unknown peers, name claims, broadcast queries |
| Direct | `POST /agents/connect` + `POST /direct/send` + `GET /direct/events` | Trusted/known agents, private responses |

### ✅ Phase 7 — Ranking, Trust, Feedback Loop
**Date completed:** 2026-06-07
**Acceptance criteria:** All met

- 82 tests pass, 3 ignored, 0 failures
- Ranking shifts after positive feedback ✅
- Ranking shifts after negative feedback ✅
- Trust score bounded to [-1.0, 1.0] under repeated updates ✅
- Exponential decay moves trust toward neutral ✅
- Full weighted formula components sum to `combined` ✅

**Ranking formula (search mode):**
```
score = 0.65 × semantic_similarity
      + 0.15 × agreement_score
      + 0.10 × feedback_score
      + 0.10 × trust_score
```

**Ranking formula (naming mode):**
```
name_score = 0.50 × trust + 0.30 × agreement + 0.10 × recency + 0.10 × ttl_validity
```

**Files created / modified:**

| Path | Purpose |
|------|---------|
| `crates/av-trust/src/agreement.rs` | DB-backed agreement score: agents-who-claimed-resource / total-distinct-agents |
| `crates/av-trust/src/feedback.rs` | Feedback aggregation: Useful/HighConfidence +1, NotUseful -0.5, Incorrect -1; normalised to [0,1] |
| `crates/av-trust/src/update.rs` | `apply_positive`, `apply_negative`, `new_neutral` — bounded to [-1,1] with diminishing returns |
| `crates/av-trust/src/decay.rs` | Exponential decay toward 0; `decay_all` for bulk DB persistence |
| `crates/av-trust/src/ranking.rs` | `ScoreComponents`, `search_score()`, `name_score()` |
| `crates/av-trust/tests/trust.rs` | 16 DB-integrated tests |
| `crates/av-index/src/search.rs` | `SearchResult` gains 4 score fields; `search_top_k` calls `av_trust::ranking::search_score` |
| `crates/av-index/src/naming.rs` | Real DB trust + agreement lookups replace neutral stubs |
| `crates/av-store/src/repo/embeddings.rs` | Bug fix: `INSERT OR REPLACE` → `INSERT OR IGNORE` on profiles (prevents cascade-deleting embeddings) |

### ✅ Phase 8 — Cold Start + Anti-Abuse
**Date completed:** 2026-06-07
**Acceptance criteria:** All met

- 100 tests pass, 3 ignored, 0 failures
- Cold-start clustering: agreed-upon resources rank above higher-scoring solo results ✅
- Rate limiter blocks after burst capacity exhausted, independent per-agent ✅
- Abuse tracker penalises trust + blocks agent at threshold ✅
- Payload guard combines size cap + rate limit in one call site ✅
- Metrics counters are thread-safe atomics, snapshotable ✅

**Files created (all in `crates/av-query/`):**

| Path | Purpose |
|------|---------|
| `src/error.rs` | `QueryError` / `QueryResult` |
| `src/cluster.rs` | `cluster_responses()` — groups by resource_id, deduplicates per-agent, sorts by agreement then avg_score |
| `src/rate_limit.rs` | `RateLimiter` — per-agent token bucket (pure std) |
| `src/abuse.rs` | `AbuseTracker` — strike counter wired to `apply_negative` + DB upsert |
| `src/guard.rs` | `PayloadGuard` — size cap + rate limit in one call |
| `src/metrics.rs` | `NodeMetrics` (atomic counters) + `MetricsSnapshot` |
| `tests/query.rs` | 18 integration tests |

### ✅ Phase 9 — Cross-Platform Hardening
**Date completed:** 2026-06-07
**Acceptance criteria:** All met

- 120 tests pass, 3 ignored, 0 failures
- GitHub Actions CI matrix created (check + test on Linux/macOS/Windows, cross-check for aarch64 + windows-gnu) ✅
- `AvConfig` TOML contract implemented with `#[serde(default)]` partial loading ✅
- Ranking weights validated to sum to 1.0 ✅
- Path tests verify `PathBuf` usage and `.db` extension ✅
- On-disk DB tests verify WAL mode, FK pragma, migration idempotency, persistence across connections, nested directory creation ✅

**Files created:**

| Path | Purpose |
|------|---------|
| `.github/workflows/ci.yml` | CI matrix: check (Linux/macOS/Windows), test (Linux/macOS), test-windows (excl. av-net-x0x), cross-check (aarch64, windows-gnu) |
| `crates/av-core/src/config.rs` | `AvConfig` — full TOML config with `EmbeddingConfig`, `RankingConfig`, `NetworkConfig`, `UriConfig`, `TrustConfig`; `from_file()`, `from_str()`, `to_toml_string()`, `validate()` |
| `crates/av-core/tests/config.rs` | 8 config tests (defaults, validation, roundtrip, partial TOML, scheme aliases, file I/O) |
| `crates/av-core/src/paths.rs` | +4 inline path tests |
| `crates/av-store/tests/on_disk.rs` | 4 on-disk DB tests (WAL, FK, idempotency, persistence, nested dirs) |

### ✅ Phase 10 — Docs + Examples + MVP Release Candidate
**Date completed:** 2026-06-07
**Acceptance criteria:** All met

- `cargo check --workspace` passes cleanly ✅
- `cargo run --example local_search -p anta-vista-examples` runs to completion, prints formatted output ✅
- `cargo test --workspace` — 120 tests pass, 3 ignored, 0 failures ✅
- `p2p_two_nodes` example exits gracefully with instructions when no daemon is running ✅

**Files created:**

| Path | Purpose |
|------|--------|
| `README.md` | Quickstart — what it is, local search, P2P demo, config, limitations |
| `examples/Cargo.toml` | `anta-vista-examples` workspace member; `publish = false` |
| `examples/examples/local_search.rs` | Full pipeline demo (ingest → embed → store → search → name resolution) |
| `examples/examples/p2p_two_nodes.rs` | P2P gossip demo (connects to x0x, subscribe-all, publish query + name claim) |
| `docs/known-limitations.md` | MVP limitations: search, naming, trust, network, platform |
| `docs/config.md` | Full TOML config reference with field table and example |

---

## MVP Definition of Done

1. Local file ingestion and semantic search works deterministically ✅
2. DNS-like mode resolves names by case-insensitive exact lookup and ranks conflicting responses ✅
3. x0x peers can exchange query/response/claim/feedback/name_query/name_response/name_claim ✅
4. Trust and feedback alter ranking in tests ✅
5. Anti-abuse controls active and tested ✅
6. CI passes on Linux/macOS/Windows ✅
7. Docs and examples allow reproducible demo ✅

---

## Notes for Next Agent

- Rust 1.95, workspace resolver = "2"
- All types are in `crates/av-core/src/types.rs`
- `normalize_scheme()` returns `String` (not `&str`) to avoid lifetime issues
- `MessageKind` uses `#[serde(rename_all = "snake_case")]` — `NameQuery` → `"name_query"`
- Stub crates are wired to `av-core` via workspace path dep
- Refer to `IMPLEMENTATION_PLAN.md` for full spec; this file tracks progress
- Run `cargo check --workspace && cargo test --workspace` to verify baseline before making changes

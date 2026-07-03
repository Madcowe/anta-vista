# anta-vista — Agentic Interoperation Skill

## Version check

This file lives at the root of the [anta-vista repository](https://github.com/Madcowe/anta-vista). Before using this skill, check the repository for a newer version of this file — it may have additional commands, changed flags, or updated library APIs.

## Overview

**Distributed semantic search + DNS-like naming for AI agents** — built on [x0x](https://github.com/saorsa-labs/x0x).

Agents share embeddings, claims, and feedback over an encrypted peer-to-peer network. Truth and ranking emerge from agreement, usage, and trust over time — no central index, no authority.

```
Index content  →  Search by meaning  →  Name resources  →  Rate & propagate
```

## Library architecture

```
av-core          →  types, config, constants, path helpers
av-store         →  SQLite repositories (resources, embeddings, names, peers, feedback, relevance)
av-ingest        →  MIME detection, metadata extraction, description synthesis
av-embed         →  embedding trait + MiniLM adapter (384-d, CPU-only, ~22 MB model)
av-index         →  local vector search + exact name resolution
av-trust         →  trust updates, feedback aggregation, ranking formula
av-query         →  cold-start clustering, rate limiting
av-net-x0x       →  x0x gossip + direct messaging transport
av-cli           →  CLI binary (av)
```

Dependency chain (bottom-up):

```
av-core
  ├─ av-store
  ├─ av-embed
  ├─ av-ingest
  └─ av-net-x0x
       └─ av-trust, av-index  (depend on av-store + av-embed)
            └─ av-query  (depends on av-trust)
                 └─ av-cli  (depends on all)
```

## CLI commands (`av`)

### Global flags

| Flag | Description |
|------|-------------|
| `--non-interactive` | JSON output, no prompts (machine mode) |
| `--config <path>` | Path to config.toml |
| `--timeout <ms>` | Network response timeout (default 5000) |
| `--stream` | Show results progressively as they arrive |
| `-v` / `-vv` | Increase log verbosity (INFO / DEBUG) |

### `av status`

Check daemon and model health:

```bash
av status
av status --non-interactive   # JSON: {"x0x_running": true, "antd_running": true, "minilm_loaded": true, ...}
```

Reports: x0x daemon running, antd daemon running, MiniLM model loaded, av listen background process running.

### `av index <uri>`

Ingest a URI: download content (if reachable), detect MIME, extract metadata, synthesise a description, embed it, and store in the local index.

```bash
av index https://example.com/doc.html
av index autonomi://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a
av index ant://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a  # alias, normalised to autonomi://
av index 711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a       # bare hex, auto-detected

av index <uri> --tags "rust,tutorial"          # add tags to description
av index <uri> --no-download                   # URI metadata only (no content fetch)
av index <uri> --no-verify                     # skip reachability check
av index <uri> --force                         # re-index even if URI already exists
```

The `--force` flag replaces the existing resource (cascade deletes embeddings + feedback).

### `av search <query>`

Semantic search across local index + network results. Queries are embedded with MiniLM and compared by cosine similarity.

```bash
av search "rust programming tutorials"
av search "image of a cat" --kind image
av search "pdf documents" --mime application/pdf
av search "my query" --scheme autonomi          # only Autonomi resources
av search "my query" --scheme https             # only HTTPS resources
av search "my query" --limit 20
av search "my query" --stream                   # show results as they arrive from network
av search "my query" --timeout 10000            # wait 10s for network responses
av search "my query" --non-interactive          # JSON output
```

Ranking formula: `0.55 × semantic + 0.15 × agreement + 0.10 × feedback + 0.10 × trust + 0.10 × relevance`

In interactive mode, after results are displayed, the user is prompted: *"Which result was most relevant? (1-N, or Enter for none)"*. This stores per-query relevance feedback, boosting that resource for future searches with the same query. If the selected result came from the network, it is automatically propagated as a local resource.

### `av name <uri> <name>`

Register a human-readable name pointing to a URI. Names are normalised (Unicode NFC + lowercase) and exact-matched on lookup — no fuzzy matching.

```bash
av name autonomi://<64hex> my-app
av name autonomi://<64hex> my-app --type service     # record type: uri (default), a, txt, service
av name autonomi://<64hex> my-app --ttl 86400        # TTL in seconds (default 3600)
av name autonomi://<64hex> my-app --no-verify        # skip URI reachability check
```

The name is gossiped to the `av.name.claim.v1` topic so other peers learn about it. Names are scored by trust, agreement, recency, and TTL validity.

### `av resolve <name>`

DNS-like name resolution. Returns the best `NameRecord` for the given name, plus alternates.

```bash
av resolve my-app
av resolve my-app --type service                     # filter by record type
av resolve my-app --scheme autonomi                  # only autonomi targets
av resolve my-app --limit 5
av resolve my-app --non-interactive                  # JSON output
```

Exact match on `normalized_name` only. No semantic/fuzzy matching for names.

### `av rate <resource_id> <rating>`

Submit a lightweight opinion about a resource. No content is stored — only the rating record.

```bash
av rate <sha256-resource-id> useful
av rate <sha256-resource-id> not-useful
av rate <sha256-resource-id> incorrect
av rate <sha256-resource-id> high-confidence
av rate <sha256-resource-id> useful --query "my search query"   # with query context
```

Ratings are gossiped to the `av.feedback.v1` topic. They feed into the `feedback_weight` component of the ranking formula.

### `av propagate <resource_id> <location> <description>`

Create a full local copy of a resource by re-embedding its description. The resource becomes searchable in your local index. No-op if the resource already exists locally.

```bash
av propagate <sha256-resource-id> autonomi://<64hex> "description text here"
av propagate <sha256-resource-id> autonomi://<64hex> "description text here" --mime text/html
av propagate <sha256-resource-id> https://example.com/doc "description text" --mime text/plain
av propagate <sha256-resource-id> autonomi://<64hex> "desc" --non-interactive
```

The description is re-embedded with MiniLM (deterministic — produces the same vector as the original). The resource gets metadata `{"propagated": true, "propagated_at": <timestamp>}`.

### `av purge`

Clear local database entries.

```bash
av purge --resource <sha256>       # delete a specific resource + embeddings
av purge --name "my-app"           # delete name records for a specific name
av purge --duplicates              # remove resources sharing the same location
av purge --cache                   # clear query cache only
av purge --all                     # clear entire local DB (with interactive confirmation)
av purge --all --no-confirm        # skip confirmation (for scripting)
av purge --all --no-confirm --non-interactive
```

### `av listen`

Background responder. Subscribes to all anta-vista gossip topics and responds to incoming queries from peers.

**Auto-start:** `av search`, `av resolve`, `av index`, `av name`, `av rate`, and `av propagate` all automatically spawn `av listen` as a background process when x0x is available — including in non-interactive mode. Once started, it keeps running after the parent command exits, so subsequent commands find it already active.

**If the spawn fails** (e.g. the `av` binary isn't at the expected path), the command still proceeds without network support — no peers are discovered and direct queries will never be sent.

```bash
av listen                        # run forever (Ctrl-C to stop)
av listen --run-for 60           # stop after 60 seconds
```

## Key distinction: Rate vs Propagate

Both commands help preserve knowledge, but operate at fundamentally different levels:

| Aspect | `av rate` | `av propagate` |
|--------|-----------|----------------|
| Purpose | Record an opinion signal | Create a full local copy |
| Creates a resource row in DB? | No — stores only `(resource_id, rating, query?)` | Yes — complete `ResourceDescriptor` + `EmbeddingRecord` |
| Requires description text? | No | Yes (re-embedded for search vector) |
| Requires MiniLM model? | No | Yes |
| Requires the resource to be reachable? | No — just the ID | No — just the embedded description |
| Network effect | Gossips `FeedbackPayload` to `av.feedback.v1` | None directly; the propagated resource is local-only and will be shared when peers query your `av listen` instance |
| Effect on ranking | Affects `feedback_weight` globally | No ranking effect — just makes the resource searchable locally |
| Survivability | Helps rank the resource higher | Ensures the resource survives if the original indexer goes offline |

### Decision guide

```
Is the resource already in your local index?
  ├─ No → Can you provide a description? 
  │       ├─ Yes → av propagate (creates searchable local copy)
  │       └─ No  → av rate (signal opinion without content)
  └─ Yes → av rate (add your feedback signal)
```

## Library API examples (Rust)

### av-core

```rust
use av_core::types::{normalize_name, normalize_scheme, ResourceDescriptor, ResourceKind, NameRecord};
use av_core::paths::db_path;
use av_core::config::AvConfig;

// Name normalisation (Unicode NFC + lowercase)
let normalized = normalize_name("Alice-App");  // → "alice-app"

// Scheme normalisation (ant → autonomi, others pass through)
let scheme = normalize_scheme("ant");        // → "autonomi"
let scheme = normalize_scheme("https");      // → "https"

// Default database path (platform-specific via `directories` crate)
let path = db_path().expect("platform data dir");

// Load config
let config = AvConfig::from_file(path::Path::new("config.toml")).ok();
config.map(|c| c.validate().expect("weights sum to 1.0"));
```

### av-store

```rust
let conn = av_store::open(&db_path).expect("open DB");
// Or in-memory for testing:
let conn = av_store::open_in_memory().expect("in-memory DB");

// ── Resources ──────────────────────────────────────────────
use av_store::repo::resources;
use av_core::types::{ResourceDescriptor, ResourceKind};

let resource = ResourceDescriptor {
    id: "sha256-hash".into(),
    kind: ResourceKind::Text,
    location: "autonomi://<64hex>".into(),
    location_scheme: Some("autonomi".into()),
    location_canonical: Some("autonomi://<64hex>".into()),
    mime_type: "text/plain".into(),
    filename: Some("readme.txt".into()),
    metadata_json: serde_json::json!({}),
    description_text: "description here".into(),
    created_at: 1_700_000_000,
};
resources::insert(&conn, &resource)?;

let found = resources::get(&conn, "sha256-hash")?;
let by_location = resources::get_by_location(&conn, "autonomi://<64hex>")?;

// ── Embeddings ──────────────────────────────────────────────
use av_store::repo::embeddings;

let profile_id = "profile-uuid";
embeddings::insert_profile(&conn, profile_id, &provider_profile)?;

let record = EmbeddingRecord {
    resource_id: "sha256-hash".into(),
    profile_id: profile_id.into(),
    vector: vec![0.1, 0.2, /* ... 384 dimensions */],
    l2_norm: 1.0,
    created_at: 1_700_000_000,
};
embeddings::insert(&conn, &record)?;

// Approximate (brute-force) cosine search
let results = embeddings::search_similar(&conn, &query_vector, profile_id, 10)?;

// ── Names ──────────────────────────────────────────────────
use av_store::repo::names;
use av_core::types::NameRecord;

let name_record = NameRecord {
    record_id: "uuid".into(),
    normalized_name: "my-app".into(),
    original_name: "My-App".into(),
    record_type: "uri".into(),
    target: "autonomi://<64hex>".into(),
    target_scheme: Some("autonomi".into()),
    target_canonical: Some("autonomi://<64hex>".into()),
    ttl_secs: 3600,
    by_agent_id: "agent-uuid".into(),
    timestamp: 1_700_000_000,
    signature: vec![],
    schema_version: 1,
};
names::insert(&conn, &name_record)?;

let found = names::lookup(&conn, "my-app")?;

// ── Peers ───────────────────────────────────────────────────
use av_store::repo::peers;

peers::upsert(&conn, "peer-agent-id", serde_json::json!({}), now_secs())?;
let recent_peers = peers::list_recent(&conn, 10)?;

// ── Relevance (per-query feedback) ──────────────────────────
use av_store::repo::relevance;

relevance::upsert(&conn, "my query", "resource-id", 1.0)?;
let score = relevance::get_score(&conn, "my query", "resource-id")?;

relevance::name_upsert(&conn, "my query", "name-record-id", 1.0)?;
let name_score = relevance::name_get_score(&conn, "my query", "name-record-id")?;
```

### av-embed

```rust
use av_embed::minilm::MiniLmProvider;
use av_embed::provider::EmbeddingProvider;
use av_embed::mock::MockEmbeddingProvider;

// Real model (downloads ~22 MB on first use)
let provider = MiniLmProvider::new().expect("load MiniLM");

// Mock (no download, returns zero vectors)
let mock = MockEmbeddingProvider::new();

// Both implement EmbeddingProvider:
let vec: Vec<f32> = provider.embed_text("some description").expect("embed");
assert_eq!(vec.len(), 384);  // MiniLM-L6-v2 produces 384-d vectors

let profile = provider.profile();
let profile_id = av_embed::provider::profile_id(&profile);
```

### av-ingest

```rust
use av_ingest::ingest::ingest_bytes;
use av_ingest::location::{analyze_location, normalize_uri, LocationInfo};

// Normalise URIs
let normalised = normalize_uri("ant://<64hex>/file.pdf");
// → "autonomi://<64hex>/file.pdf"
let normalised = normalize_uri("711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a");
// → "autonomi://711c7e20006ff3e0ac6c1f3063286a0c1a3e4c409642e8c526173fa60bb7078a"

// Analyse a location
let info: LocationInfo = analyze_location("autonomi://<64hex>/lucky.jpg");
assert_eq!(info.scheme, Some("autonomi".into()));
assert_eq!(info.canonical, Some("autonomi://<64hex>".into()));
assert_eq!(info.inferred_filename, Some("lucky.jpg".into()));

// Ingest raw bytes → ResourceDescriptor
let bytes = b"<html><title>Hello</title><body>World</body></html>";
let resource = ingest_bytes(bytes, None, "https://example.com/page.html").expect("ingest");
// resource.description_text is auto-synthesised from content_preview
// resource.mime_type is detected via `infer` crate
```

### av-index

```rust
use av_index::LocalIndex;
use av_index::filter::{QueryFilter, SchemeFilter, KindFilter, MimeFilter};
use av_index::search::SearchResult;

let index = LocalIndex::new(&conn, &provider);

// ── Search ──────────────────────────────────────────────────

// Default filter (all schemes, all kinds, all MIME types)
let results: Vec<SearchResult> = index.search("my query", 10, &QueryFilter::default())?;

// Filtered search
let mut filter = QueryFilter::default();
filter.scheme = SchemeFilter::new(["autonomi"]);
filter.kind = KindFilter::new([ResourceKind::Text]);
filter.mime = MimeFilter::new(["text/"]);
let results = index.search("my query", 10, &filter)?;

// Each SearchResult has:
//   result.resource        → &ResourceDescriptor
//   result.score           → f64  (combined ranking score)
//   result.semantic_score  → f64
//   result.agreement_score → f64
//   result.feedback_score  → f64
//   result.trust_score     → f64
//   result.relevance_score → f64

// ── Name resolution ─────────────────────────────────────────
use av_index::naming::lookup_name;

let name_results = index.resolve_name("my-app", &SchemeFilter::default())?;
// Or directly:
let name_results = lookup_name(&conn, "my-app", &SchemeFilter::default(), now_secs())?;
```

### av-trust

```rust
use av_trust::ranking::{search_score, ScoreComponents};

// Compute the combined score for a set of components:
let score = search_score(
    0.85,    // semantic similarity
    0.50,    // agreement score
    0.00,    // feedback score
    0.30,    // trust score
    0.75,    // relevance score
    None,    // query (None = no relevance boost)
);

// Or with per-query relevance:
let score = search_score(
    0.85, 0.50, 0.00, 0.30, 0.75,
    Some("my query"),
);

// ScoreComponents struct:
let components = ScoreComponents {
    semantic: 0.85,
    agreement: 0.50,
    feedback: 0.00,
    trust: 0.30,
    relevance: 0.75,
    total: score,
};
```

### av-net-x0x

```rust
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::listener::{start_listener, IncomingEvent};
use av_net_x0x::direct_listener::{start_direct_listener, DirectMessage};
use av_net_x0x::payloads::{QueryPayload, ResponsePayload, ResourceResult};
use av_core::types::{MessageKind, MessageEnvelope};
use std::sync::Arc;

// Create client + dispatcher
let client = Arc::new(X0xNetClient::new(x0x_config));
let dispatcher = MessageDispatcher::new(client.clone());

// Subscribe to all anta-vista topics
dispatcher.subscribe_all()?;

// Or subscribe individually:
dispatcher.subscribe("av.query.v1")?;
dispatcher.subscribe("av.response.v1")?;

// Publish a search query (gossip)
let query_id = dispatcher.publish_query("search text", 10, 5000, vec![])?;

// Send a direct query to a specific peer
dispatcher.connect_agent("peer-agent-id")?;
dispatcher.send_direct_query("peer-agent-id", "search text", 10, 5000, vec![])?;

// Publish a name query (gossip)
let name_query_id = dispatcher.publish_name_query("my-app", Some("uri"), 10, 5000)?;

// Listen for incoming events (SSE streams)
let gossip_rx = start_listener("http://127.0.0.1:12700", "token")?;
let direct_rx = start_direct_listener("http://127.0.0.1:12700", "token")?;

// Poll gossip events
while let Ok(Ok(event)) = gossip_rx.recv_timeout(Duration::from_millis(10)) {
    match event.envelope.kind {
        MessageKind::Response => {
            let resp: ResponsePayload = serde_json::from_value(event.envelope.payload)?;
            if resp.query_id == query_id {
                // handle results: resp.results: Vec<ResourceResult>
            }
        }
        MessageKind::NameResponse => {
            let resp: NameResponsePayload = serde_json::from_value(event.envelope.payload)?;
            // resp.results: Vec<NameRecord>
        }
        _ => {}
    }
}

// Poll direct messages
while let Ok(Ok(msg)) = direct_rx.recv_timeout(Duration::from_millis(10)) {
    // msg.sender: String
    // msg.envelope: MessageEnvelope
}
```

### av-query

```rust
use av_query::cluster::{cluster_responses, needs_clustering};

// Group network responses by resource_id
let clustered = cluster_responses(&network_results);
// clustered[i].result       → ResourceResult
// clustered[i].avg_score    → f64
// clustered[i].agreement_count → usize

// Check if clustering is needed (cold-start trust)
let should_cluster = needs_clustering(trusted_agent_count, 3);
```

## URI scheme reference

| Format | Example | Behaviour |
|--------|---------|-----------|
| `autonomi://<64hex>` | `autonomi://711c7e20006ff3e...` | Canonical form. `download.rs` uses antd REST API / ant CLI to fetch content |
| `ant://<64hex>` | `ant://711c7e20006ff3e...` | Input alias — `normalize_scheme("ant")` → `"autonomi"` |
| Bare 64-hex | `711c7e20006ff3e...` | Auto-detected by `normalize_uri()`, prepends `autonomi://` |
| `https://...` | `https://example.com/doc` | Standard HTTP — fetched via `ureq` |
| `http://...` | `http://example.com/doc` | Standard HTTP — fetched via `ureq` |

## Protocol wire topics

| Topic | MessageKind | Direction | Payload |
|-------|-------------|-----------|---------|
| `av.query.v1` | `Query` | Broadcast | `QueryPayload { query_id, query_text, max_results, timeout_ms, allowed_schemes }` |
| `av.response.v1` | `Response` | Broadcast | `ResponsePayload { query_id, results: Vec<ResourceResult> }` |
| `av.claim.v1` | `Claim` | Broadcast | `ClaimPayload { ... }` |
| `av.feedback.v1` | `Feedback` | Broadcast | `FeedbackPayload { ... }` |
| `av.name.query.v1` | `NameQuery` | Broadcast | `NameQueryPayload { query_id, name, normalized_name, record_type, max_results, timeout_ms }` |
| `av.name.response.v1` | `NameResponse` | Broadcast | `NameResponsePayload { query_id, name, results: Vec<NameRecord> }` |
| `av.name.claim.v1` | `NameClaim` | Broadcast | `NameClaimPayload { ... }` |
| `av.presence.v1` | `Presence` | Broadcast | Agent presence heartbeat |

All payloads are wrapped in a `MessageEnvelope`:
```json
{
  "schema_version": 1,
  "message_id": "uuid-v4",
  "sent_at": 1700000000,
  "from_agent_id": "agent-uuid",
  "kind": "query",
  "payload": { ... }
}
```

Envelopes are serialised as JSON, base64-encoded, and sent over HTTP to the local x0x daemon (`http://127.0.0.1:12700`). x0x fans out to subscribers via SSE.

## Config reference

Default values (all fields optional):

```toml
[embedding]
model_id = "all-MiniLM-L6-v2"
model_version = "v1"
normalized = true

[ranking]
semantic_weight  = 0.55
agreement_weight = 0.15
feedback_weight  = 0.10
trust_weight     = 0.10
relevance_weight = 0.10

[network]
query_timeout_ms = 1200
max_payload_bytes = 65536
max_messages_per_minute_per_agent = 120

[uri]
allowed_schemes = []

[trust]
decay_per_day   = 0.01
block_threshold = -0.8
```

`validate()` checks that ranking weights sum to 1.0. Partial configs are fine — omitted fields use defaults.

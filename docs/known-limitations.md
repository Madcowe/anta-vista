# Known Limitations (MVP)

## Search

### No multimodal embeddings

Images, audio, and video are indexed by their text descriptions only
(filename terms + MIME type + metadata). The actual content is not analysed.
This is intentional for the MVP — it keeps the model requirement to a single
small CPU-friendly model (`all-MiniLM-L6-v2`).

**Workaround:** Add tags or description text at index time. (There is no claim
system in practice — see "Claims and peer agreement are not wired" under
Trust below.)

### Brute-force cosine search

The local index scans all stored embeddings on every query. This is correct
and fast for small corpora (~10k documents) but will slow down at scale.

**Future:** Replace with an ANN index (HNSW or similar) in a post-MVP release.

### Embedding compatibility

Embeddings from different model profiles cannot be compared. Mixing model
versions will silently produce incorrect results if the profile check is
bypassed. Always use the pinned `EmbeddingProfile` from
`av_embed::provider::minilm_profile()`.

## Naming

### No authoritative resolution

Name conflicts are ranked by trust + agreement + recency — there is no
definitive answer. Multiple agents may claim the same name for different
targets. The caller sees all candidates with scores.

### TTL not enforced automatically

Expired name records are penalised in ranking but not automatically deleted.
Run periodic cleanup against the `name_records` table if storage matters.

### A deleted resource's name can come back from a peer

Blocking a resource suppresses the name records that point at it, so a blocked
link cannot be handed out again under a name. That check resolves the name's
target through the `resources` row, which means it can only recognise a target
this machine still holds.

"Block and delete" removes that row, along with the local name records, so the
local gap is closed. But a peer that still holds the claim can re-supply it, and
the re-supplied claim has no local resource to match against, so it will be
served. Blocking a resource *and* deleting it is therefore not durable against a
peer re-asserting the name. "Block, keep local copy" is durable. If you need the
stronger guarantee, block the name record itself as well as the resource.

### Names cached before transport origins were recorded cannot be attributed

Blocking a peer suppresses the name records that peer published, which works by
matching against `name_records.origin_agent_id` — the id the x0x daemon
authenticated when the record arrived. That column was added in migration
`006_name_origin`.

Records cached before that migration have `NULL` there, and there is nothing to
recover the sender from: the only other id in the row is `by_agent_id`, which
the peer wrote in its own payload, so trusting it would let any peer evade a
block by lying about who it is. Legacy rows are therefore left unfiltered, and
a peer you blocked keeps its previously published names resolving until those
records are replaced or the name is blocked directly.

In practice this window is small — a record is re-cached from a current
response, and re-caching rewrites the column. If it matters, purge the name
record as well as blocking the peer.

## Trust

### Claims and peer agreement are not wired

The `claims` table, the `Claim`/`ClaimPayload` types, `MessageKind::Claim` and
`TOPIC_CLAIM` all exist, and the responder subscribes to the topic — but
nothing ever emits a claim and nothing handles one:

- there is no `publish_claim` on `MessageDispatcher`
- `cmd/listen.rs` matches only `Query` and `NameQuery`; `MessageKind::Claim`
  falls through to `_ => {}` and is dropped
- the only `claims::insert` calls in the workspace are in tests
  (`av-trust/tests/trust.rs`, `av-index/tests/local_search.rs`,
  `av-store/tests/integration.rs`)

Consequences:

- `agreement_score` always returns 0.5, so the 0.15 agreement weight in
  `docs/ranking.md` contributes a constant and cannot reorder results.
- There is no record of which agent served which resource outside the lifetime
  of a single search, so resource-to-agent attribution is not available to
  anything.

`agreement_count` shown in search output ("agreed by N peers") is unrelated and
does work — it comes from in-memory response clustering in `av-query`, not from
`claims`. The two are both called agreement; they are not the same signal.

The proposed replacement is user-mediated rather than a gossiped claim stream —
see `docs/follow-ups.md`.

### Cold-start bias risk

Early agents that respond quickly may receive disproportionate initial
trust before the agreement pool is large enough. The cluster-based cold-start
mitigates this but does not eliminate it.

### No sybil resistance

An attacker can spin up many agent identities cheaply. Rate limiting and
trust decay reduce their influence but cannot prevent it entirely. Full
sybil resistance requires identity cost (stake, proof-of-work, etc.) — out of
scope for MVP.

This is why the dead `claims`/`agreement` path should not simply be switched
on: `agreement_score` is `COUNT(DISTINCT by_agent_id)` for a resource over
total distinct agents, so an attacker who registers many agent identities and
has them all claim one resource drives that term toward 1.0 for that resource
alone. The topic is gossiped, so the cost of doing so is low. Restoring the
signal needs a Sybil answer first, not just a publisher.

## Network

### x0x daemon required for P2P

The networking layer delegates to a locally running `x0xd` daemon. The daemon
must be started separately. There is no embedded daemon option in this MVP.

### No response timeout enforcement in library

`timeout_ms` in query payloads is advisory. The local library does not
enforce it — callers must implement their own timeout logic around
`MessageDispatcher::publish_query()`.

## Platform

### Windows: av-net-x0x tests are excluded on Windows

The `ureq` SSE listener uses blocking I/O that behaves differently on Windows.
The crate compiles on Windows but the SSE listener is not test-validated there.
All other crates are fully tested on Windows. Run
`cargo test --workspace --exclude av-net-x0x` there.

### Model download requires internet

`MiniLmProvider::new()` downloads ~87 MB of ONNX weights from Hugging Face on
first use, then serves every later call from disk. Weights are cached under the
OS cache dir (`av_core::paths::cache_dir`); set `FASTEMBED_CACHE_DIR` to choose a
different location. Air-gapped environments must pre-cache the model there.

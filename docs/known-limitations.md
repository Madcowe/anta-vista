# Known Limitations (MVP)

## Search

### No multimodal embeddings

Images, audio, and video are indexed by their text descriptions only
(filename terms + MIME type + metadata). The actual content is not analysed.
This is intentional for the MVP — it keeps the model requirement to a single
small CPU-friendly model (`all-MiniLM-L6-v2`).

**Workaround:** Add tags or description claims via the claim system.

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

## Trust

### Cold-start bias risk

Early agents that respond quickly may receive disproportionate initial
trust before the agreement pool is large enough. The cluster-based cold-start
mitigates this but does not eliminate it.

### No sybil resistance

An attacker can spin up many agent identities cheaply. Rate limiting and
trust decay reduce their influence but cannot prevent it entirely. Full
sybil resistance requires identity cost (stake, proof-of-work, etc.) — out of
scope for MVP.

## Network

### x0x daemon required for P2P

The networking layer delegates to a locally running `x0xd` daemon. The daemon
must be started separately. There is no embedded daemon option in this MVP.

### No response timeout enforcement in library

`timeout_ms` in query payloads is advisory. The local library does not
enforce it — callers must implement their own timeout logic around
`MessageDispatcher::publish_query()`.

## Platform

### Windows: av-net-x0x excluded from CI test run

The `ureq` SSE listener uses blocking I/O that behaves differently on Windows.
The crate compiles on Windows but the SSE listener is not test-validated there.
All other crates are fully tested on Windows.

### Model download requires internet

`MiniLmProvider::new()` downloads the ONNX model from Hugging Face on first
use. Air-gapped environments must pre-cache the model manually.

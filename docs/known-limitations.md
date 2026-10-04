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

## Autonomi in the browser

`av web` can fetch Autonomi records over WebRTC from the browser, so viewing and
indexing an `autonomi://` link needs nothing installed. These limits are about
that path.

### The browser viewer fetches one record, not a site

`Open` downloads a single record and serves it as one document on a throwaway
loopback port. Consequences for content that references other files:

- **Absolute** `http(s)` subresources load.
- **Relative** ones do not — there is no directory to resolve them against, so
  `try.autonomi.com`'s own markup will render without its stylesheet or images.
- `autonomi://` subresources cannot resolve at all. There is no browser-side
  crawl: nothing reads a record's bytes looking for references to fetch.

Multi-file sites that depend on relative paths therefore do not work through
`Open`. This is why `try.autonomi.com` is not a good demo of the feature even
though it is the obvious thing to try. A single self-contained file does work, but
`try.autonomi.com` also ships a strict `Content-Security-Policy`, and the served
page inherits nothing from the original record's headers.

Folder and manifest support is expected to arrive in the Autonomi client rather
than be reimplemented here.

### Downloads depend on how many seeds are reachable

A record resolves through the DataMap of its holders, so it can only be fetched
when enough of them are online and reachable over WebRTC. In practice this is a
small set, so a large file can time out even when a small one succeeds: there is
no size-dependent negotiation here, only whether the nodes are there. A fetch
that reaches the transfer phase and then fails is not a bug in the UI, and the
progress line will say so rather than hanging.

### Everything is held in memory

Both the browser route and the daemon fallback read the whole file into memory
before storing or serving it. The browser path additionally refuses anything
over `MAX_BODY` (32 MiB) and sends it to the daemon instead, which has the same
characteristic. There is no streaming path in either direction, so indexing a
large file costs roughly its size in RAM.

### Filenames come from the location, not the record

A bare `autonomi://<64hex>` carries no filename, and the Autonomi browser library
synthesises a placeholder of the form `public-file-<8hex>.bin` — identical in
shape for every record. Download names therefore come from the location's
optional trailing path segment, and otherwise from a short prefix of the address
plus an extension implied by the detected type. A record with no name in its
location gets a machine-style name.

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

## Opening a large or undisplayable Autonomi record
A record the browser cannot usefully display is downloaded rather than opened, and
so is anything over the 32 MB viewer limit, which `POST /api/view/blob` enforces.
In both cases the bytes are already in the browser, so the file is written straight
from memory with the server-supplied name and nothing is uploaded; the 64 MB record
at `autonomi://f6eeba94008f493a8518186b0647df6d39dbde75f41d5b79549e9a4e63a1ab43`
completes in about 35 s this way, and the status line names every reason that
applied.

The SDK reports `application/octet-stream` for most records, so the page sends the
leading 16 KB to `POST /api/view/plan` and lets the *content* decide the type
before deciding to upload anything. Images, PDFs and pages are therefore shown
rather than downloaded despite carrying no content type of their own.

That recovery is deliberately partial. A prefix may confirm a specific type but
never assert a generic one: magic numbers, an HTML marker and a markdown convention
are positive signals that survive truncation, but "this whole file is text" follows
from the *absence* of binary structure, and a prefix cannot establish an absence.
A record whose type is only knowable from the whole file — a plain `.txt` with no
marker and no trustworthy name in its location — is therefore still downloaded, and
so is a `.md` whose first line is a paragraph rather than a heading. Guessing
`text/plain` would show the former and render mojibake, which is worse than a
download that always works.

Markdown is the sharpest case of this: it has no content signature at all, so it is
recognised only when it opens with front matter or an ATX heading, and it is always
*downloaded* rather than opened even then — a browser handed `text/markdown`
treats it as an unknown type. What the recognition buys is the name, `doc.md`
instead of an untyped blob.

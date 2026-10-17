# Follow-ups

Open issues that are not yet scheduled. Each entry is a self-contained issue:
what is broken today, the evidence, and a proposed direction.

---

## Peer provenance and the dead `claims` / `agreement` path

**Status:** open
**Affects:** `av-trust` ranking (0.15 weight), `av-cli` resource-to-agent
attribution

### Problem

Two things are wanted from peer provenance, and neither is available.

1. **Ranking.** `docs/ranking.md` documents a 0.15 `agreement` term, but
   `agreement_score` (`crates/av-trust/src/agreement.rs:8`) reads the `claims`
   table, which nothing ever writes to. There is no `publish_claim` on
   `MessageDispatcher`, and `cmd/listen.rs` matches only `Query` and
   `NameQuery`, so an incoming `MessageKind::Claim` falls through to `_ => {}`.
   The only `claims::insert` calls in the workspace are in tests. The function
   therefore returns its 0.5 cold-start default on every call, making the term
   a constant that cannot reorder results.

2. **"Block every peer that served this resource."** `av block --resource`
   fully hides a resource — in search, naming, the responder, and indexing — so
   "never see this again" is already solved. What it cannot do is also stop
   hearing from the peers that supplied it, because anta-vista has no
   resource-to-agent record. `ClusteredResult.agent_ids`
   (`crates/av-query/src/cluster.rs:19`) holds the transport-authenticated
   origins, but only for the duration of one search, in memory.

`agreement_count` in search output ("agreed by N peers") *does* work — it comes
from in-memory response clustering, not from `claims`. The two are both called
agreement and are not the same signal.

### Why not just restore the claim stream

`agreement_score` is `COUNT(DISTINCT by_agent_id)` for a resource divided by
total distinct agents. An attacker who registers many agent identities and has
them all claim one resource drives that term toward 1.0 for that resource
alone, and the topic is gossiped, so the cost is low.
`crates/av-test-suite/tests/av_trust.rs:10` models exactly this (a 50-agent
cluster on one resource) but asserts only the claim count — there is no
mitigation to assert. Turning the signal on before answering the Sybil problem
would make it the dominant lever on rank, and Sybil resistance is listed as
out of scope in `docs/known-limitations.md`.

There is also a second-order problem with deriving a block list from agreement:
the set of peers that corroborated a resource is the set whose agreement was the
evidence the resource was legitimate. Blocking all of them because one result
was unwanted punishes exactly the peers that made the result trustworthy, and
gives an attacker a griefing primitive — get a resource echoed widely, then
rely on a victim blocking it to shrink that victim's reachable peer set.

### Proposed direction: user-mediated provenance

Capture provenance at the moment a human decides, using data already in hand,
rather than gossiping claims continuously.

Both existing user-facing flows are good capture points because each is a
deliberate local decision that a remote attacker cannot trigger:

- **propagate** — `propagate_resource` (`crates/av-cli/src/cmd/propagate.rs:65`)
  is reached from the interactive search prompt
  (`cmd/search.rs:388`) and the web rating handler, at which point
  `all_results[idx]["agent_ids"]` is already populated with authenticated
  origins. Recording `resource_id → agent_id` here costs nothing and is
  accurate, because it is captured exactly when a human endorses a resource.
- **block** — the same is true at the point a human blocks a result.

This would support a bounded, explicit `--peers` on `av block --resource`:
block only the peers from the current or most recent search, print the exact
list, and require confirmation. No continuous claim stream, no new gossip
topic, no ranking change, and the blast radius is visible before it is
committed.

Proposed shape:

```sql
CREATE TABLE resource_origins (
    resource_id  TEXT NOT NULL,
    agent_id     TEXT NOT NULL,
    first_seen_at INTEGER NOT NULL,
    via          TEXT NOT NULL,   -- 'propagate' | 'block'
    PRIMARY KEY (resource_id, agent_id)
);
```

Deliberately a separate table, not a provenance column on `resources`: the
capture is partial by construction (only resources a human acted on), and it
must not be mistaken for the authoritative origin that `resources` lacks.
`agent_id` must always come from the transport-authenticated
`DirectMessage::sender` / `IncomingEvent::origin`, never the envelope's
self-reported `from_agent_id`, or the attribution is trivially forgeable — same
rule as blocked-agent matching.

### What this does not solve

It does not restore the 0.15 agreement term in ranking. That needs a cross-peer
signal, and any such signal is Sybil-exposed for the reason above. It is
reasonable to let ranking stay `semantic + feedback + trust + relevance` and
treat the 0.15 slot as reserved until identity cost exists.

### Open questions

- Should `resource_origins` be pruned on `av purge --all`, and should purging
  drop it given it is provenance rather than content? Blocks deliberately
  survive purge today; this is a similar local-policy record.
- Should `av block --resource X --peers` be permitted at all, given the
  griefing shape, or is `av block --agent <id>` (already available, ids via
  `av search --show-peers`) enough?
- If origins are captured on propagate only, is a resource the user never acted
  on permanently unattributable? That seems acceptable, but it should be
  documented rather than discovered.

---

## Propagation adopts a peer's content hash without ever seeing the content

**Status:** open
**Affects:** `av-cli` propagate/reinforce, `av listen` serving, resource blocks

### Problem

`propagate_resource` (`crates/av-cli/src/cmd/propagate.rs`) stores a
`ResourceDescriptor` whose `id` is the caller's `resource_id` — and for the
interactive search prompt, the web rating handler and `av propagate` alike,
that id is the one a peer advertised alongside its search response. The
bytes are never fetched, so nothing ever checks the claim:
`resources.id` is documented as a SHA-256 content hash (`av-store`'s blocks
repo), but the only place a hash is actually computed from content is
`ingest_bytes` (`crates/av-ingest/src/ingest.rs`), which the propagate path
does not use. The stored row then carries embeddings, so
`to_publishable_resources` (`crates/av-cli/src/cmd/listen.rs`) serves it:
the unverified id and description leave this node re-stamped as local
knowledge.

Consequences today:

- A single peer — or one already-propagated row — can inject an id that does
  not match the bytes at the location, and every node that reinforces the
  result adopts the same claim. No point in the pipeline can notice, because
  no point ever sees the content.
- A later `av index` of the real bytes dedups by location (`cmd/index.rs`,
  Step 4) and reports `duplicate: true` without touching the id, so the
  wrong hash sticks at that location until `av index --force`.
- The id-keyed half of the block system keys on the same unverifiable value.
  The location-keyed walk-around — a peer advertising a blocked location
  under a different id — is now closed inside `propagate_resource` (it
  checks `blocked_locations` as well as the id, fail-closed), but a block
  placed on a propagated id still corresponds to no content hash.

### Proposed direction

Either verify or mark-and-withhold, not silence:

1. **Verify on propagation.** Download the bytes before adopting the id and
   recompute the hash; a mismatch is a peer lying and should be treated as
   one (drop the result, and consider the location suspect). This makes
   propagate expensive — it is deliberately a local metadata copy today —
   and it needs the daemon/browser fetch machinery at rating time.
2. **Mark rows as unverified and withhold them from the responder.**
   `metadata_json` already records `"propagated": true`; teach
   `to_publishable_resources` to leave those rows out of search responses
   until a real `av index` of the location has replaced the id with a hash
   computed from bytes. Keeps the local copy for search and rating, and
   stops the unverifiable claim from being re-stamped by us.

The second is the cheaper of the two and preserves the metadata-copy design
of reinforce; the first is the only one that ever turns a propagated id into
a verified one without requiring the user to re-index.

### Open questions

- Should the interactive search prompt and the web rating button say that a
  reinforced network result stays unverified until it is indexed?
- If verification-on-propagate is chosen, should it reuse the browser-side
  Autonomi fetch that indexing already has, or refuse to propagate
  Autonomi-only when no daemon is available?

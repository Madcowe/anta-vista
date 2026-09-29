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

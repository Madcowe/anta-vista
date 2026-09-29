# anta-vista Threat Model

## Scope

This document covers the principal threats applicable to the anta-vista
decentralised semantic-search and naming system.  It is intentionally concise;
detailed mitigations are developed per-component.

---

## Threat actors

| Actor | Capability |
|-------|-----------|
| Passive eavesdropper | Observes gossip traffic on shared x0x topics |
| Malicious peer | Participates in the network; can publish arbitrary messages |
| Sybil attacker | Controls many node identities to inflate trust or flood topics |
| Resource poisoner | Publishes misleading claims / name records to manipulate results |

---

## Identified threats and mitigations

### T1 — Sybil attack on trust scores

**Threat:** An adversary creates many identities and issues corroborating claims
for their own resources, artificially inflating trust and agreement scores.

**Mitigations:**
- Trust scores are updated only from agents whose own `trust_score` exceeds a
  minimum threshold (not yet finalised; suggested ≥ 0.3).
- New identities start at `trust_score = 0` and must accumulate evidence from
  already-trusted peers.
- Agreement score is weighted by the distinct high-trust nodes that agree, not
  raw count.

### T2 — Spam / payload flooding

**Threat:** A peer floods the network with high-volume or large messages,
exhausting bandwidth and memory.

**Mitigations:**
- Hard envelope size limit: `MAX_PAYLOAD_BYTES` = 1 MiB; oversized messages are
  dropped before deserialization.
- Per-sender rate limit: `DEFAULT_RATE_LIMIT_PER_MINUTE` = 60 msg/min.
- x0x transport-level connection throttling applies underneath.

### T3 — Replay attacks

**Threat:** A peer re-broadcasts a legitimately-signed historical message to
re-trigger processing (e.g. re-vote on a claim).

**Mitigations:**
- `message_id` (UUID v4) deduplication cache with a 5-minute expiry window.
- `sent_at` timestamp checked against a ±2-minute clock skew tolerance;
  messages outside this window are dropped.

### T4 — Claim / name poisoning

**Threat:** A malicious peer publishes false `Claim` or `NameRecord` entries to
redirect name resolution or pollute search results.

**Mitigations:**
- Every `Claim`, `FeedbackEvent`, and `NameRecord` is signed by the publisher.
- Unsigned or mis-signed records are dropped immediately.
- The `NAME_WEIGHT_TRUST` factor (0.50) heavily discounts records from
  low-trust sources.
- Local blocking of a name record (`av block --name <record-id>`) removes it
  from resolution and stops it being served, per T7.

### T5 — Privacy — query correlation

**Threat:** A passive observer correlates query messages from the same
`from_agent_id` over time to profile user interests.

**Mitigations (future work):**
- Query anonymisation / unlinkability is outside Phase 1 scope.
- Planned: onion-routed queries through x0x relay nodes.
- Users may generate ephemeral keypairs per query session.

### T6 — Schema version downgrade

**Threat:** An attacker injects messages with an old `schema_version` to exploit
a deprecated code path.

**Mitigations:**
- Receivers reject any `schema_version` they don't explicitly support
  (`UnsupportedSchemaVersion` error).
- Each topic name is versioned (`av.*.v1`) so future breaking changes use a new
  topic, allowing old and new nodes to coexist during migration.

### T7 — Persistent local denylist

**Threat:** Trust weighting is a soft, gradual defence. A peer that is mildly
malicious is merely ranked lower, so an operator has no way to say "never show
me anything from this source again", and no way to guarantee a specific
resource stops being re-served. `av purge` removes local data but is not a
policy: it says nothing to peers, and re-indexing or a re-announce brings the
item straight back.

**Mitigations:**

Local blocks live in the `blocks` table (migration `005_blocks`) and are
enforced on every path that touches a blocked subject:

- **Resource** — filtered in `av_index::search::search_top_k`, so it cannot
  appear in local results *or* be returned by the `av listen` responder. The
  responder additionally re-filters immediately before publishing
  (`to_publishable_resources`), because a resource blocked with `--keep-local`
  still has its embedding and would otherwise be served.
- **Name record** — filtered in `av_index::naming::lookup_name`, plus the same
  pre-send check for name responses (`to_publishable_names`).
- **Agent** — excluded from search/resolve fan-out candidate lists, and
  responses from a blocked agent are dropped on arrival, before `peer_cache`
  updates and before `--stream` output. Identity is taken from the
  transport-authenticated origin (`DirectMessage::sender` /
  `IncomingEvent::origin`), not the envelope's self-reported `from_agent_id`.

**Deliberate limits of the design:**

- Blocks are **local**. They are not gossiped and no peer learns about them, so
  blocking a resource here does not stop another node serving it.
- Blocking a resource **never infers a block on the agent that returned it**,
  and vice versa. The kinds are independent, because `resources` has no
  provenance column: anta-vista cannot know who sent a given resource, and
  guessing would let one bad result condemn an otherwise useful peer. Block an
  agent explicitly, and a network result's originating peers are shown so it can
  be done from the search UI.
- A name block is per **record id**, not per name. Names resolve by exact
  normalized match, so blocking a name would also hide an innocent peer's
  conflicting claim for the same name.
- Blocks are **permanent** — no expiry. They survive `av purge --all`, because
  purge clears data and a block is policy; use `av unblock --all` to reset.
- `av block --resource` **deletes the local copy** by default (cascading to
  `embeddings`, `feedback_events` and `relevance_judgments`). `--keep-local`
  blocks without deleting. Unblocking lifts the policy but cannot restore
  deleted data — the resource has to be re-indexed.
- Blocking is **not retroactive for already-delivered data** and does not
  retract anything already sent. It is a local filter, not a recall.
- The x0x transport block (`--x0x`) is **opt-in and best-effort**: it needs the
  daemon, and it drops *all* traffic from that agent on every topic, not just
  anta-vista results. The local block always lands; a failed escalation is
  reported as a warning rather than treated as success. `av unblock --all` does
  not lift transport blocks, because it cannot enumerate x0x contacts — use
  `av unblock --agent <id> --x0x`.

**Residual risk:** a blocked resource is still reachable by its URI through the
Autonomi network, and any node that never heard of the block can still serve it.
Blocking suppresses this node's behaviour; it is not a takedown.

---

## Out of scope (Phase 1)

- Encrypted payloads (planned via x0x group encryption in a later phase)
- Key revocation
- Byzantine fault-tolerant consensus on name records

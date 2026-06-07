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
- Nodes may maintain a local blocklist of known-bad `by_agent_id` values.

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

---

## Out of scope (Phase 1)

- Encrypted payloads (planned via x0x group encryption in a later phase)
- Key revocation
- Byzantine fault-tolerant consensus on name records

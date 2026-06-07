# anta-vista Ranking Formula

## Semantic Search Ranking

Each candidate result is scored as a weighted sum of four signals:

```
score = W_semantic  * cosine_similarity(query_vec, resource_vec)
      + W_agreement * agreement_score(resource_id)
      + W_feedback  * feedback_score(resource_id)
      + W_trust     * trust_score(by_agent_id)
```

| Signal            | Weight (`W`)        | Constant               |
|-------------------|---------------------|------------------------|
| Semantic similarity | 0.65              | `WEIGHT_SEMANTIC`      |
| Peer agreement    | 0.15                | `WEIGHT_AGREEMENT`     |
| User feedback     | 0.10                | `WEIGHT_FEEDBACK`      |
| Agent trust       | 0.10                | `WEIGHT_TRUST`         |

All weights sum to **1.0**.

### Signal definitions

- **Cosine similarity** — dot product of L2-normalised embedding vectors,
  computed in `av-index`. Requires both vectors to share the same
  `EmbeddingProfile`.
- **Agreement score** — fraction of peers that have corroborating `Claim`
  records for this resource over the last rolling window (e.g. 24 h).
- **Feedback score** — normalised positive/negative feedback ratio from
  `FeedbackEvent` records attributed to this resource. `Useful` and
  `HighConfidence` count positively; `NotUseful` and `Incorrect` negatively.
- **Trust score** — the publisher agent's `TrustState.trust_score` at query
  time, clamped to [0, 1].

## Name Resolution Ranking

When multiple `NameRecord` entries resolve the same `normalized_name`, the
winning record is selected by:

```
name_score = W_trust    * trust_score(by_agent_id)
           + W_agreement * agreement_score(record_id)
           + W_recency   * recency_score(timestamp)
           + W_ttl       * ttl_weight(ttl_secs)
```

| Signal      | Weight | Constant                |
|-------------|--------|-------------------------|
| Trust       | 0.50   | `NAME_WEIGHT_TRUST`     |
| Agreement   | 0.30   | `NAME_WEIGHT_AGREEMENT` |
| Recency     | 0.10   | `NAME_WEIGHT_RECENCY`   |
| TTL         | 0.10   | `NAME_WEIGHT_TTL`       |

- **Recency** — linearly decays with age; the most recent record scores 1.0.
- **TTL weight** — higher TTL signals more permanent / intentional records.

## Trust Update Principles

1. Trust scores are maintained in `TrustState` and updated by `av-trust`.
2. Each new piece of corroborating evidence (matching claim from a third party)
   increases the score; contradictory evidence decreases it.
3. Scores are bounded to [0.0, 1.0] and never set directly by remote nodes —
   only derived locally from observed evidence.
4. `evidence_count` is tracked for auditability; a score derived from fewer
   than a minimum threshold (TBD, suggested: 3) should be treated as
   provisional.

# anta-vista Wire Protocol

## Overview

All messages exchanged over x0x gossip channels are wrapped in a
`MessageEnvelope`. The envelope carries routing metadata and a typed payload so
that any node can decode the header without understanding every payload variant.

## MessageEnvelope

```json
{
  "schema_version": 1,
  "message_id": "<uuid-v4>",
  "sent_at": 1700000000,
  "from_agent_id": "<public-key-hex>",
  "kind": "query",
  "payload": { ... }
}
```

| Field           | Type              | Notes                                              |
|-----------------|-------------------|----------------------------------------------------|
| `schema_version`| `u16`             | Must equal `SCHEMA_VERSION` (currently `1`).       |
| `message_id`    | `String`          | UUID v4; used for deduplication.                   |
| `sent_at`       | `i64`             | Unix timestamp (seconds since epoch).              |
| `from_agent_id` | `String`          | Sender's public key / node identifier.             |
| `kind`          | `MessageKind`     | Determines how `payload` is parsed.                |
| `payload`       | `serde_json::Value` | Opaque JSON; schema determined by `kind`.        |

## MessageKind variants

| Variant         | Serialised form   | Topic constant           |
|-----------------|-------------------|--------------------------|
| `Query`         | `"query"`         | `av.query.v1`            |
| `Response`      | `"response"`      | `av.response.v1`         |
| `Claim`         | `"claim"`         | `av.claim.v1`            |
| `Feedback`      | `"feedback"`      | `av.feedback.v1`         |
| `NameQuery`     | `"name_query"`    | `av.name.query.v1`       |
| `NameResponse`  | `"name_response"` | `av.name.response.v1`    |
| `NameClaim`     | `"name_claim"`    | `av.name.claim.v1`       |
| `Presence`      | `"presence"`      | `av.presence.v1`         |

## Topic names

Topic names follow the pattern `av.<domain>.v<N>`, where `N` is the schema
version. Nodes should ignore topics with an unsupported `N`.

## Maximum payload size

Payloads larger than `MAX_PAYLOAD_BYTES` (1 MiB) **must** be rejected by the
receiver before any further processing.

## Security requirements

1. **Signatures** — Every `Claim`, `FeedbackEvent`, and `NameRecord` carries a
   `signature` field over the canonical serialisation of the struct (all fields
   except `signature` itself, sorted by field name, encoded as canonical JSON).

2. **Identity** — `from_agent_id` is a public key. The receiving node must
   verify the envelope signature against this key before processing.

3. **Replay protection** — Nodes maintain a seen-cache of recent `message_id`
   values and drop duplicates. Cache entries expire after 5 minutes.

4. **Rate limiting** — Default limit is `DEFAULT_RATE_LIMIT_PER_MINUTE` (60)
   messages per `from_agent_id` per minute.

5. **Schema version gating** — Nodes must reject envelopes whose
   `schema_version` they do not support, returning an appropriate error.

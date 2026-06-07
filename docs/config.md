# Configuration Reference

anta-vista is configured via a TOML file. All sections and fields are optional
and fall back to the documented defaults.

Load with:

```rust
use av_core::config::AvConfig;
let config = AvConfig::from_file(std::path::Path::new("config.toml"))?;
config.validate()?; // checks ranking weights sum to 1.0
```

---

## `[embedding]`

| Field | Default | Description |
|-------|---------|-------------|
| `model_id` | `"all-MiniLM-L6-v2"` | Model identifier — must match pinned runtime |
| `model_version` | `"v1"` | Pinned model version |
| `preproc_version` | `"v1"` | Tokenizer/preprocessing contract version |
| `normalized` | `true` | Whether vectors are L2-normalised before storage |

---

## `[ranking]`

Weights must sum to exactly 1.0 (enforced by `validate()`).

| Field | Default | Description |
|-------|---------|-------------|
| `semantic_weight` | `0.65` | Cosine similarity component |
| `agreement_weight` | `0.15` | Multi-agent agreement component |
| `feedback_weight` | `0.10` | Explicit user feedback component |
| `trust_weight` | `0.10` | Agent trust score component |

---

## `[network]`

| Field | Default | Description |
|-------|---------|-------------|
| `query_timeout_ms` | `1200` | Advisory timeout included in query payloads |
| `max_payload_bytes` | `65536` | Maximum accepted envelope size (bytes) |
| `max_messages_per_minute_per_agent` | `120` | Rate limit (used by `PayloadGuard`) |

---

## `[uri]`

| Field | Default | Description |
|-------|---------|-------------|
| `allowed_schemes` | `[]` | URI schemes to accept (empty = allow all) |
| `blocked_schemes` | `[]` | URI schemes to always reject |
| `scheme_aliases` | `{autonomi="ant"}` | Alias mapping for URI scheme normalisation |

---

## `[trust]`

| Field | Default | Description |
|-------|---------|-------------|
| `decay_per_day` | `0.01` | Fractional trust decay per day (toward neutral 0.0) |
| `block_threshold` | `-0.8` | Trust score below this is treated as effectively blocked |

---

## Example: full config file

```toml
[embedding]
model_id = "all-MiniLM-L6-v2"
model_version = "v1"
preproc_version = "v1"
normalized = true

[ranking]
semantic_weight  = 0.65
agreement_weight = 0.15
feedback_weight  = 0.10
trust_weight     = 0.10

[network]
query_timeout_ms = 1200
max_payload_bytes = 65536
max_messages_per_minute_per_agent = 120

[uri]
allowed_schemes = []
blocked_schemes = []
scheme_aliases = { autonomi = "ant" }

[trust]
decay_per_day   = 0.01
block_threshold = -0.8
```

Partial configs are fine — omit any field to use its default.

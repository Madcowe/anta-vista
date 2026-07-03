# anta-vista

**Distributed semantic search + DNS-like naming for AI agents** — built on [x0x](https://github.com/saorsa-labs/x0x).

Agents share embeddings, claims, and feedback over an encrypted peer-to-peer network. Truth and ranking emerge from agreement, usage, and trust over time — no central index, no authority.

---

## Install

### Linux (pre-built binary)

```bash
curl -sfL https://github.com/Madcowe/anta-vista/releases/latest/download/av -o av
chmod +x av
sudo mv av /usr/local/bin/
```

You may need to run `chmod +x` to set execute permissions.

### Windows (pre-built binary)

```powershell
Invoke-WebRequest -Uri https://github.com/Madcowe/anta-vista/releases/latest/download/av.exe -OutFile av.exe
# Move av.exe to a directory in your PATH
```

### macOS

```bash
cargo build --release -p av-cli
cp target/release/av ~/.local/bin/
```

### Quick check

```bash
av status
```

## CLI usage

| Command | Description |
|---------|-------------|
| `av status` | Show daemon and model health |
| `av search <query>` | Semantic search for resources |
| `av resolve <name>` | DNS-like name → URI resolution |
| `av name <uri> <name>` | Register a name mapping |
| `av index <uri>` | Ingest and index a URI |
| `av propagate <id> <loc> <desc>` | Re-index a resource locally |
| `av rate <id> <rating>` | Submit feedback rating |
| `av purge` | Clear local database entries |
| `av listen` | Respond to network queries |

### Workflow

```bash
# Index content hosted on Autonomi
av index autonomi://<64-hex-address>

# Search your local index
av search "my query"

# Register a name pointing to a resource
av name autonomi://<64-hex-address> my-resource-name

# Resolve a name to its target URI
av resolve my-resource-name
```

### Common flags

`--non-interactive` — JSON output (machine mode)  
`--config <path>` — path to config.toml  
`--timeout <ms>` — network response timeout  
`--stream` — show results progressively  
`-v` / `-vv` — increase log verbosity

---

## What it does

- **Semantic search** — index any content by description; query with natural language; rank by cosine similarity + trust + feedback + agreement
- **DNS-like naming** — resolve human-readable names (`alice.app`, `my-service`) to URIs; conflicting claims are ranked, not arbitrated
- **Emergent trust** — agent reputation grows from agreement and useful feedback; decays over time; never hard-blocks without evidence
- **x0x transport** — gossip broadcast for discovery; direct messaging for established relationships; post-quantum encrypted, NAT-traversing

---

## Architecture

```
Your machine
─────────────────────────────────────────────────────────
  av-ingest   →  MIME + description synthesis
  av-embed    →  all-MiniLM-L6-v2 (384-d, CPU-only)
  av-store    →  SQLite (WAL, bundled, cross-platform)
  av-index    →  brute-force cosine search + name lookup
  av-trust    →  agreement · feedback · trust · decay
  av-query    →  cold-start clustering · rate limiting
  av-net-x0x  →  x0x gossip + direct messaging
─────────────────────────────────────────────────────────
```

## Workspace crates

| Crate | Description |
|-------|-------------|
| `av-core` | Domain types, config, constants, path helpers |
| `av-store` | SQLite repositories and migrations |
| `av-ingest` | MIME detection, metadata, description synthesis |
| `av-embed` | `EmbeddingProvider` trait, MiniLM adapter, mock provider |
| `av-index` | Local vector search and exact name resolution |
| `av-trust` | Trust updates, feedback aggregation, ranking formula |
| `av-query` | Cold-start clustering, rate limiting, abuse tracking |
| `av-net-x0x` | x0x network transport (gossip + direct) |
| `av-cli` | CLI binary (`av`) with all subcommands |
| `av-probe` | Standalone test tool for multi-machine integration testing |

---

## Quick start — local search (no network required)

```bash
git clone <this-repo>
cd anta-vista
cargo run --example local_search -p anta-vista-examples
```

Output shows:
- MIME detection and description synthesis for each document
- Top-3 semantic matches per query with score breakdown
- Name resolution for a registered `autonomi://` record

Uses `MockEmbeddingProvider` by default — no model download, runs instantly.

### Using the real MiniLM model

The real `sentence-transformers/all-MiniLM-L6-v2` model downloads on first use (~22 MB).
Swap `MockEmbeddingProvider` for `MiniLmProvider` in the example:

```rust
use av_embed::MiniLmProvider;
let provider = MiniLmProvider::new().expect("load model");
```

---

## Quick start — P2P demo (requires x0x)

### 1. Install x0x

```bash
# Linux x86_64
curl -sfL https://github.com/saorsa-labs/x0x/releases/latest/download/x0x-linux-x64-gnu.tar.gz \
  | tar xz
cp x0x-linux-x64-gnu/x0xd ~/.local/bin/
cp x0x-linux-x64-gnu/x0x  ~/.local/bin/
```

### 2. Start a daemon

```bash
x0x start
x0x health   # verify it's running
```

### 3. Run the P2P example

```bash
cargo run --example p2p_two_nodes -p anta-vista-examples
```

The example broadcasts a search query and a name claim over the x0x gossip network.
Any other anta-vista node subscribed to `av.query.v1` and `av.name.claim.v1` will receive them.
If no daemon is running the example exits gracefully with setup instructions.

---

## Configuration

Create `config.toml` anywhere and load it with `AvConfig::from_file(path)`:

```toml
[embedding]
model_id = "all-MiniLM-L6-v2"
model_version = "v1"
normalized = true

[ranking]
semantic_weight  = 0.55   # must sum to 1.0
agreement_weight = 0.15
feedback_weight  = 0.10
trust_weight     = 0.10
relevance_weight = 0.10

[network]
query_timeout_ms = 1200
max_payload_bytes = 65536
max_messages_per_minute_per_agent = 120

[uri]
allowed_schemes = []          # empty = allow all

[trust]
decay_per_day   = 0.01
block_threshold = -0.8
```

All fields have defaults — partial configs work fine. See [`docs/config.md`](docs/config.md) for the full reference.

---

## Distributed integration testing with av-probe

`av-probe` is a diagnostic tool for validating `anta-vista` networks. It runs 13 tests across transport, naming, search, and trust categories.

### Build

```bash
cargo build --release -p av-probe
```

### Multi-machine mode

Run the seed and probe on separate machines, each with their own x0x daemon running.

```bash
# Machine A: Start the seed node
target/release/av-probe --role seed --real-model

# Machine B: Auto-detect via gossip broadcast
target/release/av-probe --role probe --wait 30

# Or specify the seed Agent ID directly
target/release/av-probe --role probe --peer <SEED_AGENT_ID>
```

---

## Running tests

```bash
cargo test --workspace                       # all unit + integration tests
cargo test --workspace -- --include-ignored  # + real MiniLM model tests (needs internet)
```

---

## Known limitations

See [`docs/known-limitations.md`](docs/known-limitations.md) for the full list. Key points:

- **No multimodal embeddings** — images/audio are described via filename and metadata, not content
- **Brute-force search** — no ANN index yet; scales to ~100k documents before needing optimisation
- **No sybil resistance** — identity cost is lightweight; anti-abuse relies on trust decay
- **Mock embeddings in examples** — real semantic quality requires the MiniLM model download

---

## License

MIT

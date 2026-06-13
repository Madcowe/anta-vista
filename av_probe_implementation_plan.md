# Multi-Machine Test Application for anta-vista (Completed)

Build a standalone, cross-compilable test binary (`av-probe`) that comprehensively tests all anta-vista communication avenues across 2–3 physical machines, without requiring Rust or the full repository on each machine.

---

## 1. Product Scope

### In Scope
- Single standalone executable binary (`av-probe`) targeted for **x86_64 Linux**. [COMPLETED]
- Support for two roles: `--role seed` (starts up, registers name claims, indexes content, and listens) and `--role probe` (queries the seed). [COMPLETED]
- **Seed Autodetection**: The `--peer <agent_id>` argument is **optional**. If omitted, the probe node automatically listens to the gossip network (e.g. name claims or presence announcements) and auto-locks onto the first seed node it hears from. [COMPLETED]
- **Naming tests**: DNS-like name claim, name query and resolution, case-insensitivity, and scheme normalization (mapping `autonomi://` to `ant://`). [COMPLETED]
- **Search tests**: Semantic search queries over mock or real embedding providers. [COMPLETED]
- **Transport validation**: Gossip topics (`av.query.v1`, `av.response.v1`, etc.) and direct messaging via x0xd REST APIs (`/publish`, `/direct/send`, `/direct/events`). [COMPLETED]
- **Trust levels**: Manual trust stage validation (prompting the user to alter peer trust). [COMPLETED]
- **Outputs**: Output raw JSON-lines *plus* a final human-readable Markdown summary report. [COMPLETED]

### Out of Scope
- Running multiple `x0x` daemons on the same machine (not supported by x0x).
- Auto-changing trust settings via the REST API during trust-level tests (delegated to manual prompts for strict agent safety).

---

## 2. Updated Decisions & Constraints

### 2.1 Interactive Trust Prompts
When a trust-level test scenario is triggered, the program will print a clear, high-visibility block to the console containing the exact, copy-pasteable command.
*Example output:*
```
┌──────────────────────────────────────────────────────────┐
│ TRUST LEVEL CHANGE REQUIRED                              │
│                                                          │
│ Please run the following command on this machine:        │
│                                                          │
│   x0x trust set 8a3f8902c...dd71 trusted                 │
│                                                          │
│ Press ENTER once executed to resume the test...          │
└──────────────────────────────────────────────────────────┘
```

### 2.2 Seed Autodetection (Optional `--peer`)
- If the probe is run simply as `./av-probe --role probe` (without `--peer`), it will subscribe to the gossip topic `av.name.claim.v1` and wait.
- When the seed starts, it broadcasts its name claims. The probe captures the incoming name claim envelope, extracts the `from_agent_id`, prints `[INFO] Autodetected seed peer agent ID: 8a3f89...`, and automatically initiates the test suite targeting that peer.
- The user can still override this behavior by explicitly passing `--peer <agent_id>`.

### 2.3 Embedding Models (`--real-model` flag)
- By default, `av-probe` uses the `MockEmbeddingProvider` to avoid downloading models on test machines.
- Adding a `--real-model` CLI flag. When provided, the application will initialize `MiniLmProvider` which auto-downloads and runs the real `sentence-transformers/all-MiniLM-L6-v2` model.

### 2.4 Double Output Format
`av-probe` will support printing:
1. **JSON-lines (Default)**: Each test execution logs a single-line JSON string representing the test outcome. This is easily aggregatable by analysis scripts or coding agents.
2. **Summary Report**: Once the test run concludes, the program outputs a clean Markdown Table summary outlining passed/failed tests, durations, and status codes.

---

## 3. Crate Architecture: `av-probe`

A new crate `crates/av-probe` added to the Cargo workspace.

```text
crates/av-probe/
  Cargo.toml
  src/
    main.rs         # Entry point, CLI args parsing, dispatch
    cli.rs          # Argument definitions
    output.rs       # JSON-lines + Markdown Summary formatting
    seed.rs         # Seed node logic (claim, index, listen, respond)
    probe.rs        # Probe node logic (execute query/search suites, peer detection)
    tests/
      mod.rs        # Test runner interface
      naming.rs     # DNS / Name resolution tests
      search.rs     # Semantic search tests
      transport.rs  # Gossip vs Direct test scenarios
      trust.rs      # Interactive trust scenarios
      helpers.rs    # SSE polling, CLI prompts, rest helpers
```

### Dependency list
```toml
[package]
name = "av-probe"
version = "0.1.0"
edition = "2024"

[dependencies]
av-core = { path = "../av-core" }
av-store = { path = "../av-store" }
av-embed = { path = "../av-embed" }
av-ingest = { path = "../av-ingest" }
av-index = { path = "../av-index" }
av-net-x0x = { path = "../av-net-x0x" }
av-trust = { path = "../av-trust" }
av-query = { path = "../av-query" }

clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }
ureq = "2"
ctrlc = "3"
```

---

## 4. Test Scenarios

### 4.1 Naming Suite (`naming.rs`)
- **`gossip_name_claim`**: Seed publishes a name claim for `seed.av`. Probe listens on `av.name.claim.v1` and verifies the record arrives. Also used for peer autodetection.
- **`name_query_response`**: Probe broadcasts a query on `av.name.query.v1`; seed replies on `av.name.response.v1`.
- **`case_insensitive`**: Verify name resolution of `SeEd.Av` correctly matches `seed.av`.
- **`scheme_alias`**: Verify location URI translation from `autonomi://<address>` to canonical `ant://<address>`.

### 4.2 Search Suite (`search.rs`)
- **`gossip_search`**: Probe sends query payload via gossip, seed computes mock/real cosine similarity on local resources, responds with matches.
- **`direct_search`**: Probe establishes direct connection to seed's agent ID, transmits direct query, receives private response.
- **`scheme_filtering`**: Search query requests only `ant` scheme. Result contains `ant` schemes only; `https` matches are omitted.

### 4.3 Transport Suite (`transport.rs`)
- **`gossip_delivery`**: Verifies general pub/sub packet flow and schema validation.
- **`direct_delivery`**: Verifies bidirectional direct messages via `x0xd`.
- **`deduplication`**: Verifies that duplicate `message_id` payloads are ignored.

### 4.4 Trust Suite (`trust.rs`)
- **`unknown_trust`**: Baseline performance; no trust weights applied.
- **`known_trust`**: Probe prompts user to trust the seed using:
  `x0x trust set <seed_id> known`
  Verify score adjustments.
- **`blocked_trust`**: Probe prompts user to block the seed using:
  `x0x trust set <seed_id> blocked`
  Verify gossip packets and direct messages are drop-filtered.

---

## 5. Execution & Verification Flow

### 5.1 Real Multi-Machine Setup (2 Nodes)

#### 1. Build Target
Run local compilation for x86_64 Linux:
```bash
cargo build --release -p av-probe
# Compiled binary located at: target/release/av-probe
```

#### 2. Seed Node Execution (Machine A)
Copy the executable to Machine A and run:
```bash
./av-probe --role seed
```
*Console output:*
```
[INFO] Connected to x0x daemon (Agent ID: 8a3f8902c67de...)
[INFO] Subscribed to av.* topics.
[INFO] Indexed 5 sample resources.
[INFO] Ready. Waiting for incoming queries...
```

#### 3. Probe Node Execution (Machine B - with Autodetection)
Copy the executable to Machine B and run:
```bash
./av-probe --role probe
```
*Console output during autodetection:*
```
[INFO] Listening for seed node announcement...
[INFO] Autodetected seed peer agent ID: 8a3f8902c67de...
[INFO] Commencing test suite...
```
*The probe will run the test suite and output progress sequentially.*

### 5.2 Verification Output (Markdown Summary example)

When the run finishes on the Probe, the summary will be written:

```markdown
# Anta-Vista Test Suite Summary
**Date**: 2026-06-11
**Node ID**: 9f8e7d...
**Peer ID**: 8a3f89...

| Test ID | Category | Name | Transport | Status | Duration |
|---|---|---|---|---|---|
| N1 | Naming | gossip_name_claim | gossip | PASS | 120ms |
| N2 | Naming | name_query_response | gossip | PASS | 450ms |
| N3 | Naming | case_insensitive | gossip | PASS | 80ms |
| S1 | Search | gossip_search | gossip | PASS | 620ms |
| S2 | Search | direct_search | direct | PASS | 310ms |
| T3 | Trust  | blocked_trust | gossip | PASS | 1200ms |

**Execution Result**: 6/6 Passed.
```

---

## 6. Implementation Stages for Agent

1. **Phase 1: Workspace setup**: Create `crates/av-probe`, update Cargo workspace manifests with Rust `edition = "2024"`. [COMPLETED]
2. **Phase 2: CLI & Output Formatting**: Implement CLI parsing (including `--real-model` and format switching) and the output summary generators. [COMPLETED]
3. **Phase 3: Core Test Engine**: Implement test executors, SSE stream buffers, and direct event dispatch wrappers. [COMPLETED]
4. **Phase 4: Test Suites & Autodetection**: Implement autodetection loop, `naming.rs`, `search.rs`, `transport.rs`, and `trust.rs` (with interactive user-prompts). [COMPLETED]
5. **Phase 5: Offline/Mock validation**: Create integration tests using `MockNetClient` to ensure the logic runs correctly within local mock boundaries before physical deployment. [COMPLETED]

# Anta-Vista — Agent Guide

## Workspace
- Rust workspace, `resolver = "2"`, **no toolchain file** — CI uses `stable` via `dtolnay/rust-toolchain`. All crates `edition = "2024"`, version `0.2.0`.
- No `.cargo/config`, no `rustfmt.toml`, no `clippy.toml`.
- **Binary**: `av-cli` publishes the `av` binary (`crates/av-cli/src/main.rs`). All other crates are libraries.
- **Crate dependency graph** (bottom-up, a fan not a chain):
  `av-core` → `av-store`, `av-embed`, `av-ingest`, `av-net-x0x` → `av-trust`, `av-index` → `av-query` → `av-cli`, `av-probe`, `av-test-suite`.
  Note: `av-query` depends only on `av-core`/`av-store`/`av-trust`/`av-net-x0x` — it **bypasses** `av-index`/`av-embed`. Do not add `av-query → av-index` deps without reason.
- `ANTA_VISTA_DB_PATH` env var overrides the default SQLite db path (derived from `directories::ProjectDirs` via `av_core::paths::db_path`).

## URI scheme (easy to get wrong)
- Canonical scheme is **`autonomi://`** (flipped from `ant://` in commit `408e97c`, with DB migration `004_canonical_scheme`).
- `ant://` is accepted as an **input alias** only — `av_core::constants::{ANT_SCHEME, AUTONOMI_SCHEME}` and `normalize_scheme()` map `ant` → `autonomi`; indexing/ranking normalize stored targets to `autonomi://`.
- Bare 64-hex strings (no scheme) are auto-prefixed with `autonomi://` (in `av-ingest/src/location.rs` `normalize_uri`).
- New URI-handling code should treat `autonomi` as canonical and normalize aliases early via `av_core` helpers.

## Commands
```sh
cargo check --workspace                   # CI check step
cargo test --workspace                    # CI test step (Linux + macOS)
cargo test --workspace -- --include-ignored  # include real MiniLM + x0x live-daemon tests
cargo test --workspace --exclude av-net-x0x  # Windows CI: skip x0x-dependent tests
cargo build --release -p av-probe          # distributed test tool
cargo run --example local_search -p anta-vista-examples    # local demo (mock embeddings)
cargo run --example p2p_two_nodes -p anta-vista-examples   # P2P demo (needs x0x daemon)
```

## Testing quirks
- **x0x live-daemon tests** (`av-test-suite/tests/x0x_live_daemon.rs`) are `#[ignore]` **and** body-guarded by `skip_if_no_daemon()`. The guard is *discovery-based*, not a health probe: it checks the x0x data dir (`ProjectDirs::from("", "", "x0x")`) for readable `api.port` + `api-token` files. Runs only with `--include-ignored`.
- `skip_if_no_daemon()` / `inject_gossip_payload()` / `spawn_named_instance()` live in `av-test-suite/src/x0x_harness.rs`.
- `av-test-suite` has no `[[test]]` entries in `Cargo.toml` but **does** ship integration tests via auto-discovery (`tests/av_*.rs`, `integration_*.rs`) alongside its shared helpers (`fixtures`, `generators`, `attacks`, `prelude`). Both run under `cargo test -p av-test-suite`.
- Real MiniLM model tests (`--include-ignored`) download ~22 MB on first run via `fastembed`/ORT → `.fastembed_cache/`.
- `proptest` is a workspace dependency — property-based tests exist but are not CI-gated separately.

## CLI
```sh
av <subcommand> [options]
```
Subcommands: `status`, `resolve <name>`, `search <query>`, `name <uri> <name>`, `index <uri>`, `rate <resource_id> <rating>`, `purge`, `listen`, `propagate <resource_id> <location> <description>`.
Flags: `--non-interactive` (JSON machine mode; may appear before or after subcommand), `--config <path>`, `--timeout <ms>` (default 10000), `--stream`, `-v`/`-vv`.

- **Startup gating** (`crates/av-cli/src/startup.rs`): `status`, `purge`, `listen` run offline; `resolve`/`search`/`name`/`index`/`rate` require an x0x daemon. In interactive mode the CLI offers to install/start x0x, and `name`/`index` offer the ant CLI + `antd` daemon (port 8082) for `ant://` URIs — keep this flow in mind when touching CLI.
- Machine-mode (`--non-interactive`) exits with JSON errors instead of prompting; missing deps are hard errors there.
- **Search/resolve fan-out** (`crates/av-cli/src/network.rs`): queries go out BOTH as direct messages to recent peers (`peers::list_recent`) AND as a gossip broadcast. Direct responses now match by any issued `query_id` (each direct query mints its own id, distinct from the gossip id) — a past bug dropped direct replies. Direct messaging is the fast path; gossip is mesh-driven and slower.

## Config
- TOML via `AvConfig::from_file(path)`; all fields optional; `validate()` checks ranking weights sum to 1.0 (tolerance 0.01).
- **Two independent weight schemes — don't confuse them.** `AvConfig` defaults (`av-core/src/config.rs`): semantic=0.65, agreement=0.15, feedback=0.10, trust=0.10 (validated). But `av-core/src/constants.rs` has a *different* 5-weight set (semantic=0.55, relevance=0.10, agreement=0.15, feedback=0.10, trust=0.10) used by `av-trust::ranking`. Also name-ranking constants (`NAME_WEIGHT_*`). The constants set is not validated against config.

## Key dependencies & quirks
- `fastembed` v4 with features `["ort-download-binaries", "hf-hub-rustls-tls"]` — declared inline in `av-embed`, not a workspace dep.
- `rusqlite` with `bundled` feature everywhere — WAL mode + foreign keys enabled per `IMPLEMENTATION_PLAN.md`. Declared inline in `av-trust`/`av-query` despite being a workspace dep; depends on same spec.
- `ureq` (sync HTTP) — no tokio or async runtime in the project.
- `infer` for MIME detection — `kamadak-exif` for image metadata, `id3` for audio (`av-ingest`).
- `directories` for platform data paths — defined in `av_core::paths`.

## Protocol
- Wire topics: `av.query.v1`, `av.response.v1`, `av.claim.v1`, `av.feedback.v1`, `av.name.query.v1`, `av.name.response.v1`, `av.name.claim.v1`, `av.presence.v1`
- `MessageEnvelope` wraps all payloads with `schema_version`, `message_id` (UUIDv4), `sent_at`, `from_agent_id`, `kind`, `payload`
- Name resolution: **exact match on `normalized_name`** (Unicode NFC + lowercase). No fuzzy/semantic matching in naming mode.
- See `docs/protocol.md`, `docs/ranking.md`, `docs/threat-model.md`, and root `SKILL.md` for deeper reference.

## Network tests
- Require x0x daemon installed: `https://github.com/saorsa-labs/x0x` — run `x0x start` then `x0x health`.
- `av-probe` multi-machine testing: `--role seed` / `--role probe`, plus `--peer <agent_id>`, `--wait <secs>`, `--test <name>`, `--real-model`.
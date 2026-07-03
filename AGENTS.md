# Anta-Vista — Agent Guide

## Workspace
- Rust workspace, `resolver = "2"`, **no toolchain file** — CI uses `stable` via `dtolnay/rust-toolchain`
- **Binary**: `av-cli` publishes the `av` binary (`crates/av-cli/src/main.rs`). All other crates are libraries.
- All crates use `edition = "2024"`.
- No `.cargo/config`, no `rustfmt.toml`, no `clippy.toml`.
- **Crate dependency order** (bottom-up): `av-core` → `av-store`, `av-embed`, `av-ingest`, `av-net-x0x` → `av-trust`, `av-index` → `av-query` → `av-cli`, `av-probe`, `av-test-suite`.
- `ANTA_VISTA_DB_PATH` env var overrides the default SQLite db path (derived from `directories::ProjectDirs` via `av_core::paths::db_path`).

## Commands
```sh
cargo check --workspace                   # CI check step
cargo test --workspace                    # CI test step (Linux + macOS)
cargo test --workspace -- --include-ignored  # include real MiniLM model tests (needs internet)
cargo test --workspace --exclude av-net-x0x  # Windows CI: skip x0x-dependent tests
cargo build --release -p av-probe          # distributed test tool
cargo run --example local_search -p anta-vista-examples    # local demo (mock embeddings)
cargo run --example p2p_two_nodes -p anta-vista-examples   # P2P demo (needs x0x daemon)
```

## Testing quirks
- **x0x-dependent tests** are guarded by `skip_if_no_daemon()` — they silently skip if `x0xd` is not running.
- `av-test-suite` is a **lib crate** (no `[[test]]` targets), providing shared test helpers (`fixtures`, `generators`, `attacks`, `x0x_harness`) used by other crates' integration tests.
- Real MiniLM model tests (`--include-ignored`) download ~22 MB on first run via `fastembed`/ORT.
- `proptest` is a workspace dependency — property-based tests exist but are not CI-gated separately.

## CLI
```sh
av <subcommand> [options]
```
Subcommands: `status`, `resolve <name>`, `search <query>`, `name <uri> <name>`, `index <uri>`, `rate <resource_id> <rating>`, `purge`, `listen`, `propagate <resource_id> <location> <description>`.
Flags: `--non-interactive` (machine mode; may appear before or after subcommand), `--config <path>`, `--timeout <ms>`, `--stream`, `-v`/`-vv`.

## Config
TOML file loaded via `AvConfig::from_file(path)`. All fields optional — `validate()` checks ranking weights sum to 1.0. Default ranking: `semantic=0.65, agreement=0.15, feedback=0.10, trust=0.10`.

## Key dependencies & quirks
- `fastembed` v4 with features `["ort-download-binaries", "hf-hub-rustls-tls"]` — model caches to `.fastembed_cache/`
- `rusqlite` with `bundled` feature everywhere — WAL mode + foreign keys enabled per `IMPLEMENTATION_PLAN.md`
- `ureq` (sync HTTP) — no tokio or async runtime in the project
- `infer` for MIME detection — `kamadak-exif` for image metadata, `id3` for audio
- `directories` for platform data paths — defined in `av_core::paths`

## Protocol
- Wire topics: `av.query.v1`, `av.response.v1`, `av.claim.v1`, `av.feedback.v1`, `av.name.query.v1`, `av.name.response.v1`, `av.name.claim.v1`, `av.presence.v1`
- `MessageEnvelope` wraps all payloads with `schema_version`, `message_id` (UUIDv4), `sent_at`, `from_agent_id`, `kind`, `payload`
- Name resolution: **exact match on `normalized_name`** (Unicode NFC + lowercase). No fuzzy/semantic matching in naming mode.

## Network tests
- Require x0x daemon installed: `https://github.com/saorsa-labs/x0x` — run `x0x start` then `x0x health`
- Multi-machine testing: `av-probe` with `--role seed` / `--role probe`

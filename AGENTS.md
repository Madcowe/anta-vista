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
Subcommands: `web`, `status`, `resolve <name>`, `search <query>`, `name <uri> <name>`, `index <uri>`, `rate <resource_id> <rating>`, `purge`, `listen`, `stop`, `propagate <resource_id> <location> <description>`.
Flags: `--non-interactive` (JSON machine mode; may appear before or after subcommand), `--config <path>`, `--timeout <ms>` (default 10000), `--stream`, `-v`/`-vv`.

- **Bare `av`** is now valid: `cli.command` is an `Option<Commands>`, and `None` launches the web interface. `is_offline_command(None)` is true, so bare `av` starts with no x0x installed. Machine mode must still refuse: bare `av --non-interactive` returns `{"ok":false,"error":"no_subcommand"}` with **exit 2** (asserted in `cli_tests.rs`).
- **Startup gating** (`crates/av-cli/src/startup.rs`): `status`, `purge`, `listen`, `stop` run offline; `resolve`/`search`/`name`/`index`/`rate` require an x0x daemon. In interactive mode the CLI offers to install/start x0x, and `name`/`index` offer the ant CLI + `antd` daemon (port 8082) for `ant://` URIs — keep this flow in mind when touching CLI.
- **Machine-mode** (`--non-interactive`) exits with JSON errors instead of prompting; missing deps are hard errors there.
- **Listener management** (`crates/av-cli/src/listener.rs`): `av listen` writes `<data_dir>/av-listener.pid` (protected by a `PidFileGuard` drop-guard in `cmd/listen.rs` so a crash/timeout rarely leaves a stale PID). `stop_all()` computes every running listener — `/proc` cmdline sweep for `av ... listen` (unix) ∪ PID-file PID — SIGTERM then SIGKILL after ~3s (`--force` skips the grace period), and clears the PID file. `ensure_running()` is called from startup for network commands and spawns a detached `av listen` from `current_exe` if `is_running()` is false; it only checks aliveness, so an old binary never triggers a respawn by itself — use `av stop` then the next network command to upgrade the responder.

## Web interface
`crates/av-cli/src/web/` + `crates/av-cli/assets/`, served in-process by `tiny_http`. Launched by `av web`, `av gui`, or bare interactive `av`.

- **Reuse the CLI, don't reimplement it.** `web/handlers.rs` calls `cmd::search::collect_results`, `cmd::search::reinforce`, `cmd::rate::record_feedback`, `cmd::rate::is_locally_indexed`, `cmd::index::index_resource` and `cmd::status::database_counts`. If a behaviour changes in `cmd/`, the browser must change with it — extend the shared function rather than adding a web-only copy. `web` deliberately does **not** depend on `av-index`/`av-embed` crates directly.
- **Startup state is a live snapshot, not a boot constant.** `AppState.startup` is an `RwLock<StartupState>`. Search decides whether to fan out to peers from `startup.x0x_config`, so starting a daemon from the setup banner is useless unless `refresh_network_deps()` runs. Every start/install handler calls it. Handlers take one read guard for the whole request rather than re-reading, so a mid-request change cannot alter the fan-out.
- **`miniLM` loads lazily** (`EmbeddingCache(OnceLock<Result<Arc<MiniLmProvider>, String>>)`) — binding the socket must not wait on a ~22 MB model. Success *and* failure are cached so a broken cache isn't retried per keystroke.
- **Per-request `av_store::open()`** is required (`Connection` isn't `Sync`). This is why `av-store`'s migration bookkeeping uses `INSERT OR IGNORE`: several threads can reach a fresh database at once, all see the migrations as pending, and all of the DDL is idempotent. `web/mod.rs` also pre-warms the database before binding so the race is not hit in practice. Covered by `test_concurrent_opens_of_a_fresh_database` in `av-store/tests/on_disk.rs`.
- **Shutdown is a poll, not `Server::unblock()`.** `tiny_http::Server::unblock()` releases exactly **one** waiting thread ("If there are several such threads, only one is unblocked"), so a 4-worker pool would hang on `--run-for`. Workers use `recv_timeout(200ms)` against an `AtomicBool` instead.
- **Host-header guard** rejects anything that isn't a loopback name, which is the DNS-rebinding defence for a loopback-bound server. Keep it in front of every route.
- **Test harness quirks** (`crates/av-cli/tests/web_tests.rs`): reads the child's stdout **byte at a time** and only parses port numbers out of *complete* lines — reading a byte at a time and testing after each one parses the half-arrived `"8787"` as `87`. stderr is drained on a background thread (an undrained pipe fills and blocks the child, and hides why a server died). `connect` retries. Tests must not assume the host lacks `x0x`/`ant`; the Autonomi tests assert the invariant that holds either way.
- **UI honesty rules.** Repeatable-but-true warnings ("peers do not consume feedback yet") are shown and offer "don't show again" (`anta-vista-muted-warnings` in `localStorage`, keyed by the server's `warning_id` — a *different* warning still gets through). Install commands come from the **server** (`ant_install_command()`) and travel with the error, so the page never hardcodes a POSIX command and shows the wrong one on Windows. Autonomi open/download controls stay disabled; `/api/download` returns 501 rather than pretending.
- **Known gaps to keep in mind:** search blocks (SSE is the follow-up); peers do not consume `av.feedback.v1`, `av.claim.v1` or `av.presence.v1` messages, so a broadcast rating changes nobody's ranking; `NameRecord`s cannot receive feedback because `feedback_events.resource_id` references `resources(id)`; `--kind watchlist` becomes `ResourceKind::Other("watchlist")` (unrelated to the web UI, but do not add a kind dropdown until fixed).

- **Search/resolve fan-out** (`crates/av-cli/src/network.rs`): queries go out BOTH as direct messages AND as a gossip broadcast. Direct fan-out candidates = recent `peer_cache` entries ∪ x0x-discovered agents (`/agents/discovered`, `/presence/online` via `X0xNetClient::discover_agent_ids`), deduped, self excluded — so peers never contacted before (and online-only peers) are still queried. Direct queries are sent from detached threads per candidate, each with its own `query_id` registered **before** the send, and `send_direct` permits slow relay/`gossip_inbox` routing (90 s HTTP timeout) — a past bug gated sends on a 2 s `connect_agent`, silently dropping every relay-routed peer. Responses match by any issued `query_id` (direct ids differ from the gossip id). Direct messaging is the fast path; gossip is mesh-driven and slower.

## Config
- TOML via `AvConfig::from_file(path)`; all fields optional; `validate()` checks ranking weights sum to 1.0 (tolerance 0.01).
- **Two independent weight schemes — don't confuse them.** `AvConfig` defaults (`av-core/src/config.rs`): semantic=0.65, agreement=0.15, feedback=0.10, trust=0.10 (validated). But `av-core/src/constants.rs` has a *different* 5-weight set (semantic=0.55, relevance=0.10, agreement=0.15, feedback=0.10, trust=0.10) used by `av-trust::ranking`. Also name-ranking constants (`NAME_WEIGHT_*`). The constants set is not validated against config.

## Key dependencies & quirks
- `fastembed` v4 with features `["ort-download-binaries", "hf-hub-rustls-tls"]` — declared inline in `av-embed`, not a workspace dep.
- `rusqlite` with `bundled` feature everywhere — WAL mode + foreign keys enabled per `IMPLEMENTATION_PLAN.md`. Declared inline in `av-trust`/`av-query` despite being a workspace dep; depends on same spec.
- `ureq` (sync HTTP) — no tokio or async runtime in the project. The web server is `tiny_http` on plain threads, not an async runtime either.
- `tiny_http` 0.12 — its `unblock()` releases only one waiting thread, so shutdown is polled instead (see Web interface).
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
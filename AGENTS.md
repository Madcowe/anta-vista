# Anta-Vista — Agent Guide

## Workspace
- Rust workspace, `resolver = "2"`, **no toolchain file** — build with `stable` (no `rust-toolchain` file pins a version). All crates `edition = "2024"`, version `0.2.0`.
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

## Git
- **Never run `git commit` unless the user asks for that commit in that message.** Finishing a task is not permission to commit. Approval to commit once is not standing approval — a later "remove X and Y" still means edit the working tree and stop, leaving `git status` dirty for review.
- Commit only when the request explicitly names committing ("commit this", "make a commit"), and never bundle unrelated work into a commit just because the tree is already staged.
- `git add`/`git rm` to inspect or stage is fine, but if staging was only for review, say so and leave it staged rather than committing.
- Never `push` without being asked. Both commits in the web-GUI line of work are local only.

## Commands
```sh
cargo check --workspace                   # type-check everything
cargo test --workspace                    # full suite (Linux + macOS)
cargo test --workspace -- --include-ignored  # include real MiniLM + x0x live-daemon tests
cargo test --workspace --exclude av-net-x0x  # Windows: skip x0x-dependent tests
cargo build --release -p av-probe          # distributed test tool
cargo run --example local_search -p anta-vista-examples    # local demo (mock embeddings)
cargo run --example p2p_two_nodes -p anta-vista-examples   # P2P demo (needs x0x daemon)
```

## Testing quirks
- **x0x live-daemon tests** (`av-test-suite/tests/x0x_live_daemon.rs`) are `#[ignore]` **and** body-guarded by `skip_if_no_daemon()`. The guard is *discovery-based*, not a health probe: it checks the x0x data dir (`ProjectDirs::from("", "", "x0x")`) for readable `api.port` + `api-token` files. Runs only with `--include-ignored`.
- `skip_if_no_daemon()` / `inject_gossip_payload()` / `spawn_named_instance()` live in `av-test-suite/src/x0x_harness.rs`.
- `av-test-suite` has no `[[test]]` entries in `Cargo.toml` but **does** ship integration tests via auto-discovery (`tests/av_*.rs`, `integration_*.rs`) alongside its shared helpers (`fixtures`, `generators`, `attacks`, `prelude`). Both run under `cargo test -p av-test-suite`.
- Real MiniLM model tests (`--include-ignored`) download ~87 MB on first run via `fastembed`/ORT, cached under the OS cache dir (`av_core::paths::cache_dir`); `FASTEMBED_CACHE_DIR` overrides it.
- `proptest` is a workspace dependency — property-based tests exist but are not run as a separate gate.

## CLI
```sh
av <subcommand> [options]
```
Subcommands: `web`, `status`, `resolve <name>`, `search <query>`, `name <uri> <name>`, `index <uri>`, `rate <resource_id> <rating>`, `block`, `unblock`, `purge`, `listen`, `stop`, `propagate <resource_id> <location> <description>`.
Flags: `--non-interactive` (JSON machine mode; may appear before or after subcommand), `--config <path>`, `--timeout <ms>` (default 10000), `--stream`, `-v`/`-vv`.

- **Bare `av`** is now valid: `cli.command` is an `Option<Commands>`, and `None` launches the web interface. `is_offline_command(None)` is true, so bare `av` starts with no x0x installed. Machine mode must still refuse: bare `av --non-interactive` returns `{"ok":false,"error":"no_subcommand"}` with **exit 2** (asserted in `cli_tests.rs`).
- **Startup gating** (`crates/av-cli/src/startup.rs`): `status`, `purge`, `listen`, `stop`, `block` and `unblock` run offline; `resolve`/`search`/`name`/`index`/`rate` require an x0x daemon. In interactive mode the CLI offers to install/start x0x, and `name`/`index` offer the ant CLI + `antd` daemon (port 8082) for `ant://` URIs — keep this flow in mind when touching CLI. `block`/`unblock` are offline because blocking a misbehaving peer has to work when the daemon is unavailable.
- **Machine-mode** (`--non-interactive`) exits with JSON errors instead of prompting; missing deps are hard errors there.
- **Listener management** (`crates/av-cli/src/listener.rs`): `av listen` writes `<data_dir>/av-listener.pid` (protected by a `PidFileGuard` drop-guard in `cmd/listen.rs` so a crash/timeout rarely leaves a stale PID). `stop_all()` computes every running listener — `/proc` cmdline sweep for `av ... listen` (unix) ∪ PID-file PID — SIGTERM then SIGKILL after ~3s (`--force` skips the grace period), and clears the PID file. `ensure_running()` is called from startup for network commands and spawns a detached `av listen` from `current_exe` if `is_running()` is false; it only checks aliveness, so an old binary never triggers a respawn by itself — use `av stop` then the next network command to upgrade the responder.

## Web interface
`crates/av-cli/src/web/` + `crates/av-cli/assets/`, served in-process by `tiny_http`. Launched by `av web`, `av gui`, or bare interactive `av`.

- **Reuse the CLI, don't reimplement it.** `web/handlers.rs` calls `cmd::search::collect_results`, `cmd::search::reinforce`, `cmd::rate::record_feedback`, `cmd::rate::is_locally_indexed`, `cmd::index::index_resource` and `cmd::status::database_counts`. If a behaviour changes in `cmd/`, the browser must change with it — extend the shared function rather than adding a web-only copy. `web` deliberately does **not** depend on `av-index`/`av-embed` crates directly.
- **Startup state is a live snapshot, not a boot constant.** `AppState.startup` is an `RwLock<StartupState>`. Search decides whether to fan out to peers from `startup.x0x_config`, so starting a daemon from the setup banner is useless unless `refresh_network_deps()` runs. Every start/install handler calls it. Handlers take one read guard for the whole request rather than re-reading, so a mid-request change cannot alter the fan-out.
- **`miniLM` loads lazily** (`EmbeddingCache(OnceLock<Result<Arc<MiniLmProvider>, String>>)`) — binding the socket must not wait on an ~87 MB model. Success *and* failure are cached so a broken cache isn't retried per keystroke.
- **Per-request `av_store::open()`** is required (`Connection` isn't `Sync`). This is why `av-store`'s migration bookkeeping uses `INSERT OR IGNORE`: several threads can reach a fresh database at once, all see the migrations as pending, and all of the DDL is idempotent. `web/mod.rs` also pre-warms the database before binding so the race is not hit in practice. Covered by `test_concurrent_opens_of_a_fresh_database` in `av-store/tests/on_disk.rs`.
- **Shutdown is a poll, not `Server::unblock()`.** `tiny_http::Server::unblock()` releases exactly **one** waiting thread ("If there are several such threads, only one is unblocked"), so a 4-worker pool would hang on `--run-for`. Workers use `recv_timeout(200ms)` against an `AtomicBool` instead.
- **Host-header guard** rejects anything that isn't a loopback name, which is the DNS-rebinding defence for a loopback-bound server. Keep it in front of every route.
- **Test harness quirks** (`crates/av-cli/tests/web_tests.rs`): reads the child's stdout **byte at a time** and only parses port numbers out of *complete* lines — reading a byte at a time and testing after each one parses the half-arrived `"8787"` as `87`. stderr is drained on a background thread (an undrained pipe fills and blocks the child, and hides why a server died). `connect` retries. Tests must not assume the host lacks `x0x`/`ant`; the Autonomi tests assert the invariant that holds either way.
- **The web `/api/block` cannot escalate to x0x at all.** It rejects a caller-supplied `x0x` field with a 400 rather than ignoring it, because a loopback server is reachable by any page the user has open. `av block --agent <id> --x0x` is the only route. Warning text comes from `cmd::block::warning_for` so browser and terminal cannot drift.
- **UI honesty rules.** Repeatable-but-true warnings ("peers do not consume feedback yet") are shown and offer "don't show again" (`anta-vista-muted-warnings` in `localStorage`, keyed by the server's `warning_id` — a *different* warning still gets through). Install commands come from the **server** (`ant_install_command()`) and travel with the error, so the page never hardcodes a POSIX command and shows the wrong one on Windows. Autonomi open/download controls stay disabled; `/api/download` returns 501 rather than pretending.
- **Known gaps to keep in mind:** search blocks (SSE is the follow-up); peers do not consume `av.feedback.v1`, `av.claim.v1` or `av.presence.v1` messages, so a broadcast rating changes nobody's ranking. Nothing ever *emits* an `av.claim.v1` either: the `claims` table, `ClaimPayload`, `MessageKind::Claim` and `TOPIC_CLAIM` all exist and the responder subscribes, but there is no `publish_claim` and no receive arm, so `claims` is always empty and the 0.15 `agreement` weight in `av-trust::ranking` is a constant 0.5 that cannot reorder results. Do not treat agreement as a live ranking signal, and do not add a claim producer without solving the Sybil problem first — `docs/follow-ups.md` has the write-up and a user-mediated replacement; `NameRecord`s cannot receive feedback because `feedback_events.resource_id` references `resources(id)`; `--kind watchlist` becomes `ResourceKind::Other("watchlist")` (unrelated to the web UI, but do not add a kind dropdown until fixed).

- **Search/resolve fan-out** (`crates/av-cli/src/network.rs`): queries go out BOTH as direct messages AND as a gossip broadcast. Direct fan-out candidates = recent `peer_cache` entries ∪ x0x-discovered agents (`/agents/discovered`, `/presence/online` via `X0xNetClient::discover_agent_ids`), deduped, self excluded — so peers never contacted before (and online-only peers) are still queried. Direct queries are sent from detached threads per candidate, each with its own `query_id` registered **before** the send, and `send_direct` permits slow relay/`gossip_inbox` routing (90 s HTTP timeout) — a past bug gated sends on a 2 s `connect_agent`, silently dropping every relay-routed peer. Responses match by any issued `query_id` (direct ids differ from the gossip id). Direct messaging is the fast path; gossip is mesh-driven and slower.

## Local blocking
`blocks` table, migration `005_blocks`, repo at `crates/av-store/src/repo/blocks.rs`. Three **independent** kinds: `resource` (`resources.id`), `name` (`name_records.record_id`), `agent` (64-hex x0x id). CLI in `crates/av-cli/src/cmd/block.rs`; web at `/api/block`, `/api/unblock`, `/api/blocks`.

- **The invariant: a blocked resource is never served, and a blocked agent is never consulted.** Any new egress path must re-check. Current guard points: `av_index::search::search_top_k` and `av_index::naming::lookup_name` (primary — also what the responder reads from), `to_publishable_resources`/`to_publishable_names` in `cmd/listen.rs` (pre-send, catches `--keep-local` resources that still have embeddings), the `--x0x` agent filter in `network.rs` (both candidate lists and response drops), `propagate_resource` (shared by `av propagate`, the search prompt and the web rating handler), and an early refusal in `cmd::index::index_resource` before any store write.
- **Do not add a provenance column to `resources` to enable resource→agent blocking.** The kinds are independent by design: anta-vista does not know who sent a resource, and inferring it would let one bad result condemn a useful peer. `ClusteredResult.agent_ids` preserves transport origin for *display* so a human can block a peer deliberately.
- **Never gossip blocks.** They are local policy; a peer learning about them is a privacy leak.
- **Identity comes from the transport, not the envelope.** Blocked-agent matching uses `DirectMessage::sender` / `IncomingEvent::origin`, not the self-reported `from_agent_id` — otherwise blocking is trivially bypassed by editing the envelope.
- **Unblock is not undo.** `block --resource` deletes the local copy by default; `--keep-local` is the escape hatch. Unblocking lifts the row and cannot restore deleted data. Never word a response as though it restored anything.
- **Partial x0x success is reported, not hidden.** `BlockOutcome::x0x_applied` is `None` (not requested), `Some(Ok)` or `Some(Err)`; the local block lands regardless. `av unblock --all` deliberately does **not** lift transport blocks, because it cannot enumerate x0x contacts.
- Agent ids are validated as 64 hex. A mistyped id stored as a block silently never matches anything, which reads as "the block is working".
- **Block policy reads must fail closed.** A `blocks` read that errors is not evidence that nothing is blocked, so `blocks::blocked_ids` propagates instead of returning an empty set, and every caller on a serving path (`search_top_k`, `lookup_name`, both `to_publishable_*` in `cmd/listen.rs`, `cmd/index.rs`, `propagate_resource`) propagates too. `unwrap_or_default()`/`unwrap_or(false)` in any of those is a latent "serve everything the user suppressed" bug — there is a test that drops the `blocks` table to pin this.
- **Never print an abbreviated id where a block argument is expected.** A block is keyed by *exact* match, so `av search`/`av resolve`/`av block --list` print full ids. Re-introducing `abbrev(&id, 16)` there silently makes the feature unusable. These lines deliberately do **not** print a ready-made `to block: av block --…` command — it was tried and removed as noise; the id plus `av block --help` is the intended path. Peer ids are hidden behind `av search --show-peers` (and always present in `--non-interactive` JSON) because the count is already in the result header and printing every id is unusable at real cluster sizes. `expand_known_prefix` in `cmd/block.rs` additionally refuses a short id that is a strict prefix of a locally-held subject and prints the full id; the `LIKE` there must use two placeholders (`?1` pattern, `?2` exact id) or a full id matches itself and nothing can be blocked at all. An id we do *not* hold is a normal network result, so that case is allowed and only warns. `unblock` checks for an existing exact row *before* the prefix check, or a dead short block left by an earlier build could never be removed except with `--all`.

## Config
- TOML via `AvConfig::from_file(path)`; all fields optional; `validate()` checks ranking weights sum to 1.0 (tolerance 0.01).
- **Two independent weight schemes — don't confuse them.** `AvConfig` defaults (`av-core/src/config.rs`): semantic=0.65, agreement=0.15, feedback=0.10, trust=0.10 (validated). But `av-core/src/constants.rs` has a *different* 5-weight set (semantic=0.55, relevance=0.10, agreement=0.15, feedback=0.10, trust=0.10) used by `av-trust::ranking`. Also name-ranking constants (`NAME_WEIGHT_*`). The constants set is not validated against config.

## Key dependencies & quirks
- `fastembed` v4 with features `["ort-download-binaries", "hf-hub-rustls-tls"]` — declared inline in `av-embed`, not a workspace dep.
- `rusqlite` with `bundled` feature everywhere — every `av-store` connection enables WAL mode and `PRAGMA foreign_keys` (see `crates/av-store/src/db.rs`). Declared inline in `av-trust`/`av-query` despite being a workspace dep; depends on same spec.
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
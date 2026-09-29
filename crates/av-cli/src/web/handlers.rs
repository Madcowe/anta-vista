//! HTTP handlers for the web interface.
//!
//! Every handler is blocking and returns a ready-made response; failures are
//! already turned into JSON error responses, so these return `WebResponse`
//! directly and only a genuine internal fault bubbles up as `Err`.
//!
//! The search handler deliberately reuses `cmd::search::collect_results` so the
//! browser and the CLI rank identically.

use std::sync::Arc;

use serde_json::json;

use super::server::{WebResponse, json_error, json_error_fields, json_response};
use super::state::AppState;

/// Percent-decode and parse a query string into key/value pairs.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(&v.replace('+', " ")))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Open a per-request connection, turning a failure into an error response.
fn db_conn(state: &Arc<AppState>) -> Result<rusqlite::Connection, WebResponse> {
    state
        .conn()
        .map_err(|e| json_error(500, "database_error", &e))
}

/// Get the shared embedding model, or an error response explaining why not.
fn provider(state: &Arc<AppState>) -> Result<Arc<av_embed::MiniLmProvider>, WebResponse> {
    state.provider.get().cloned().map_err(|e| {
        json_error(
            503,
            "model_unavailable",
            &format!("could not load the embedding model: {e}"),
        )
    })
}

/// Map a `CliError` onto a status code and a stable machine-readable code.
fn error_response(e: crate::cmd::CliError) -> WebResponse {
    use crate::cmd::CliError;
    let (status, code) = match &e {
        CliError::MissingDependency { code, .. } => (503, *code),
        CliError::Validation(_) => (400, "validation_error"),
        CliError::Network(_) => (502, "network_error"),
        CliError::Database(_) => (500, "database_error"),
        CliError::Model(_) => (503, "model_unavailable"),
        _ => (500, "internal_error"),
    };
    json_error(status, code, &e.to_string())
}

/// `GET /api/status` — the same fields `av status` prints, plus the live
/// dependency probe the setup banner needs.
pub fn status(state: &Arc<AppState>) -> WebResponse {
    let database = match db_conn(state) {
        Ok(conn) => {
            let counts = crate::cmd::status::database_counts(&conn);
            drop(conn);
            counts
        }
        Err(r) => return r,
    };

    // Re-probe rather than reusing the startup snapshot: the user may have
    // started or stopped a daemon since the server booted.
    let deps = crate::startup::probe_dependencies();
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return json_error(500, "state_error", "startup state is poisoned"),
    };
    let uptime = super::state::now_secs().saturating_sub(state.started_at);

    json_response(
        json!({
            "ok": true,
            "x0x_daemon": {
                "running": deps.x0x_running,
                "agent_id": deps.x0x_agent_id,
                "api": deps.x0x_api,
            },
            "av_listener": {
                "running": deps.av_listener_running,
                "pid": deps.av_listener_pid,
            },
            "ant_daemon": {
                "running": deps.antd_running,
                "network": if deps.antd_running { "default" } else { "offline" },
            },
            "ant_cli": { "installed": deps.ant_cli_installed },
            "minilm_model": {
                "loaded": state.provider.is_loaded(),
                "dimensions": 384,
            },
            "database": database,
            "config_path": startup.config_path.as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            "uptime_seconds": uptime,
        })
        .to_string(),
    )
}

/// `GET /api/deps` — what the setup banner renders, including the literal
/// install command for this platform so the user can see it before consenting.
pub fn deps(_state: &Arc<AppState>) -> WebResponse {
    let deps = crate::startup::probe_dependencies();
    json_response(
        json!({
            "ok": true,
            "deps": deps,
            // True when the network features are unavailable.
            "needs_setup": !deps.x0x_running || !deps.av_listener_running,
        })
        .to_string(),
    )
}

/// `GET /api/search?q=...&scheme=...&kind=...&mime=...&limit=...`
pub fn search(state: &Arc<AppState>, query: &str) -> WebResponse {
    let params = parse_query(query);
    let Some(q) = param(&params, "q").map(str::trim).filter(|s| !s.is_empty()) else {
        return json_error(400, "validation_error", "missing required parameter 'q'");
    };
    // Borrowed from `params`, so clone the pieces we hand to the search.
    let q = q.to_string();

    let limit = param(&params, "limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(10)
        .clamp(1, 100);
    let scheme = param(&params, "scheme").map(|s| s.to_string());
    let kind = param(&params, "kind").map(|s| s.to_string());
    let mime = param(&params, "mime").map(|s| s.to_string());

    let provider = match provider(state) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let conn = match db_conn(state) {
        Ok(c) => c,
        Err(r) => return r,
    };

    // Take the startup snapshot once and hold it, so a dependency started from
    // the setup banner mid-request cannot change the fan-out underneath us.
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return json_error(500, "state_error", "startup state is poisoned"),
    };

    // Search blocks until every peer has replied or the fan-out times out.
    // Streaming results (SSE) is a deliberate follow-up, not something to fake.
    let merged = match crate::cmd::search::collect_results(
        &state.cli,
        &startup,
        &conn,
        &provider,
        &q,
        &crate::cmd::search::SearchOptions {
            scheme,
            kind,
            mime,
            limit,
            show_peers: false,
        },
    ) {
        Ok(m) => m,
        Err(e) => return error_response(e),
    };

    json_response(
        json!({
            "ok": true,
            "query": q,
            "results": merged.results,
            "total": merged.results.len(),
            "clustering_active": merged.clustering_active,
        })
        .to_string(),
    )
}

/// `GET /api/download?location=...` — deliberately not implemented yet.
///
/// Opening and downloading Autonomi locations needs the ant client to hand us
/// real bytes. The UI shows these controls as unavailable rather than
/// pretending, so this answers 501 until that path exists.
pub fn download(_state: &Arc<AppState>, query: &str) -> WebResponse {
    let params = parse_query(query);
    let location = param(&params, "location").unwrap_or("");
    json_error(
        501,
        "not_implemented",
        &format!(
            "downloading Autonomi locations from the browser is not implemented yet (requested: {location:?})"
        ),
    )
}

/// `POST /api/rate` — body `{resource_id, rating, query, result?}`.
///
/// A positive rating does more than store feedback: it also boosts the
/// exact-query relevance score and, for a resource we only heard about from a
/// peer, keeps a local metadata copy so the listener can serve it onward.
pub fn rate(state: &Arc<AppState>, body: &str) -> WebResponse {
    let payload: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return json_error(400, "validation_error", &format!("invalid JSON: {e}")),
    };

    let resource_id = payload["resource_id"].as_str().unwrap_or("").to_string();
    if resource_id.is_empty() {
        return json_error(400, "validation_error", "missing 'resource_id'");
    }
    let rating = payload["rating"].as_str().unwrap_or("").to_string();
    if rating.is_empty() {
        return json_error(400, "validation_error", "missing 'rating'");
    }
    let query = payload["query"].as_str().map(|s| s.to_string());

    let conn = match db_conn(state) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return json_error(500, "state_error", "startup state is poisoned"),
    };

    // "useful" and "high-confidence" are endorsements; the rest are complaints
    // and must not reinforce anything.
    let positive = matches!(rating.as_str(), "useful" | "high-confidence");
    let source = payload["result"]["source"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // `feedback_events` references `resources(id)`, so a rating is only
    // storable once the resource is held locally. For an endorsement of
    // something a peer told us about, propagate the metadata first, which
    // inserts the row and satisfies the key.
    let mut reinforced = false;
    let mut reinforce_error = None;
    let mut propagated = false;

    let held_locally = || crate::cmd::rate::is_locally_indexed(&conn, &resource_id);
    // The result the page already rendered, echoed back so reinforcement does
    // not have to re-run the search. Sent on every rating, so the fallback is
    // only reached by a hand-rolled request.
    let result_for = || match payload.get("result") {
        Some(r) if r.is_object() => r.clone(),
        _ => json!({ "resource_id": resource_id, "source": "local" }),
    };

    // Reinforce against the stored query.
    //
    // Two failure kinds, kept apart deliberately: the outer error is a ready
    // response (the embedding model is unavailable) which the caller may choose
    // to return, and the inner error is a reinforcement failure worth
    // reporting without discarding the rating that was already stored.
    let reinforce =
        |q: &str, result: &serde_json::Value| -> Result<Result<(), String>, WebResponse> {
            let provider = provider(state)?;
            Ok(
                crate::cmd::search::reinforce(&conn, &provider, &startup, q, result)
                    .map_err(|e| e.to_string()),
            )
        };

    let rateable = source != "name";
    if positive
        && rateable
        && !held_locally()
        && let Some(q) = query.as_deref().filter(|q| !q.trim().is_empty())
    {
        match reinforce(q, &result_for()) {
            Ok(Ok(())) => {
                reinforced = true;
                propagated = held_locally();
            }
            // Without the model we cannot propagate, so the rating has nowhere
            // to attach. Say so rather than storing nothing.
            Err(response) => return response,
            Ok(Err(e)) => {
                tracing::warn!("propagation failed: {e}");
                reinforce_error = Some(e);
            }
        }
    }

    // A complaint about something we do not have has nothing to attach to, and
    // propagating it in order to complain about it would be wrong.
    if !held_locally() {
        let detail = if source == "name" {
            format!(
                "cannot store feedback for the name {resource_id:?}: feedback rows \
                 attach to indexed resources, and name records are tracked separately."
            )
        } else {
            format!(
                "cannot record feedback for {resource_id}: it is not in the local \
                 index. Add the link first, then rate it."
            )
        };
        return json_error(400, "resource_not_indexed", &detail);
    }

    let outcome = match crate::cmd::rate::record_feedback(
        &conn,
        &startup,
        &resource_id,
        &rating,
        query.clone(),
    ) {
        Ok(o) => o,
        Err(e) => return error_response(e),
    };

    // A name result has no `resources` row, so there is nothing to reinforce
    // beyond the re-broadcast that `reinforce` already performs.
    if positive
        && !reinforced
        && let Some(q) = query.as_deref().filter(|q| !q.trim().is_empty())
    {
        match reinforce(q, &result_for()) {
            Ok(Ok(())) => reinforced = true,
            // The rating is already stored; a missing model only means the
            // local boost was skipped.
            Ok(Err(e)) => {
                tracing::warn!("rating recorded but reinforcement failed: {e}");
                reinforce_error = Some(e);
            }
            Err(_) => {
                tracing::warn!("rating recorded but no model to reinforce with");
                reinforce_error = Some("embedding model unavailable".to_string());
            }
        }
    }

    json_response(
        json!({
            "ok": true,
            "feedback_id": outcome.feedback_id,
            "resource_id": resource_id,
            "rating": rating,
            "query": query,
            "broadcast": outcome.broadcast,
            "relevance_boosted": reinforced,
            "propagated": propagated,
            // Honest about the limit: peers ignore feedback messages today, so
            // a broadcast does not actually change anyone else's ranking. The
            // id lets the UI offer "don't show this again" without having to
            // pattern-match the wording.
            "warning": if outcome.broadcast {
                Some(
                    "Feedback was published, but peers do not consume feedback yet, so this does not affect their ranking."
                )
            } else {
                Some("Stored locally. Not broadcast because the x0x daemon is offline.")
            },
            "warning_id": if outcome.broadcast {
                "peers_ignore_feedback"
            } else {
                "not_broadcast_offline"
            },
            "reinforce_error": reinforce_error,
        })
        .to_string(),
    )
}

/// `POST /api/index` — body `{uri, tags?, force?}`, the "Add a Link" action.
pub fn index(state: &Arc<AppState>, body: &str) -> WebResponse {
    let payload: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return json_error(400, "validation_error", &format!("invalid JSON: {e}")),
    };

    let raw_uri = payload["uri"].as_str().unwrap_or("").trim().to_string();
    if raw_uri.is_empty() {
        return json_error(400, "validation_error", "missing 'uri'");
    }
    let uri = av_ingest::location::normalize_uri(&raw_uri);

    // Autonomi locations are only reachable with ant tooling. Unlike the CLI we
    // cannot prompt, so report the missing dependency and let the setup banner
    // offer the install.
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return json_error(500, "state_error", "startup state is poisoned"),
    };
    let lower = uri.to_lowercase();
    if (lower.starts_with("ant://") || lower.starts_with("autonomi://"))
        && !startup.antd_running
        && !crate::startup::ant_cli_binary_available()
    {
        // The command is the platform's, and it is returned rather than baked
        // into the message: the UI has to display the exact command before the
        // user consents to running it, and a hardcoded POSIX command would be
        // wrong on Windows.
        let command = crate::startup::ant_install_command();
        return json_error_fields(
            503,
            "no_download_backend",
            &format!(
                "Neither antd daemon nor ant CLI are available, so this Autonomi location cannot be fetched. Install the ant CLI with: {command}"
            ),
            json!({
                "dependency": "ant",
                "command": command,
                "start": false,
            }),
        );
    }

    let tags: Vec<String> = payload["tags"]
        .as_str()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let provider = match provider(state) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let conn = match db_conn(state) {
        Ok(c) => c,
        Err(r) => return r,
    };

    let outcome = match crate::cmd::index::index_resource(
        &startup,
        &conn,
        &provider,
        &uri,
        &tags,
        false, // always download: "Add a Link" means fetch the content
        false, // verify the URI is reachable
        payload["force"].as_bool().unwrap_or(false),
        None,
    ) {
        Ok(o) => o,
        Err(e) => return error_response(e),
    };

    json_response(
        json!({
            "ok": true,
            "resource_id": outcome.resource_id,
            "location": outcome.location,
            "mime_type": outcome.mime_type,
            "kind": outcome.kind,
            "description": outcome.description,
            "embedding_dim": outcome.embedding_dim,
            "duplicate": outcome.duplicate,
            "duplicate_by": outcome.duplicate_by,
            "broadcast": outcome.broadcast,
        })
        .to_string(),
    )
}

/// `POST /api/listener/stop` — stop every running `av listen`.
pub fn listener_stop(_state: &Arc<AppState>) -> WebResponse {
    let stopped = crate::listener::stop_all(false);
    json_response(
        json!({
            "ok": true,
            "stopped_pids": stopped,
            "count": stopped.len(),
        })
        .to_string(),
    )
}

/// `POST /api/listener/start`
///
/// Spawns the same detached `av listen` that a networked CLI command would, and
/// is a no-op when one is already up. This is what lets the setup banner
/// actually start responding to peers: without a listener, search can only send.
pub fn listener_start(state: &Arc<AppState>) -> WebResponse {
    let was_running = crate::listener::is_running();
    // `ensure_running` is what the CLI calls for every networked command, so
    // the web UI and the CLI converge on one listener. It returns true when a
    // listener is up, whether it was already running or was just spawned.
    let ok = crate::listener::ensure_running();

    // The spawn is detached, so it may not have written its PID file yet; the
    // liveness check is the one that decides what to report.
    let running = ok || crate::listener::is_running();
    let mut detail = if running {
        "background listener is running".to_string()
    } else {
        "could not start the listener; run `av listen` to see why".to_string()
    };

    // A listener with no x0x cannot reach anyone, so say so rather than letting
    // the user wonder why searches return nothing.
    if running {
        let has_x0x = state
            .startup
            .read()
            .map(|s| s.x0x_config.is_some())
            .unwrap_or(false);
        if !has_x0x {
            detail.push_str(" (x0x is not running, so peers will not be reached)");
        }
    }

    json_response(
        json!({
            "ok": running,
            "already_running": was_running,
            "running": running,
            "pid": crate::listener::pid_path()
                .and_then(|_| crate::startup::probe_dependencies().av_listener_pid),
            "detail": detail,
        })
        .to_string(),
    )
}

/// `POST /api/deps/x0x/start`
pub fn start_x0x(state: &Arc<AppState>) -> WebResponse {
    let started = crate::startup::start_x0x_daemon();
    // Search decides whether to fan out to peers from the startup snapshot, so
    // it has to learn about the daemon we just started.
    state.refresh_network_deps();
    json_response(
        json!({
            "ok": started,
            "dependency": "x0x",
            "running": started,
            "detail": if started {
                "x0x daemon started."
            } else {
                "Could not start the x0x daemon. Install it first, then try again."
            },
        })
        .to_string(),
    )
}

/// `POST /api/deps/antd/start`
pub fn start_antd(state: &Arc<AppState>) -> WebResponse {
    let started = crate::startup::start_antd_daemon();
    state.refresh_network_deps();
    json_response(
        json!({
            "ok": started,
            "dependency": "antd",
            "running": started,
            "detail": if started {
                "antd daemon started."
            } else {
                "Could not start the antd daemon. Install ant CLI or antd first."
            },
        })
        .to_string(),
    )
}

/// `POST /api/deps/x0x/install` — runs the platform installer.
///
/// The UI shows the exact command from `/api/deps` first and requires a
/// separate click before calling this, so an install is never implicit.
pub fn install_x0x(state: &Arc<AppState>) -> WebResponse {
    let command = crate::startup::x0x_install_command();
    if crate::startup::x0x_binary_available() {
        return json_response(
            json!({
                "ok": true,
                "dependency": "x0x",
                "installed": true,
                "already_installed": true,
                "command": command,
            })
            .to_string(),
        );
    }
    // Capture installer output so it cannot interleave with the server's.
    let ok = crate::startup::install_x0x_with(std::process::Stdio::piped());
    state.refresh_network_deps();
    json_response(
        json!({
            "ok": ok,
            "dependency": "x0x",
            "installed": crate::startup::x0x_binary_available(),
            "command": command,
        })
        .to_string(),
    )
}

/// `POST /api/deps/ant/install`
pub fn install_ant(state: &Arc<AppState>) -> WebResponse {
    let command = crate::startup::ant_install_command();
    if crate::startup::ant_cli_binary_available() {
        return json_response(
            json!({
                "ok": true,
                "dependency": "ant",
                "installed": true,
                "already_installed": true,
                "command": command,
            })
            .to_string(),
        );
    }
    let ok = crate::startup::install_ant_cli_with(std::process::Stdio::piped());
    // The installer may have dropped ant somewhere not yet on this PATH.
    crate::startup::refresh_ant_path();
    state.refresh_network_deps();
    json_response(
        json!({
            "ok": ok,
            "dependency": "ant",
            "installed": crate::startup::ant_cli_binary_available(),
            "command": command,
        })
        .to_string(),
    )
}

// --- Local block policy -----------------------------------------------------
//
// These three handlers are thin adapters over `cmd::block`, deliberately: the
// browser and the CLI must apply identical policy, and a block is a promise
// about what this node will and will not do. Reimplementing the rules here
// would be how the two surfaces start to disagree about whether something is
// blocked.

/// `GET /api/blocks` — every current block, with per-kind counts.
pub fn blocks_list(state: &Arc<AppState>) -> WebResponse {
    let conn = match db_conn(state) {
        Ok(c) => c,
        Err(r) => return r,
    };

    let entries = match crate::cmd::block::list_blocks(&conn) {
        Ok(e) => e,
        Err(e) => return json_error(500, "database_error", &e.to_string()),
    };
    let counts = match crate::cmd::block::counts(&conn) {
        Ok(c) => c,
        Err(e) => return json_error(500, "database_error", &e.to_string()),
    };

    json_response(
        json!({
            "ok": true,
            "count": entries.len(),
            "blocks": entries
                .iter()
                .map(|e| json!({
                    "subject_kind": e.subject_kind.as_str(),
                    "subject_id": e.subject_id,
                    "reason": e.reason,
                    "blocked_at": e.blocked_at,
                    "source": e.source,
                }))
                .collect::<Vec<_>>(),
            "counts": counts
                .iter()
                .map(|(k, n)| json!({ "subject_kind": k.as_str(), "count": n }))
                .collect::<Vec<_>>(),
        })
        .to_string(),
    )
}

/// `POST /api/block` — block one subject.
///
/// `keep_local` defaults to false, matching the CLI: blocking a resource
/// deletes the local copy. The response says whether that happened, so the page
/// can tell the user what actually changed instead of implying everything
/// succeeded.
pub fn block(state: &Arc<AppState>, body: &str) -> WebResponse {
    let payload: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return json_error(400, "validation_error", &format!("invalid JSON: {e}")),
    };

    let kind = match payload["subject_kind"]
        .as_str()
        .and_then(av_store::repo::blocks::BlockKind::parse)
    {
        Some(k) => k,
        None => {
            return json_error(
                400,
                "validation_error",
                "'subject_kind' must be one of: resource, name, agent",
            );
        }
    };
    let subject_id = payload["subject_id"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if subject_id.is_empty() {
        return json_error(400, "validation_error", "missing 'subject_id'");
    }
    let reason = payload["reason"].as_str().map(|s| s.to_string());
    let keep_local = payload["keep_local"].as_bool().unwrap_or(false);
    // The browser never escalates to the x0x transport on its own. That block
    // is global, drops everything from that agent on every topic, and needs the
    // daemon; offering it as a one-click toggle in a search result row would
    // make a local preference do something much larger than it looks.
    //
    // This is deliberately not read from the payload. A loopback server is
    // reachable by any page the user visits, and honouring a caller-supplied
    // `x0x` flag would let a request from one make a lasting global change the
    // UI never offered. `av block --agent <id> --x0x` is the explicit,
    // user-typed way to do that.
    const X0X: bool = false;
    if payload.get("x0x").is_some() {
        return json_error(
            400,
            "validation_error",
            "x0x transport blocking is not available over the web interface. \
             Use 'av block --agent <id> --x0x' instead.",
        );
    }

    let conn = match db_conn(state) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return json_error(500, "state_error", "startup state is poisoned"),
    };

    match crate::cmd::block::block_subject(
        &conn,
        &startup,
        kind,
        &subject_id,
        reason.as_deref(),
        keep_local,
        X0X,
        "web",
    ) {
        Ok(o) => {
            // Reuse the CLI's warning text so a block made in the browser says
            // the same thing as the same block made from the terminal. The one
            // addition is that the browser's escalation is structurally absent.
            let mut warning = crate::cmd::block::warning_for(&o);
            if warning.is_none() && o.kind == crate::cmd::block::BlockKind::Agent {
                warning = Some(
                    "Blocked in anta-vista only. This peer can still reach the node and the \
                     responder will still count its claims. Use 'av block --agent <id> --x0x' \
                     to drop it at the transport too."
                        .to_string(),
                );
            }
            json_response(
                json!({
                    "ok": true,
                    "subject_kind": o.kind.as_str(),
                    "subject_id": o.subject_id,
                    "blocked": o.blocked,
                    "removed_local": o.removed_local,
                    "kept_local": o.kept_local,
                    "cascaded": o.cascaded,
                    "x0x_requested": o.x0x_requested,
                    "warning": warning,
                })
                .to_string(),
            )
        }
        Err(e) => validation_response(&e),
    }
}

/// `POST /api/unblock` — lift one block, or every block with `all: true`.
pub fn unblock(state: &Arc<AppState>, body: &str) -> WebResponse {
    let payload: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return json_error(400, "validation_error", &format!("invalid JSON: {e}")),
    };

    let all = payload["all"].as_bool().unwrap_or(false);
    let kind = payload["subject_kind"]
        .as_str()
        .and_then(av_store::repo::blocks::BlockKind::parse);
    let subject_id = payload["subject_id"].as_str().map(|s| s.trim().to_string());

    if !all && kind.is_none() {
        return json_error(
            400,
            "validation_error",
            "provide 'subject_kind' and 'subject_id', or set all: true",
        );
    }
    if !all && subject_id.as_deref().unwrap_or("").is_empty() {
        return json_error(400, "validation_error", "missing 'subject_id'");
    }

    let conn = match db_conn(state) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return json_error(500, "state_error", "startup state is poisoned"),
    };

    match crate::cmd::block::unblock_subject(
        &conn,
        &startup,
        kind.unwrap_or(av_store::repo::blocks::BlockKind::Resource),
        subject_id.as_deref(),
        all,
        // The browser does not touch x0x transport state; `av unblock --agent
        // <id> --x0x` is the explicit way to do that.
        false,
    ) {
        Ok(o) => json_response(
            json!({
                "ok": true,
                "subject_kind": o.kind.as_str(),
                "subject_id": o.subject_id,
                "unblocked": o.unblocked,
                "removed": o.removed,
                "warning": if o.removed > 0 {
                    Some(
                        "Lifted the block. Deleted resources are not restored — re-index them to get them back."
                            .to_string(),
                    )
                } else {
                    Some("That subject was not blocked.".to_string())
                },
            })
            .to_string(),
        ),
        Err(e) => validation_response(&e),
    }
}

/// Map a `CliError` onto the right HTTP status.
///
/// `block_subject` rejects a malformed agent id as a validation failure, and the
/// browser needs to see that as a 400 rather than a 500 — the request was wrong,
/// not the server.
fn validation_response(e: &crate::cmd::CliError) -> WebResponse {
    use crate::cmd::CliError;
    match e {
        CliError::Validation(msg) => json_error(400, "validation_error", msg),
        CliError::Database(msg) => json_error(500, "database_error", msg),
        other => json_error(500, "internal_error", &other.to_string()),
    }
}

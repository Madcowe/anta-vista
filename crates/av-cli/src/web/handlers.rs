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

use super::server::{WebResponse, binary_response, json_error, json_error_fields, json_response};
use super::state::AppState;
use crate::download::DownloadOptions;

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

/// `GET /api/ping` — the cheapest possible "is this anta-vista?" answer.
///
/// No database, no dependency probe: `web::run` uses it to tell a second
/// launch that the port it wanted is already held by another copy of this
/// program (and so should be reused) rather than by something else entirely.
pub fn ping() -> WebResponse {
    json_response(
        json!({
            "ok": true,
            "service": "anta-vista",
            "version": env!("CARGO_PKG_VERSION"),
        })
        .to_string(),
    )
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
            // The banner is for *installs only*: a service that is merely
            // stopped is started automatically (boot, search/index, or the
            // drawer), so it never needs a banner. Only a missing binary does.
            "needs_install": !deps.x0x_installed,
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

    // The browser skips its ensure call while its cached deps already say the
    // daemon is running, so a snapshot taken while x0x was down can stay
    // `None` forever while the daemon is actually up — and `None` below means
    // this search silently skips the fan-out. Refresh once *before* the
    // request-long read guard (refreshing takes the write lock), so "x0x is
    // running" always reaches the one read that decides.
    let needs_refresh = state.startup.read().is_ok_and(|s| s.x0x_config.is_none());
    if needs_refresh {
        state.refresh_network_deps();
    }

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

/// `GET /api/download?location=...` — fetch a location through the local daemon.
///
/// The daemon is the *fallback* transport: the browser fetches Autonomi bytes
/// itself over WebRTC, which needs nothing installed. This route exists for the
/// cases where that cannot be used — a file too large to hand back to the server,
/// or a browser that cannot run the SDK.
pub fn download(state: &Arc<AppState>, query: &str) -> WebResponse {
    let params = parse_query(query);
    let location = param(&params, "location").unwrap_or("").trim().to_string();
    if location.is_empty() {
        return json_error(400, "validation_error", "missing 'location'");
    }
    let uri = av_ingest::location::normalize_uri(&location);

    if let Err(err) = refuse_if_blocked(state, &uri) {
        return err;
    }
    if let Some(err) = no_backend_error(state, &uri) {
        return err;
    }

    let bytes = match crate::download::download_content(&uri, None, DownloadOptions::default()) {
        Ok(b) => b,
        Err(e) => return json_error(502, "download_failed", &e.to_string()),
    };

    // Same policy the indexer uses, so a file served here is typed the way it was
    // stored. The location's optional filename segment is the only hint there is.
    let filename = av_ingest::location::analyze_location(&uri).inferred_filename;
    let mime = av_ingest::mime::detect_mime_with_hint(&bytes, filename.as_deref())
        .unwrap_or_else(|_| "application/octet-stream".to_string());

    // Inline: the browser decides whether to render or save, exactly as it does
    // for an http result. Only the bytes and their type — never an error the
    // caller might save to disk by mistake, which is why failures above are JSON.
    binary_response(200, &mime, bytes)
}

/// Refuse to fetch a location the user has blocked, on any route that reaches the
/// network on their behalf — or, for [`view_plan`] and [`blocked`], on any route
/// the page consults *before* it reaches the network itself.
///
/// Shared by [`download`], [`view_fetch`], [`view_plan`], [`index`] and
/// [`blocked`] so a new egress path cannot come up without it, which is the
/// failure this exists to prevent: these routes take a *location* rather than a
/// `resources.id`, so none can use `blocks::is_blocked`, and the browser's own
/// WebRTC transfer goes through no server route at all — which is why the page
/// asks [`blocked`] first and why `view_plan` (the one server call every open
/// makes) re-checks. Reading the network first and deciding afterwards — or not
/// at all — is the bug; failing closed on a `blocks` read error is the rule.
///
/// Fails closed. A `blocks` read that errors is not evidence that nothing is
/// blocked, so this returns the error rather than allowing the fetch; see
/// `blocks::is_location_blocked` for the query and its reasoning.
fn refuse_if_blocked(state: &Arc<AppState>, uri: &str) -> Result<(), WebResponse> {
    let conn = match state.conn() {
        Ok(c) => c,
        // Cannot open the database, so cannot check. Refusing is the only answer
        // that is not a guess.
        Err(_) => {
            return Err(json_error(
                503,
                "block_check_failed",
                "could not read the block list, so this location was not fetched",
            ));
        }
    };
    match av_store::repo::blocks::is_location_blocked(&conn, uri) {
        Ok(true) => Err(json_error(
            403,
            "blocked",
            "that location is blocked on this machine",
        )),
        Ok(false) => Ok(()),
        Err(_) => Err(json_error(
            503,
            "block_check_failed",
            "could not read the block list, so this location was not fetched",
        )),
    }
}

/// The `503 no_download_backend` refusal when an Autonomi location has no way to
/// be fetched here: neither the antd daemon nor the ant CLI.
///
/// Carries the install command so the caller never has to invent one for the
/// platform it is running on.
fn no_backend_error(state: &Arc<AppState>, uri: &str) -> Option<WebResponse> {
    let lower = uri.to_lowercase();
    if !(lower.starts_with("ant://") || lower.starts_with("autonomi://")) {
        return None;
    }
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return Some(json_error(500, "state_error", "startup state is poisoned")),
    };
    if startup.antd_running || crate::startup::ant_cli_binary_available() {
        return None;
    }
    let command = crate::startup::ant_install_command();
    Some(json_error_fields(
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
    ))
}

/// The name we already hold for a location, if any.
///
/// This exists because an Autonomi record's *canonical* location is the address
/// alone — that is its identity, for dedup and for the block bridge — so the
/// `/welcome.md` an author published is stripped before a search result ever
/// reaches the page. The name is not lost, it is in `resources.filename`, but
/// nothing was reading it: both view endpoints derived the name from the URI
/// alone, so every download came out as `52af8a181c6171f8.md`.
///
/// A read failure is deliberately **not** fatal here. The caller falls back to
/// the address-derived name, which is always safe to produce; this is a
/// best-effort improvement to a label, not a policy decision, and it is not the
/// kind of "we could not check" that the block readers must fail closed on.
fn stored_filename(state: &Arc<AppState>, uri: &str) -> Option<String> {
    let conn = state.conn().ok()?;
    av_store::repo::resources::find_filename(&conn, uri)
        .ok()
        .flatten()
        .filter(|name| !name.trim().is_empty())
}

/// `POST /api/view/blob` — raw body is the file; metadata arrives in headers.
///
/// Publishes the bytes on a throwaway loopback port so they can be opened as a
/// real document on an origin that cannot reach the API. See [`crate::web::viewer`]
/// for why a `blob:` URL is not usable here.
pub fn view_blob(state: &Arc<AppState>, uri: &str, body: &[u8]) -> WebResponse {
    let uri = av_ingest::location::normalize_uri(uri);
    let filename = av_ingest::location::analyze_location(&uri).inferred_filename;
    let mime = match av_ingest::mime::detect_mime_with_hint(body, filename.as_deref()) {
        Ok(m) => m,
        // An empty body cannot be sniffed, and there is nothing to show.
        Err(e) => {
            return json_error(
                400,
                "validation_error",
                &format!("cannot serve this file: {e}"),
            );
        }
    };

    let known = stored_filename(state, &uri);
    let name = av_ingest::filename::suggest_filename_with(&uri, known.as_deref(), &mime);
    match crate::web::viewer::publish(body, &mime, &name) {
        Ok(handle) => json_response(
            json!({
                "ok": true,
                "url": handle.url,
                "mime_type": mime,
                "bytes": body.len(),
                // The name comes from the server so the browser's Save action
                // and the CLI cannot invent two different names for one file.
                "filename": name,
                "expires_in": handle.expires_in,
            })
            .to_string(),
        ),
        Err(e) => json_error(500, "viewer_failed", &e),
    }
}

/// `POST /api/view/fetch` — fetch a location here, reporting progress as it goes.
///
/// The daemon half of Open and Save, for the two cases WebRTC cannot cover: a
/// browser that cannot run the SDK, and a record larger than `MAX_UPLOAD`. The
/// browser asks for no bytes at all — the request body is empty — so the 32 MiB
/// upload cap does not apply, and a large record is no longer one that only
/// works if the browser can carry it.
///
/// The body is newline-delimited JSON rather than one JSON object, because the
/// interesting part happens *during* the fetch. `Content-Length` is unknown, so
/// the page reads `response.body` incrementally and learns the outcome as it
/// happens; a single response would hold every chunk back until the last byte.
///
/// Why `fetch` and not the existing `GET /api/download`: this is a POST, so it
/// inherits the same `Sec-Fetch-Site` guard that keeps another site from driving
/// this loopback server, and it never sends a caller-supplied byte count.
pub fn view_fetch(state: &Arc<AppState>, uri: &str) -> WebResponse {
    let uri = av_ingest::location::normalize_uri(uri);
    let info = av_ingest::location::analyze_location(&uri);
    let lower = uri.to_lowercase();
    let is_http = lower.starts_with("http://") || lower.starts_with("https://");
    if info.canonical.is_none() && !is_http {
        return json_error(
            400,
            "validation_error",
            "that is not a location we can fetch",
        );
    }

    // Everything that can refuse this happens *here*, before the job exists and
    // before a thread is started. A blocked location and a missing backend are
    // answers with a status and a code, not something the page has to watch a
    // progress log turn into — which is why the caller treats a non-2xx here as a
    // decline to fall back from.
    if let Err(err) = refuse_if_blocked(state, &uri) {
        return err;
    }
    if let Some(err) = no_backend_error(state, &uri) {
        return err;
    }

    let job = super::fetchjob::start();
    let worker = Arc::clone(state);
    let job_for_thread = job.clone();
    let fetched = uri.clone();
    std::thread::spawn(move || run_view_fetch(&worker, &fetched, &job_for_thread));

    json_response(
        json!({
            "ok": true,
            "job": job,
            // Where to ask for events. A path rather than a second absolute URL so
            // the page cannot be pointed at another origin by this response.
            "status": format!("/api/view/fetch/{job}"),
        })
        .to_string(),
    )
}

/// `GET /api/view/fetch/<job>?since=N` — the events a page has not seen.
///
/// Polling rather than a streamed body, because tiny_http cannot stream one: it
/// copies a response body through a chunked encoder that buffers 8 KB and never
/// flushes, so every event would arrive in one burst at the end. See
/// [`crate::web::fetchjob`] for the whole argument.
pub fn view_fetch_status(job: &str, query: &str) -> WebResponse {
    // Only a page that already knows the id gets here, and the id is a 32-hex
    // token, so this needs no session of its own. Anything that is not one is
    // rejected rather than looked up: a `blocks`-style scan is not worth the
    // possibility of a lookup being a directory of every fetch in flight.
    if job.is_empty() || job.len() > 64 || !job.bytes().all(|b| b.is_ascii_hexdigit()) {
        return json_error(404, "not_found", "no such fetch");
    }
    // A malformed cursor reads as 0 rather than as a refusal: it can only come
    // from a page, and resending the whole log is harmless. Refusing it would turn
    // a stale bookmark into a dead tab.
    let since = parse_query(query)
        .iter()
        .find(|(k, _)| k == "since")
        .and_then(|(_, v)| v.parse::<u64>().ok())
        .unwrap_or(0);

    match super::fetchjob::read(job, since) {
        Some(snapshot) => json_response(
            json!({
                "ok": true,
                "events": snapshot.events,
                // The page sends this back verbatim. Counting received events
                // instead would be equal to the cursor only while nothing has been
                // trimmed, and a long fetch is exactly when that stops holding.
                "next": snapshot.next,
                "done": snapshot.done,
            })
            .to_string(),
        ),
        // Expired and never-existed are the same answer. A page that polled past
        // the retention window gets one refusal, not a stream of them.
        None => json_error(404, "not_found", "no such fetch"),
    }
}

/// Run one fetch on its own thread, appending events to its job.
///
/// The thread, not the request, is what makes progress observable: the caller
/// gets a job id back immediately and can ask about it while this is still
/// running. Every path out of here calls [`fetchjob::finish`] — which is what
/// stops the page polling — so it is called once, at the end, rather than left to
/// each branch to remember.
fn run_view_fetch(state: &Arc<AppState>, uri: &str, job: &str) {
    use super::fetchjob::FetchEvent;
    use crate::download::DownloadEvent;

    let report = |event: DownloadEvent| {
        let recorded = match event {
            DownloadEvent::Status(message) => FetchEvent::Status {
                message: message.to_string(),
            },
            // The ant CLI has started; its own output is a status line rather
            // than a byte count, and the diagnostics file is what carries the
            // counts.
            DownloadEvent::SubprocessOutput => FetchEvent::Status {
                message: "Downloading via the ant CLI...".to_string(),
            },
            DownloadEvent::Chunk(chunk) => FetchEvent::Chunk {
                index: chunk.index,
                bytes: chunk.bytes,
                chunks: chunk.chunks,
                total_bytes: chunk.total_bytes,
                retried: chunk.retried,
            },
        };
        super::fetchjob::push(job, recorded);
    };

    let outcome = fetch_and_publish(state, uri, &report);

    match outcome {
        Ok(done) => super::fetchjob::push(job, done),
        Err(message) => super::fetchjob::push(job, FetchEvent::Error { message }),
    }
    // Last, and on both paths: this is the page's signal to stop asking.
    super::fetchjob::finish(job);
}

/// Fetch, type it, name it, and hand it to the viewer.
///
/// Split from [`run_view_fetch`] so every failure is one `Err(String)` on one
/// path, and the "mark it finished" step cannot be skipped by a new error case
/// added to this one.
fn fetch_and_publish(
    state: &Arc<AppState>,
    uri: &str,
    report: &dyn Fn(crate::download::DownloadEvent),
) -> Result<super::fetchjob::FetchEvent, String> {
    use super::fetchjob::FetchEvent;

    let bytes = crate::download::download_content(
        uri,
        Some(report),
        // On, unlike every other caller: this is the one transport whose chunk
        // detail can be had, and the whole point of the route is to show it.
        crate::download::DownloadOptions {
            chunk_progress: true,
        },
    )
    .map_err(|e| e.to_string())?;

    // Exactly `view_blob`'s policy, so a file served by this route is named and
    // typed identically to the same record uploaded by the browser.
    let hint = av_ingest::location::analyze_location(uri).inferred_filename;
    let mime = av_ingest::mime::detect_mime_with_hint(&bytes, hint.as_deref())
        .map_err(|e| format!("cannot serve this file: {e}"))?;
    let known = stored_filename(state, uri);
    let name = av_ingest::filename::suggest_filename_with(uri, known.as_deref(), &mime);

    let handle = crate::web::viewer::publish(&bytes, &mime, &name)?;

    // Read before `mime` moves into the event below.
    let renderable = av_ingest::mime::is_browser_renderable(&mime);

    Ok(FetchEvent::Done {
        url: handle.url,
        mime_type: mime,
        filename: name,
        bytes: bytes.len() as u64,
        expires_in: handle.expires_in,
        // Named so the page can say which transport actually ran it, rather than
        // letting "opened" cover both.
        transport: "daemon".to_string(),
        renderable,
    })
}

/// The bodyless half of `/api/view/blob`: a name and a renderability verdict,
/// with no need to send the file.
///
/// Save needs a filename and nothing else. It used to POST the whole record to
/// get one, which worked until a record was larger than `MAX_BODY` — at which
/// point saving a file the browser had already downloaded failed with an error
/// about the upload, having nothing to do with the save.
pub fn view_plan(state: &Arc<AppState>, uri: &str, mime_hint: &str, prefix: &[u8]) -> WebResponse {
    let uri = av_ingest::location::normalize_uri(uri);
    let info = av_ingest::location::analyze_location(&uri);
    if info.canonical.is_none() {
        return json_error(
            400,
            "validation_error",
            "that is not a location we can name a file for",
        );
    }

    // The page calls this before *any* bytes move — a browser WebRTC fetch goes
    // through no other server route, so this is where an open learns the user
    // has blocked the location. Naming a file for it would be work done on the
    // way to serving it, and the check fails closed like every other block read.
    if let Err(err) = refuse_if_blocked(state, &uri) {
        return err;
    }

    // A hint we cannot place is treated as absent, which makes the verdict
    // "download it" — the outcome that always works. Guessing "renderable" from
    // an unknown type is how this path produced a blank window.
    let normalised = av_ingest::mime::canonicalize_mime(mime_hint);
    let hinted = match normalised.as_str() {
        "application/octet-stream" | "" => "application/octet-stream",
        other => other,
    };

    // Bytes beat a reported type whenever they say something specific. The
    // Autonomi SDK hands us `application/octet-stream` for most real records —
    // a DataMap chunk carries no content type — so trusting `hinted` alone sent
    // every real image and PDF to a download instead of showing it.
    //
    // The author's own trailing name, when there is one, resolves the generic
    // end, exactly as it does for a local file. An error is not a failure here:
    // `detect_mime_prefix` cannot fail on non-empty input, and falling back to
    // the hint means a surprise still yields an answer the page can act on.
    let guess = if prefix.is_empty() {
        hinted.to_string()
    } else {
        av_ingest::mime::detect_mime_prefix(prefix, info.inferred_filename.as_deref())
            .unwrap_or_else(|_| hinted.to_string())
    };

    json_response(
        json!({
            "ok": true,
            "filename": av_ingest::filename::suggest_filename_with(
                &uri,
                stored_filename(state, &uri).as_deref(),
                &guess,
            ),
            "renderable": av_ingest::mime::is_browser_renderable(&guess),
            "mime_type": guess,
        })
        .to_string(),
    )
}

/// `GET /api/blocked?location=...` — the bytes-free gate the page asks before it
/// lets the SDK touch the network.
///
/// A browser WebRTC fetch goes through no server route, so the open and index
/// flows ask this first: `200 {"ok":true,"blocked":false}` means go ahead, and
/// the `403 blocked` / `503 block_check_failed` refusals come straight from
/// [`refuse_if_blocked`], identical to what every fetching route would have
/// answered. One code path, so the page and the server cannot disagree about
/// what is blocked.
pub fn blocked(state: &Arc<AppState>, query: &str) -> WebResponse {
    let params = parse_query(query);
    let Some(location) = param(&params, "location") else {
        return json_error(400, "validation_error", "missing 'location'");
    };
    let location = location.trim();
    if location.is_empty() {
        return json_error(400, "validation_error", "missing 'location'");
    }
    let uri = av_ingest::location::normalize_uri(location);
    if let Err(err) = refuse_if_blocked(state, &uri) {
        return err;
    }
    json_response(json!({ "ok": true, "blocked": false }).to_string())
}

/// `POST /api/index/bytes` — raw body is the file; metadata arrives in headers.
///
/// The browser half of indexing: the SDK has already fetched the bytes over
/// WebRTC, so this stores exactly what the daemon path would have stored, with
/// no second network round trip and no dependency installed.
pub fn index_bytes(
    state: &Arc<AppState>,
    uri: &str,
    tags: &str,
    force: bool,
    body: &[u8],
) -> WebResponse {
    let uri = av_ingest::location::normalize_uri(uri.trim());
    if uri.is_empty() {
        return json_error(400, "validation_error", "missing 'X-Anta-Uri'");
    }
    // Before the provider loads and before any store write: a blocked
    // location is refused here exactly as the JSON `/api/index` route and
    // every fetch route refuse it. The content-hash check inside
    // `index_resource_from_bytes` cannot stand in for this one — a block is
    // keyed by location (the row it deleted is gone, so its id cannot match
    // the id new bytes would compute), and re-indexing the address the user
    // blocked would write the very row the block removed.
    if let Err(err) = refuse_if_blocked(state, &uri) {
        return err;
    }

    let tags: Vec<String> = tags
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
    let startup = match state.startup.read() {
        Ok(s) => s,
        Err(_) => return json_error(500, "state_error", "startup state is poisoned"),
    };

    match crate::cmd::index::index_resource_from_bytes(
        &startup, &conn, &provider, body, &uri, &tags, force,
    ) {
        Ok(outcome) => json_response(outcome_json(&outcome).to_string()),
        Err(e) => json_error(500, "index_failed", &e.to_string()),
    }
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

    // Indexing reaches the network on the user's behalf — `index_resource`
    // verifies the URI and downloads it — so it refuses here, before any
    // dependency probing, exactly as the browser half refuses before its fetch.
    if let Err(err) = refuse_if_blocked(state, &uri) {
        return err;
    }

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

    json_response(outcome_json(&outcome).to_string())
}

/// The JSON body both index routes return.
///
/// Shared so the daemon path (`POST /api/index`) and the browser path
/// (`POST /api/index/bytes`) cannot drift apart in what they report: the same
/// run through either transport has to produce the same response, or "indexed"
/// would mean two different things depending on which transport the browser
/// happened to pick.
fn outcome_json(outcome: &crate::cmd::index::IndexOutcome) -> serde_json::Value {
    json!({
        "ok": true,
        "resource_id": outcome.resource_id,
        "location": outcome.location,
        "mime_type": outcome.mime_type,
        "kind": outcome.kind,
        "description": outcome.description,
        "embedding_dim": outcome.embedding_dim,
        "bytes_len": outcome.bytes_len,
        "duplicate": outcome.duplicate,
        "duplicate_by": outcome.duplicate_by,
        "broadcast": outcome.broadcast,
    })
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

/// `POST /api/services/ensure` — start x0x and the listener on demand.
///
/// Blocking: it returns only once the services are up (or known not to be
/// startable), so a search that called it can fan out on its next line. Safe to
/// call when everything is already running.
pub fn ensure_services(state: &Arc<AppState>) -> WebResponse {
    let report = state.ensure_services();
    json_response(
        json!({
            "ok": report.x0x_running,
            "x0x_running": report.x0x_running,
            "x0x_started": report.x0x_started,
            "listener_running": report.listener_running,
            "listener_started": report.listener_started,
            "detail": report.detail,
        })
        .to_string(),
    )
}

/// `POST /api/deps/x0x/stop` — stop the default x0x daemon.
///
/// Momentary by design: the next search/index will start it again.
pub fn stop_x0x(state: &Arc<AppState>) -> WebResponse {
    let stopped = crate::startup::stop_x0x_daemon();
    state.refresh_network_deps();
    json_response(
        json!({
            "ok": stopped,
            "dependency": "x0x",
            "running": !stopped,
            "detail": if stopped {
                "x0x daemon stopped."
            } else {
                "Could not stop the x0x daemon."
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

/// Advice for a successful install: Anta-Vista finds the binary in the
/// installer's home directory on its own, but the user's own terminal will not
/// unless they take the PATH advice the installer itself printed.
fn installed_hint(name: &str) -> String {
    if cfg!(target_os = "windows") {
        // Neither Windows installer modifies PATH; it only drops files into
        // %USERPROFILE%\.local\bin. "Open a new terminal" would be a lie.
        format!(
            "Installed. Anta-Vista finds {name} automatically; for your own \
             terminal, add %USERPROFILE%\\.local\\bin to PATH."
        )
    } else {
        format!(
            "Installed. Anta-Vista finds {name} in ~/.local/bin automatically; \
             for your own terminal, add it to PATH: \
             export PATH=\"$HOME/.local/bin:$PATH\""
        )
    }
}

/// `POST /api/deps/x0x/install` — runs the platform installer.
///
/// The UI shows the exact command from `/api/deps` first and requires a
/// separate click before calling this, so an install is never implicit.
///
/// `ok` here means *the request ran*, not *the dependency now works*: the
/// installer is a third-party script and can fail in ways only its log shows,
/// so the transport stays `ok: true` and the outcome lives in `installed`,
/// `running`, `log` and `hint`.
pub fn install_x0x(state: &Arc<AppState>) -> WebResponse {
    let command = crate::startup::x0x_install_command();
    if crate::startup::x0x_binary_available() {
        return json_response(
            json!({
                "ok": true,
                "dependency": "x0x",
                "installed": true,
                "already_installed": true,
                "running": crate::startup::x0x_running(),
                "command": command,
                "log": "",
                "hint": null,
            })
            .to_string(),
        );
    }
    let (ran_ok, log) = crate::startup::install_x0x_with();
    state.refresh_network_deps();
    let installed = crate::startup::x0x_binary_available();
    let running = crate::startup::x0x_running();
    let hint = if !installed {
        Some(format!(
            "The installer {} without leaving a working x0x binary. \
             Read the log below — most often the download failed, or the \
             binary landed somewhere unexpected.",
            if ran_ok {
                "exited successfully"
            } else {
                "failed"
            },
        ))
    } else if !running {
        Some(
            "x0x is installed, but its daemon is not answering yet. \
             Try Start in the status drawer, or run `x0x start` in a terminal."
                .to_string(),
        )
    } else {
        Some(installed_hint("x0x"))
    };
    json_response(
        json!({
            "ok": true,
            "dependency": "x0x",
            "installed": installed,
            "running": running,
            "command": command,
            "log": log,
            "hint": hint,
        })
        .to_string(),
    )
}

/// `POST /api/deps/ant/install`
///
/// Same contract as [`install_x0x`]: `ok` is transport-level; the outcome is
/// in `installed`/`log`/`hint`.
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
                "log": "",
                "hint": null,
            })
            .to_string(),
        );
    }
    let (ran_ok, log) = crate::startup::install_ant_cli_with();
    // Detection consults the installer's candidate directories as well as
    // PATH, so a binary outside this process's PATH is still found here.
    state.refresh_network_deps();
    let installed = crate::startup::ant_cli_binary_available();
    let hint = if !installed {
        Some(format!(
            "The installer {} without leaving a working ant binary. \
             Read the log below — most often the download failed, or the \
             binary landed somewhere unexpected.",
            if ran_ok {
                "exited successfully"
            } else {
                "failed"
            },
        ))
    } else {
        Some(installed_hint("ant"))
    };
    json_response(
        json!({
            "ok": true,
            "dependency": "ant",
            "installed": installed,
            "command": command,
            "log": log,
            "hint": hint,
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

    let report = match crate::cmd::block::block_report(&conn) {
        Ok(r) => r,
        Err(e) => return json_error(500, "database_error", &e.to_string()),
    };

    json_response(report.to_json().to_string())
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

/// `POST /api/purge` — remove one locally indexed resource.
///
/// Backs the "Remove from local index" button. This frees local storage and
/// nothing else: it writes no block, so a peer holding the same link can hand
/// it back on a later search. The response names the counts so the page reports
/// what actually went rather than implying a clean sweep.
///
/// An id we do not hold is a success with zero counts, not a 404 — "already
/// gone" is the state the user asked for, and the row is only visible in the UI
/// because it was there a moment ago.
pub fn purge(state: &Arc<AppState>, body: &str) -> WebResponse {
    let payload: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return json_error(400, "validation_error", &format!("invalid JSON: {e}")),
    };

    let resource_id = payload["resource_id"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if resource_id.is_empty() {
        return json_error(400, "validation_error", "missing 'resource_id'");
    }

    let mut conn = match db_conn(state) {
        Ok(c) => c,
        Err(r) => return r,
    };

    match crate::cmd::purge::purge_resource(&mut conn, &resource_id) {
        Ok(o) => json_response(
            json!({
                "ok": true,
                "resource_id": resource_id,
                "deleted": {
                    "resources": o.resources,
                    "embeddings": o.embeddings,
                    "feedback_events": o.feedback_events,
                },
                "warning": "Freed local storage only. A peer holding this link can still \
                            return it; use block to stop that.",
            })
            .to_string(),
        ),
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

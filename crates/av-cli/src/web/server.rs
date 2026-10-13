use std::io::{Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use super::handlers;
use super::render;
use super::state::AppState;

/// Header carrying the location on the two binary endpoints, whose request body
/// is the file itself and so has no room for a JSON envelope.
const URI_HEADER: &str = "X-Anta-Uri";
const TAGS_HEADER: &str = "X-Anta-Tags";
const FORCE_HEADER: &str = "X-Anta-Force";
/// The MIME the page believes the fetched bytes have, used only to decide
/// whether a browser can display them. It is a hint, not an assertion: the
/// server still sniffs the bytes whenever it actually has them.
const MIME_HEADER: &str = "X-Anta-Mime";

/// Number of worker threads. Search fans out to peers over HTTP and can block
/// for seconds, so a single-threaded server would let one slow query wedge the
/// whole UI. Requests are otherwise cheap and SQLite connections are
/// per-request, so a small fixed pool is plenty.
const WORKERS: usize = 4;

/// Largest request body we will buffer, to bound memory from a stray upload.
const MAX_BODY: usize = 32 * 1024 * 1024;

/// How many leading bytes `POST /api/view/plan` will accept.
///
/// Every MIME decision worth making is made in the first few hundred bytes —
/// magic numbers are shorter still, and the HTML check reads 512 — so this is
/// generous rather than tuned. It exists to bound the request, not to truncate
/// a signal: a caller sending more is a bug worth reporting.
const PLAN_PREFIX_CAP: usize = 16 * 1024;

/// tiny_http buffers string bodies in a `Cursor`; one alias keeps signatures short.
///
/// Not boxed. A `Response<R>` can be handed any reader, which looks like a way to
/// stream a response — and it is not one, here. tiny_http writes the body with
/// `io::copy` through `chunked_transfer::Encoder`, which buffers 8 KB and never
/// flushes between reads, so nothing reaches the socket until the reader is
/// exhausted. `/api/view/fetch` would have answered with a single trailing
/// burst, exactly like the buffered reply it replaced. Progress therefore travels
/// by polling; see `fetch_jobs`.
pub type WebResponse = Response<Cursor<Vec<u8>>>;

/// Bind the server, spawn the worker pool, and block until shutdown.
///
/// Returns once every worker has stopped.
pub fn serve(state: Arc<AppState>, server: Server, run_for: Option<u64>) -> Result<(), String> {
    let server = Arc::new(server);
    let shutdown = Arc::new(AtomicBool::new(false));

    if let Some(secs) = run_for {
        let flag = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            flag.store(true, Ordering::SeqCst);
        });
    }

    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let srv = Arc::clone(&server);
        let st = Arc::clone(&state);
        let flag = Arc::clone(&shutdown);
        handles.push(std::thread::spawn(move || {
            worker_loop(&srv, &st, &flag);
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    // Retire any viewer listeners still holding a port and a temp file, rather
    // than leaving them to outlive the server that authorised them.
    super::viewer::stop_all();

    Ok(())
}

/// How often a blocked worker re-checks the shutdown flag. This is a poll
/// rather than a wakeup because `Server::unblock` releases only a single
/// waiting thread, and every worker here is waiting.
const SHUTDOWN_POLL: Duration = Duration::from_millis(200);

fn worker_loop(server: &Server, state: &Arc<AppState>, shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::SeqCst) {
        let request = match server.recv_timeout(SHUTDOWN_POLL) {
            Ok(Some(r)) => r,
            // Timed out with nothing to do: loop round and re-check shutdown.
            Ok(None) => continue,
            // The server itself is gone.
            Err(_) => return,
        };
        // Handle each request on its own thread so one slow handler cannot
        // block the others. `Request` is owned by the closure because
        // `respond` consumes it.
        let state = state.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle(request, state) {
                tracing::warn!("failed to answer request: {e}");
            }
        });
    }
}

fn handle(mut request: Request, state: Arc<AppState>) -> Result<(), String> {
    let url = request.url().to_string();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url, String::new()),
    };
    let method = request.method().clone();

    // DNS-rebinding guard: a page on another origin can still send requests to
    // localhost, so only accept a Host naming this machine's loopback interface
    // (or the address we actually bound).
    if let Err(resp) = check_host(&request, &state) {
        return request.respond(resp).map_err(|e| e.to_string());
    }

    // Second, independent guard, and neither substitutes for the other. The
    // Host check cannot see a cross-origin request that legitimately names us;
    // this one cannot see a non-browser caller. Only state-changing methods are
    // gated, because a cross-site GET of a page is harmless.
    if method == Method::Post
        && let Err(resp) = check_fetch_site(&request)
    {
        return request.respond(resp).map_err(|e| e.to_string());
    }

    let response = route(&mut request, &method, &path, &query, &state);

    let response = match response {
        Ok(r) => r,
        Err(RouteError::Ready(r)) => r,
        Err(RouteError::Internal(msg)) => json_error(500, "internal_error", &msg),
    };
    request.respond(response).map_err(|e| e.to_string())
}

fn route(
    request: &mut Request,
    method: &Method,
    path: &str,
    query: &str,
    state: &Arc<AppState>,
) -> Result<WebResponse, RouteError> {
    match (method, path) {
        (_, "/") | (_, "/index.html") => Ok(render::index_page(state)),

        // The theme is a preference, not a server setting, so the client keeps
        // it in localStorage; these are just the stylesheets themselves.
        (_, "/assets/retro.css") => Ok(render::retro_css()),
        (_, "/assets/modern.css") => Ok(render::modern_css()),
        (_, "/assets/app.js") => Ok(render::app_js()),
        (_, "/assets/retro.js") => Ok(render::retro_js()),
        // The header logo, one per theme; the page references both and CSS
        // shows the one matching <html data-theme>.
        (_, "/assets/wordmark-retro.jpeg") => Ok(render::wordmark_retro()),
        (_, "/assets/wordmark-modern.jpeg") => Ok(render::wordmark_modern()),

        // The cheapest route here: no state, no database, no probe. A second
        // `av web` hitting a busy port asks this before concluding the
        // squatter is another copy of the program.
        (Method::Get, "/api/ping") => Ok(handlers::ping()),
        (Method::Get, "/api/status") => Ok(handlers::status(state)),
        (Method::Get, "/api/deps") => Ok(handlers::deps(state)),
        (Method::Get, "/api/search") => Ok(handlers::search(state, query)),
        (Method::Get, "/api/download") => Ok(handlers::download(state, query)),
        // One segment of a path, matched by a prefix arm rather than a literal,
        // because the job id is not known ahead of time and tiny_http gives no
        // capture groups. The handler rejects anything that is not a job id
        // shape, so this cannot be widened into a prefix match by accident.
        (Method::Get, p) if p.starts_with("/api/view/fetch/") => Ok(handlers::view_fetch_status(
            p.trim_start_matches("/api/view/fetch/"),
            query,
        )),
        (Method::Get, "/api/blocks") => Ok(handlers::blocks_list(state)),
        // Stands alone on its own origin: no app.js, no API access, and the only
        // input is an enum, so an SDK error string can never be reflected here.
        (Method::Get, "/view/error") => Ok(render::view_error_page(query)),
        // Also standalone and app-origin, unlike `/view/error`: this page is
        // navigated to by the tab we opened and then written to by `postMessage`,
        // and it holds no peer bytes — those stay on the viewer port, so a second
        // origin still separates them from `/api/*`.
        (Method::Get, "/view/progress") => Ok(render::view_progress_page()),

        (Method::Post, p) => {
            // Binary bodies are dispatched *before* the UTF-8 read below, which
            // would reject them outright. These two carry an uploaded file, so
            // there is nothing to parse as JSON.
            match p {
                // No request body at all, and that is the point: the server does
                // the fetching, so a record larger than `MAX_UPLOAD` is no longer
                // reachable only by browsers that can carry it. It has to be
                // matched before the arms that read a body, because there is
                // nothing here to read.
                //
                // Answers immediately with a job id. The fetch runs on its own
                // thread and the page polls for events, because tiny_http cannot
                // stream a response — see `fetchjob`.
                "/api/view/fetch" => {
                    let uri = header(request, URI_HEADER).unwrap_or_default();
                    Ok(handlers::view_fetch(state, &uri))
                }
                "/api/index/bytes" => {
                    // Metadata rides in headers because the body is the file.
                    let uri = header(request, URI_HEADER).unwrap_or_default();
                    let tags = header(request, TAGS_HEADER).unwrap_or_default();
                    let force = header(request, FORCE_HEADER).as_deref() == Some("1");
                    let body = read_body_bytes(request).map_err(RouteError::ready)?;
                    Ok(handlers::index_bytes(state, &uri, &tags, force, &body))
                }
                "/api/view/blob" => {
                    let uri = header(request, URI_HEADER).unwrap_or_default();
                    let body = read_body_bytes(request).map_err(RouteError::ready)?;
                    Ok(handlers::view_blob(state, &uri, &body))
                }
                // A bounded prefix of the bytes, never the whole file. It exists
                // so the page can learn a filename it cannot invent itself, and
                // whether the record is worth displaying, without uploading the
                // file to ask — the mistake that made a 64 MB record fail with a
                // "request body is larger" error after a successful fetch.
                //
                // The prefix is what makes that verdict honest: the Autonomi SDK
                // reports `application/octet-stream` for most records, so
                // trusting the reported type alone sent real images and PDFs to
                // a download.
                "/api/view/plan" => {
                    let uri = header(request, URI_HEADER).unwrap_or_default();
                    let mime = header(request, MIME_HEADER).unwrap_or_default();
                    let prefix =
                        read_body_capped(request, PLAN_PREFIX_CAP).map_err(RouteError::ready)?;
                    Ok(handlers::view_plan(state, &uri, &mime, &prefix))
                }
                _ => {
                    let body = read_body(request)?;
                    Ok(match p {
                        "/api/rate" => handlers::rate(state, &body),
                        "/api/index" => handlers::index(state, &body),
                        "/api/block" => handlers::block(state, &body),
                        "/api/purge" => handlers::purge(state, &body),
                        "/api/unblock" => handlers::unblock(state, &body),
                        "/api/listener/stop" => handlers::listener_stop(state),
                        "/api/listener/start" => handlers::listener_start(state),
                        "/api/deps/x0x/start" => handlers::start_x0x(state),
                        "/api/deps/x0x/stop" => handlers::stop_x0x(state),
                        "/api/services/ensure" => handlers::ensure_services(state),
                        "/api/deps/antd/start" => handlers::start_antd(state),
                        "/api/deps/x0x/install" => handlers::install_x0x(state),
                        "/api/deps/ant/install" => handlers::install_ant(state),
                        other => {
                            json_error(404, "not_found", &format!("no API route for POST {other}"))
                        }
                    })
                }
            }
        }

        (Method::Get, p) if p.starts_with("/api/") => Ok(json_error(
            404,
            "not_found",
            &format!("no API route for GET {p}"),
        )),

        // The vendored Autonomi SDK: ES modules and the WASM binary, served from
        // a versioned prefix rather than the hand-listed asset arms above,
        // because the file set is whatever its import graph reaches.
        (_, p) => match render::vendor_rel_path(p) {
            Some(rel) => Ok(render::vendor_asset(rel).unwrap_or_else(|| not_found(p))),
            None => Ok(not_found(p)),
        },
    }
}

fn not_found(path: &str) -> WebResponse {
    text_response(
        404,
        "text/plain; charset=utf-8",
        &format!("Not found: {path}\n"),
    )
}

fn read_body(request: &mut Request) -> Result<String, String> {
    let mut buf = Vec::new();
    Read::take(request.as_reader(), MAX_BODY as u64)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| format!("request body is not valid UTF-8: {e}"))
}

/// Why a route could not produce a response.
///
/// Two kinds, and the distinction is load-bearing: [`Self::Ready`] is a refusal
/// that is *already* the right answer (an oversized body is a 400 with a code the
/// browser branches on), and collapsing it into `Internal` would report a
/// client-side problem as a 500 and tell the user the server broke.
enum RouteError {
    Ready(WebResponse),
    Internal(String),
}

impl RouteError {
    fn ready(response: WebResponse) -> Self {
        Self::Ready(response)
    }
}

impl From<String> for RouteError {
    fn from(msg: String) -> Self {
        Self::Internal(msg)
    }
}

/// Read a binary request body, refusing anything over [`MAX_BODY`].
///
/// Returns a ready-made 400 rather than a bare message: an oversized upload is
/// the caller's problem to fix by taking the daemon route, and a 500 would say
/// the server broke when it did not. The previous JSON `read` silently truncated
/// at the cap via `Read::take`, which would have indexed a corrupt prefix of a
/// large file and reported success — so the cap is *refused*, never clamped.
fn read_body_bytes(request: &mut Request) -> Result<Vec<u8>, WebResponse> {
    let mut buf = Vec::new();
    // Read one byte past the cap so "exactly at the limit" stays allowed and
    // "over it" is detectable, instead of the cap being a rounding error.
    Read::take(request.as_reader(), MAX_BODY as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| json_error(400, "read_failed", &e.to_string()))?;
    if buf.len() > MAX_BODY {
        return Err(json_error(
            400,
            "body_too_large",
            &format!(
                "request body is larger than the {MAX_BODY} byte limit; \
                 this file has to be fetched by the local daemon instead"
            ),
        ));
    }
    Ok(buf)
}

/// Read a request body up to `cap` bytes, refusing anything larger.
///
/// Same rule as [`read_body_bytes`] — refused, never clamped — but with a cap of
/// the caller's choosing rather than the upload limit. A truncated prefix would
/// still be a valid prefix, so clamping here would not corrupt anything, but a
/// silent clamp hides a page that is uploading far more than it means to, and
/// this endpoint's whole premise is that it uploads very little.
fn read_body_capped(request: &mut Request, cap: usize) -> Result<Vec<u8>, WebResponse> {
    let mut buf = Vec::new();
    Read::take(request.as_reader(), cap as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| json_error(400, "read_failed", &e.to_string()))?;
    if buf.len() > cap {
        return Err(json_error(
            400,
            "body_too_large",
            &format!("request body is larger than the {cap} byte limit"),
        ));
    }
    Ok(buf)
}

/// Read one request header by name.
///
/// `&'static str` rather than `&str`: `Header::equiv` needs the name to outlive
/// `'static`, which is why the guards above can only pass literals and consts.
fn header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

/// Reject cross-site state-changing requests.
///
/// A page on any origin can issue a request to this loopback server, and the
/// Host header cannot catch it, because such a request genuinely arrives
/// carrying our own Host. Browsers additionally send `Sec-Fetch-Site`, which
/// they compute themselves and script cannot forge, so a POST arriving as
/// `cross-site` came from a page we did not serve. Without this, peer-supplied
/// content viewed in one of our own origins could drive our API.
///
/// Deliberately permissive when the header is absent: this defends against
/// browser CSRF from other origins, it is not authentication. `curl` and the
/// test harness send no such header and have to keep working, and an attacker
/// who is not a browser is not stopped by anything here.
fn check_fetch_site(request: &Request) -> Result<(), WebResponse> {
    let site = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Sec-Fetch-Site"))
        .map(|h| h.value.as_str())
        .unwrap_or_default();

    if site.eq_ignore_ascii_case("cross-site") {
        return Err(json_error(
            403,
            "cross_site_request",
            "Refusing a state-changing request that came from another site.",
        ));
    }
    Ok(())
}

/// Reject requests whose `Host` header names somewhere other than this machine.
///
/// A page served from the open internet can still issue requests to
/// `http://127.0.0.1:8787`; browsers attach the correct Host for the URL they
/// targeted, so an unexpected value means the request came from an origin the
/// user did not intend to grant loopback access.
fn check_host(request: &Request, state: &AppState) -> Result<(), WebResponse> {
    let raw = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Host"))
        .map(|h| h.value.as_str())
        .filter(|v| !v.is_empty());

    let Some(raw) = raw else {
        return Err(text_response(
            400,
            "text/plain; charset=utf-8",
            "Missing Host header.\n",
        ));
    };

    // Strip the port. Bracketed IPv6 literals keep their brackets until here.
    let host = match raw.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => raw,
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');

    let allowed = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == state.bound_host;

    if allowed {
        Ok(())
    } else {
        Err(text_response(
            403,
            "text/plain; charset=utf-8",
            "Refusing cross-origin request.\n",
        ))
    }
}

pub fn json_response(body: String) -> WebResponse {
    Response::from_string(body)
        .with_status_code(StatusCode(200))
        .with_header(content_type("application/json; charset=utf-8"))
        .with_header(Header::from_bytes("Cache-Control", "no-store").expect("static header"))
        .with_header(
            Header::from_bytes("X-Content-Type-Options", "nosniff").expect("static header"),
        )
}

pub fn json_error(status: u16, code: &str, detail: &str) -> WebResponse {
    json_error_fields(status, code, detail, serde_json::json!({}))
}

/// A JSON error carrying extra top-level fields, for errors that have something
/// actionable to hand back — such as the platform install command the UI must
/// show before the user consents to running it.
pub fn json_error_fields(
    status: u16,
    code: &str,
    detail: &str,
    extra: serde_json::Value,
) -> WebResponse {
    let mut body = serde_json::json!({
        "ok": false,
        "error": code,
        "detail": detail,
    });
    if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in source {
            target.insert(k.clone(), v.clone());
        }
    }
    let body = body.to_string();
    Response::from_string(body)
        .with_status_code(StatusCode(status))
        .with_header(content_type("application/json; charset=utf-8"))
        .with_header(Header::from_bytes("Cache-Control", "no-store").expect("static header"))
        .with_header(
            Header::from_bytes("X-Content-Type-Options", "nosniff").expect("static header"),
        )
}

pub fn text_response(status: u16, mime: &str, body: &str) -> WebResponse {
    Response::from_string(body.to_string())
        .with_status_code(StatusCode(status))
        .with_header(content_type(mime))
}

/// A text response with extra headers of its own.
///
/// For the one page that wants a policy [`text_response`] does not express: a
/// `Content-Security-Policy` that forbids this page from making any request of
/// its own. Everything it shows arrives by `postMessage`.
pub fn text_response_with(status: u16, mime: &str, body: &str, headers: &[Header]) -> WebResponse {
    let mut response = Response::from_string(body.to_string())
        .with_status_code(StatusCode(status))
        .with_header(content_type(mime));
    for header in headers {
        response = response.with_header(header.clone());
    }
    response
}

/// A response carrying raw bytes rather than text.
///
/// Used for the vendored WASM — a browser refuses to stream-compile it unless
/// it arrives as `application/wasm`, so the type is load-bearing rather than
/// cosmetic — and for serving a downloaded file to the one-shot viewer.
pub fn binary_response(status: u16, mime: &str, body: Vec<u8>) -> WebResponse {
    Response::from_data(body)
        .with_status_code(StatusCode(status))
        .with_header(content_type(mime))
        .with_header(
            Header::from_bytes("X-Content-Type-Options", "nosniff").expect("static header"),
        )
}

pub(crate) fn content_type(mime: &str) -> Header {
    Header::from_bytes("Content-Type", mime).expect("static content type is a valid header")
}

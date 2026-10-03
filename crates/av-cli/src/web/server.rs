use std::io::{Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use super::handlers;
use super::render;
use super::state::AppState;

/// Number of worker threads. Search fans out to peers over HTTP and can block
/// for seconds, so a single-threaded server would let one slow query wedge the
/// whole UI. Requests are otherwise cheap and SQLite connections are
/// per-request, so a small fixed pool is plenty.
const WORKERS: usize = 4;

/// Largest request body we will buffer, to bound memory from a stray upload.
const MAX_BODY: usize = 32 * 1024 * 1024;

/// tiny_http buffers string bodies in a `Cursor`; one alias keeps signatures short.
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

    let response = route(&mut request, &method, &path, &query, &state);

    let response = match response {
        Ok(r) => r,
        Err(msg) => json_error(500, "internal_error", &msg),
    };
    request.respond(response).map_err(|e| e.to_string())
}

fn route(
    request: &mut Request,
    method: &Method,
    path: &str,
    query: &str,
    state: &Arc<AppState>,
) -> Result<WebResponse, String> {
    match (method, path) {
        (_, "/") | (_, "/index.html") => Ok(render::index_page(state)),

        // The theme is a preference, not a server setting, so the client keeps
        // it in localStorage; these are just the stylesheets themselves.
        (_, "/assets/retro.css") => Ok(render::retro_css()),
        (_, "/assets/modern.css") => Ok(render::modern_css()),
        (_, "/assets/app.js") => Ok(render::app_js()),
        (_, "/assets/retro.js") => Ok(render::retro_js()),

        (Method::Get, "/api/status") => Ok(handlers::status(state)),
        (Method::Get, "/api/deps") => Ok(handlers::deps(state)),
        (Method::Get, "/api/search") => Ok(handlers::search(state, query)),
        (Method::Get, "/api/download") => Ok(handlers::download(state, query)),
        (Method::Get, "/api/blocks") => Ok(handlers::blocks_list(state)),

        (Method::Post, p) => {
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
                "/api/deps/antd/start" => handlers::start_antd(state),
                "/api/deps/x0x/install" => handlers::install_x0x(state),
                "/api/deps/ant/install" => handlers::install_ant(state),
                other => json_error(404, "not_found", &format!("no API route for POST {other}")),
            })
        }

        (Method::Get, p) if p.starts_with("/api/") => Ok(json_error(
            404,
            "not_found",
            &format!("no API route for GET {p}"),
        )),

        _ => Ok(text_response(
            404,
            "text/plain; charset=utf-8",
            "Not found. The web interface is served from '/'.\n",
        )),
    }
}

fn read_body(request: &mut Request) -> Result<String, String> {
    let mut buf = Vec::new();
    Read::take(request.as_reader(), MAX_BODY as u64)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| format!("request body is not valid UTF-8: {e}"))
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

fn content_type(mime: &str) -> Header {
    Header::from_bytes("Content-Type", mime).expect("static content type is a valid header")
}

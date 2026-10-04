//! Serving a downloaded file from a second, throwaway loopback origin.
//!
//! # Why a second origin at all
//!
//! The obvious way to show downloaded bytes is a `blob:` URL built by the page
//! that fetched them. That does not work here: a blob URL inherits the origin of
//! the page that created it, so the document would be *same-origin* with the
//! loopback server. Peer-supplied HTML would then run with access to `/api/*`,
//! and the Host guard would not help, because such a request genuinely arrives
//! carrying our own Host. The http(s) results are safe only because navigating to
//! them leaves our origin behind entirely.
//!
//! So the bytes are POSTed to us and re-served from an ephemeral port. A
//! different port is a different origin, which is exactly the isolation we need,
//! and it costs one socket and one thread per download.
//!
//! Deliberately *not* additionally sandboxed with a `Content-Security-Policy:
//! sandbox` header: the port is the boundary, and sandboxing would also break
//! legitimate pages. The one residual — peer content issuing a simple
//! cross-origin `POST` back to the main port — is closed by the `Sec-Fetch-Site`
//! guard in [`super::server`], which sees such a request as `cross-site`.
//!
//! # Lifetime
//!
//! One listener per file, serving one unguessable token path and nothing else,
//! for a short grace period so a reload still works. The bytes live in a
//! [`NamedTempFile`] owned by the listener thread, so the file is deleted when
//! the thread exits — on expiry, on [`stop_all`], or on process exit — with no
//! cleanup path to forget.

use std::io::{Read, Seek, Write};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tempfile::Builder;
use tiny_http::{Method, Request, Response, Server, StatusCode};
use uuid::Uuid;

/// How long a view stays available after it is created.
///
/// Long enough that a reload or a back/forward within the session still works,
/// short enough that a forgotten tab does not leave a file and a listening
/// socket around indefinitely.
const VIEW_TTL: Duration = Duration::from_secs(120);

/// How often the listener re-checks its deadline and the stop flag.
const POLL: Duration = Duration::from_millis(200);

/// Set when the web server is shutting down, so listeners retire promptly
/// instead of holding their sockets for the rest of the TTL.
///
/// A static rather than a field on `AppState` because these threads are spawned
/// from request handlers that do not carry the app's shutdown flag, which is
/// itself local to `server::serve`.
///
/// Deliberately one-way: nothing clears it. Resetting it would be wrong, not
/// merely unnecessary — once `stop_all` has run, the process is on its way out,
/// so a later `publish` should not hand out a URL whose listener will expire
/// instantly.
static STOPPED: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(false));

/// Ask every live viewer to shut down. Called when the web server stops.
pub fn stop_all() {
    STOPPED.store(true, Ordering::SeqCst);
}

/// A view that has been created and is waiting to be opened.
pub struct ViewHandle {
    /// Absolute URL to open in a tab.
    pub url: String,
    /// Seconds until the listener retires, so the caller can say so.
    pub expires_in: u64,
}

/// Publish `bytes` on a fresh loopback port and return the URL to open.
///
/// The path carries an unguessable token so the listener answers exactly one
/// resource, and `mime` should come from
/// [`av_ingest::mime::detect_mime_with_hint`] so the type served here agrees
/// with the type stored for the same bytes.
pub fn publish(bytes: &[u8], mime: &str, filename: &str) -> Result<ViewHandle, String> {
    // Bind before writing anything, so a port conflict costs nothing.
    let server =
        Server::http("127.0.0.1:0").map_err(|e| format!("could not bind a viewer port: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| "viewer bound an address that is not an IP".to_string())?;

    let mut file = Builder::new()
        .prefix("av-view-")
        .tempfile()
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;

    let token = Uuid::new_v4();
    let path = format!("/v/{token}");
    let url = format!("http://127.0.0.1:{port}{path}");
    let mime = mime.to_string();
    let filename = filename.to_string();

    std::thread::spawn(move || {
        serve_one(server, file, path, mime, filename);
    });

    Ok(ViewHandle {
        url,
        expires_in: VIEW_TTL.as_secs(),
    })
}

/// Serve one file at one token path until the grace period ends.
fn serve_one(
    server: Server,
    mut file: tempfile::NamedTempFile,
    path: String,
    mime: String,
    filename: String,
) {
    let deadline = std::time::Instant::now() + VIEW_TTL;

    while std::time::Instant::now() < deadline {
        if STOPPED.load(Ordering::SeqCst) {
            return;
        }

        let request = match server.recv_timeout(POLL) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            // The server is gone; nothing left to serve.
            Err(_) => return,
        };

        let mut request = request;
        let response = respond(&mut request, &path, &mime, &filename, &mut file);
        // A dropped connection here is the browser navigating away mid-response.
        // Nothing to do about it and nothing to report: the file stays until the
        // deadline either way.
        let _ = request.respond(response);
    }
}

fn respond(
    request: &mut Request,
    path: &str,
    mime: &str,
    filename: &str,
    file: &mut tempfile::NamedTempFile,
) -> Response<std::io::Cursor<Vec<u8>>> {
    // Read-only, so refuse anything that is not a plain fetch of the document.
    // There is no other route on this listener to protect, but being explicit
    // keeps that true if one is ever added.
    if request.method() != &Method::Get && request.method() != &Method::Head {
        return plain(405, "Only GET is supported here.\n");
    }

    // Same guard as the main server, applied to this listener's own address. It
    // strips the port, so this rejects a request that reached us through a
    // hostname the user did not intend to grant loopback access to.
    if !host_is_loopback(request) {
        return plain(403, "Refusing cross-origin request.\n");
    }

    if request.url() != path {
        return plain(404, "Not found.\n");
    }

    let mut buf = Vec::new();
    if let Err(e) = file
        .seek(std::io::SeekFrom::Start(0))
        .and_then(|_| file.read_to_end(&mut buf))
    {
        return plain(500, &format!("could not read the stored file: {e}\n"));
    }

    // `Content-Type` is built without `header()`: a caller passing a string with
    // a newline in it must not be able to panic this thread, and it must not be
    // able to inject a second header either. Today `mime` comes from
    // `detect_mime_with_hint` and is always a fixed canonical type, so this is
    // belt-and-braces for the next caller rather than a live bug — but a header
    // value is exactly the place to be untrusting.
    let mut response = Response::from_data(buf).with_status_code(StatusCode(200));
    response = match tiny_http::Header::from_bytes("Content-Type", mime) {
        Ok(h) => response.with_header(h),
        Err(_) => response.with_header(header("Content-Type", "application/octet-stream")),
    };
    // Inline, so the browser renders it: a downloaded webpage has to open as a
    // webpage, which is the entire point of the stopgap. But only for a type a
    // browser can actually display — `inline` on an unrenderable type is a blank
    // window with no error, which is the failure this whole branch exists to
    // remove. The decision is made here as well as in the page, deliberately: if
    // the two ever disagree, this response still does the safe thing.
    let disposition = if av_ingest::mime::is_browser_renderable(mime) {
        header("Content-Disposition", "inline")
    } else {
        let value = attachment_of(filename);
        match tiny_http::Header::from_bytes("Content-Disposition", value.as_str()) {
            Ok(h) => h,
            // A filename that cannot be expressed as a header value still has to
            // be downloadable, so fall back to a bare attachment rather than
            // serving it inline.
            Err(_) => header("Content-Disposition", "attachment"),
        }
    };
    response
        .with_header(disposition)
        // The type is what we sniffed, not what the address suggests, and the
        // browser must not second-guess it by sniffing the bytes again.
        .with_header(header("X-Content-Type-Options", "nosniff"))
        // Peer content should not outlive the tab, or leak into a shared cache.
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Referrer-Policy", "no-referrer"))
}

/// A `Content-Disposition` value naming `filename`, with the quoting RFC 6266
/// asks for.
///
/// `suggest_filename` already reduces this to one path segment, but a viewer URL
/// is served on its own port and its response headers are the one place a
/// caller-supplied name reaches a parser that is not us, so a quote or a
/// newline is neutralised here rather than trusted. Escaped quotes inside a
/// quoted-string are legal, so this cannot terminate the value early.
fn attachment_of(filename: &str) -> String {
    let mut out = String::from("attachment; filename=\"");
    for c in filename.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\r' | '\n' => {}
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn host_is_loopback(request: &Request) -> bool {
    let raw = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Host"))
        .map(|h| h.value.as_str())
        .unwrap_or_default();
    if raw.is_empty() {
        return false;
    }

    let host = match raw.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => raw,
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');

    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn header(field: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(field, value)
        .unwrap_or_else(|_| panic!("static header {field}: {value} is invalid"))
}

fn plain(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body.to_string())
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
}

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
///
/// The filename is appended as a final path segment so the tab is titled
/// `welcome.md` instead of a uuid. It is decoration only: the **token is the
/// entire authorization**, and [`respond`] matches on the token whether or not a
/// name follows, so a viewer URL is no more or less private than before.
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
    let url = format!("http://127.0.0.1:{port}{}/{}", path, path_segment(filename));
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

    if !path_is_ours(request.url(), path) {
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
    // `filename` is sent for *both* dispositions, not just attachments. Without
    // it an inline image answers the browser's own "Save image as..." with the
    // last path segment — the uuid, with no extension — so right-clicking a
    // picture the user had just opened produced a file the OS could not open.
    // It does not name the tab (only the path does that); it names the file the
    // browser offers to write.
    let disposition = if av_ingest::mime::is_browser_renderable(mime) {
        match disposition_header("inline", filename) {
            Some(h) => h,
            // A name we cannot express is not a reason to stop displaying the
            // file, so a bare `inline` is the fallback rather than an attachment.
            None => header("Content-Disposition", "inline"),
        }
    } else {
        match disposition_header("attachment", filename) {
            Some(h) => h,
            // A filename that cannot be expressed as a header value still has to
            // be downloadable, so fall back to a bare attachment rather than
            // serving it inline.
            None => header("Content-Disposition", "attachment"),
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

/// Whether `url` addresses the file published at `path`.
///
/// `path` is the unguessable token and is the whole of the authorization. A
/// filename may be appended as one further segment purely so the tab is titled
/// with the file's name, so the token alone is still accepted and the name is
/// never required.
///
/// The trailing segment is validated rather than trusted, because the request
/// line is attacker-controlled and this listener serves peer content: one
/// segment of URL-unreserved characters or well-formed percent triplets, no
/// `.` or `..`, and a second `/` means the remainder is not a segment at all.
/// None of that weakens the token — it only decides whether a name is
/// *appended* to an already-authorized request.
fn path_is_ours(url: &str, path: &str) -> bool {
    if url == path {
        return true;
    }
    let Some(rest) = url.strip_prefix(path) else {
        return false;
    };
    let Some(segment) = rest.strip_prefix('/') else {
        // `/v/<token>x` — a longer token-looking prefix must not match.
        return false;
    };
    !segment.is_empty() && segment != "." && segment != ".." && is_name_segment(segment)
}

/// Whether `segment` is URL-unreserved characters and well-formed percent
/// triplets, and nothing else.
///
/// `%` has to be allowed, because [`path_segment`] emits `%20` for a space —
/// `suggest_filename` permits spaces in a name — so a charset check that stopped
/// at the unreserved set made every viewer URL with a space in the filename 404.
/// A *stray* `%` is still refused, so this is not a blanket exemption.
fn is_name_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            i += 1;
        } else if b == b'%' {
            let triplet = bytes.get(i + 1..i + 3).unwrap_or_default();
            if triplet.len() != 2 || !triplet.iter().all(u8::is_ascii_hexdigit) {
                return false;
            }
            i += 3;
        } else {
            return false;
        }
    }

    // Defence in depth for a future change that *does* use the name: the segment
    // is decoration today and never locates bytes, so `%2e%2e` is harmless right
    // now. Refusing it anyway keeps a traversal from being one refactor away.
    let decoded = percent_decode_lossy(segment);
    !decoded.is_empty() && decoded != "." && decoded != ".." && !decoded.contains(['/', '\\', '\0'])
}

/// Percent-decode for validation only; invalid escapes are left as written.
fn percent_decode_lossy(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(triplet) = bytes.get(i + 1..i + 3)
            && triplet.len() == 2
            && triplet.iter().all(u8::is_ascii_hexdigit)
        {
            let hex = std::str::from_utf8(triplet).unwrap_or("00");
            out.push(u8::from_str_radix(hex, 16).unwrap_or(b'%'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `filename` as a single URL path segment, percent-encoded.
///
/// `suggest_filename` reduces a name to `[A-Za-z0-9._- ]`, and a **space** is in
/// that set, so encoding is required rather than tidiness — an unencoded space
/// would produce a URL a browser rewrites and this listener then 404s. Anything
/// outside the unreserved set is dropped, so the result cannot contain a `/`
/// that would read as another segment.
fn path_segment(filename: &str) -> String {
    let mut out = String::with_capacity(filename.len());
    for byte in filename.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push_str("%20");
        }
    }
    // A segment that is only dots would be a traversal, not a name.
    if out.is_empty() || out.chars().all(|c| c == '.') {
        return "file".to_string();
    }
    out
}

/// `disposition; filename="..."`, or `None` if the value will not parse.
///
/// Built with [`tiny_http::Header::from_bytes`] rather than `header()` precisely
/// so a newline in the name can neither panic nor inject a second header.
fn disposition_header(kind: &str, filename: &str) -> Option<tiny_http::Header> {
    let value = disposition_of(kind, filename);
    tiny_http::Header::from_bytes("Content-Disposition", value.as_str()).ok()
}

fn disposition_of(kind: &str, filename: &str) -> String {
    let mut out = String::from(kind);
    // No quotes here: `quoted_body` supplies them, and adding a second pair is
    // how this briefly emitted `filename=""x""`.
    out.push_str("; filename=");
    out.push_str(&quoted_body(filename));
    out
}

/// `filename` as an RFC 6266 quoted-string body, quotes included.
///
/// `suggest_filename` already reduces this to one path segment, but a viewer URL
/// is served on its own port and its response headers are the one place a
/// caller-supplied name reaches a parser that is not us, so a quote or a
/// newline is neutralised here rather than trusted. Escaped quotes inside a
/// quoted-string are legal, so this cannot terminate the value early.
fn quoted_body(filename: &str) -> String {
    let mut out = String::from("\"");
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

#[cfg(test)]
mod tests {
    use super::{disposition_of, path_is_ours, path_segment};

    const TOKEN: &str = "/v/3f8a9c2e-1b4d-4a77-9e21-0c5b6d8f2a13";

    #[test]
    fn the_token_is_the_whole_of_the_authorisation() {
        // A name may be appended, and the token alone must still work: the name
        // is there to title a tab, so a viewer that never learned one can still
        // be opened, and widening the match cannot have widened access.
        assert!(path_is_ours(TOKEN, TOKEN));
        assert!(path_is_ours(&format!("{TOKEN}/welcome.md"), TOKEN));
        assert!(path_is_ours(&format!("{TOKEN}/my%20notes.md"), TOKEN));
    }

    #[test]
    fn everything_that_is_not_this_token_is_refused() {
        // The neighbouring-path half is the load-bearing half of this: there is
        // no cookie and no shared secret, so if any other request is answered,
        // one listener's bytes are reachable through another's port.
        for url in [
            "/v/not-the-token",
            "/",
            "",
            // A longer token-looking prefix must not match.
            &format!("{TOKEN}x"),
            &format!("{TOKEN}?x=1"),
            &format!("{TOKEN}#f"),
            // Traversal, in the shapes a URL can carry it.
            &format!("{TOKEN}/../{TOKEN}"),
            "/v/../v/3f8a9c2e-1b4d-4a77-9e21-0c5b6d8f2a13",
            // A second segment is a different resource, not a longer name.
            &format!("{TOKEN}/name/extra"),
            // Empty and dot-only names.
            &format!("{TOKEN}/"),
            &format!("{TOKEN}/."),
            &format!("{TOKEN}/.."),
            // Percent-encoded traversal must not decode into one.
            &format!("{TOKEN}/%2e%2e"),
            // A slash smuggled in encoded is still a slash once decoded.
            &format!("{TOKEN}/a%2Fb"),
            &format!("{TOKEN}/a%5Cb"),
            // A backslash, which some URL parsers normalise to a slash.
            &format!("{TOKEN}/a%5Cb"),
            // A stray percent that is not a triplet.
            &format!("{TOKEN}/100%.md"),
            &format!("{TOKEN}/%zz.md"),
        ] {
            assert!(
                !path_is_ours(url, TOKEN),
                "{url} must not be served by this listener"
            );
        }
    }

    #[test]
    fn a_name_becomes_one_encoded_segment() {
        assert_eq!(path_segment("welcome.md"), "welcome.md");
        assert_eq!(path_segment("52af8a181c6171f8.md"), "52af8a181c6171f8.md");
        // `suggest_filename` allows spaces, and an unencoded space makes a URL a
        // browser rewrites — which this listener would then 404.
        assert_eq!(path_segment("my notes.md"), "my%20notes.md");
        // Anything outside the unreserved set is dropped rather than encoded, so
        // the result cannot contain a `/` that would read as another segment.
        assert_eq!(path_segment("a/b.md"), "ab.md");
        assert_eq!(path_segment("a?b#c.md"), "abc.md");
        assert_eq!(path_segment("naïve.md"), "nave.md");
        // A segment of only dots is a traversal, not a name.
        assert_eq!(path_segment(".."), "file");
        assert_eq!(path_segment("."), "file");
        assert_eq!(path_segment(""), "file");
    }

    #[test]
    fn both_dispositions_name_the_file_and_neither_can_be_escaped() {
        // `filename` on an inline response is the fix for the browser answering
        // "Save image as..." with the last path segment — the uuid, untyped.
        assert_eq!(
            disposition_of("inline", "welcome.md"),
            "inline; filename=\"welcome.md\""
        );
        assert_eq!(
            disposition_of("attachment", "welcome.md"),
            "attachment; filename=\"welcome.md\""
        );
        // Exactly one pair of quotes: an extra pair is what a refactor here once
        // emitted, and `filename=""x""` is a name the user sees on disk.
        assert!(!disposition_of("inline", "a.md").contains("\"\""));
        // A quote or newline in the name cannot terminate the value early.
        assert_eq!(
            disposition_of("attachment", "a\"b.md"),
            "attachment; filename=\"a\\\"b.md\""
        );
        assert_eq!(
            disposition_of("attachment", "a\r\nb.md"),
            "attachment; filename=\"ab.md\""
        );
    }
}

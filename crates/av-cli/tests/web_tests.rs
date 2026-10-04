//! Integration tests for the web interface.
//!
//! These drive a real server over real HTTP against a real (temporary)
//! database, because the things most likely to break — routing, the Host
//! guard, JSON error shapes, and the theme bootstrap — only show up end to end.
//!
//! Nothing here needs the x0x daemon or the Autonomi client: the web interface
//! is specifically built to come up without them, which is what makes it the
//! right place for the setup flow.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A server running in its own process, torn down when the guard drops.
/// A server running in its own process, torn down when the guard drops.
struct TestServer {
    child: Child,
    port: u16,
    db: PathBuf,
    /// Everything the server wrote to stderr, drained on a background thread.
    /// A silent server that exits is otherwise indistinguishable from one that
    /// is merely slow to accept.
    stderr: Arc<Mutex<String>>,
    /// Everything the server wrote to stdout, drained the same way.
    stdout: Arc<Mutex<String>>,
}

/// Read a pipe to end-of-stream, appending to `sink` as it goes.
///
/// Draining is not optional, and it is not the same as "read a byte at a time":
///
/// * An unread pipe fills and blocks the child, hiding why a server died.
/// * If the read end is *dropped*, the server's next `println!` fails with
///   `EPIPE` — and `println!` panics on a write error, so the process dies with
///   "failed printing to stdout: Broken pipe". That is how this harness killed a
///   perfectly healthy server at random.
///
/// The read must go into a buffer with real capacity: `read(&mut Vec::new())`
/// passes a zero-length slice, which returns `Ok(0)` straight away, looks
/// exactly like EOF, and drops the pipe.
fn drain<R: Read + Send + 'static>(mut pipe: R, sink: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if let Ok(mut s) = sink.lock() {
                        s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
                }
            }
        }
    });
}

impl TestServer {
    fn start() -> Self {
        let dir = std::env::temp_dir().join(format!("av-web-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db = dir.join("test.db");

        let mut child = Command::new(env!("CARGO_BIN_EXE_av"))
            .args(["web", "--no-open", "--port", "0", "--run-for", "60"])
            .env("ANTA_VISTA_DB_PATH", &db)
            // The listener PID file comes from the platform data dir, not from
            // the database path. Redirect it into the temp dir as well, or a
            // test run would read and clear the developer's real PID file — and
            // `stop_all` would kill a listener they actually wanted running.
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("HOME", dir.join("home"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn av web");

        let stdout = Arc::new(Mutex::new(String::new()));
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(pipe) = child.stdout.take() {
            drain(pipe, Arc::clone(&stdout));
        }
        if let Some(pipe) = child.stderr.take() {
            drain(pipe, Arc::clone(&stderr));
        }

        let mut server = TestServer {
            child,
            port: 0,
            db,
            stderr,
            stdout,
        };
        server.port = server.wait_for_port();
        server
    }

    /// The server's stderr so far, for failure messages.
    fn stderr(&self) -> String {
        self.stderr.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// The chosen port is only known once the server has bound, which it
    /// reports on stdout.
    fn wait_for_port(&mut self) -> u16 {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let seen = self.stdout.lock().map(|s| s.clone()).unwrap_or_default();
            // Scan only complete lines. Testing the buffer after every arriving
            // byte would parse the half-arrived "87" of "8787" as a valid port.
            for line in seen.split('\n') {
                let Some(rest) = line.split("127.0.0.1:").nth(1) else {
                    continue;
                };
                let port: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(p) = port.parse::<u16>()
                    && p > 0
                {
                    return p;
                }
            }
            if let Some(status) = self.child.try_wait().ok().flatten() {
                panic!(
                    "server exited with {status} before reporting a port:\nstdout:\n{seen}\nstderr:\n{}",
                    self.stderr()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "server never reported a port; server stderr:\n{}",
            self.stderr()
        );
    }

    fn get(&self, path: &str) -> HttpResponse {
        self.request("GET", path, None, None)
    }

    fn post(&self, path: &str, body: &str) -> HttpResponse {
        self.request("POST", path, Some(body), None)
    }

    /// Issue one request. `host` overrides the Host header when given, which is
    /// how the DNS-rebinding guard is tested.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        host: Option<&str>,
    ) -> HttpResponse {
        self.request_raw(method, path, body.unwrap_or("").as_bytes(), host, &[])
    }

    /// POST a body of bytes with extra headers.
    ///
    /// Separate from [`Self::request`] because the binary endpoints take a file
    /// as the request body and carry their metadata in `X-Anta-*` headers, and
    /// because the cross-site guard is driven by a `Sec-Fetch-Site` header. The
    /// JSON-only helper above cannot express either.
    fn post_binary(&self, path: &str, body: &[u8], headers: &[(&str, &str)]) -> HttpResponse {
        self.request_raw("POST", path, body, None, headers)
    }

    fn request_raw(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        host: Option<&str>,
        extra: &[(&str, &str)],
    ) -> HttpResponse {
        let host = host.unwrap_or("127.0.0.1");
        // The banner is printed from the thread that has already bound, so the
        // socket is listening by the time we see the port. Retry anyway, and
        // generously: on a loaded machine — a full `cargo test --workspace`
        // rebuilding every crate at once — the first few connects can lose to
        // scheduling. A flaky test that looks like a product bug helps nobody,
        // and a slow machine is not a broken server.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut stream = None;
        let mut last_err = None;
        while Instant::now() < deadline {
            match TcpStream::connect(("127.0.0.1", self.port)) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        let mut stream = stream.unwrap_or_else(|| {
            panic!(
                "connect to test server on port {} failed: {last_err:?}\nserver stderr:\n{}",
                self.port,
                self.stderr()
            )
        });

        let mut head = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {host}:{}\r\n\
             Connection: close\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n",
            self.port,
            body.len()
        );
        for (name, value) in extra {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");

        let mut wire = head.into_bytes();
        wire.extend_from_slice(body);
        stream.write_all(&wire).expect("write request");
        stream.flush().ok();

        // Read bytes, not text: the viewer serves arbitrary file content back,
        // and a UTF-8 read would mangle or reject it.
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| panic!("malformed HTTP response: {response:?}"));
        let (head, body) = response.split_at(split);
        let body = &body[4..];
        let head = String::from_utf8_lossy(head).into_owned();
        let lower = head.to_lowercase();
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("status line");

        // Anything big enough is sent chunked, and a naive reader would return
        // the chunk-size lines as if they were content — which looks exactly
        // like corrupt data rather than like framing.
        let body = if lower.contains("transfer-encoding: chunked") {
            decode_chunked(body)
        } else {
            body.to_vec()
        };

        HttpResponse {
            status,
            headers: lower,
            body: String::from_utf8_lossy(&body).into_owned(),
            raw: body,
        }
    }

    fn json(&self, path: &str) -> Value {
        let res = self.get(path);
        assert_eq!(res.status, 200, "GET {path} -> {}", res.body);
        serde_json::from_str(&res.body).unwrap_or_else(|e| panic!("GET {path}: {e}: {}", res.body))
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(dir) = self.db.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Decode a chunked transfer body.
///
/// Only what a server actually emits: `<hex-size>[;ext]CRLF data CRLF` repeated,
/// ended by a zero-size chunk, then trailers. Anything malformed is returned as
/// received rather than panicking, so a broken framing shows up as a content
/// mismatch in the assertion that cares.
fn decode_chunked(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(nl) = find_crlf(body) else {
            return out;
        };
        let size_line = String::from_utf8_lossy(&body[..nl]);
        // Strip any chunk extension after `;`.
        let size_text = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            return out;
        };
        if size == 0 {
            return out;
        }
        let start = nl + 2;
        let end = start + size;
        if end > body.len() {
            out.extend_from_slice(&body[start..]);
            return out;
        }
        out.extend_from_slice(&body[start..end]);
        // Step over the data and the CRLF that terminates it.
        body = &body[(end + 2).min(body.len())..];
    }
}

fn find_crlf(haystack: &[u8]) -> Option<usize> {
    haystack.windows(2).position(|w| w == b"\r\n")
}

struct HttpResponse {
    status: u16,
    headers: String,
    /// Lossy rendering of `raw`, for the JSON tests that assert on text.
    body: String,
    /// The response body as received, for the tests that assert on bytes.
    raw: Vec<u8>,
}

impl HttpResponse {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("response body is JSON")
    }
}

/// One server for the whole file: each start loads the embedding model, which
/// is far too slow to repeat per test.
fn server() -> &'static TestServer {
    use std::sync::OnceLock;
    static SERVER: OnceLock<TestServer> = OnceLock::new();
    SERVER.get_or_init(TestServer::start)
}

#[test]
fn serves_the_html_shell() {
    let res = server().get("/");
    assert_eq!(res.status, 200);
    assert!(res.headers.contains("text/html"));
    // The shell must reference the assets and the API-driven regions.
    for marker in [
        "/assets/app.js",
        "search-form",
        "addlink-heading",
        "install-dialog",
        "result-template",
    ] {
        assert!(res.body.contains(marker), "page is missing {marker}");
    }
}

#[test]
fn serves_its_assets() {
    for (path, kind) in [
        ("/assets/app.js", "javascript"),
        ("/assets/retro.js", "javascript"),
        ("/assets/retro.css", "text/css"),
        ("/assets/modern.css", "text/css"),
    ] {
        let res = server().get(path);
        assert_eq!(res.status, 200, "{path}");
        assert!(
            res.headers.contains(kind),
            "{path} served as {:?}",
            res.headers
        );
        assert!(!res.body.is_empty(), "{path} is empty");
    }
}

#[test]
fn theme_bootstrap_reads_local_storage() {
    let body = &server().get("/").body;
    let boot = body
        .find("localStorage.getItem")
        .expect("no theme bootstrap");
    // The theme has to be resolved in <head>, before the body paints, or the
    // page flashes the wrong stylesheet on every load.
    let body_start = body.find("<body").expect("no body");
    assert!(
        boot < body_start,
        "theme is resolved after the body renders, so it will flash"
    );
    // And it must not be deferred, or it runs after parsing anyway.
    let script = &body[body[..boot].rfind("<script").expect("no script tag")..];
    assert!(
        !script.contains("defer") && !script.contains("async"),
        "theme bootstrap must run synchronously: {script}"
    );
}

#[test]
fn status_reports_counts_and_dependencies() {
    let s = server().json("/api/status");
    assert_eq!(s["ok"], true);
    for key in [
        "x0x_daemon",
        "av_listener",
        "ant_daemon",
        "ant_cli",
        "minilm_model",
        "database",
        "uptime_seconds",
    ] {
        assert!(s.get(key).is_some(), "status is missing {key}");
    }
    let db = &s["database"];
    for key in ["resources", "name_records", "embeddings", "feedback_events"] {
        assert!(db.get(key).is_some(), "database counts missing {key}");
    }
    // x0x is not required for the web interface to answer.
    assert_eq!(s["x0x_daemon"]["running"], false);
    assert!(s["minilm_model"]["dimensions"].as_u64().unwrap() > 0);
}

#[test]
fn deps_include_literal_install_commands() {
    let s = server().json("/api/deps");
    let deps = &s["deps"];
    let x0x = deps["x0x_install_cmd"].as_str().expect("no x0x command");
    let ant = deps["ant_install_cmd"].as_str().expect("no ant command");
    // The dialog shows these verbatim, so they must be real, non-empty commands.
    assert!(!x0x.trim().is_empty());
    assert!(!ant.trim().is_empty());
    assert_eq!(
        s["needs_setup"], true,
        "no x0x in tests, so setup is needed"
    );
}

#[test]
fn search_requires_a_query() {
    let res = server().get("/api/search");
    assert_eq!(res.status, 400);
    let body = res.json();
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "validation_error");
    assert!(body["detail"].as_str().unwrap().contains("'q'"));
}

#[test]
fn search_returns_a_well_formed_envelope() {
    let s = server().json("/api/search?q=example");
    assert_eq!(s["ok"], true);
    assert_eq!(s["query"], "example");
    assert!(s["results"].is_array());
    assert!(s["total"].is_number());
    assert!(s.get("clustering_active").is_some());
}

#[test]
fn search_percent_decodes_the_query() {
    let s = server().json("/api/search?q=two%20words%20%26%20more");
    assert_eq!(s["query"], "two words & more");
}

#[test]
fn download_is_a_real_daemon_fallback_not_a_stub() {
    // A random address cannot exist, and whether *this* machine has ant tooling
    // decides which honest failure comes back: 503 with an install command when
    // it does not, 502 when the fetch itself fails. Assert the invariant that
    // holds either way rather than assuming a host with nothing installed.
    let res = server().get(&format!(
        "/api/download?location=autonomi%3A%2F%2F{}",
        "ab".repeat(32)
    ));
    assert_ne!(
        res.status, 501,
        "download must not pretend to be unimplemented"
    );
    assert!(
        res.status == 502 || res.status == 503,
        "expected a daemon failure, got {} {}",
        res.status,
        res.body
    );
    let body = res.json();
    assert_eq!(body["ok"], false);
    assert!(
        body["error"] == "download_failed" || body["error"] == "no_download_backend",
        "unexpected error code: {}",
        body["error"]
    );
}

#[test]
fn download_still_validates_before_doing_any_work() {
    let res = server().get("/api/download");
    assert_eq!(res.status, 400);
    assert_eq!(res.json()["error"], "validation_error");
}

/* ------------------------------------------------------- browser fetch path -- */

/// A distinct, well-formed Autonomi address per call site.
///
/// Hand-built rather than hashed, because the tests only need the SDK-shaped
/// constraints to hold: 64 hex characters, and no two tests sharing a location
/// so a stored resource cannot satisfy the wrong assertion. No hashing crate is
/// needed for that, and these are not security-sensitive values.
fn test_address(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Two 16-hex halves make 32 bytes, which is the length the SDK expects.
    format!("autonomi://{:016x}{:016x}", tag.len(), n)
}

/// Split a viewer URL into its port and request path.
fn split_viewer_url(url: &str) -> (u16, String) {
    let (_, after_host) = url
        .split_once("127.0.0.1:")
        .expect("viewer url is loopback");
    let (port, path) = match after_host.split_once('/') {
        Some((port, path)) => (port, format!("/{path}")),
        None => (after_host, "/".to_string()),
    };
    let port: u16 = port
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("viewer url has no port: {url}"));
    (port, path)
}

/// GET against an arbitrary loopback port.
///
/// The viewer deliberately binds a *different* port from the API, so testing it
/// through the shared `TestServer` request helper would test nothing. Asserting
/// the port differs is itself one of the security properties: it is what keeps
/// the document off the origin that holds the API.
fn get_on_port(port: u16, path: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to viewer");
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read");
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("malformed response");
    let head = String::from_utf8_lossy(&response[..split]).into_owned();
    let body = response[split + 4..].to_vec();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    (status, head.to_lowercase(), body)
}

#[test]
fn the_viewer_serves_the_bytes_inline_from_its_own_origin() {
    let uri = test_address("viewer-html");
    let page = b"<!doctype html><title>viewer test</title><p>hello from the viewer";
    let res = server().post_binary(
        "/api/view/blob",
        page,
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("Content-Type", "application/octet-stream"),
        ],
    );
    assert_eq!(res.status, 200, "view_blob failed: {}", res.body);
    let body = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["mime_type"], "text/html");
    assert_eq!(body["bytes"], page.len());
    /* A name from the server, never the SDK's `public-file-<8hex>.bin`. */
    let filename = body["filename"].as_str().expect("filename");
    assert!(
        !filename.starts_with("public-file-"),
        "got SDK filename {filename}"
    );
    assert!(filename.ends_with(".html"), "got {filename}");

    let (port, path) = split_viewer_url(body["url"].as_str().expect("url"));

    /* The isolation property: not the API's origin. */
    assert_ne!(port, server().port, "viewer must not share the API origin");

    let (status, headers, served) = get_on_port(port, &path);
    assert_eq!(status, 200);
    assert_eq!(
        served, page,
        "viewer must serve the exact bytes it was given"
    );
    assert!(headers.contains("content-type: text/html"), "{headers}");
    /* Inline, so a document renders as a document; and locked down, because this
     * is attacker-supplied content on a loopback port. */
    assert!(headers.contains("content-disposition: inline"), "{headers}");
    assert!(
        headers.contains("x-content-type-options: nosniff"),
        "{headers}"
    );
    assert!(headers.contains("cache-control: no-store"), "{headers}");
    assert!(
        headers.contains("referrer-policy: no-referrer"),
        "{headers}"
    );
}

#[test]
fn the_viewer_only_serves_its_own_token() {
    let uri = test_address("viewer-scope");
    let body = server()
        .post_binary(
            "/api/view/blob",
            b"secret bytes",
            &[("X-Anta-Uri", uri.as_str())],
        )
        .json();
    let (port, path) = split_viewer_url(body["url"].as_str().expect("url"));
    let token = path.trim_start_matches("/v/").to_string();

    /* The exact token works... */
    assert_eq!(get_on_port(port, &path).0, 200);
    /* ...and nothing else on that port does. An unguessable path is the entire
     * authorisation: there is no cookie and no shared secret, so a second
     * listener's bytes must not be reachable through the first one's port. */
    assert_eq!(get_on_port(port, "/v/not-the-token").0, 404);
    assert_eq!(get_on_port(port, "/").0, 404);
    assert_eq!(get_on_port(port, &format!("/v/{token}/../{token}")).0, 404);
    assert_eq!(get_on_port(port, &format!("/v/{token}x")).0, 404);
}

#[test]
fn a_cross_site_post_is_refused() {
    // The viewer page is on another origin, and the API is reachable by any page
    // the user has open, so a state-changing POST from elsewhere is refused.
    let res = server().post_binary(
        "/api/index/bytes",
        b"payload from another site",
        &[
            ("Sec-Fetch-Site", "cross-site"),
            ("X-Anta-Uri", "autonomi://deadbeef"),
        ],
    );
    assert_eq!(
        res.status, 403,
        "cross-site POST was not refused: {}",
        res.body
    );
    assert_eq!(res.json()["error"], "cross_site_request");
}

#[test]
fn the_same_origin_post_is_allowed() {
    // The guard must not block the app's own fetches: a browser sends
    // `same-origin` for these, and curl and the tests send nothing at all.
    let res = server().post_binary(
        "/api/view/blob",
        b"<!doctype html><p>same origin",
        &[
            ("Sec-Fetch-Site", "same-origin"),
            ("X-Anta-Uri", "autonomi://sameorigin"),
        ],
    );
    assert_eq!(
        res.status, 200,
        "same-origin POST was refused: {}",
        res.body
    );
}

#[test]
fn bytes_indexed_by_the_browser_are_stored_and_searchable() {
    let uri = test_address("browser-index");
    let body = "zorbulax quixotic telemetry beacon for the browser upload path";
    let res = server().post_binary(
        "/api/index/bytes",
        body.as_bytes(),
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("X-Anta-Tags", "browser,upload"),
            ("Content-Type", "application/octet-stream"),
        ],
    );
    assert_eq!(res.status, 200, "index/bytes failed: {}", res.body);
    let out = res.json();
    assert_eq!(out["ok"], true);
    assert_eq!(out["location"], uri.as_str());
    assert!(out["resource_id"].is_string());
    /* The resource id is the SHA-256 of the bytes, so it is stable across
     * transports and matches what a peer would report for the same file. */
    assert_eq!(out["resource_id"].as_str().unwrap().len(), 64);

    // And it is really in the index, not just acknowledged.
    let found = server().json("/api/search?q=zorbulax");
    let hit = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["location"] == uri.as_str())
        .expect("browser-uploaded bytes are searchable");
    assert_eq!(hit["source"], "local");
}

#[test]
fn an_oversized_upload_is_refused_rather_than_truncated() {
    // Truncating here would index a corrupt prefix of a large file and report
    // success, so over-cap must be a refusal with a code the browser can branch
    // on to take the daemon route instead.
    let uri = test_address("oversize");
    let over = vec![b'a'; 32 * 1024 * 1024 + 1];
    let res = server().post_binary("/api/index/bytes", &over, &[("X-Anta-Uri", uri.as_str())]);
    assert_eq!(
        res.status, 400,
        "oversized body was not refused: {}",
        res.status
    );
    assert_eq!(res.json()["error"], "body_too_large");
}

#[test]
fn the_vendored_sdk_is_served_with_types_the_browser_accepts() {
    let s = server();
    let js = s.get("/assets/vendor/autonomi-sdk/0.1.1/index.js");
    assert_eq!(js.status, 200, "vendor index.js missing: {}", js.body);
    assert!(js.headers.contains("javascript"), "{}", js.headers);

    // The WASM is the reason the WASM MIME rule exists: served as
    // `application/octet-stream` a module import fails to stream-compile.
    let wasm = s.get("/assets/vendor/autonomi-sdk/0.1.1/wasm/ant_core_bg.wasm");
    assert_eq!(wasm.status, 200);
    assert!(
        wasm.headers.contains("application/wasm"),
        "{}",
        wasm.headers
    );
    assert_eq!(&wasm.raw[0..4], b"\0asm", "not a WASM module");

    // The runtime prefix and the boot global the classic app.js reads must agree.
    let page = s.get("/");
    assert!(
        page.body.contains("/assets/vendor/autonomi-sdk/0.1.1/"),
        "page does not name the sdk base"
    );
}

#[test]
fn vendor_paths_cannot_escape_the_vendored_directory() {
    for path in [
        "/assets/vendor/antonomi-sdk/0.1.1/../../../etc/passwd",
        "/assets/vendor/autonomi-sdk/0.1.1/wasm/../../../../Cargo.toml",
    ] {
        let res = server().get(path);
        assert!(
            res.status == 404 || res.status == 400,
            "traversal through {path} returned {}",
            res.status
        );
    }
}

#[test]
fn an_autonomi_location_looks_clickable_because_it_is() {
    // Regression: `.result-loc` carried `cursor: help` and a comment claiming an
    // Autonomi location had "no open/download path yet". The path existed, but
    // only on a button elsewhere in the row — so the address underlined on
    // hover, showed a question-mark cursor, and did nothing when clicked. The
    // affordance has to match the behaviour.
    let s = server();
    let modern = s.get("/assets/modern.css").body;
    let retro = s.get("/assets/retro.css").body;

    // Inert by default, pointer once app.js says it wired a click.
    let inert =
        css_rule(&modern, r#"[data-theme="modern"] .result-loc"#).expect("modern .result-loc");
    assert!(
        !inert.contains("cursor"),
        "the default state must not claim to be clickable: {inert}"
    );
    for (label, css) in [("modern", &modern), ("retro", &retro)] {
        let openable = css_rule(
            css,
            &format!(r#"[data-theme="{label}"] .result-loc.is-openable"#),
        )
        .unwrap_or_else(|| panic!("{label} has no .result-loc.is-openable rule"));
        assert!(
            openable.contains("cursor: pointer"),
            "{label} .result-loc.is-openable does not take a pointer cursor: {openable}"
        );
    }

    // And the click is really wired, keyed as well as pointed.
    let js = s.get("/assets/app.js").body;
    assert!(
        js.contains("is-openable"),
        "app.js never marks the location openable"
    );
    assert!(
        js.contains(r#"loc.addEventListener('click'"#),
        "the location has no click handler"
    );
    assert!(
        js.contains("event.key === 'Enter'"),
        "the location is focusable but has no key handler"
    );
}

#[test]
fn the_page_offers_open_and_save_for_an_autonomi_row() {
    let script = server().get("/assets/app.js");
    assert_eq!(script.status, 200);
    let js = script.body.as_str();
    for needle in [
        "data-role=\"open\"",
        "data-role=\"save\"",
        "autonomiAddress",
        "/api/index/bytes",
        "/api/view/blob",
    ] {
        assert!(js.contains(needle), "app.js is missing {needle}");
    }
    /* No 501 stub is left to call, and the honest refusal to guess a filename
     * must not come back: the server supplies the name. */
    assert!(
        !js.contains("not_implemented"),
        "app.js still expects a not-implemented download"
    );
}

#[test]
fn rejects_a_foreign_host_header() {
    // DNS-rebinding guard: a page on another origin can reach loopback.
    let res = server().request("GET", "/api/status", None, Some("evil.example.com"));
    assert_eq!(res.status, 403, "foreign Host was not rejected");
}

#[test]
fn accepts_localhost_and_loopback_hosts() {
    for host in ["localhost", "127.0.0.1", "[::1]"] {
        let res = server().request("GET", "/api/status", None, Some(host));
        assert_eq!(res.status, 200, "Host {host} was rejected");
    }
}

#[test]
fn unknown_api_routes_are_404_json() {
    let res = server().get("/api/nope");
    assert_eq!(res.status, 404);
    assert_eq!(res.json()["error"], "not_found");
}

#[test]
fn malformed_post_bodies_are_rejected() {
    for path in ["/api/rate", "/api/index"] {
        let res = server().post(path, "not json");
        assert_eq!(res.status, 400, "{path}");
        assert_eq!(res.json()["error"], "validation_error");
    }
}

#[test]
fn rate_validates_its_input() {
    let server = server();

    let res = server.post("/api/rate", r#"{"rating":"useful"}"#);
    assert_eq!(res.status, 400);
    assert!(
        res.json()["detail"]
            .as_str()
            .unwrap()
            .contains("resource_id")
    );

    let res = server.post("/api/rate", r#"{"resource_id":"abc"}"#);
    assert_eq!(res.status, 400);
    assert!(res.json()["detail"].as_str().unwrap().contains("rating"));

    let res = server.post("/api/rate", r#"{"resource_id":"abc","rating":"bogus"}"#);
    assert_eq!(res.status, 400, "an unknown rating must be rejected");
}

#[test]
fn rating_an_unindexed_resource_explains_why() {
    // Feedback rows reference resources(id), so a rating about something we do
    // not hold has nowhere to attach. The message has to say that rather than
    // surfacing a bare foreign-key error.
    let res = server().post(
        "/api/rate",
        r#"{"resource_id":"definitely-not-indexed","rating":"useful","query":"x"}"#,
    );
    assert_eq!(res.status, 400);
    let body = res.json();
    assert_eq!(body["error"], "resource_not_indexed");
    assert!(body["detail"].as_str().unwrap().contains("local index"));
}

#[test]
fn index_requires_a_uri() {
    let res = server().post("/api/index", r#"{"tags":"a,b"}"#);
    assert_eq!(res.status, 400);
    assert!(res.json()["detail"].as_str().unwrap().contains("uri"));
}

#[test]
fn indexing_an_autonomi_uri_never_succeeds_without_a_backend() {
    // This test must not depend on whether the developer running it happens to
    // have the ant CLI installed, so assert the invariant that holds either
    // way: a made-up autonomi:// hash cannot be indexed, and the failure says
    // why.
    let res = server().post("/api/index", r#"{"uri":"autonomi://abc123"}"#);
    let body = res.json();

    assert_ne!(res.status, 200, "a fake hash must never index: {body}");
    if res.status == 503 {
        // No backend at all.
        assert_eq!(body["error"], "no_download_backend");
        assert!(body["detail"].as_str().unwrap().contains("ant CLI"));
    } else {
        // A backend exists but the fetch failed, which is equally honest.
        assert!(
            body["detail"].is_string(),
            "failure must explain itself: {body}"
        );
        assert!(!body["ok"].as_bool().unwrap_or(true));
    }
}

#[test]
fn rating_an_unknown_resource_explains_the_foreign_key_instead_of_leaking_it() {
    // A rating for something we do not hold and cannot store has to fail with a
    // reason a user can act on. The underlying cause is a foreign key on
    // `relevance_judgments`, which must never reach the browser as-is.
    let res = server().post(
        "/api/rate",
        r#"{"resource_id":"not-a-real-resource","rating":"useful","query":"x",
            "result":{"resource_id":"not-a-real-resource","source":"local"}}"#,
    );
    assert_eq!(res.status, 400);
    let body = res.json();
    assert_eq!(body["error"], "resource_not_indexed");
    let detail = body["detail"].as_str().unwrap();
    assert!(!detail.to_lowercase().contains("foreign key"), "{detail}");
    assert!(detail.contains("local index"), "{detail}");
}

#[test]
fn the_listener_can_be_started_and_stopped_through_the_api() {
    // `start` spawns the same detached `av listen` the CLI uses, so this test
    // stops it again afterwards rather than leaving a stray process behind.
    let res = server().post("/api/listener/start", "{}");
    assert_eq!(res.status, 200);
    let body = res.json();
    assert_eq!(body["ok"], true, "listener should start: {body}");
    // A listener with no x0x behind it is up but useless, and must say so
    // rather than letting the user assume the network is covered.
    let deps = server().get("/api/deps").json();
    if !deps["deps"]["x0x_running"].as_bool().unwrap_or(false) {
        assert!(
            body["detail"].as_str().unwrap().contains("x0x"),
            "should explain that peers are unreachable: {body}"
        );
    }

    let stopped = server().post("/api/listener/stop", "{}").json();
    assert_eq!(stopped["ok"], true, "{stopped}");
}

#[test]
fn missing_backend_errors_carry_the_platform_install_command() {
    // The UI must show the exact command before the user consents to running
    // it, so it arrives with the error instead of being hardcoded in the page.
    let res = server().post("/api/index", r#"{"uri":"autonomi://abc123"}"#);
    if res.status != 503 {
        return; // a backend is installed here; the other test covers that path
    }
    let body = res.json();
    let command = body["command"].as_str().expect("no command in the error");
    assert!(!command.is_empty());
    assert_eq!(body["dependency"], "ant");
    // Must be the platform's command, so a Windows user is never shown curl.
    if !cfg!(windows) {
        assert!(
            command.contains("curl"),
            "expected a POSIX command: {command}"
        );
    }
}

#[test]
fn listener_stop_is_safe_when_idle() {
    // Nothing should be running, and asking anyway must not error.
    let res = server().post("/api/listener/stop", "{}");
    assert_eq!(res.status, 200);
    let body = res.json();
    assert_eq!(body["ok"], true);
    assert!(body["count"].is_number());
}

#[test]
fn api_responses_are_not_cacheable() {
    let res = server().get("/api/status");
    assert!(
        res.headers.contains("cache-control: no-store"),
        "API responses must not be cached: {}",
        res.headers
    );
}

#[test]
fn bare_av_in_machine_mode_refuses_and_exits_two() {
    // Bare `av` launches the web interface, which is not allowed in machine
    // mode: an agent that forgot a subcommand needs a hard failure, not a
    // socket that blocks forever.
    let out = Command::new(env!("CARGO_BIN_EXE_av"))
        .arg("--non-interactive")
        .env(
            "ANTA_VISTA_DB_PATH",
            std::env::temp_dir().join("av-web-nosub.db"),
        )
        .output()
        .expect("run av --non-interactive");

    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "no_subcommand");
}

// --- Blocking ---------------------------------------------------------------
//
// The web surface has to agree with the CLI about what a block does, so these
// drive the HTTP endpoints and then read the state back through /api/blocks.
// The server is shared across this file, so each test uses a distinct subject.

#[test]
fn the_block_list_is_readable_and_well_formed() {
    // Deliberately does not assert `count == 0`. Every test in this file shares
    // one server and one database and they run in parallel, so asserting the
    // list is empty depends on which tests happen to have run first. This used
    // to be `block_list_is_empty_on_a_fresh_database` and it was a coin flip.
    let s = server().json("/api/blocks");
    assert_eq!(s["ok"], true);
    let blocks = s["blocks"].as_array().unwrap();
    assert_eq!(s["count"].as_u64().unwrap() as usize, blocks.len());
    for b in blocks {
        assert!(b["subject_kind"].is_string(), "kind must be present");
        assert!(b["subject_id"].is_string(), "id must be present");
        // Ids are matched exactly when a block is applied or lifted, so a
        // truncated id in the list cannot be copied into an unblock.
        let id = b["subject_id"].as_str().unwrap();
        assert!(
            !id.is_empty() && !id.ends_with('\u{2026}'),
            "listed id must be complete, got {id:?}"
        );
    }
}

#[test]
fn blocking_a_resource_through_the_api_is_listed_back() {
    let s = server();
    let res = s.post(
        "/api/block",
        r#"{"subject_kind":"resource","subject_id":"web-blocked-1","reason":"spam"}"#,
    );
    assert_eq!(res.status, 200);
    let body = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["blocked"], true);
    // Nothing was held locally for this id, so nothing was deleted and the
    // response must not claim otherwise.
    assert_eq!(body["removed_local"], false);
    assert_eq!(body["cascaded"], 0);

    let listed = s.json("/api/blocks");
    let found = listed["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["subject_id"] == "web-blocked-1" && b["subject_kind"] == "resource");
    assert!(found, "the block should be listed: {}", listed);
    // The source is recorded so `av block --list` shows where it came from.
    let entry = listed["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["subject_id"] == "web-blocked-1")
        .unwrap();
    assert_eq!(entry["source"], "web");
    assert_eq!(entry["reason"], "spam");
}

#[test]
fn blocking_twice_reports_that_nothing_changed() {
    let s = server();
    let payload = r#"{"subject_kind":"resource","subject_id":"web-blocked-2"}"#;
    assert_eq!(s.post("/api/block", payload).json()["blocked"], true);
    // The second call must be honest rather than reporting a fresh block.
    let again = s.post("/api/block", payload).json();
    assert_eq!(again["blocked"], false);
    assert!(
        again["warning"]
            .as_str()
            .unwrap()
            .contains("already blocked"),
        "the response should say nothing changed: {again}"
    );
}

#[test]
fn unblocking_through_the_api_lifts_the_block() {
    let s = server();
    s.post(
        "/api/block",
        r#"{"subject_kind":"resource","subject_id":"web-blocked-3"}"#,
    );
    let res = s.post(
        "/api/unblock",
        r#"{"subject_kind":"resource","subject_id":"web-blocked-3"}"#,
    );
    assert_eq!(res.status, 200);
    let body = res.json();
    assert_eq!(body["unblocked"], true);
    assert_eq!(body["removed"], 1);

    let listed = s.json("/api/blocks");
    assert!(
        !listed["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["subject_id"] == "web-blocked-3"),
        "the block should be gone"
    );
}

#[test]
fn unblocking_something_that_was_not_blocked_says_so() {
    let body = server()
        .post(
            "/api/unblock",
            r#"{"subject_kind":"resource","subject_id":"never-blocked"}"#,
        )
        .json();
    assert_eq!(body["unblocked"], false);
    assert_eq!(body["removed"], 0);
    assert!(
        body["warning"].as_str().unwrap().contains("not blocked"),
        "the response should say it was not blocked: {body}"
    );
}

#[test]
fn the_block_api_rejects_a_malformed_agent_id_as_a_client_error() {
    let res = server().post(
        "/api/block",
        r#"{"subject_kind":"agent","subject_id":"nothex"}"#,
    );
    // A 400, not a 500: the request was wrong. A block stored against a
    // mistyped id would silently never match anything.
    assert_eq!(res.status, 400);
    let body = res.json();
    assert_eq!(body["ok"], false);
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("not a valid agent id")
    );
}

#[test]
fn the_block_api_rejects_an_unknown_kind_and_a_missing_id() {
    let s = server();
    let bad_kind = s.post(
        "/api/block",
        r#"{"subject_kind":"planet","subject_id":"x"}"#,
    );
    assert_eq!(bad_kind.status, 400);
    assert!(
        bad_kind.json()["detail"]
            .as_str()
            .unwrap()
            .contains("subject_kind")
    );

    let no_id = s.post("/api/block", r#"{"subject_kind":"resource"}"#);
    assert_eq!(no_id.status, 400);
    assert!(
        no_id.json()["detail"]
            .as_str()
            .unwrap()
            .contains("subject_id")
    );
}

#[test]
fn blocking_a_name_record_works_through_the_api() {
    let s = server();
    let res = s.post(
        "/api/block",
        r#"{"subject_kind":"name","subject_id":"web-name-1"}"#,
    );
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["blocked"], true);
    // A name block hides rather than deletes, and must not claim to have
    // removed anything.
    assert_eq!(res.json()["removed_local"], false);
}

#[test]
fn the_three_kinds_are_reported_separately_in_the_counts() {
    let s = server();
    for payload in [
        r#"{"subject_kind":"resource","subject_id":"counts-res"}"#,
        r#"{"subject_kind":"name","subject_id":"counts-name"}"#,
    ] {
        assert_eq!(s.post("/api/block", payload).status, 200);
    }

    let listed = s.json("/api/blocks");
    let counts = listed["counts"].as_array().unwrap();
    for kind in ["resource", "name"] {
        let entry = counts.iter().find(|c| c["subject_kind"] == kind);
        assert!(entry.is_some(), "no count for {kind}: {counts:?}");
        assert!(entry.unwrap()["count"].as_i64().unwrap() >= 1);
    }
    // The three kinds are independent policies, so each is counted on its own.
    let total: i64 = counts.iter().map(|c| c["count"].as_i64().unwrap()).sum();
    assert_eq!(total, listed["count"].as_i64().unwrap());
}

#[test]
fn the_page_offers_a_block_control() {
    let body = &server().get("/").body;
    // The control and its dialog have to exist in the shell, or the button can
    // never be rendered.
    for marker in ["data-role=\"block\"", "block-dialog", "block-title"] {
        assert!(body.contains(marker), "page is missing {marker}");
    }
}

#[test]
fn the_client_script_knows_how_to_block() {
    let js = server().get("/assets/app.js");
    assert_eq!(js.status, 200);
    for marker in ["/api/block", "/api/unblock", "blockTarget", "askBlock"] {
        assert!(js.body.contains(marker), "app.js is missing {marker}");
    }
}

#[test]
fn the_web_api_refuses_to_escalate_a_block_to_x0x() {
    let s = server();
    // A loopback server is reachable by any page the user has open, so a
    // caller-supplied `x0x` flag would let a request make a lasting global
    // transport change the UI never offered. It must be rejected outright.
    let res = s.post(
        "/api/block",
        r#"{"subject_kind":"agent","subject_id":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","x0x":true}"#,
    );
    assert_eq!(res.status, 400);
    let body = res.json();
    assert_eq!(body["ok"], false);
    let detail = format!("{} {}", body["error"], body["detail"]);
    assert!(
        detail.contains("av block --agent"),
        "the error must point at the CLI instead: {body}"
    );
    // Nothing was stored, so the transport is untouched.
    let listed = s.json("/api/blocks");
    assert!(
        !listed["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["subject_id"]
                == "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        "a rejected request must not have created a block row"
    );
}

#[test]
fn an_agent_blocked_in_the_browser_says_the_transport_is_untouched() {
    let s = server();
    let id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    let res = s.post(
        "/api/block",
        &format!(r#"{{"subject_kind":"agent","subject_id":"{id}"}}"#),
    );
    assert_eq!(res.status, 200);
    let body = res.json();
    assert_eq!(body["blocked"], true);
    assert_eq!(
        body["x0x_requested"], false,
        "the browser must never request a transport block"
    );
    // The block works locally, so it must not be described as a no-op, but it
    // also must not imply the peer is unreachable.
    let warning = body["warning"]
        .as_str()
        .expect("expected an honest warning");
    assert!(
        warning.contains("av block --agent"),
        "warning should name the CLI escalation, got: {warning}"
    );
    assert!(
        !warning.contains("deleted"),
        "nothing was deleted: {warning}"
    );
}

// --- Remove from local index ----------------------------------------------
//
// `POST /api/purge` backs the browser's "Remove from local index" button, so
// these pin what it actually does: frees the rows, reports the counts, writes
// no block, and treats an id we never held as a no-op rather than an error.

/// Seed a resource directly, so the test does not have to run the embedder to
/// get something into the index.
fn seed_resource(db: &std::path::Path, id: &str, location: &str) {
    use av_core::types::{ResourceDescriptor, ResourceKind};

    let conn = av_store::open(db).expect("open test db");
    av_store::repo::resources::insert(
        &conn,
        &ResourceDescriptor {
            id: id.to_string(),
            kind: ResourceKind::Text,
            location: location.to_string(),
            location_scheme: Some("autonomi".to_string()),
            location_canonical: Some(location.to_string()),
            mime_type: "text/plain".to_string(),
            filename: None,
            metadata_json: serde_json::json!({}),
            description_text: "seeded for the purge test".to_string(),
            created_at: 1_700_000_000,
        },
    )
    .expect("insert resource");

    // OR IGNORE: this profile is shared by every seeded resource in the file,
    // and the tests share one database.
    conn.execute(
        "INSERT OR IGNORE INTO embedding_profiles
           (profile_id, model_id, model_version, dim, normalized, preproc_version)
         VALUES ('p-purge', 'all-MiniLM-L6-v2', 'v1', 384, 1, 'v1')",
        [],
    )
    .expect("insert profile");
    conn.execute(
        "INSERT INTO embeddings (resource_id, profile_id, vector_json, l2_norm, created_at)
         VALUES (?1, 'p-purge', '[1.0]', 1.0, 1700000000)",
        rusqlite::params![id],
    )
    .expect("insert embedding");
}

/// Count rows for one subject. `resources` keys on `id` while everything that
/// references it uses `resource_id`, so the column is passed in.
fn count_rows(db: &std::path::Path, table: &str, column: &str, id: &str) -> i64 {
    let conn = av_store::open(db).expect("open test db");
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
        rusqlite::params![id],
        |r| r.get(0),
    )
    .unwrap_or_else(|e| panic!("count {table}: {e}"))
}

#[test]
fn purge_removes_the_resource_and_reports_the_counts() {
    let server = server();
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    seed_resource(&server.db, id, "autonomi://purge-me");

    let res = server.post("/api/purge", &format!(r#"{{"resource_id":"{id}"}}"#));
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["deleted"]["resources"], 1);
    assert_eq!(body["deleted"]["embeddings"], 1);
    assert_eq!(
        count_rows(&server.db, "resources", "id", id),
        0,
        "the resource must be gone from the index"
    );
    assert_eq!(
        count_rows(&server.db, "embeddings", "resource_id", id),
        0,
        "a successful purge must free the embedding too, or the row is a leak"
    );
}

#[test]
fn purge_writes_no_block_so_the_network_can_still_return_it() {
    // The whole difference between Remove and Block. If this ever starts
    // writing a block row, the button is quietly doing something much larger
    // than "free local storage", and the peer will never send the link back.
    let server = server();
    let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    seed_resource(&server.db, id, "autonomi://purge-no-block");

    let res = server.post("/api/purge", &format!(r#"{{"resource_id":"{id}"}}"#));
    assert_eq!(res.status, 200);

    let conn = av_store::open(&server.db).expect("open test db");
    let blocked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM blocks WHERE subject_id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .expect("count blocks");
    assert_eq!(blocked, 0, "purge must not create a block");
}

#[test]
fn purging_an_id_we_do_not_hold_is_a_successful_no_op() {
    let res = server().post("/api/purge", r#"{"resource_id":"never-indexed-at-all"}"#);
    assert_eq!(
        res.status, 200,
        "already gone is the state the user asked for, not an error"
    );
    let body = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["deleted"]["resources"], 0);
}

#[test]
fn purge_validates_its_input() {
    let res = server().post("/api/purge", r#"{}"#);
    assert_eq!(res.status, 400);
    assert!(
        res.json()["detail"]
            .as_str()
            .unwrap()
            .contains("resource_id")
    );

    let res = server().post("/api/purge", r#"{"resource_id":"  "}"#);
    assert_eq!(res.status, 400, "a blank id is not an id");
}

#[test]
fn results_offer_one_rating_and_separate_remove_from_block() {
    // The shape of the actions is the contract: one Recommend, a Remove that is
    // only meaningful for local results, and a Block. The old three-button
    // rating row and its "Rate:" label must not come back.
    let body = &server().get("/").body;

    assert_eq!(
        body.matches("data-rating=").count(),
        1,
        "exactly one rating action may exist"
    );
    assert!(
        body.contains(r#"data-rating="useful""#),
        "the value stays useful"
    );
    assert!(body.contains(">Recommend</button>"));
    assert!(!body.contains("data-rating=\"not-useful\""));
    assert!(!body.contains("data-rating=\"incorrect\""));
    assert!(!body.contains(">Not useful<"));
    assert!(!body.contains(">Incorrect<"));
    assert!(!body.contains("rate-label"), "the orphaned label is gone");

    assert!(body.contains("Remove from local index"));
    assert!(body.contains(r#"data-role="remove""#));
    assert!(body.contains(r#"data-role="block""#));
}

#[test]
fn recommend_asks_before_sharing_a_network_link() {
    let body = &server().get("/").body;

    assert!(body.contains(r#"id="recommend-dialog""#));
    assert!(body.contains("Share this link?"));
    assert!(body.contains("may share the link with other people"));
    assert!(body.contains(">No, keep it to myself<"));
    assert!(body.contains(">Yes, recommend<"));
}

#[test]
fn removing_says_it_is_not_a_block() {
    // "Remove" and "Block" are both deletions to the user but not the same one:
    // only a block stops a peer returning the link. The confirm has to say so,
    // because the button label alone cannot.
    let js = &server().get("/assets/app.js").body;
    assert!(js.contains("/api/purge"));
    assert!(js.contains("Remove from local index?"));
    assert!(js.contains("Use Block if you want it gone for good."));
    assert!(
        !js.contains("requestSubmit"),
        "removing or blocking must not re-run the search; re-querying could hand \
         the same link straight back"
    );
}

#[test]
fn result_rows_lead_with_the_location_like_the_cli() {
    // `av search` prints the location first and the description indented beneath
    // it, for every source. The browser branched on the URL *scheme* instead, so
    // an `http(s)` result used the embedded description as its link text and
    // never showed the URL at all, while an Autonomi result showed the location
    // with the description below. Two layouts for the same field, and neither
    // matched the terminal.
    let js = &server().get("/assets/app.js").body;

    assert!(
        !js.contains("title.textContent = result.description || location"),
        "a web result must not use the embedded description as its heading"
    );
    assert!(
        js.contains(
            "const heading = source === 'name' ? result.description || location : location"
        ),
        "the heading is the location, with name results as the one exception"
    );
    assert!(
        js.contains("const detail = source === 'name' ? location : result.description || ''"),
        "the description belongs underneath the heading"
    );
    // Both branches have to render `heading`. If either fell back to the
    // description the two schemes would disagree again, which is the bug.
    assert!(
        js.contains("title.textContent = heading;") && js.contains("span.textContent = heading;"),
        "the linked and unlinked branches must render the same heading"
    );
}

#[test]
fn the_remove_confirm_names_the_location_not_the_embedded_text() {
    // `description` is the text anta-vista embeds, so for a text or PDF document
    // it is up to 800 characters of the file's own content. The confirm used to
    // interpolate it, which turned "are you sure?" into a wall of prose. The
    // location is the identity of the thing being deleted and reads as one.
    let js = &server().get("/assets/app.js").body;

    let fn_start = js.find("function askRemove(").expect("askRemove exists");
    let body = &js[fn_start
        ..js[fn_start..]
            .find("\nfunction ")
            .unwrap_or(js.len() - fn_start)
            + fn_start];

    assert!(
        !body.contains("result.description"),
        "the remove confirm must not interpolate the embedded description"
    );
    assert!(
        body.contains("result.location"),
        "the remove confirm should name the resource by its location"
    );
}

#[test]
fn blocking_and_deleting_a_resource_reports_the_name_records_that_went_with_it() {
    // The count in the response is the only way the user learns that a name
    // pointing at this resource disappeared too. If it is not counted, the
    // deletion happens silently and looks like it only removed a search result.
    let server = server();
    let id = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let location = "autonomi://block-with-a-name";
    seed_resource(&server.db, id, location);

    {
        let conn = av_store::open(&server.db).expect("open test db");
        av_store::repo::names::insert(
            &conn,
            &av_core::types::NameRecord {
                schema_version: 1,
                record_id: "nr-purge-cascade".into(),
                normalized_name: "cascade-name".into(),
                original_name: "cascade-name".into(),
                record_type: av_core::types::NameRecordType::Uri,
                target: location.into(),
                target_scheme: Some("autonomi".into()),
                target_canonical: Some(location.into()),
                ttl_secs: 86_400,
                by_agent_id: "agent-abc".into(),
                origin_agent_id: None,
                timestamp: 1_700_000_000,
                signature: vec![],
            },
        )
        .expect("insert name record");
    }

    let res = server.post(
        "/api/block",
        &format!(r#"{{"subject_kind":"resource","subject_id":"{id}","keep_local":false}}"#),
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["removed_local"], true);

    let cascaded = body["cascaded"].as_u64().expect("cascaded must be a count");
    assert!(
        cascaded >= 2,
        "expected the embedding and the name record, got {cascaded}"
    );

    assert_eq!(
        count_rows(&server.db, "name_records", "record_id", "nr-purge-cascade"),
        0,
        "a claim must not outlive the only thing it resolves to"
    );
}

#[test]
fn blocking_a_peer_asks_first_and_explains_what_it_does_not_do() {
    // An agent block filters search responses and nothing else. The dialog has to
    // carry both halves, because "Block peer" on its own reads like a
    // disconnect and a cleanup, and it is neither.
    let body = &server().get("/").body;

    assert!(body.contains(r#"id="peer-block-dialog""#));
    assert!(body.contains("Block this peer?"));
    assert!(body.contains(">Block this peer</button>"));
    assert!(body.contains(">Cancel</button>"));

    let js = &server().get("/assets/app.js").body;
    assert!(
        js.contains("askPeerBlock"),
        "the peer button must confirm before blocking"
    );
    for promise in [
        "stop asking peer",
        "ignore anything it sends back",
        "does not disconnect it",
        "can still reach this node",
        "stay searchable",
        "av unblock",
    ] {
        assert!(
            js.contains(promise),
            "the peer-block dialog omits a limit: {promise}"
        );
    }

    /* The dialog used to promise that names the peer had already published
     * still resolved "because those are stored locally". That was true when an
     * agent block only filtered search responses, and stopped being true once
     * name records carried a transport origin that agent blocks filter on. The
     * stale line is worse than a missing one: it told the user a suppressed
     * peer was still reachable through names. Assert it cannot come back
     * without pinning the exact string, which a reword would dodge. */
    for gone in ["still resolve", "stored locally", "Nothing is deleted"] {
        assert!(
            !js.contains(gone),
            "the peer-block dialog still claims a blocked peer stays usable: {gone}"
        );
    }

    assert!(
        !js.contains("'av block --agent ${agent} --x0x'"),
        "the dialog should offer the transport route as a command to run, not claim to do it"
    );
}

#[test]
fn the_peer_block_dialog_keeps_its_paragraph_breaks() {
    // The promise and its limits are separate claims. Run together in one
    // paragraph the caveats stop reading as caveats.
    let css = &server().get("/assets/modern.css").body;
    assert!(
        css.contains("white-space: pre-line"),
        "the peer-block blurb needs pre-line or its blank lines collapse"
    );
}

#[test]
fn hidden_wins_over_the_layout_rules_of_the_panels_it_hides() {
    // This app shows and hides things by flipping `hidden` in app.js. The
    // browser's own `[hidden] { display: none }` lives in the *user-agent*
    // stylesheet, so any author rule that sets `display` silently outranks it —
    // and both panels that app.js hides set `display: flex`. Without the guard
    // below the status drawer and the setup banner are painted from first
    // paint, and the drawer's close button sets `hidden = true` and changes
    // nothing at all on screen, which is indistinguishable from a dead button.
    let css = &server().get("/assets/modern.css").body;
    let guard = css_rule(&css, "[hidden]").expect("modern.css needs a [hidden] rule");
    assert!(
        guard.contains("display: none") && guard.contains("!important"),
        "[hidden] must be display:none !important, or an author `display` rule \
         outranks it and the panels app.js hides stay on screen; got: {guard}"
    );

    // Both offenders are still `display: flex`, which is what makes the guard
    // load-bearing rather than decorative. If this ever fails because one of
    // them dropped the declaration, the other still needs the guard.
    for selector in [".drawer", ".banner"] {
        let rule = css_rule(&css, selector)
            .unwrap_or_else(|| panic!("modern.css should still have a {selector} rule"));
        assert!(
            rule.contains("display: flex"),
            "{selector} is expected to stay display:flex; the [hidden] guard exists \
             because author display rules outrank the UA sheet; got: {rule}"
        );
    }
}

#[test]
fn the_status_drawer_leaves_room_to_dismiss_itself() {
    // A drawer that spans the full width of a phone leaves nothing to click to
    // put it away: it covers the header, so the trigger is under it too, and
    // the overlay click-out is unreachable. Capping the width keeps a strip of
    // page visible at every size.
    let css = &server().get("/assets/modern.css").body;
    let rule = css_rule(&css, ".drawer").expect("modern.css should have a .drawer rule");
    assert!(
        !rule.contains("100%"),
        "the drawer must not be sized at 100% of the viewport or it goes \
         full-bleed on a phone; got: {rule}"
    );
    assert!(
        rule.contains("100vw"),
        "the drawer width should be capped against the viewport so page stays \
         visible beside it; got: {rule}"
    );
}

#[test]
fn the_drawers_fixed_ends_cannot_be_squeezed_out_by_a_long_status_list() {
    // `.drawer-body` is `flex: 1`, so its basis is 0 and it contributes nothing
    // to flex shrink — which means the header absorbs essentially all of the
    // shrink on a short viewport with a long status list. The close button lives
    // in that header, so it is what gets clipped. Pinning both ends as
    // non-shrinking keeps the controls reachable; the body is the part that
    // should give way, and it scrolls.
    let css = &server().get("/assets/modern.css").body;
    for selector in [".drawer-head", ".drawer-foot"] {
        let rule = css_rule(&css, selector)
            .unwrap_or_else(|| panic!("modern.css should have a {selector} rule"));
        assert!(
            rule.contains("flex: 0 0 auto"),
            "{selector} must not shrink, or a long status list clips the close \
             button; got: {rule}"
        );
    }
}

/// The first rule in `css` whose selector list contains `selector` exactly,
/// comments stripped and the opening brace kept.
///
/// Asserting on CSS with `contains` alone is how these tests went wrong: the
/// explanatory comments name the very selectors being looked for, and a fixed
/// character window around a match silently depends on how much prose follows
/// it. Stripping comments first and matching whole selectors keeps an assertion
/// pointing at the rule it claims to.
fn css_rule(css: &str, selector: &str) -> Option<String> {
    let mut stripped = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        stripped.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => return None, // unterminated comment: not a stylesheet
        };
    }
    stripped.push_str(rest);

    stripped.split('}').find_map(|chunk| {
        let (prelude, _) = chunk.split_once('{')?;
        prelude
            .split(',')
            .any(|s| s.trim() == selector)
            // Splitting on `}` drops the brace; put it back so the value handed
            // to an assertion is the rule as written.
            .then(|| format!("{} }}", chunk.trim()))
    })
}

#[test]
fn both_panels_start_hidden_in_the_served_markup() {
    // The guard above only matters because the markup opts out of first paint.
    let body = server().get("/").body;
    assert!(
        body.contains(r#"<aside id="status-drawer" class="drawer" hidden"#),
        "the status drawer must not be painted before the user asks for it"
    );
    assert!(
        body.contains(r#"<div id="setup-banner" class="banner" hidden"#),
        "the setup banner must not be painted before /api/deps reports a problem"
    );
}

#[test]
fn the_status_trigger_is_a_toggle_rather_than_an_open_button() {
    // Pressing the trigger again has to put the panel away: it is the only
    // control that is always reachable, so "open" only would leave the drawer
    // with no way back that does not depend on the drawer itself.
    let js = server().get("/assets/app.js").body;
    assert!(
        js.contains(
            "ui.statusBtn.addEventListener('click', () => setStatusOpen(ui.statusDrawer.hidden))"
        ),
        "the status trigger must derive the next state from the current one so it toggles"
    );
    // And the trigger has to say so: the label and the ARIA state both follow.
    assert!(
        js.contains("ui.statusBtn.setAttribute('aria-expanded', open ? 'true' : 'false')"),
        "aria-expanded must track the drawer, not sit permanently at false"
    );
    assert!(
        js.contains("ui.statusBtn.textContent = open ? 'Hide status' : 'Status'"),
        "the trigger label must track the drawer so it reads correctly when closed"
    );
    // The markup starts collapsed, which is the state the toggle expects.
    assert!(
        server()
            .get("/")
            .body
            .contains(r#"id="open-status" class="btn btn-quiet" aria-expanded="false""#),
        "the status trigger must advertise its collapsed state up front"
    );
}

#[test]
fn css_rule_lookup_finds_a_rule_and_ignores_prose() {
    // Not a test of the app: a guard on the helper the CSS assertions use. If
    // `css_rule` stopped stripping comments it would start matching selectors
    // named in the explanatory comments and quietly pass for the wrong reason.
    let css = "/* mentions [hidden] and .drawer { width: 1px } in prose */\n\
               [hidden] { display: none !important; }\n\
               .drawer, .banner { display: flex; }";
    assert_eq!(
        css_rule(css, "[hidden]").as_deref(),
        Some("[hidden] { display: none !important; }")
    );
    // A grouped rule is found by either of its selectors.
    assert_eq!(
        css_rule(css, ".drawer").as_deref(),
        Some(".drawer, .banner { display: flex; }")
    );
    assert_eq!(
        css_rule(css, ".banner").as_deref(),
        css_rule(css, ".drawer").as_deref()
    );
    assert_eq!(css_rule(css, ".nope"), None);
}

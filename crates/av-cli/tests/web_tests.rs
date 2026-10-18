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
        Self::spawn(false)
    }

    /// A server on its own fresh database **with** starter seeding on.
    ///
    /// The shared harness opts out (see [`Self::spawn`]), because the search
    /// tests assert exact result sets on a fresh database; this is the one
    /// server that proves the seeding path actually runs.
    fn start_seeded() -> Self {
        Self::spawn(true)
    }

    fn spawn(seeding: bool) -> Self {
        Self::spawn_with(seeding, &[])
    }

    /// Spawn with extra environment on top of the standard isolation.
    ///
    /// Used by the installer tests: `ANTA_VISTA_X0X_INSTALL_CMD` /
    /// `ANTA_VISTA_ANT_INSTALL_CMD` point the install endpoints at stub
    /// scripts, so a test never reaches the network. Each server gets its own
    /// temp HOME, so the stubs and any binaries they drop stay inside the
    /// test.
    fn spawn_with(seeding: bool, extra_env: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("av-web-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db = dir.join("test.db");

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_av"));
        cmd.args(["web", "--no-open", "--port", "0", "--run-for", "60"])
            .env("ANTA_VISTA_DB_PATH", &db)
            // The listener PID file comes from the platform data dir, not from
            // the database path. Redirect it into the temp dir as well, or a
            // test run would read and clear the developer's real PID file — and
            // `stop_all` would kill a listener they actually wanted running.
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("HOME", dir.join("home"))
            // The UI starts x0x on boot; a test run must not spawn the
            // developer's real daemon. `ensure_services` respects this and
            // returns without touching the network.
            .env("ANTA_VISTA_NO_AUTOSTART", "1");
        if !seeding {
            // Fresh-install starters would land three extra rows in every
            // test database and break the exact-count assertions; only
            // `start_seeded` leaves them on.
            cmd.env("ANTA_VISTA_NO_SEED", "1");
        }
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        let mut child = cmd
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

    /// The temp HOME this server was given, so a test can pre-seed
    /// `~/.local/bin` before or after spawn.
    fn home(&self) -> PathBuf {
        self.db.parent().expect("db has a parent dir").join("home")
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
fn the_90s_are_the_default_theme_and_both_bootstrap_fallbacks_agree() {
    let page = server().get("/").body;
    assert!(
        page.contains(r#"<html lang="en" data-theme="retro">"#),
        "the default first paint must be the 90s"
    );
    /* Two independent fallbacks exist: an unreadable stored value, and storage
     * being unavailable at all. Both must land on retro, or a hardened browser
     * boots into the other theme while the markup still says retro. */
    assert!(
        page.contains("if (t !== 'retro' && t !== 'modern') t = 'retro';"),
        "the stored-value fallback is not retro"
    );
    assert!(
        page.contains("document.documentElement.dataset.theme = 'retro';"),
        "the storage-disabled fallback is not retro"
    );
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("applyTheme(document.documentElement.dataset.theme || 'retro');"),
        "applyTheme's own fallback must match the markup's"
    );
}

#[test]
fn the_theme_button_describes_the_theme_it_is_showing_not_the_one_it_opens() {
    let js = js_code(&server().get("/assets/app.js").body);
    /* The label names what is on screen and clicking leaves it. The previous
     * pairing was inverted: on the default it claimed to dislike the 90s and
     * then went *to* them, so the button read as an escape that was not one. */
    assert!(
        js.contains(
            r"theme === 'retro' ? 'I don\u2019t like the 90\u2019s' : 'I don\u2019t like now'"
        ),
        "the label must name the current theme, not the one it opens"
    );
    assert!(
        !js.contains(r"theme === 'retro' ? 'I don\u2019t like now'"),
        "the inverted label pairing is back"
    );
    assert!(
        js.contains(
            "applyTheme(document.documentElement.dataset.theme === 'retro' ? 'modern' : 'retro');"
        ),
        "the button must flip the theme rather than jump somewhere else"
    );
    assert!(
        js.contains("localStorage.setItem(THEME_KEY, theme);"),
        "a chosen theme must survive a reload"
    );
}

#[test]
fn the_static_label_matches_the_default_so_the_header_does_not_flash_a_wrong_one() {
    let page = server().get("/").body;
    let js = js_code(&server().get("/assets/app.js").body);
    /* Before app.js runs, the header shows whatever markup wrote; afterwards
     * applyTheme rewrites it. They are two copies of one string in two
     * languages, and both must say the same thing for the default theme or the
     * button visibly changes its mind on load. */
    assert!(
        page.contains("I don&rsquo;t like the 90&rsquo;s"),
        "the markup label does not match the retro default"
    );
    assert!(
        js.contains(r"theme === 'retro' ? 'I don\u2019t like the 90\u2019s'"),
        "applyTheme would replace the markup label with a different one"
    );
}

#[test]
fn the_page_declares_an_empty_favicon_so_the_browser_stops_asking() {
    // Browsers request /favicon.ico on their own; without a declaration
    // every load left a 404 in the console — noise that looked like a
    // failed route of ours, and one click away from being read as one
    // (clicking it shows the server's "Not found: /favicon.ico"). An empty
    // data: URL answers the question without a file.
    let page = server().get("/").body;
    assert!(
        page.contains(r#"<link rel="icon" href="data:,"#),
        "the page still invites a /favicon.ico 404"
    );
}

#[test]
fn the_scroll_banner_does_not_outlive_the_theme_that_gives_it_a_reason_to_exist() {
    let js = js_code(&server().get("/assets/retro.js").body);
    /* The banner is a second, `aria-hidden` copy of `#results-count` and it is
     * inserted *before* it, inside the same heading. Leaving it behind after a
     * switch to modern prints the count twice: once as a stray unstyled line
     * (it has no `[data-theme="retro"]` rules left) and once as the heading. */
    assert!(
        js.contains("strip.remove()"),
        "the banner is never taken down"
    );
    assert!(
        js.contains("root.dataset.theme === 'retro') install();"),
        "the toggle only ever installs"
    );
    assert!(js.contains("else uninstall();"), "leaving retro is ignored");
    assert!(
        js.contains("observer.disconnect();"),
        "a detached banner would keep its observer awake forever"
    );
    /* And the 90s themselves must be untouched: retro still installs the
     * banner and still mirrors the live heading into it. */
    assert!(
        js.contains("heading.parentNode.insertBefore(strip, heading);"),
        "retro lost its banner"
    );
    assert!(
        js.contains("track.textContent = heading.textContent || 'Anta-Vista';"),
        "the banner no longer carries real content"
    );
}

#[test]
fn the_wordmark_is_a_theme_swapped_logo_with_the_name_in_alt_text() {
    // The header carries a logo image per theme rather than the "Anta·Vista"
    // text, and both ship in the page: CSS decides which one shows, so the
    // decision needs no script and no flash of the wrong one.
    let s = server();
    let page = s.get("/");
    assert_eq!(page.status, 200);
    assert!(
        !page.body.contains("Anta<span class=\"dot\">"),
        "the text wordmark is still in the served markup"
    );
    assert!(
        page.body
            .contains(r#"<img class="wordmark-retro" src="/assets/wordmark-retro.jpeg""#),
        "the retro logo is missing from the page"
    );
    assert!(
        page.body
            .contains(r#"<img class="wordmark-modern" src="/assets/wordmark-modern.jpeg""#),
        "the modern logo is missing from the page"
    );

    // The screen-reader name lives in `alt`. Exactly one copy is ever
    // displayed — the other is `display:none`, which takes it out of the
    // accessibility tree — so the h1 announces "Anta-Vista" once, from
    // whichever theme is on screen, and an image that fails to load still
    // leaves the name as visible text.
    assert_eq!(
        page.body.matches("alt=\"Anta-Vista\"").count(),
        2,
        "both logo images need the text alternative"
    );

    // The intrinsic pixels are in the markup, so the aspect ratio is known
    // before any stylesheet applies and `height: auto` scales the image
    // without ever distorting it.
    assert!(
        page.body.contains(r#"width="1038" height="440""#),
        "the retro logo's real dimensions are missing"
    );
    assert!(
        page.body.contains(r#"width="1104" height="274""#),
        "the modern logo's real dimensions are missing"
    );

    // Both are served as bytes, typed as images, under our own no-store
    // policy — a swapped-in logo appears on the next reload, like the CSS.
    for path in [
        "/assets/wordmark-retro.jpeg",
        "/assets/wordmark-modern.jpeg",
    ] {
        let logo = s.get(path);
        assert_eq!(logo.status, 200, "{path} missing");
        assert!(logo.headers.contains("image/jpeg"), "{}", logo.headers);
        assert!(logo.headers.contains("no-store"), "{}", logo.headers);
        assert_eq!(&logo.raw[0..2], b"\xff\xd8", "{path} is not a JPEG");
    }
}

#[test]
fn css_picks_the_theme_logo_and_keeps_its_aspect_ratio() {
    // Both images are in the markup; which one shows is decided here, from
    // the `data-theme` the boot script set in <head> before first paint.
    let modern = server().get("/assets/modern.css").body;
    let retro = server().get("/assets/retro.css").body;

    // Sizing is unscoped on purpose: retro recolours but does not lay out,
    // so scoping it would leave the default theme's logo unsized. `height:
    // auto` is what keeps each logo's own ratio (with the width/height
    // attributes in the markup).
    let sized =
        css_rule(&modern, ".wordmark img").expect("modern.css must size the wordmark images");
    assert!(sized.contains("height: auto"), "{sized}");
    assert!(sized.contains("width: 26rem"), "{sized}");

    // The badge is hidden for everyone and re-shown under the retro theme by
    // an attribute selector, which outranks modern.css's hide whatever order
    // the two sheets are applied in. The strip needs the mirror rule or the
    // retro bar would show both logos at once.
    let hide_badge = css_rule(&modern, ".wordmark img.wordmark-retro")
        .expect("modern.css must hide the retro badge by default");
    assert!(hide_badge.contains("display: none"), "{hide_badge}");

    let show_badge = css_rule(
        &retro,
        r#"[data-theme="retro"] .wordmark img.wordmark-retro"#,
    )
    .expect("retro.css must show the badge in the retro theme");
    assert!(show_badge.contains("display: block"), "{show_badge}");
    // The same rule scales the badge to ~80% of modern.css's shared width;
    // its specificity is what lets the override win.
    assert!(show_badge.contains("width: 21rem"), "{show_badge}");

    let hide_strip = css_rule(
        &retro,
        r#"[data-theme="retro"] .wordmark img.wordmark-modern"#,
    )
    .expect("retro.css must hide the modern strip in the retro theme");
    assert!(hide_strip.contains("display: none"), "{hide_strip}");
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
fn a_fresh_web_install_ships_with_the_starter_resources() {
    // One server *with* seeding, on its own database: the shared harness
    // opts out via ANTA_VISTA_NO_SEED, because most tests assert exact
    // result sets and three bundled rows would crowd them. Seeding is pure
    // local SQL over baked data — no network, no model — so this needs
    // neither x0x nor MiniLM.
    let s = TestServer::start_seeded();
    let status = s.json("/api/status");
    let db = &status["database"];
    assert_eq!(
        db["resources"].as_i64(),
        Some(3),
        "fresh install must ship exactly the three starters: {db}"
    );
    assert_eq!(
        db["embeddings"].as_i64(),
        Some(3),
        "every starter must carry its baked embedding: {db}"
    );
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
    // The banner is install-only, so `needs_install` tracks the binary, not
    // whether a daemon is running. The test host may or may not have x0x, so
    // assert the relationship rather than a host-dependent value.
    assert_eq!(
        s["needs_install"],
        !deps["x0x_installed"].as_bool().unwrap_or(false),
        "needs_install must follow the x0x binary being absent, not the daemon being down"
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
fn a_renderable_type_is_served_inline_and_a_binary_one_is_offered_as_a_file() {
    // The disposition was hardcoded `inline`, which for a type a browser cannot
    // display is an empty window with no error and no download — the complaint
    // that an Autonomi link "just closes". The two cases have to differ, decided
    // by `is_browser_renderable` rather than by anything the page believes.
    let s = server();

    let html = s.post_binary(
        "/api/view/blob",
        b"<!doctype html><title>renderable</title>",
        &[("X-Anta-Uri", test_address("disp-html").as_str())],
    );
    assert_eq!(html.status, 200, "{}", html.body);
    let (port, path) = split_viewer_url(&html.json()["url"].as_str().unwrap());
    let (status, head, _) = get_on_port(port, &path);
    assert_eq!(status, 200);
    assert!(
        head.contains("content-disposition: inline"),
        "a webpage must open as a webpage: {head}"
    );

    // Bytes with nothing to display them: named, and downloaded.
    let bin = s.post_binary(
        "/api/view/blob",
        &[0x00, 0x01, 0x02, 0xff, 0xfe],
        &[("X-Anta-Uri", test_address("disp-bin").as_str())],
    );
    assert_eq!(bin.status, 200, "{}", bin.body);
    let name = bin.json()["filename"].as_str().unwrap().to_string();
    let (port, path) = split_viewer_url(&bin.json()["url"].as_str().unwrap());
    let (status, head, _) = get_on_port(port, &path);
    assert_eq!(status, 200);
    assert!(
        head.contains("content-disposition: attachment"),
        "an undisplayable type must be offered as a file, not shown as a blank window: {head}"
    );
    assert!(
        head.contains(&format!("filename=\"{name}\"")),
        "the attachment does not name the file ({name}): {head}"
    );
}

#[test]
fn a_hostile_filename_cannot_inject_a_viewer_header() {
    // The name reaches a header the browser parses, on an origin that is allowed
    // to serve text/html. `suggest_filename` reduces this to one path segment
    // today, so the escaping is belt-and-braces — but a builder that cannot
    // escape is a trap, and this one is four lines.
    let s = server();
    let uri = format!(
        "autonomi://{}/\"injected\":\r\nX-Evil:\r\nyes",
        "a".repeat(64)
    );
    let res = s.post_binary(
        "/api/view/blob",
        &[0u8, 1, 2],
        &[("X-Anta-Uri", uri.as_str())],
    );
    // The location may be refused outright, which is also fine; what must never
    // happen is a second header appearing.
    if res.status == 200 {
        let (port, path) = split_viewer_url(&res.json()["url"].as_str().unwrap());
        let (_, head, _) = get_on_port(port, &path);
        assert!(
            !head.contains("x-evil"),
            "a filename injected a response header: {head}"
        );
        assert!(
            !head.contains("\r\ninjected"),
            "a filename broke out of the header value: {head}"
        );
    }
}

#[test]
fn view_plan_answers_from_a_prefix_without_the_file() {
    // The whole point: a 64 MB record was fetched successfully, then failed
    // because asking for its *name* meant uploading all 64 MB to `/api/view/blob`
    // and hitting `MAX_BODY`. So the plan sends at most a bounded prefix, and
    // an empty body still works — the hint-only answers below are the
    // no-bytes-at-all case, not a special mode.
    let s = server();
    let uri = test_address("plan");

    let res = s.request_raw(
        "POST",
        "/api/view/plan",
        b"",
        None,
        &[("X-Anta-Uri", uri.as_str()), ("X-Anta-Mime", "text/html")],
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["renderable"], true, "text/html is displayable: {body}");
    assert!(
        body["filename"].as_str().unwrap().ends_with(".html"),
        "the name should come from the location and type: {body}"
    );

    // A type we cannot place must answer "download it", which always works.
    let vague = s.request_raw(
        "POST",
        "/api/view/plan",
        b"",
        None,
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("X-Anta-Mime", "application/octet-stream"),
        ],
    );
    assert_eq!(vague.json()["renderable"], false, "{:?}", vague.body);

    // And an absent hint is treated as unknown rather than as permission.
    let none = s.request_raw(
        "POST",
        "/api/view/plan",
        b"",
        None,
        &[("X-Anta-Uri", uri.as_str())],
    );
    assert_eq!(none.json()["renderable"], false, "{:?}", none.body);
}

#[test]
fn the_plan_reads_the_bytes_when_the_sdk_reports_octet_stream() {
    // The Autonomi SDK hands the page `application/octet-stream` for most real
    // records, because a DataMap chunk carries no content type. Believing that
    // sent every real image and PDF to a download instead of showing it, so the
    // plan gets the leading bytes and lets content win.
    let s = server();
    let uri = test_address("prefix-sniff");

    // A JPEG's magic number is three bytes. Octet-stream hint, JPEG bytes.
    let jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00";
    let res = s.request_raw(
        "POST",
        "/api/view/plan",
        jpeg,
        None,
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("X-Anta-Mime", "application/octet-stream"),
        ],
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["mime_type"], "image/jpeg", "{body}");
    assert_eq!(body["renderable"], true, "a JPEG is displayable: {body}");
    assert!(
        body["filename"].as_str().unwrap().ends_with(".jpg"),
        "the name should follow the sniffed type: {body}"
    );

    // A prefix that decodes cleanly but proves nothing stays unrenderable.
    // Calling it text would display mojibake; "download it" always works.
    let vague = s.request_raw(
        "POST",
        "/api/view/plan",
        b"just some leading bytes that say nothing at all",
        None,
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("X-Anta-Mime", "application/octet-stream"),
        ],
    );
    let vague = vague.json();
    assert_eq!(vague["mime_type"], "application/octet-stream", "{vague}");
    assert_eq!(vague["renderable"], false, "{vague}");

    // An HTML page is recognised from the marker, so a fetched web page opens
    // rather than downloading even though its bytes are only a prefix.
    let page = b"<!DOCTYPE html>\n<html><head><title>t</title></head><body>hi</body></html>";
    let page = s.request_raw(
        "POST",
        "/api/view/plan",
        page,
        None,
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("X-Anta-Mime", "application/octet-stream"),
        ],
    );
    assert_eq!(page.json()["renderable"], true, "{}", page.body);
}

#[test]
fn a_download_is_named_with_an_extension_we_know() {
    // The author's name wins, but a name with no extension leaves the operating
    // system unable to open what it just saved. We already know the type by this
    // point, so the extension is added rather than left off.
    let s = server();
    let uri = test_address("named-record");

    // A record the author published as `autonomi://<hex>/photo`.
    let named = format!("{uri}/photo");
    let res = s.request_raw(
        "POST",
        "/api/view/plan",
        b"\xff\xd8\xff\xe0\x00\x10JFIF\x00",
        None,
        &[("X-Anta-Uri", named.as_str())],
    );
    let body = res.json();
    assert_eq!(body["filename"], "photo.jpg", "{body}");

    // An extension the author did choose is left exactly as they wrote it.
    let chosen = format!("{uri}/photo.png");
    let kept = s
        .request_raw(
            "POST",
            "/api/view/plan",
            b"\xff\xd8\xff\xe0\x00\x10JFIF\x00",
            None,
            &[("X-Anta-Uri", chosen.as_str())],
        )
        .json();
    assert_eq!(kept["filename"], "photo.png", "{kept}");
}

#[test]
fn a_seeded_autonomi_record_is_searchable() {
    // The end of the chain the prefix sniff exists to serve: a real Autonomi
    // location, embedded under the profile search actually loads, turns up in
    // the search the page runs. The content here is what
    // `indexAutonomiInBrowser` stores, so if that path changed shape this would
    // be the test to notice.
    let server = server();
    let id = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let uri = test_address("seeded-record");
    seed_searchable_resource(&server.db, id, &uri, "autonomi", "text/html");

    let found = server.json("/api/search?q=quokka%20telemetry");
    let hit = found["results"]
        .as_array()
        .expect("results array")
        .iter()
        .find(|r| r["resource_id"] == id)
        .unwrap_or_else(|| panic!("the seeded record was not searchable: {found}"));
    assert_eq!(hit["location"], uri.as_str());
    assert_eq!(hit["source"], "local", "it is held locally: {hit}");
}

#[test]
fn the_scheme_filter_separates_autonomi_from_http_results() {
    // The web toggle's whole contract: with no `scheme` param both seeded
    // records come back; with `scheme=autonomi` only the Autonomi one does.
    // The local index applies the filter via `location_scheme`, so this
    // exercises the path the checkbox drives.
    let server = server();
    let auto_id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let http_id = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let auto_uri = test_address("scheme-filter-auto");
    seed_searchable_resource(&server.db, auto_id, &auto_uri, "autonomi", "text/html");
    seed_searchable_resource(
        &server.db,
        http_id,
        "https://example.com/quokka-telemetry",
        "https",
        "text/html",
    );

    let both = server.json("/api/search?q=quokka%20telemetry");
    let ids = |found: &serde_json::Value| -> Vec<String> {
        found["results"]
            .as_array()
            .expect("results array")
            .iter()
            .filter(|r| r["resource_id"] == auto_id || r["resource_id"] == http_id)
            .map(|r| r["resource_id"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let mut unfiltered = ids(&both);
    unfiltered.sort();
    assert_eq!(
        unfiltered,
        vec![auto_id.to_string(), http_id.to_string()],
        "without a scheme filter both records are visible: {both}"
    );

    let filtered = server.json("/api/search?q=quokka%20telemetry&scheme=autonomi");
    let mut only = ids(&filtered);
    only.sort();
    assert_eq!(
        only,
        vec![auto_id.to_string()],
        "with scheme=autonomi only the Autonomi record is visible: {filtered}"
    );

    // The alias has to work too: `ant` is what a user (or an old client) may
    // still type, and stored `location_scheme` is canonical `autonomi`.
    let aliased = server.json("/api/search?q=quokka%20telemetry&scheme=ant");
    let mut only = ids(&aliased);
    only.sort();
    assert_eq!(
        only,
        vec![auto_id.to_string()],
        "scheme=ant must behave as scheme=autonomi: {aliased}"
    );
}

#[test]
fn the_search_form_offers_the_autonomi_only_filter() {
    // The control the API filter above is driven from. The checkbox lives
    // beside the form, and the page wires it before any search runs.
    let page = server().get("/").body;
    assert!(
        page.contains(r#"<input type="checkbox" id="autonomi-only">"#),
        "the page is missing the Autonomi-only checkbox"
    );
    assert!(
        page.contains("<span>Autonomi only</span>"),
        "the checkbox has no label"
    );

    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("&scheme=autonomi"),
        "the search request never carries the scheme filter"
    );
    assert!(
        js.contains("'anta-vista-autonomi-only'"),
        "the filter is not persisted across reloads"
    );
}

#[test]
fn flipping_the_autonomi_toggle_filters_the_shown_rows_without_a_new_search() {
    // A re-query to repaint the same rows is slow and second-chances removed
    // results: the rows already carry their locations, so the decision is
    // computable in the page. The fresh-search path still sends
    // `scheme=autonomi`; only the toggle changed.
    let js = js_code(&server().get("/assets/app.js").body);

    // The change handler filters in place; it must not call runSearch.
    let change = js
        .find("ui.autonomiOnly.addEventListener('change'")
        .expect("the toggle must have a change handler");
    let change_end = js[change..]
        .find("function setCount(")
        .map(|i| change + i)
        .expect("setCount follows the handler");
    let handler = &js[change..change_end];
    assert!(
        handler.contains("applySchemeFilter()"),
        "the toggle must filter the rows in place: {handler}"
    );
    assert!(
        !handler.contains("runSearch("),
        "flipping the toggle must not re-query the network: {handler}"
    );

    // Rows carry the location the filter decides on.
    assert!(
        js.contains("node.dataset.location = result.location"),
        "result rows must carry their location for the filter"
    );

    // The scheme is derived the way the server derives it, and an undecidable
    // one fails closed.
    assert!(
        js.contains("scheme === 'ant' ? 'autonomi' : scheme"),
        "the client must normalise the ant alias like the server"
    );

    // The "just indexed" row is feedback, not a search result: the filter
    // spares it, or indexing an http link with the filter on would show an
    // empty screen.
    assert!(
        js.contains("if (row.dataset.fromIndex) continue;"),
        "the filter must not hide the just-indexed row"
    );

    // The count follows the visible rows, not the DOM children.
    assert!(
        js.contains(".filter((row) => !row.hidden).length"),
        "the result count must count visible rows"
    );
}

#[test]
fn an_octet_stream_typed_record_is_served_inline_when_its_bytes_say_otherwise() {
    // The server half of the fix. The viewer already sniffed full bytes, so this
    // did not change; it is pinned because the *client* used to decide against
    // the SDK's reported type and download a record the viewer would happily
    // have displayed. If this regresses, the page stops being able to open real
    // images and only the download path remains.
    let server = server();
    let uri = test_address("inline-jpeg");
    let jpeg = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00";

    let body = server
        .post_binary(
            "/api/view/blob",
            jpeg,
            &[
                ("X-Anta-Uri", uri.as_str()),
                ("X-Anta-Mime", "application/octet-stream"),
            ],
        )
        .json();
    let (port, path) = split_viewer_url(body["url"].as_str().expect("url"));
    let (status, head, _bytes) = get_on_port(port, &path);
    let head = head.to_lowercase();

    assert_eq!(status, 200);
    assert!(
        head.contains("content-disposition: inline"),
        "a JPEG must be shown, not downloaded: {head}"
    );
    assert!(
        head.contains("content-type: image/jpeg"),
        "served as the sniffed type: {head}"
    );
}

#[test]
fn the_client_and_server_agree_on_the_prefix_cap() {
    // Two constants in two languages. If they drift, the page either gets a 400
    // it cannot explain or uploads more than it means to, and neither is visible
    // from either side alone.
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("PLAN_PREFIX = 16 * 1024"),
        "the page's prefix cap changed without the server following"
    );
    assert!(
        js.contains("blob.slice(0, PLAN_PREFIX)"),
        "the page stopped bounding what it sends"
    );
}

#[test]
fn a_markdown_record_is_named_from_its_content() {
    // Markdown has no signature, so `infer` cannot see it, and the SDK reports
    // octet-stream for it anyway. It arrived as an untyped blob with no
    // extension at all; it is now named for what it is.
    let s = server();
    let uri = test_address("markdown");
    let doc = b"# Weather log\n\nWind is picking up from the west.\n\n## Tomorrow\n\nRain.";

    let res = s.request_raw(
        "POST",
        "/api/view/plan",
        doc,
        None,
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("X-Anta-Mime", "application/octet-stream"),
        ],
    );
    let body = res.json();
    assert_eq!(body["mime_type"], "text/markdown", "{body}");
    assert!(
        body["filename"].as_str().unwrap().ends_with(".md"),
        "a markdown record should be named .md: {body}"
    );
    // Not displayable, and deliberately so: a browser given `text/markdown`
    // treats it as an unknown type, so opening it is the blank-window case. The
    // name was the thing that was missing.
    assert_eq!(body["renderable"], false, "{body}");
}

#[test]
fn the_plan_refuses_a_body_larger_than_its_prefix_cap() {
    // Clamping would be defensible here (a truncated prefix is still a prefix)
    // but it would hide a page uploading far more than it means to, and this
    // endpoint's whole premise is that it uploads very little.
    let s = server();
    let uri = test_address("prefix-cap");
    let over = vec![b'a'; 16 * 1024 + 1];
    let res = s.request_raw(
        "POST",
        "/api/view/plan",
        &over,
        None,
        &[("X-Anta-Uri", uri.as_str())],
    );
    assert_eq!(res.status, 400, "{}", res.body);
    assert_eq!(res.json()["error"], "body_too_large", "{}", res.body);

    // Exactly at the cap is allowed, so the limit is not off by one.
    let at = vec![b'a'; 16 * 1024];
    let ok = s.request_raw(
        "POST",
        "/api/view/plan",
        &at,
        None,
        &[("X-Anta-Uri", uri.as_str())],
    );
    assert_eq!(ok.status, 200, "{}", ok.body);
}

#[test]
fn a_failed_fetch_explains_itself_in_the_tab_it_opened() {
    // The tab used to close on every error, leaving the explanation behind it.
    let s = server();
    for code in [
        "fetch_failed",
        "no_backend",
        "blocked_popup",
        "anything_else",
    ] {
        let res = s.get(&format!("/view/error?code={code}"));
        assert_eq!(res.status, 200, "{code}: {}", res.body);
        assert!(
            res.body.contains("</html>"),
            "{code} did not render a page: {}",
            res.body
        );
    }

    // Nothing from the network reaches this page: an SDK error string is
    // attacker-influenced, so the vocabulary is these codes and nothing else.
    let injected = s.get("/view/error?code=%3Cscript%3Ealert(1)%3C/script%3E");
    assert!(
        !injected.body.contains("<script>"),
        "the code parameter was reflected as markup: {}",
        injected.body
    );
}

#[test]
fn opening_severs_the_opener_and_saving_never_uploads_the_file() {
    // Two regressions, both static. `window.open`'s `noopener` feature returns
    // null, which is why the handle is used and `opener` is nulled by hand —
    // cross-origin stops a fetched document reading the app, but nothing stopped
    // it navigating the app, and content from a peer is exactly the untrusted
    // input here.
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("tab.opener = null"),
        "the opened tab keeps a reference to the app"
    );
    assert!(
        !js.contains("'noopener'"),
        "the noopener feature returns null and would break the redirect"
    );

    // The download path — now the branch of `runOpen` taken when the browser
    // cannot display the type (or the file is over the viewer cap); it used to
    // live under a separate Save button — used to POST the record to
    // /api/view/blob to read back a filename, so saving anything over
    // MAX_BODY failed after downloading it fine.
    assert!(
        js.contains("startSWDownload(plan.filename, plan.mime_type, opened.totalBytes, 'row')"),
        "an over-cap file headed for disk must stream through the worker"
    );
    assert!(
        js.contains("downloadBlob(blob, plan.filename)"),
        "an undisplayable file is fetched but never saved"
    );
    // Exactly two whole-file uploads exist in the page, one per open path
    // (a reopen of bytes already here, and a fresh fetch), and both open a
    // displayable copy for the viewer. Learning a filename must never be one.
    assert_eq!(
        js.matches("await postBlob(").count(),
        2,
        "the file must be uploaded only to open a displayable copy, never to learn its name"
    );
    // The streamed save itself cannot slip one in: the region between starting
    // the worker download and the buffered fallback's first read holds the
    // pipe and the success report, and nothing that talks to /api/view/blob.
    // Bounded on `plan.filename` so the media player's own Save — which also
    // calls startSWDownload, for a dialog that is not this flow — cannot be
    // mistaken for it, and on code rather than the fallback's comment
    // (`js_code` strips those, so a comment boundary would let the region run
    // to end of file and trip over the indexing helper's own `postBlob`).
    let streamed = js
        .split(
            "const port = await startSWDownload(plan.filename, plan.mime_type, opened.totalBytes, 'row');",
        )
        .nth(1)
        .expect("the download branch no longer starts a worker download")
        .split("blob = await restToBlob(handle, prefixParts, received, opened);")
        .next()
        .expect("the buffered fallback is gone");
    assert!(
        !streamed.contains("postBlob"),
        "streaming a download must not upload the file: {streamed}"
    );

    // Asking the server now means sending *some* bytes, because the Autonomi SDK
    // reports octet-stream for most records and the type has to come from the
    // content. What must never come true is sending the whole file: that is the
    // bug this endpoint exists to avoid, and a prefix makes it cheap to keep
    // avoiding it.
    assert!(
        js.contains("blob.slice(0, PLAN_PREFIX)"),
        "the plan sends the whole file rather than a prefix"
    );
    let plan = js
        .split("const postPlan")
        .nth(1)
        .expect("no postPlan")
        .split("function downloadBlob")
        .next()
        .unwrap_or_default();
    assert!(
        !plan.contains("postBlob"),
        "the plan routes the file through the full-body uploader: {plan}"
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
    /* The token is the *first* segment only. The published URL also carries the
     * filename so the tab is titled with it, and taking the whole remainder here
     * would have quietly turned `/v/{token}x` into a well-formed named path —
     * which is why this test needed rewriting rather than loosening: the
     * neighbour assertions are the load-bearing half. */
    let token = path
        .trim_start_matches("/v/")
        .split('/')
        .next()
        .expect("a token")
        .to_string();

    /* The published URL works, and so does the bare token — the name is
     * decoration, so a viewer that never learned it can still be opened. */
    assert_eq!(get_on_port(port, &path).0, 200);
    assert_eq!(get_on_port(port, &format!("/v/{token}")).0, 200);
    /* ...and nothing else on that port does. An unguessable path is the entire
     * authorisation: there is no cookie and no shared secret, so a second
     * listener's bytes must not be reachable through the first one's port. */
    assert_eq!(get_on_port(port, "/v/not-the-token").0, 404);
    assert_eq!(get_on_port(port, "/").0, 404);
    assert_eq!(get_on_port(port, &format!("/v/{token}/../{token}")).0, 404);
    assert_eq!(get_on_port(port, &format!("/v/{token}x")).0, 404);
    /* A second segment is not a name, it is a different resource: the name is
     * one segment and only ever appended, so nothing may follow it. */
    assert_eq!(get_on_port(port, &format!("/v/{token}/name/extra")).0, 404);
    /* Traversal dressed as a name, and an empty one. */
    assert_eq!(get_on_port(port, &format!("/v/{token}/..")).0, 404);
    assert_eq!(get_on_port(port, &format!("/v/{token}/")).0, 404);
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
    //
    // The real cap is 512 MiB — far too big to post in a test — so a dedicated
    // server is spawned with the cap lowered through the same env seam the
    // route reads it from. What is pinned here is the refusal mechanism and
    // its code; the number lives in the two-constant pin below.
    let s = TestServer::spawn_with(false, &[("ANTA_VISTA_INDEX_BODY_CAP", "65536")]);
    let uri = test_address("oversize");
    let over = vec![b'a'; 65536 + 1];
    let res = s.post_binary("/api/index/bytes", &over, &[("X-Anta-Uri", uri.as_str())]);
    assert_eq!(
        res.status, 400,
        "oversized body was not refused: {}",
        res.status
    );
    assert_eq!(res.json()["error"], "body_too_large");

    // Exactly at the cap is allowed — the cap is "at most", not "under" — and
    // the file really lands in the index rather than being acknowledged and
    // dropped.
    let uri = test_address("at-cap");
    let at = vec![b'a'; 65536];
    let out = s
        .post_binary("/api/index/bytes", &at, &[("X-Anta-Uri", uri.as_str())])
        .json();
    assert_eq!(
        out["ok"], true,
        "a body exactly at the cap was refused: {out}"
    );
    let found = s.json("/api/search?q=aaaaaa");
    assert!(
        found["results"]
            .as_array()
            .expect("results")
            .iter()
            .any(|r| r["location"] == uri.as_str()),
        "an at-cap upload was acknowledged but not indexed: {found}"
    );

    // The two caps live in two languages and must agree: the page refuses the
    // upload before the server would refuse the body, so a disagreement is a
    // fetch the page will not start for a file the server would have taken.
    let js = s.get("/assets/app.js").body;
    assert!(
        js.contains("MAX_INDEX_UPLOAD = 512 * 1024 * 1024"),
        "the page's index cap no longer matches the server's INDEX_BODY"
    );
}

#[test]
fn the_vendored_sdk_is_served_with_types_the_browser_accepts() {
    let s = server();
    let js = s.get("/assets/vendor/autonomi-sdk/0.1.2/index.js");
    assert_eq!(js.status, 200, "vendor index.js missing: {}", js.body);
    assert!(js.headers.contains("javascript"), "{}", js.headers);

    // The WASM is the reason the WASM MIME rule exists: served as
    // `application/octet-stream` a module import fails to stream-compile.
    let wasm = s.get("/assets/vendor/autonomi-sdk/0.1.2/wasm/ant_core_bg.wasm");
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
        page.body.contains("/assets/vendor/autonomi-sdk/0.1.2/"),
        "page does not name the sdk base"
    );
}

#[test]
fn the_streaming_service_worker_is_served_as_a_script_it_can_register() {
    // The download route for large non-displayable records: the page pipes
    // chunks to this worker and the browser spools them straight to disk.
    // Registration is by this exact path (a rename strands every download
    // click on a 404), the MIME must be a script type or the browser refuses
    // to register it, and `no-store` is what lets a rebuilt worker replace
    // the one the browser is already running across navigations.
    let s = server();
    let sw = s.get("/autonomi-stream-sw.js");
    assert_eq!(sw.status, 200, "worker script missing: {}", sw.body);
    assert!(sw.headers.contains("javascript"), "{}", sw.headers);
    assert!(
        sw.headers.contains("no-store"),
        "a cached worker outlives the code it was compiled from: {}",
        sw.headers
    );

    // Both halves of the handshake must exist, or the click and the worker
    // never meet: the page names the script, mints tokens and speaks the
    // download route; the worker answers the same route with a pull protocol.
    let js = s.get("/assets/app.js").body;
    for needle in [
        "/autonomi-stream-sw.js",
        "av-download",
        "MessageChannel",
        "serviceWorker",
    ] {
        assert!(js.contains(needle), "app.js is missing {needle}");
    }
    for needle in ["av-download", "pull", "Content-Disposition"] {
        assert!(sw.body.contains(needle), "the worker is missing {needle}");
    }
}

#[test]
fn the_media_route_is_the_patched_upstream_worker() {
    // The player streams through the same worker, on upstream's
    // `__autonomi_stream` route — merged *patched*, and the patches are the
    // whole reason it is merged rather than copied: `nosniff` on every
    // response and an audio/video-only Content-Type allowlist, so a
    // same-origin route can never serve peer-supplied bytes as a document.
    // The page teaches the worker the MIME our sniffing decided
    // (`av-media-type`) because the SDK reports octet-stream for most
    // records — and octet-stream is exactly what the allowlist must refuse.
    let sw = server().get("/autonomi-stream-sw.js").body;
    for needle in [
        "/__autonomi_stream/",
        "autonomi-file-range",
        "av-media-type",
        "X-Content-Type-Options",
        "parseRange",
        "audio|video",
    ] {
        assert!(sw.contains(needle), "the worker is missing {needle}");
    }
    // The refused answer exists, or the allowlist is decoration.
    assert!(
        sw.contains("serves audio and video only"),
        "a non-media type has no refusal on the media route"
    );
    // Seeks and teardown cancel the stream while a range fetch is in
    // flight; enqueuing into the cancelled stream throws Chrome's
    // "Cannot enqueue a chunk into a closed readable stream", which looked
    // like a fault in the console and was not one. The cancel flag keeps
    // that noise out without silencing real failures.
    let media = sw
        .split("async function handleStreamRequest(")
        .nth(1)
        .expect("the media route is gone")
        .split("function parseRange(")
        .next()
        .expect("the media route is unbounded");
    assert!(
        media.contains("cancelled"),
        "a cancelled media stream still logs its wasted range as a failure: {media}"
    );
}

#[test]
fn the_inline_media_player_streams_and_saves_through_the_worker() {
    // Audio and video play in the main page, streamed by the SDK's media
    // source through the worker's range route — never uploaded, never held
    // as a Blob. Save to disk reuses the download route from the same
    // worker. The controls live in the dialog, not the result row: the
    // row's fetch group holds only Stop while bytes move.
    let s = server();
    let page = s.get("/");
    for needle in [
        "id=\"media-dialog\"",
        "id=\"media-player\"",
        "id=\"media-save\"",
        "id=\"media-open-tab\"",
    ] {
        assert!(page.body.contains(needle), "the page is missing {needle}");
    }

    let js = js_code(&s.get("/assets/app.js").body);
    for needle in [
        "createMediaSource",
        "av-media-type",
        "teachMediaType",
        "mediaSessionId",
        "closePlayerIf",
        "isMediaMime",
    ] {
        assert!(js.contains(needle), "app.js is missing {needle}");
    }
    // Non-modal on purpose: listening while browsing is the point, and a
    // modal dialog would hide the row's own Block button while its file
    // plays — which the close-on-block hook exists to honour.
    assert!(
        js.contains("ui.mediaDialog.show()"),
        "the player must not block the page behind it"
    );
    assert!(
        !js.contains("ui.mediaDialog.showModal()"),
        "a modal player makes block-while-playing unreachable"
    );
}

#[test]
fn the_worker_is_kept_awake_for_the_whole_length_of_a_transfer() {
    // Firefox terminates a service worker that has heard nothing for ~30
    // seconds — even one that is mid-stream, spooling a download to disk
    // (StreamSaver's README states the timeout outright; immich #21490
    // reproduces the kill failing large saves). The dead worker takes the
    // response body with it, and Firefox reports the half-written `.part`
    // file as one whose "source file could not be read" — which reads as
    // the file being broken, not as the worker being killed. Chrome counts
    // an open response body as activity and waits minutes, so the same
    // save works there. A message event *is* activity at the worker, so
    // the page pings while anything of ours rides it: the save, from
    // starting the download to the pipe settling, and a playing session
    // for as long as it exists — the worker holds the session's taught
    // MIME, and a paused player resumed after the kill would get a 400
    // instead of its bytes.
    let s = server();
    let js = js_code(&s.get("/assets/app.js").body);
    let sw = s.get("/autonomi-stream-sw.js").body;

    assert!(
        js.contains("type: 'av-ping'"),
        "the page never pings the worker"
    );
    assert!(sw.contains("av-ping"), "the worker has no keepalive arm");

    // The ping must outrun the clock: an interval at or past Firefox's
    // window would be a ping that arrives after the kill it exists to
    // prevent.
    let keepalive = js
        .split("function keepaliveAdd(")
        .nth(1)
        .expect("no keepaliveAdd")
        .split("function keepaliveRemove(")
        .next()
        .expect("keepaliveAdd has no end");
    let interval: u64 = keepalive
        .rsplit("}, ")
        .next()
        .and_then(|tail| tail.split(')').next())
        .and_then(|ms| ms.trim().parse().ok())
        .expect("the keepalive interval is not a plain number of milliseconds");
    assert!(
        interval < 30_000,
        "a ping every {interval}ms cannot outrun Firefox's ~30s idle kill"
    );

    // Both lifetimes are wired: the save is added where the download
    // starts and released when the pipe settles; the playing session is
    // added where the session is set and released in teardown, which every
    // exit path (close, block, replacement) goes through.
    for needle in [
        "keepaliveAdd('download')",
        "keepaliveRemove('download')",
        "keepaliveAdd('media')",
        "keepaliveRemove('media')",
    ] {
        assert!(js.contains(needle), "app.js is missing {needle}");
    }
}

#[test]
fn the_media_save_opens_the_file_before_it_starts_the_download() {
    // The player's Save used to start the browser download first and open
    // the Autonomi stream second, so every save began with seconds of a
    // `.part` file receiving nothing while the lookup ran — the exact
    // silence in which Firefox's idle clock kills the worker — and a
    // failure to open left that hungry download hung behind it. Open
    // first: the first chunk follows the click, a failed open starts no
    // download at all, and the no-worker fallback drains the reader that
    // is open rather than fetching the whole file a second time.
    let js = js_code(&server().get("/assets/app.js").body);
    let save = js
        .split("ui.mediaSave.addEventListener(")
        .nth(1)
        .expect("the player's Save handler is gone")
        .split("function wireFetch(")
        .next()
        .expect("the save region is unbounded");
    let open = save
        .find("await openAutonomi(session.address")
        .expect("the save never opens the file");
    let start = save
        .find("await startSWDownload(")
        .expect("the save never starts a worker download");
    let pipe = save
        .find("await pipeReaderToPort(")
        .expect("the save never pipes the bytes it opened");
    assert!(
        open < start && start < pipe,
        "the save must open the file before starting the download: {save}"
    );
    assert!(
        !save.contains("fetchAutonomi("),
        "the fallback re-fetches a file whose reader is already open: {save}"
    );
    assert!(
        save.contains("drainRest(handle"),
        "the fallback does not drain the open reader: {save}"
    );
    // A cancel has to be said out loud here: this Save offers no Stop of its
    // own, so a `DownloadCancelled` reaching the catch is the browser's
    // download UI (or the worker reporting it) dropping the transfer — the
    // silent return it used to take read as a finished save that produced
    // no file, with nothing in the console to paste into a report either.
    // The frozen progress line stays in front of the sentence: the count of
    // chunks delivered is the datum that shows how far the transfer got.
    assert!(
        save.contains("console.warn('the media save was cancelled")
            && save.contains("nothing was saved")
            && save.contains("lastProgress"),
        "a cancelled media save hides its progress: {save}"
    );
}

#[test]
fn the_saved_file_is_named_by_the_server_not_by_the_sdk() {
    // `file.name` is always the SDK's synthetic `public-file-<8hex>.bin`,
    // and the player session's filename is the one the Save button downloads
    // under and every console line about the save quotes. SDK-first put that
    // placeholder on the user's disk and named the failure reports after it —
    // so a dialog save and a row save of the same file could not be matched
    // to their console lines by name at all. The server's `plan.filename`
    // (suggest_filename) wins; the synthetic name is only a last resort.
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("filename: plan.filename || source.file.name || 'file'"),
        "the saved file is named from the SDK's synthetic name first"
    );
}

#[test]
fn the_download_declares_the_length_the_datamap_already_knows() {
    // `openFile` exposes the record's size before a single chunk moves, and
    // the page hands it to the worker with the token: the browser's
    // download UI then shows a total instead of an anonymous crawl, and a
    // body that stops short of the declared length fails loudly rather than
    // leaving a silent hole in a file that looks complete. Unknown (-1)
    // means no header — still a valid response, never a wrong one.
    let s = server();
    let js = js_code(&s.get("/assets/app.js").body);
    assert!(
        js.contains("startSWDownload(plan.filename, plan.mime_type, opened.totalBytes, 'row')"),
        "the row's save does not pass the size it knows to the worker"
    );
    assert!(
        js.contains("size: Number.isSafeInteger(size) && size >= 0 ? size : -1"),
        "the size travels with the token unvalidated"
    );
    let sw = s.get("/autonomi-stream-sw.js").body;
    let download = sw
        .split("function streamDownload(")
        .nth(1)
        .expect("the download route is gone")
        .split("/* -----")
        .next()
        .expect("the download route is unbounded");
    assert!(
        download.contains("'Content-Length'"),
        "the worker does not declare the length it was given: {download}"
    );
}

#[test]
fn the_streamed_row_save_delivers_the_bytes_the_plan_already_read() {
    // The plan reads the file's leading bytes before the verdict is known
    // (the first 1 MiB read covers the 16 KiB prefix cap), and the streaming
    // save then pipes the *rest* while the worker declares the DataMap's
    // full length. Those prefix bytes have to go down the same pipe: with
    // `received` starting at 0, every streamed save ended in
    // `received !== totalBytes` — small files failing on the spot ("0 of N")
    // and large ones after minutes ("N-minus-a-chunk of N") — and the
    // resulting `controller.error` against a body short of its own
    // Content-Length is exactly the interrupted-download symptom in the
    // browser's download UI. The player's Save reads no prefix and must
    // stay a plain pipe.
    let js = js_code(&server().get("/assets/app.js").body);
    let row = js
        .split(
            "const port = await startSWDownload(plan.filename, plan.mime_type, opened.totalBytes, 'row');",
        )
        .nth(1)
        .expect("the row's save no longer starts a worker download")
        .split("blob = await restToBlob(handle, prefixParts, received, opened);")
        .next()
        .expect("the buffered fallback is gone");
    assert!(
        row.contains("parts: prefixParts"),
        "the pipe never receives the bytes the plan already read: {row}"
    );
    let pipe = js
        .split("function pipeReaderToPort(")
        .nth(1)
        .expect("the pipe is gone")
        .split("const downloadWhy = ")
        .next()
        .expect("the pipe region is unbounded");
    for needle in ["parts = []", "pending.shift()", "received !== totalBytes"] {
        assert!(
            pipe.contains(needle),
            "the pipe is missing {needle}: {pipe}"
        );
    }
    let save = js
        .split("ui.mediaSave.addEventListener(")
        .nth(1)
        .expect("the player's Save handler is gone")
        .split("function wireFetch(")
        .next()
        .expect("the save region is unbounded");
    assert!(
        !save.contains("parts: prefixParts"),
        "the player's Save has no plan prefix to seed: {save}"
    );
    // A failed save names its error in the page console as well as on the
    // status line: the dialog's status is easy to overlook once the attempt
    // is over, and the console line is what gets pasted into a report.
    assert!(
        save.contains("console.error('the media save failed'"),
        "a failed media save leaves nothing in the console: {save}"
    );
}

#[test]
fn saves_through_memory_unless_the_file_is_past_the_cap() {
    // Buffering is the save route every browser completes: the bytes
    // assemble into a Blob and an object URL downloads them, with no
    // service worker in the path at all. It is now the default, and the
    // worker's stream — whose download body Firefox tears down before a
    // byte — is reserved for a file too large to hold in memory, which is
    // the one thing buffering cannot do. Both flows gate the worker behind
    // SAVE_BUFFER_CAP and fall through to the buffered save underneath it,
    // so the order in each is: decide from the size, stream only past the
    // cap, buffer otherwise (and when no worker answers).
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("const SAVE_BUFFER_CAP = 512 * 1024 * 1024"),
        "the line between buffering and streaming is not drawn"
    );
    let row = js
        .split("const why = downloadWhy(plan.renderable, opened.totalBytes, playerFailed);")
        .nth(1)
        .expect("the row's download branch is gone")
        .split("await fetchViaDaemon(")
        .next()
        .expect("the row's download branch is unbounded");
    let gate = row
        .find("opened.totalBytes > SAVE_BUFFER_CAP")
        .expect("the row's save is not gated on the cap");
    let stream = row
        .find(
            "const port = await startSWDownload(plan.filename, plan.mime_type, opened.totalBytes, 'row');",
        )
        .expect("the over-cap stream is gone");
    let buffer = row
        .find("downloadBlob(blob, plan.filename)")
        .expect("the buffered save is gone");
    assert!(
        gate < stream && stream < buffer,
        "the row must decide from the size, then stream past the cap, then buffer: {row}"
    );
    let save = js
        .split("ui.mediaSave.addEventListener(")
        .nth(1)
        .expect("the player's Save handler is gone")
        .split("function wireFetch(")
        .next()
        .expect("the save region is unbounded");
    let gate = save
        .find("opened.totalBytes > SAVE_BUFFER_CAP")
        .expect("the player's save is not gated on the cap");
    let stream = save
        .find("await startSWDownload(")
        .expect("the over-cap stream is gone");
    let buffer = save
        .find("drainRest(handle")
        .expect("the buffered save is gone");
    assert!(
        gate < stream && stream < buffer,
        "the player's save must decide from the size, then stream past the cap, then buffer: {save}"
    );
}

#[test]
fn the_worker_logs_why_a_transfer_failed() {
    // The only console line Firefox gives for an errored worker body is the
    // generic "a ServiceWorker intercepted the request" one — no route, no
    // reason — and a download interrupted in the browser's own UI logs
    // nothing at all in the page, so this failure class was invisible. The
    // worker says what happened: a fresh realm (the length and the taught
    // MIME died with the previous one), a page-reported failure with its
    // message, a token that raced the registration or was spent, and a
    // media range that timed out or was refused. Each failure is also
    // *announced* to every window client, because Firefox shows no worker
    // console in the page console: the worker posts, the page repeats.
    let s = server();
    let sw = s.get("/autonomi-stream-sw.js").body;
    for needle in [
        "av-stream worker started",
        "av-stream: download failed",
        "av-stream: unknown download token",
        "av-stream: media range request failed",
        "av-stream: the browser cancelled this download",
        // The cancel report says how far it got and which flow started it:
        // "cancelled at 0 bytes" versus "cancelled at 40 MB" are different
        // bugs, and a dialog save and a row save failing in one session are
        // two downloads that filename alone cannot tell apart.
        "bytes delivered",
        "source: typeof msg.source === 'string' ? msg.source : ''",
    ] {
        assert!(sw.contains(needle), "the worker is missing {needle}");
    }
    assert!(
        sw.contains("av-stream-failure"),
        "the worker's failures die in a console Firefox never shows"
    );
    let js = js_code(&s.get("/assets/app.js").body);
    assert!(
        js.contains("av-stream-failure"),
        "the page never hears the worker's announced failures"
    );
    // Lifecycle lines reach the page as logs, not errors: a realm restart
    // (the idle-kill signature) must show in the console the user pastes,
    // and at log level so an error filter does not hide the restart that
    // explains the errors.
    assert!(
        js.contains("av-stream-log") && js.contains("console.log(msg.label"),
        "the worker's realm restarts never reach the page console"
    );
    // Every download attempt is logged before anything can fail: which
    // file, which flow — without it a session with two downloads under two
    // names is undecidable from the failure lines alone.
    assert!(
        js.contains("console.log('av-download started'"),
        "a download starts with no trace of which file or flow asked for it"
    );
}

#[test]
fn the_worker_confirms_a_download_registration_before_the_click() {
    // The token reaches the worker by postMessage and the download by a
    // navigation — two routes with no guaranteed arrival order. A fetch that
    // wins gets the worker's empty 404 (a failed entry in the browser's
    // download manager — Chrome's "file wasn't available", Firefox's
    // `.part` that could not be read), while the page's pipe waits forever
    // on pulls that can never come, because the 404 branch starts no stream.
    // So the worker answers on the port it was handed, the page clicks only
    // after that answer, and a worker that never answers gets no click at
    // all — the caller's buffered fallback needs no worker, so the save
    // degrades instead of breaking. Pipe failures are also named in the page
    // console, because Firefox shows no worker logs there.
    let js = js_code(&server().get("/assets/app.js").body);
    let start = js
        .split("async function startSWDownload(")
        .nth(1)
        .expect("the worker download entry point is gone")
        .split("function pipeReaderToPort(")
        .next()
        .expect("the startSWDownload region is unbounded");
    let ready = start
        .find("await confirmed")
        .expect("the page never waits for the worker's confirmation");
    let click = start
        .find("downloadUrl(")
        .expect("the download click is gone");
    assert!(
        ready < click,
        "the click must wait for the confirmation: {start}"
    );
    assert!(
        start.contains("return null"),
        "a worker that never confirms must not be clicked anyway: {start}"
    );
    // A registration update activating between the handshake and the click
    // starts a fresh realm whose token map is empty — the click would land
    // on that realm's 404, which is exactly the failure the confirm exists
    // to prevent, arriving from the other side. So the controller is
    // re-checked after the confirm and the token re-registered with
    // whoever will actually answer the navigation.
    assert!(
        start.contains("active !== controller"),
        "a controller replaced mid-flow is not re-registered: {start}"
    );
    let sw = server().get("/autonomi-stream-sw.js").body;
    assert!(
        sw.contains("type: 'ready'"),
        "the worker never confirms the registration"
    );
    assert!(
        js.contains("console.error('the download pipe failed'"),
        "a pipe failure is invisible in Firefox's page console"
    );
    // Silence after the click is a failure too: a worker that confirmed and
    // then never asks for a byte (the fetch 404'd on a realm that never saw
    // the token, and Firefox's download UI reports that on its own schedule)
    // used to leave the pipe waiting forever, with the save looking hung
    // rather than broken.
    let pipe = js
        .split("function pipeReaderToPort(")
        .nth(1)
        .expect("the pipe is gone")
        .split("const downloadWhy = ")
        .next()
        .expect("the pipe region is unbounded");
    assert!(
        pipe.contains("FIRST_PULL_TIMEOUT_MS"),
        "a worker that never pulls is waited on forever: {pipe}"
    );
    assert!(
        pipe.contains("the worker never asked for any data"),
        "the silent-worker failure has no name: {pipe}"
    );
}

#[test]
fn every_phase_of_a_streaming_save_names_the_side_that_went_silent() {
    // A transfer that died mid-file froze the dialog on its last count: the
    // browser's download UI reports an interrupted body only when the
    // download is inspected, Firefox shows no worker logs in the page
    // console, and the pipe had exactly one deadline — armed once for the
    // first pull and cleared only by `finish` — so its 15 s window fired
    // into every healthy download that outlived it (killing it with a
    // message about a pull that had long since arrived) while a genuinely
    // dead one sat waiting out the same window with no name for what
    // stopped. Each phase now arms one shared timer with its own message
    // — the first pull, the network's next chunk, the worker's next ask —
    // and re-arming replaces the previous deadline, so a window can never
    // outlive its phase. A controller swap (a registration update
    // terminating the serving realm mid-save, whose body then dies under
    // the browser) fails at once with that named instead of waiting the
    // silence out; the listener exists only while a pipe runs.
    let js = js_code(&server().get("/assets/app.js").body);
    let pipe = js
        .split("function pipeReaderToPort(")
        .nth(1)
        .expect("the pipe is gone")
        .split("const downloadWhy = ")
        .next()
        .expect("the pipe region is unbounded");
    for needle in [
        "arm(FIRST_PULL_TIMEOUT_MS",
        "the worker never asked for any data",
        "arm(STALL_TIMEOUT_MS",
        "the network did not deliver the next chunk",
        "the worker stopped asking for data",
        "controllerchange",
        "the streaming worker was replaced mid-save",
        "removeEventListener('controllerchange'",
    ] {
        assert!(
            pipe.contains(needle),
            "the pipe is missing {needle}: {pipe}"
        );
    }
    // Exactly one timer exists in the pipe, inside `arm`: every phase goes
    // through it, so arming a new phase always clears the old deadline.
    assert_eq!(
        pipe.matches("setTimeout(").count(),
        1,
        "a phase owns a timer outside the shared arm: {pipe}"
    );
    // Both live phases re-arm the stall window: after a pull the network
    // owes a chunk, after a chunk the worker owes a pull.
    assert_eq!(
        pipe.matches("arm(STALL_TIMEOUT_MS").count(),
        2,
        "a running transfer is not watched in both directions: {pipe}"
    );
}

#[test]
fn the_page_makes_itself_controlled_by_the_worker_before_anything_needs_it() {
    // The media route hard-requires a *controlled* page: the SDK's bridge
    // waits for `navigator.serviceWorker.controller` to become ours, and
    // `clients.claim()` is known to skip the very document that called
    // `register()` — the browser's and the SDK's shared remedy, and the
    // SDK's own error text, is a single reload. Without one, every player
    // click paid the SDK's 10 s wait twice and fell through to a fallback,
    // while saves merely degraded to the buffered path — which is why the
    // failure looked media-only. So the page settles control at boot:
    // register, give claim its chance, and reload exactly once if the
    // controller still has not arrived; after that reload the active
    // worker controls the page at load, by ordinary rules.
    let js = js_code(&server().get("/assets/app.js").body);

    assert_eq!(
        js.matches("location.reload()").count(),
        1,
        "the reload-once must be one site, reachable only from the boot handshake"
    );
    assert_eq!(
        js.matches("ensureWorkerControlAtBoot();").count(),
        1,
        "the handshake must exist and be called exactly once"
    );
    let boot = js
        .split("applyTheme(document.documentElement.dataset.theme || 'retro');")
        .nth(1)
        .expect("the boot line is gone")
        .split("refreshDeps();")
        .next()
        .expect("the boot section is unbounded");
    assert!(
        boot.contains("ensureWorkerControlAtBoot();"),
        "the handshake must run at boot, before any click can need the worker: {boot}"
    );

    let handshake = js
        .split("function ensureWorkerControlAtBoot(")
        .nth(1)
        .expect("there is no boot control handshake")
        .split("let keepaliveReasons = new Set();")
        .next()
        .expect("the handshake is unbounded");
    assert!(
        handshake.contains("if (navigator.serviceWorker.controller) return;"),
        "a page the worker already controls must not register or reload: {handshake}"
    );
    let guard = handshake
        .find("sessionStorage.getItem")
        .expect("the handshake has no reload guard");
    let flag = handshake
        .find("sessionStorage.setItem")
        .expect("the handshake sets no flag");
    let refused = handshake
        .find("catch (_)")
        .expect("storage that cannot hold the flag is unguarded");
    let register = handshake
        .find("ensureServiceWorker()")
        .expect("the handshake never registers");
    let reload = handshake
        .find("location.reload()")
        .expect("the handshake never reloads");
    assert!(
        guard < flag && flag < refused && refused < register && register < reload,
        "the flag must be spent before any reload, refused storage must skip both, and claim must get its chance first: {handshake}"
    );
    assert!(
        handshake.contains("if (controller) return;"),
        "claim that worked must not reload: {handshake}"
    );
}

#[test]
fn vendor_paths_cannot_escape_the_vendored_directory() {
    for path in [
        "/assets/vendor/antonomi-sdk/0.1.2/../../../etc/passwd",
        "/assets/vendor/autonomi-sdk/0.1.2/wasm/../../../../Cargo.toml",
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
    // only on buttons elsewhere in the row (since removed — the location is now
    // the sole open path) — so the address underlined on
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

        // …and it hovers the way the link it stands in for does. An http
        // result's heading is a real <a> and takes the theme's link-hover
        // colour; an Autonomi result's heading is this span, so withholding
        // the same hover made the two layouts advertise clickability
        // differently. Modern's rule is grouped with `.result-title:hover`
        // and retro's mirrors its red; both are found by the same helper.
        let hover = css_rule(
            css,
            &format!(r#"[data-theme="{label}"] .result-loc.is-openable:hover"#),
        )
        .unwrap_or_else(|| panic!("{label} has no hover rule for the openable location"));
        let expected = if label == "modern" {
            "color: var(--accent)"
        } else {
            "color: #ee0000"
        };
        assert!(
            hover.contains(expected),
            "{label}'s openable location must hover like the title link: {hover}"
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
fn the_autonomi_row_opens_through_its_location_and_carries_no_buttons() {
    // The row deliberately has no Open or Save button: the location is the open
    // path (a click opens what the browser can show and downloads what it
    // cannot), the outcome lands on the status line, and the fetch group holds
    // nothing but Stop — revealed only while bytes are moving.
    let script = server().get("/assets/app.js");
    assert_eq!(script.status, 200);
    let js = script.body.as_str();
    for needle in [
        "data-role=\"fetch-stop\"",
        "autonomiAddress",
        "/api/index/bytes",
        "/api/view/blob",
    ] {
        assert!(js.contains(needle), "app.js is missing {needle}");
    }
    for gone in ["data-role=\"open\"", "data-role=\"save\""] {
        assert!(
            !js.contains(gone),
            "app.js still wires the removed control {gone}"
        );
    }
    // And the markup agrees: no button to click, but a stoppable fetch.
    let page = server().get("/").body;
    assert!(
        !page.contains(">Open</button>"),
        "the template still offers an Open button"
    );
    assert!(
        !page.contains(">Save</button>"),
        "the template still offers a Save button"
    );
    assert!(
        page.contains(">Stop</button>"),
        "a running fetch can no longer be stopped"
    );
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
fn ensure_services_reports_the_services_it_looked_at() {
    // The harness disables automatic startup, so this must not touch the
    // network; it should still report the shape the drawer and search read.
    let res = server().post("/api/services/ensure", "{}");
    assert_eq!(res.status, 200);
    let body = res.json();
    for key in [
        "ok",
        "x0x_running",
        "x0x_started",
        "listener_running",
        "listener_started",
        "detail",
    ] {
        assert!(
            body.get(key).is_some(),
            "ensure response is missing {key}: {body}"
        );
    }
    assert_eq!(body["x0x_started"], false, "autostart is disabled in tests");
    assert_eq!(
        body["listener_started"], false,
        "autostart is disabled in tests"
    );
}

#[test]
fn stopping_x0x_is_safe_when_it_is_not_running() {
    // Never stop the developer's own daemon from a test.
    let running = server().get("/api/deps").json()["deps"]["x0x_running"]
        .as_bool()
        .unwrap_or(false);
    if running {
        return;
    }
    let res = server().post("/api/deps/x0x/stop", "{}");
    assert_eq!(res.status, 200);
    let body = res.json();
    assert_eq!(
        body["ok"], true,
        "stopping an absent daemon is a no-op success: {body}"
    );
    assert_eq!(body["running"], false);
}

#[test]
fn the_drawer_has_start_and_stop_for_both_services() {
    // Per-service controls, because Stop is momentary and the user needs to be
    // able to bring each one back without a whole `av web` restart.
    let body = server().get("/").body;
    for id in ["x0x-start", "x0x-stop", "listener-start", "listener-stop"] {
        assert!(
            body.contains(&format!("id=\"{id}\"")),
            "the status drawer should offer {id}: {body}"
        );
    }
    assert!(
        !body.contains("id=\"stop-listener\""),
        "the lone stop-listener button was replaced by per-service controls"
    );
}

#[test]
fn the_setup_banner_is_for_installs_only() {
    // A service that is merely stopped is started automatically, so the banner
    // must key on a missing binary and never on a daemon being down.
    let js = js_code(&server().get("/assets/app.js").body);
    let start = js
        .find("async function refreshDeps(")
        .expect("refreshDeps should still exist");
    let end = js
        .find("async function ensureServices(")
        .expect("ensureServices should exist");
    let deps = &js[start..end];
    assert!(
        deps.contains("data.needs_install"),
        "the banner must key on the binary being absent: {deps}"
    );
    assert!(
        !deps.contains("x0x_running"),
        "a stopped daemon must not re-raise the banner: {deps}"
    );
    assert!(
        !deps.contains("av_listener_running"),
        "a stopped listener must not re-raise the banner: {deps}"
    );
    assert!(
        !deps.contains("toast("),
        "the removed start branch also removed the undefined toast() calls: {deps}"
    );
}

#[test]
fn search_and_index_ensure_the_services_first() {
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("/api/services/ensure"),
        "the page must call the ensure endpoint"
    );
    let search = js.find("async function runSearch(").expect("search body");
    let add = js
        .find("ui.addForm.addEventListener('submit'")
        .expect("add handler");
    assert!(
        js[search..add].contains("await ensureServices("),
        "search must bring the services up before it queries the network"
    );
    assert!(
        js[add..].contains("await ensureServices("),
        "index must bring the services up before it queries the network"
    );
}

#[test]
fn the_index_dialog_is_the_only_index_output() {
    // The one-line `#add-status` paragraph was too easy to miss; the dialog is
    // the sole output now, so nothing else may still write status text.
    let body = server().get("/").body;
    assert!(
        body.contains(r#"id="index-progress-dialog""#),
        "the index dialog must exist: {body}"
    );
    assert!(
        !body.contains("add-status"),
        "the old status line is gone from the markup"
    );
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        !js.contains("addStatus"),
        "app.js must not keep writing to the removed status line"
    );
}

#[test]
fn byte_progress_replaces_one_line_and_stages_append() {
    // The SDK reports many times a second; appending every event would fill the
    // log and push the stages above it out of view.
    let js = js_code(&server().get("/assets/app.js").body);
    let helpers = js
        .find("function setIndexProgressLive")
        .expect("setIndexProgressLive helper");
    let after = js[helpers..]
        .find("\nfunction ")
        .unwrap_or(js.len() - helpers);
    let body = &js[helpers..helpers + after];
    // The live helper pops the previous line rather than growing the log.
    assert!(
        body.contains("lines.pop()"),
        "byte progress must replace the previous line, not append: {body}"
    );
    // The buffer always ends with '\n', so `split` leaves a trailing '' — pop it
    // *before* popping the live line, or each update is glued onto the last one
    // and the log reads "Fetching…Fetching…Fetching…".
    assert!(
        body.contains("if (lines[lines.length - 1] === '') lines.pop();"),
        "the trailing newline must be dropped before the live line: {body}"
    );
    assert!(
        body.contains("lines.join('\\n') + '\\n'"),
        "the rebuilt buffer must end with exactly one newline: {body}"
    );
    // A stage line goes in on top and marks the next update as a new append.
    assert!(
        js.contains("function indexProgressLine"),
        "stage lines still append"
    );
}

#[test]
fn showModal_is_guarded_so_a_dialog_cannot_be_opened_twice() {
    // showModal() on an already-open dialog throws InvalidStateError, which is
    // how the browser route broke when the submit handler also opened it.
    let js = js_code(&server().get("/assets/app.js").body);
    let opens = js.matches("ui.indexProgress.showModal()").count();
    assert_eq!(opens, 1, "exactly one showModal call site");
    assert!(
        js.contains("if (!ui.indexProgress.open) ui.indexProgress.showModal()"),
        "that single call site must be guarded by the open state"
    );
}

#[test]
fn closing_the_index_dialog_shows_the_indexed_resource() {
    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("ui.indexProgress.addEventListener('close', flushPendingIndex)"),
        "the dialog's close event must reveal the indexed row"
    );
    let render = js
        .find("function renderIndexedResult")
        .expect("renderIndexedResult");
    let after = js[render..]
        .find("\nfunction ")
        .unwrap_or(js.len() - render);
    let body = &js[render..render + after];
    for needle in [
        "ui.results.replaceChildren(",
        "renderResult({",
        "Just indexed",
        "resource_id: outcome.resource_id",
    ] {
        assert!(
            body.contains(needle),
            "the indexed row must be built from the outcome: {body}"
        );
    }
    // A fresh index has no score; the row hides the element rather than lying.
    assert!(
        js.contains("scoreEl.hidden = true"),
        "an unscored row must hide its score element"
    );
}

#[test]
fn the_index_dialog_ends_on_a_short_line_naming_the_resource() {
    // The description can be 800 characters of the document's own text, and the
    // log is scrolled to the bottom — so the success line was a wall of text you
    // had to scroll up from. The last line must be short and name the location.
    let js = js_code(&server().get("/assets/app.js").body);
    let added = js
        .find("data.embedding_dim} dimensions")
        .expect("the added line should still report the embedding size");
    let closing = js
        .find("has been indexed.")
        .expect("the dialog must end on a closing line");
    let finish = js[added..]
        .find("finishIndex(data)")
        .map(|i| added + i)
        .expect("finishIndex call");
    assert!(
        added < closing && closing < finish,
        "the closing line must come after the added line and before the flush: added={added} closing={closing} finish={finish}"
    );
    let line = &js[closing - 60..closing + 20];
    assert!(
        line.contains("Resource: ${data.location}"),
        "the closing line must name the location, not the description: {line}"
    );
    // A blank line separates it from the Added line above, so it does not read
    // as the tail of that paragraph.
    let blank = js[added..closing]
        .find("indexProgressLine('')")
        .expect("a blank line before the closing line");
    assert!(
        blank < closing - added,
        "the blank line must sit between the added line and the closing line"
    );
}

#[test]
fn a_search_that_lands_during_an_index_keeps_its_results() {
    // The index request can outlive its dialog (Esc-dismissed, or closed while
    // a search is in flight); its late flush must not replace the search's
    // results with the indexed row and a note claiming the screen shows it.
    let js = js_code(&server().get("/assets/app.js").body);

    // The search bumps the epoch on submit and again when it renders. The
    // body lives in `runSearch`, shared by the form's submit and the
    // "Autonomi only" toggle's re-run, so both entry points take the epoch.
    let search = js.find("async function runSearch(").expect("search body");
    let add = js
        .find("ui.addForm.addEventListener('submit'")
        .expect("add handler");
    let search_body = &js[search..add];
    assert!(
        search_body.contains("resultsEpoch += 1"),
        "a submitted search must take over the results epoch: {search_body}"
    );
    // The rows go away on submit, and the note is written *about* those rows —
    // it must go with them, or "This is the resource you just indexed" sits
    // under "Searching…" for the whole wait (renderResults only runs on resolve).
    let note_at = search_body
        .find("setResultsNote('', '')")
        .expect("a fresh search must clear the results note on submit");
    let count_at = search_body
        .find("setCount('Searching")
        .expect("the search must still announce itself");
    assert!(
        note_at < count_at,
        "the stale note must be cleared before \u{201c}Searching\u{2026}\u{201d} appears"
    );
    let render = js.find("function renderResults(").expect("renderResults");
    let render_end = js[render..]
        .find("\nfunction ")
        .map(|i| render + i)
        .unwrap_or(js.len());
    assert!(
        js[render..render_end].contains("resultsEpoch += 1"),
        "a rendered search must take over the results epoch"
    );

    // The index records the epoch it started under...
    assert!(
        js[add..].contains("indexEpoch = resultsEpoch"),
        "the index must record which epoch it belongs to"
    );
    // ...and the flush only fires while that epoch still holds.
    let flush = js
        .find("function flushPendingIndex(")
        .expect("flushPendingIndex");
    let flush_end = js[flush..]
        .find("\nfunction ")
        .map(|i| flush + i)
        .unwrap_or(js.len());
    let flush_body = &js[flush..flush_end];
    assert!(
        flush_body.contains("if (resultsEpoch !== indexEpoch) return"),
        "a late flush must yield the screen to the search: {flush_body}"
    );
    assert!(
        flush_body.contains("renderIndexedResult(outcome)"),
        "the uncontested flush still renders the row: {flush_body}"
    );
}

#[test]
fn the_retro_progress_terminal_is_theme_scoped() {
    // retro.css loads after modern.css, so an unscoped rule there would win in
    // the modern theme too.
    let css = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/retro.css"),
    )
    .expect("retro.css");
    assert_eq!(
        css_rule(&css, "[data-theme=\"retro\"] .progress-output").as_deref(),
        Some(
            "[data-theme=\"retro\"] .progress-output {\n  margin: 6px 0;\n  padding: 6px 8px;\n  \
             background: #000;\n  color: #00ff00;\n  border: 2px inset #c0c0c0;\n  max-height: \
             240px;\n  overflow-y: auto;\n  font-family: var(--mono, monospace);\n  font-size: \
             11px;\n  white-space: pre-wrap;\n  word-break: break-word; }"
        ),
        "the retro terminal style must be scoped to the retro theme"
    );
    // And the unscoped selector must not exist — it would beat modern.css.
    assert!(
        css_rule(&css, ".progress-output").is_none(),
        "retro.css must not carry an unscoped .progress-output rule"
    );
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

// --- The stored name, end to end -------------------------------------------
//
// A record published at `autonomi://<hex>/welcome.md` stores the name, and its
// *canonical* location is the address alone — that is its identity, for dedup and
// for the block bridge. So the search result carries `autonomi://<hex>`, the page
// echoes that back, and the name is nowhere in the request. These pin that the
// name still reaches every route that names a file.

/// The `X-Anta-Uri` the page actually sends for a record we hold: the canonical
/// address, with the author's path segment already gone.
fn held_record_address(tag: &str) -> (String, String) {
    let hex = format!("{:0>64}", tag);
    let canonical = format!("autonomi://{hex}");
    let with_name = format!("{canonical}/welcome.md");
    (canonical, with_name)
}

/// Seed the shape `ingest_bytes` produces for an Autonomi record with a name.
fn seed_named_record(db: &std::path::Path, id: &str, canonical: &str, location: &str) {
    use av_core::types::{ResourceDescriptor, ResourceKind};

    let conn = av_store::open(db).expect("open test db");
    av_store::repo::resources::insert(
        &conn,
        &ResourceDescriptor {
            id: id.to_string(),
            kind: ResourceKind::Text,
            location: location.to_string(),
            location_scheme: Some("autonomi".to_string()),
            location_canonical: Some(canonical.to_string()),
            mime_type: "text/markdown".to_string(),
            filename: Some("welcome.md".to_string()),
            metadata_json: serde_json::json!({}),
            description_text: "seeded for the stored-name test".to_string(),
            created_at: 1_700_000_000,
        },
    )
    .expect("insert resource");
}

#[test]
fn the_plan_names_a_file_from_the_name_we_stored_for_it() {
    // The reported bug, exactly: `autonomi://<hex>/welcome.md` saved as
    // `52af8a181c6171f8.md` because the name was written to `resources.filename`
    // and read back out of the URI, which by then was the address alone.
    let s = server();
    let id = "5aaf8a181c6171f8e12896ca734cdf60839c3b324dfbeadf857226dfc7cb453b";
    let (canonical, location) = held_record_address(id);
    seed_named_record(&s.db, id, &canonical, &location);

    let res = s.request_raw(
        "POST",
        "/api/view/plan",
        b"# Weather log\n\nWind is picking up from the west.",
        None,
        &[
            ("X-Anta-Uri", canonical.as_str()),
            ("X-Anta-Mime", "application/octet-stream"),
        ],
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(
        body["filename"], "welcome.md",
        "the stored name must reach the download: {body}"
    );
    assert_eq!(body["mime_type"], "text/markdown", "{body}");
}

#[test]
fn an_unheld_record_still_falls_back_to_the_address() {
    // The lookup is a best-effort improvement to a label, not a new requirement:
    // a record we have never indexed has no stored name, and the address-derived
    // name is always safe to produce.
    let s = server();
    let uri = test_address("never-indexed");
    let res = s.request_raw(
        "POST",
        "/api/view/plan",
        b"# Weather log\n\nWind.",
        None,
        &[
            ("X-Anta-Uri", uri.as_str()),
            ("X-Anta-Mime", "application/octet-stream"),
        ],
    );
    let body = res.json();
    assert_eq!(res.status, 200, "{}", res.body);
    assert!(
        body["filename"].as_str().unwrap().ends_with(".md"),
        "still typed from the content: {body}"
    );
}

#[test]
fn an_opened_record_is_served_under_its_stored_name() {
    // The open route, which never named anything: the tab showed a bare uuid and
    // the browser's own "Save image as..." offered the same uuid with no
    // extension. Both come from the name reaching the viewer URL and the
    // disposition.
    let s = server();
    let id = "6aaf8a181c6171f8e12896ca734cdf60839c3b324dfbeadf857226dfc7cb453b";
    let (canonical, location) = held_record_address(id);
    seed_named_record(&s.db, id, &canonical, &location);

    let res = s.post_binary(
        "/api/view/blob",
        b"<!doctype html><title>Weather log</title>",
        &[("X-Anta-Uri", canonical.as_str())],
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["filename"], "welcome.md", "{body}");

    let url = body["url"].as_str().expect("url");
    assert!(
        url.ends_with("/welcome.md"),
        "the tab must be titled with the name, not the token: {url}"
    );
    let (port, path) = split_viewer_url(url);
    let (status, head, _) = get_on_port(port, &path);
    assert_eq!(status, 200);
    assert!(
        head.contains("content-disposition: inline; filename=\"welcome.md\""),
        "an inline response must still name the file the browser would write: {head}"
    );
    assert!(
        !head.contains("filename=\"\""),
        "a doubled pair of quotes is a name the user sees on disk: {head}"
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

/// Seed a resource that a real search will actually return.
///
/// The `p-purge` helper above is deliberately invisible to search: it embeds
/// under a profile no search loads, so it cannot perturb another test's
/// results. This one is the opposite — it exists to be *found* — so it embeds
/// under the profile `MockEmbeddingProvider` reports (`all-MiniLM-L6-v2:v1`).
///
/// The vector must be 384-dimensional to match that profile, and unit-length
/// because the profile declares `normalized`. `cosine_similarity` opens with
/// `debug_assert_eq!(a.len(), b.len())`, so a short vector panics every search
/// in this file under `cargo test`; with assertions compiled out it would be
/// worse, since `iter().zip()` would silently score against a truncated vector.
/// A one-hot vector is unit-length by construction.
fn seed_searchable_resource(
    db: &std::path::Path,
    id: &str,
    location: &str,
    scheme: &str,
    mime: &str,
) {
    use av_core::types::{ResourceDescriptor, ResourceKind};

    const PROFILE: &str = "all-MiniLM-L6-v2:v1";
    const DIM: usize = 384;

    let conn = av_store::open(db).expect("open test db");
    av_store::repo::resources::insert(
        &conn,
        &ResourceDescriptor {
            id: id.to_string(),
            kind: ResourceKind::Text,
            location: location.to_string(),
            location_scheme: Some(scheme.to_string()),
            location_canonical: Some(location.to_string()),
            mime_type: mime.to_string(),
            filename: None,
            metadata_json: serde_json::json!({}),
            description_text: "a seeded record about quokka telemetry".to_string(),
            created_at: 1_700_000_000,
        },
    )
    .expect("insert resource");

    // OR IGNORE: the tests share one database, so another test may have created
    // this profile already.
    conn.execute(
        "INSERT OR IGNORE INTO embedding_profiles
           (profile_id, model_id, model_version, dim, normalized, preproc_version)
         VALUES (?1, 'all-MiniLM-L6-v2', 'v1', ?2, 1, 'v1')",
        rusqlite::params![PROFILE, DIM as i64],
    )
    .expect("insert profile");

    // One-hot: a single 1.0 in an otherwise zero vector. Unit length, so it is a
    // legal normalised embedding without depending on the real model.
    let mut vector = vec![0.0f32; DIM];
    vector[0] = 1.0;
    conn.execute(
        "INSERT INTO embeddings (resource_id, profile_id, vector_json, l2_norm, created_at)
         VALUES (?1, ?2, ?3, 1.0, 1700000000)",
        rusqlite::params![id, PROFILE, serde_json::to_string(&vector).unwrap()],
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
fn a_result_row_shows_the_name_we_hold_beside_the_location() {
    // The name is an annotation of the identifier, not a replacement for it: the
    // location is what every command takes, and for Autonomi it is the bare
    // address, which reads as noise on its own. Asserted against the CSS with
    // `css_rule`-style whole-selector matching in spirit — the element must exist
    // in the template, app.js must fill it from `result.filename`, and it must be
    // `hidden` when there is no name, so a peer result shows no empty gap.
    let html = &server().get("/").body;
    assert!(
        html.contains("data-role=\"name\""),
        "the row has no place to show a name"
    );

    let js = &server().get("/assets/app.js").body;
    let js = js_code(&js);
    assert!(
        js.contains("const storedName = result.filename || ''"),
        "app.js must read the name the CLI projects"
    );
    assert!(
        js.contains("nameEl.hidden = !storedName"),
        "and hide the element when there is none, rather than leaving a gap"
    );
    // It must not become the heading. That is the identifier's job, and the
    // parity test above is what keeps the two layouts from drifting again.
    assert!(
        !js.contains(
            "const heading = source === 'name' ? result.description || location : storedName"
        ),
        "the stored name must not take over the heading"
    );

    // Both themes style it, so a theme switch cannot leave it unstyled.
    for sheet in ["modern.css", "retro.css"] {
        let css = &server().get(&format!("/assets/{sheet}")).body;
        assert!(
            css.contains(".result-name"),
            "{sheet} does not style the stored name"
        );
    }
}

#[test]
fn the_cli_prints_the_stored_name_beside_the_location_too() {
    // The browser and the terminal must not disagree about what a row shows, and
    // this is the half of that the JavaScript assertions cannot see. Only local
    // rows carry a name: a peer does not send one, so printing an empty field for
    // a network result would be inventing data.
    let src = include_str!("../src/cmd/search.rs");
    assert!(
        src.contains("\"filename\": r.resource.filename"),
        "the local result must project the stored name"
    );
    assert!(
        src.contains("let filename = result[\"filename\"].as_str().unwrap_or(\"\")"),
        "the local row must read it back out"
    );
    assert!(
        src.contains("console::style(filename).dim()"),
        "and annotate the identifier with it. Deliberately not asserting on the \
         separator glyph: the source carries the escape, not the character, so \
         such an assertion tests the spelling rather than the behaviour."
    );
    // A network row must not grow one.
    let network = &src[src.find("\"source\": \"network\"").unwrap_or(0)..];
    let before_next = &network[..network
        .find("\"source\": \"name\"")
        .unwrap_or(network.len())];
    assert!(
        !before_next.contains("\"filename\""),
        "peers do not send a filename; the field would be empty"
    );
}

#[test]
fn the_search_rating_prompt_warns_that_a_network_recommendation_may_share() {
    // The browser confirms before recommending a network link ("Share this
    // link?" / "Recommending it may share the link with other people using
    // this network") because the recommendation propagates the link into
    // the local index, where the listener serves it on. The terminal's one
    // prompt covers every result at once, so the same warning goes up with
    // the question — only when the list holds a network result, or it
    // would train the user to ignore warnings on local-only searches.
    let src = include_str!("../src/cmd/search.rs");
    assert!(
        src.contains("console::style(RATE_SHARING_NOTE).yellow()"),
        "the interactive rating prompt must show the sharing warning"
    );
    assert!(
        src.contains("r[\"source\"].as_str() == Some(\"network\")"),
        "the warning must appear only when a network result is there to share"
    );
    assert!(
        src.contains("Which result was most relevant?"),
        "the rating question itself must survive unchanged"
    );
    let note = av_cli::cmd::search::RATE_SHARING_NOTE;
    assert!(
        note.contains("may share the link with other people using this network"),
        "the note must carry the GUI's own warning clause: {note}"
    );
    let web = include_str!("../src/web/render.rs");
    assert!(
        web.contains("Recommending it may share the link with other people using this network."),
        "the GUI's confirm is the counterpart this note mirrors — if one \
         side stops saying it, both must be reconsidered together"
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
fn the_ping_endpoint_identifies_the_server() {
    let body = server().get("/api/ping").body;
    let v: Value = serde_json::from_str(&body).expect("ping answers JSON");
    assert_eq!(v["ok"], true);
    assert_eq!(v["service"], "anta-vista");
}

/// Run `av` in the same isolated environment the server harness uses, so a
/// second launch cannot touch the developer's real database or daemons.
fn isolated_web(args: &[&str]) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("av-web-second-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    Command::new(env!("CARGO_BIN_EXE_av"))
        .args(args)
        .env("ANTA_VISTA_DB_PATH", dir.join("test.db"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("HOME", dir.join("home"))
        .env("ANTA_VISTA_NO_AUTOSTART", "1")
        .env("ANTA_VISTA_NO_SEED", "1")
        .output()
        .expect("run av web")
}

#[test]
fn a_second_web_on_a_busy_port_reuses_the_running_server() {
    // The browser's close button does not stop the server, so the natural
    // second start — another double-click of the binary, or `av web` while
    // one runs — lands on a busy port. It must find the first instance, say
    // where it is, and exit successfully, not die on the bind with an error
    // an icon launch has nowhere to show.
    let running = server();
    let port = running.port.to_string();
    let out = isolated_web(&["web", "--no-open", "--port", &port, "--run-for", "3"]);
    assert!(
        out.status.success(),
        "second launch should reuse the first; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("already running"),
        "second launch should say where the running server is; stdout:\n{stdout}"
    );
}

#[test]
fn a_foreign_listener_on_the_port_is_not_mistaken_for_av() {
    // Anything else on the port — a hung anta-vista included, since a hung
    // one answers nothing — must still get the bind error rather than being
    // "reused" into a browser tab that cannot work.
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = occupied.local_addr().expect("addr").port().to_string();
    let out = isolated_web(&["web", "--no-open", "--port", &port, "--run-for", "3"]);
    assert!(
        !out.status.success(),
        "a foreign occupant must not pass for anta-vista"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed to bind"),
        "the bind error should still be reported; stderr:\n{stderr}"
    );
}

#[test]
fn the_dead_tab_banner_reconnects_on_its_own() {
    let markup = server().get("/").body;
    assert!(
        markup.contains(r#"<div id="server-banner" class="banner" hidden role="status"></div>"#),
        "the dead-server banner must exist and start hidden"
    );

    let js = js_code(&server().get("/assets/app.js").body);
    // A rejected fetch — from the poll or from any user action — raises the
    // banner at once rather than waiting for the next interval.
    assert!(
        js.contains("noteServerDown()"),
        "a failed fetch must mark the server down"
    );
    // The next successful poll takes it down, which is what makes the tab
    // come back on its own after av is restarted on the same address.
    assert!(
        js.contains("noteServerUp()"),
        "a successful poll must clear the down state"
    );
    assert!(
        js.contains("serverDown ? SERVER_RETRY_MS : SERVER_POLL_MS"),
        "a down server must be re-checked fast, so a restart is noticed promptly"
    );
    assert!(
        js.contains("button.textContent = 'Check now'"),
        "the banner must offer an immediate check"
    );
    // The lazy interval is gone: a down server would otherwise take up to
    // 15s to be noticed as back.
    assert!(
        !js.contains("setInterval(refreshDeps"),
        "the deps poll must be a chain whose delay depends on the down state"
    );
}

/// Strip `/* */` and `//` comments from JavaScript, so an assertion about code
/// cannot pass or fail on the word appearing in the comment explaining it.
///
/// The same trap `css_rule` exists to avoid: this file's own comments name the
/// identifiers the tests look for, including the one this feature is about, so a
/// bare `contains` here would match prose. Not a full parser — string literals
/// are passed through untouched, which is fine because the assertions only look
/// for call shapes.
fn js_code(js: &str) -> String {
    let mut out = String::with_capacity(js.len());
    let mut rest = js;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => return out,
        };
    }
    out.push_str(rest);

    out.lines()
        .map(|line| match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
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

/* ------------------------------------------------------ daemon fetch progress -- */

/// Serve one body, slowly, so a fetch against it stays in flight long enough to
/// observe that progress is reported *while* it runs.
///
/// Returns the port. The delay is between the headers and the body, which is
/// what `download_content` blocks on — so a status event must be readable while
/// this is still sleeping.
fn slow_body_server(body: &'static [u8], delay: Duration) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        // Read the request line so the client is not still writing when we reply.
        let mut seen = [0u8; 1024];
        let _ = stream.read(&mut seen);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        std::thread::sleep(delay);
        let _ = stream.write_all(body);
        let _ = stream.flush();
    });
    port
}

/// Start a daemon fetch and return its job id, or panic with the server's answer.
fn start_fetch(s: &TestServer, uri: &str) -> String {
    let res = s.post_binary("/api/view/fetch", b"", &[("X-Anta-Uri", uri)]);
    assert_eq!(res.status, 200, "start failed: {}", res.body);
    res.json()["job"]
        .as_str()
        .unwrap_or_else(|| panic!("no job id in {}", res.body))
        .to_string()
}

/// Read a job until it is done, collecting every event it reports.
///
/// Mirrors what the page does, so this exercises the same route the page polls.
fn poll_until_done(s: &TestServer, job: &str) -> (Vec<Value>, Duration) {
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut since = 0usize;
    for _ in 0..2000 {
        let res = s.get(&format!("/api/view/fetch/{job}?since={since}"));
        assert_eq!(res.status, 200, "poll failed: {}", res.body);
        let body = res.json();
        for event in body["events"].as_array().cloned().unwrap_or_default() {
            since += 1;
            seen.push(event);
        }
        if body["done"] == serde_json::Value::Bool(true) {
            return (seen, started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("fetch job {job} never finished");
}

#[test]
fn a_daemon_fetch_reports_progress_before_it_finishes() {
    let s = server();
    let port = slow_body_server(b"a small file body", Duration::from_secs(3));
    let uri = format!("http://127.0.0.1:{port}/record.bin");

    let job = start_fetch(&s, &uri);

    // The fetch is against a server that will not answer for three seconds, so
    // anything readable now is genuinely progress and not the finished result.
    let early = s.get(&format!("/api/view/fetch/{job}?since=0"));
    assert_eq!(early.status, 200, "{}", early.body);
    let early = early.json();
    assert!(
        !early["events"].as_array().unwrap().is_empty(),
        "the fetch had not started reporting: {}",
        early
    );
    assert_eq!(
        early["done"], false,
        "it must not claim to be finished while the body is still in flight"
    );

    let (events, _) = poll_until_done(&s, &job);
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "status" || e["type"] == "chunk"),
        "no progress was ever reported: {events:?}"
    );
    let done = events.last().expect("at least one event");
    assert_eq!(done["type"], "done", "{events:?}");
    assert_eq!(done["transport"], "daemon");
    assert!(done["url"].as_str().unwrap().contains("/v/"), "{done}");
}

#[test]
fn a_poll_returns_each_event_once_and_resumes_from_a_cursor() {
    // The page's cursor is the only thing keeping a resumed poll from re-showing
    // events the user has already read, so it is worth pinning directly.
    let port = slow_body_server(b"cursor body", Duration::from_millis(300));
    let uri = format!("http://127.0.0.1:{port}/cursor.bin");
    let job = start_fetch(server(), &uri);

    let first = server()
        .get(&format!("/api/view/fetch/{job}?since=0"))
        .json();
    let count = first["events"].as_array().unwrap().len();
    assert!(count > 0, "no events to resume from");

    let resumed = server()
        .get(&format!("/api/view/fetch/{job}?since={count}"))
        .json();
    let rest = resumed["events"].as_array().unwrap();
    // Only the tail is new, and nothing from the first page is repeated.
    for event in rest {
        assert!(
            !first["events"].as_array().unwrap().contains(event),
            "an event was delivered twice: {event}"
        );
    }
}

#[test]
fn a_job_that_was_never_started_is_not_a_404_with_a_list() {
    // An absent job and an empty one look the same from outside, so this is one
    // refusal rather than a way to probe which fetches are in flight.
    let res = server().get("/api/view/fetch/deadbeefdeadbeef?since=0");
    assert_eq!(res.status, 404);
    assert_eq!(res.json()["error"], "not_found");

    // And a shape that is not a job id is refused without being looked up.
    for bad in ["", "..", "x", &"z".repeat(200)] {
        let res = server().get(&format!("/api/view/fetch/{bad}"));
        assert!(
            res.status == 404 || res.status == 400,
            "{bad:?} should not be a job: {}",
            res.body
        );
    }
}

#[test]
fn a_daemon_fetch_serves_the_record_under_the_same_name_and_type_as_an_upload() {
    // The two routes must agree, or the same record is called one thing when the
    // browser fetches it and another when the daemon does.
    let port = slow_body_server(b"%PDF-1.7\nsmall pdf body\n", Duration::from_millis(200));
    let uri = format!("http://127.0.0.1:{port}/report.pdf");
    let job = start_fetch(server(), &uri);
    let (events, _) = poll_until_done(server(), &job);
    let done = events
        .iter()
        .find(|e| e["type"] == "done")
        .unwrap_or_else(|| panic!("no done event: {events:?}"));

    assert_eq!(done["filename"], "report.pdf");
    assert_eq!(done["mime_type"], "application/pdf");

    // And the bytes it published are actually servable.
    let (viewer_port, path) = split_viewer_url(done["url"].as_str().unwrap());
    let mut stream = TcpStream::connect(("127.0.0.1", viewer_port)).expect("connect viewer");
    // `Connection: close`, because the viewer deliberately keeps its port open for
    // 120 s. Without this the read below blocks until that lapses, which turns a
    // test about filenames and bytes into a minute-long test.
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .expect("write");
    let mut out = Vec::new();
    stream.read_to_end(&mut out).expect("read");
    assert!(
        out.windows(8).any(|w| w == b"%PDF-1.7"),
        "the viewer did not serve the fetched bytes"
    );
}

#[test]
fn a_failed_daemon_fetch_reports_the_error_and_stops_the_page_polling() {
    // A fetch that fails has to end the job. A job left un-finished would leave
    // the tab's progress line spinning forever.
    // Port 1 is reserved and nothing listens on it, so the connection is
    // refused straight away — a fetch that fails immediately still has to end
    // its job.
    let job = start_fetch(server(), "http://127.0.0.1:1/never.bin");
    let (events, elapsed) = poll_until_done(server(), &job);
    assert!(
        elapsed < Duration::from_secs(30),
        "a refused connection should not take this long: {elapsed:?}"
    );
    let last = events.last().expect("at least one event");
    assert_eq!(last["type"], "error", "{events:?}");
    assert!(last["message"].as_str().unwrap().len() > 3, "{last}");
}

#[test]
fn a_blocked_location_is_refused_before_any_route_reaches_the_network() {
    let s = server();
    let uri = test_address("blocked-fetch");
    let indexed = s.post_binary(
        "/api/index/bytes",
        b"a record that is about to be blocked",
        &[("X-Anta-Uri", uri.as_str())],
    );
    assert_eq!(indexed.status, 200, "{}", indexed.body);
    let id = indexed.json()["resource_id"]
        .as_str()
        .expect("resource_id")
        .to_string();

    // Nothing is blocked yet, so the guard must not be what answers. On a host
    // with no autonomi backend this is 503, and either way it is not 403 — which
    // is what makes this assertion hold whatever ant and antd are installed.
    let before = s.post_binary("/api/view/fetch", b"", &[("X-Anta-Uri", uri.as_str())]);
    assert_ne!(
        before.status, 403,
        "nothing is blocked yet, so 403 is the guard misfiring: {}",
        before.body
    );

    let blocked = s.post(
        "/api/block",
        &format!(r#"{{"subject_kind":"resource","subject_id":"{id}"}}"#),
    );
    assert_eq!(blocked.status, 200, "{}", blocked.body);

    // Both routes that fetch on the user's behalf. `/api/download` had no check at
    // all until now, which is why this is one test rather than two.
    let fetch = s.post_binary("/api/view/fetch", b"", &[("X-Anta-Uri", uri.as_str())]);
    assert_eq!(fetch.status, 403, "view/fetch: {}", fetch.body);
    assert_eq!(fetch.json()["error"], "blocked");

    let download_path = format!("/api/download?location={}", urlencode(&uri));
    let download = s.get(&download_path);
    assert_eq!(download.status, 403, "download: {}", download.body);
    assert_eq!(download.json()["error"], "blocked");

    // The plan is asked *before* the bytes are in hand — it is the browser's
    // own pre-fetch refusal — so a blocked location must be turned away here
    // too, or the guard only exists on paths the page has already stopped
    // taking. And indexing writes rows, so it refuses before any store write.
    let plan = s.post_binary(
        "/api/view/plan",
        b"prefix bytes",
        &[("X-Anta-Uri", uri.as_str()), ("X-Anta-Mime", "text/plain")],
    );
    assert_eq!(plan.status, 403, "view/plan: {}", plan.body);
    assert_eq!(plan.json()["error"], "blocked");

    let index = s.post_binary(
        "/api/index/bytes",
        b"more bytes for a blocked location",
        &[("X-Anta-Uri", uri.as_str())],
    );
    assert_eq!(index.status, 403, "index/bytes: {}", index.body);
    assert_eq!(index.json()["error"], "blocked");

    // The page's pre-fetch check asks this before any byte moves. A blocked
    // location gets the *same* 403 every fetching route would have answered —
    // one refusal shape, so the page and the server cannot disagree.
    let ask = s.get(&format!("/api/blocked?location={}", urlencode(&uri)));
    assert_eq!(ask.status, 403, "blocked?: {}", ask.body);
    assert_eq!(ask.json()["error"], "blocked");

    // Unblocking lifts it, so the guard is not just refusing everything.
    let unblocked = s.post(
        "/api/unblock",
        &format!(r#"{{"subject_kind":"resource","subject_id":"{id}"}}"#),
    );
    assert_eq!(unblocked.status, 200);
    assert_ne!(
        s.post_binary("/api/view/fetch", b"", &[("X-Anta-Uri", uri.as_str())])
            .status,
        403,
        "the block outlived the unblock"
    );
    let ask = s.get(&format!("/api/blocked?location={}", urlencode(&uri)));
    assert_eq!(ask.status, 200, "blocked?: {}", ask.body);
    assert_eq!(
        ask.json()["blocked"],
        false,
        "blocked? outlived the unblock"
    );
}

/// Percent-encode a location for a query string.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/* -------------------------------------------------------- progress reporting -- */

#[test]
fn the_progress_tab_updates_one_line_for_byte_counts_and_appends_for_phases() {
    // The two kinds of SDK event are not the same kind of thing, and treating them
    // alike makes the page useless exactly when it is needed.
    //
    // The SDK emits `downloading` many times a second. Appended, those fill the
    // 40-line log with "1.2 MB of 8.0 MB" long before the file arrives and push
    // out the phase transitions — "finding the nodes holding this file" — which
    // are the lines that tell the user the thing is still working. So a byte count
    // replaces a single element and a phase goes to the headline.
    let page = server().get("/view/progress").body;
    assert!(
        page.contains(r#"case 'live':"#),
        "the page has no channel for a continuous count"
    );

    // And it replaces rather than appends: no `addLine` on that path, which is the
    // half that actually causes the flooding.
    let live_case = page
        .split("case 'live':")
        .nth(1)
        .and_then(|rest| rest.split("break;").next())
        .expect("no live case");
    assert!(
        !live_case.contains("addLine("),
        "a byte count must replace its line, not append: {live_case}"
    );
    assert!(
        live_case.contains("live.textContent"),
        "the live line must be the element that changes: {live_case}"
    );

    // The element has to exist and have a `[hidden]` rule of its own, for the same
    // reason `.counts` does: `hidden` is defeated by any author `display`, and the
    // UA `[hidden]` rule lives in a different origin's stylesheet.
    assert!(page.contains(r#"id="live""#), "no live element to write to");
    assert!(
        page.contains(".live[hidden]"),
        "the live line would show before the first event arrives"
    );
}

#[test]
fn sdk_progress_drives_the_detail_line_and_never_the_counter() {
    // This used to require the SDK's byte counts to go to the `live` channel. That is
    // now deliberately wrong, and the reason is worth keeping: we count chunks read
    // against `reader.size` ourselves, so the SDK's counts are a *second* source for
    // the same read-out. Both arriving made the line alternate between
    // "Downloading — 1.2 MB of 8.0 MB" and "12 chunks · 1.4 MB of 8.0 MB" as each
    // fired, which reads as a broken counter rather than a progress one.
    //
    // So the SDK drives the detail line only, and nothing sends its progress as
    // `status` either — that appends, and it is what floods the log.
    let js = js_code(&server().get("/assets/app.js").body);
    let body = js
        .split("function reportSdkProgress")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("reportSdkProgress is gone");

    assert!(
        body.contains("type: 'phase'"),
        "a phase belongs in the detail line: {body}"
    );
    assert!(
        !body.contains("type: 'live'"),
        "the SDK must not share the counter we already own: {body}"
    );
    assert!(
        !body.contains("type: 'status'"),
        "sending SDK progress as `status` is what floods the log: {body}"
    );
    // The label still has to be derived from the event, including the numbers, so
    // the headline reads honestly rather than as a bare phase name.
    assert!(
        body.contains("progressLine(event)"),
        "the phase has to reach the page somehow: {body}"
    );
}

#[test]
fn the_daemon_client_polls_a_job_rather_than_reading_a_stream() {
    // tiny_http cannot stream (see `web::fetchjob`), so a reader-based client is
    // not a style choice — it receives every event in one burst at the end, which
    // is the blank tab with a streaming header. The server's own module carries the
    // argument; this pins the client to match.
    let js = js_code(&server().get("/assets/app.js").body);
    let body = js
        .split("async function fetchViaDaemon")
        .nth(1)
        .and_then(|rest| rest.split("\nasync function").next())
        .expect("fetchViaDaemon is gone");

    assert!(
        !body.contains("getReader("),
        "reading a response body as a stream cannot work against this server: {body}"
    );
    assert!(
        body.contains("/api/view/fetch"),
        "the daemon route is missing: {body}"
    );
    // The cursor must come from the server. Counting delivered events is equal to
    // the cursor only while nothing has been trimmed, which stops being true on
    // exactly the long fetches the progress page exists for.
    assert!(
        body.contains("update.next"),
        "the page must resume from the server's cursor: {body}"
    );
    assert!(
        !body.contains("since += 1"),
        "counting delivered events silently stalls the tab once the log is trimmed: {body}"
    );
}

#[test]
fn the_tab_is_told_the_library_load_before_the_sdk_can_report_it() {
    // Regression, from a real report: the progress tab sat on "Starting…" for the
    // entire fetch.
    //
    // The cause is not in this project's code at all. `loadSdk()` is a dynamic
    // `import()` of a package carrying 5.4 MB of WASM, and the SDK's first progress
    // event does not fire until after `getNetworkDefaults` has compiled and
    // instantiated it — inside `connect`, before its reporter is even built. So
    // the longest silent stretch in the whole fetch produced no events, and the
    // page's opening sentence was all the user saw for most of the wait.
    //
    // The opener knows what it is doing at the instant it starts, so it says so,
    // without waiting for a third-party callback that cannot report yet.
    let js = js_code(&server().get("/assets/app.js").body);
    let body = js
        .split("async function openAutonomi")
        .nth(1)
        .and_then(|rest| rest.split("\nasync function drainRest").next())
        .expect("openAutonomi is gone");

    let step_at = body
        .find("onStep")
        .expect("the open never announces the library load");
    let load_at = body.find("await loadSdk()").expect("loadSdk is gone");
    assert!(
        step_at < load_at,
        "the load must be announced before it starts, not after: {body}"
    );
}

#[test]
fn the_client_is_connected_once_and_the_abort_stays_with_the_reader() {
    // `AutonomiClient.connect(bootstrap, options)` treats a plain object in the
    // first argument as a network profile and takes its settings from a rest-spread
    // of *that object*; the second argument is only read for the string form (one
    // explicit WebRTC multiaddress). So `connect({ onProgress }, { signal })` looks
    // right and silently drops the signal — that rule now lives in `getSharedClient`,
    // which passes one object and no second argument at all.
    //
    // The per-operation signal is gone from `connect` on purpose: the client is
    // shared by every open, save and index on the page, so no one operation owns
    // the connection. Stop reaches its own work where it always did — on `openFile`
    // and on the stream — and a shared client whose connection has died is replaced
    // by an invalidate-and-retry rather than kept until the next page load.
    let js = js_code(&server().get("/assets/app.js").body);

    assert_eq!(
        js.matches("AutonomiClient.connect(").count(),
        1,
        "there must be exactly one connect on the page, behind the shared helper: {js}"
    );

    let shared = js
        .split("async function getSharedClient")
        .nth(1)
        .and_then(|rest| rest.split("\nasync function openAutonomi").next())
        .expect("getSharedClient is gone");
    assert!(
        shared.contains("AutonomiClient.connect({ onProgress })"),
        "the connect must take its settings in the single options object: {shared}"
    );
    assert!(
        !shared.contains(", { signal }"),
        "a shared connection is not one operation's to abort: {shared}"
    );
    // A failed connect must not be cached, or one bad minute condemns the
    // browser route until the next page load.
    assert!(
        shared.contains("sharedClientPromise = null"),
        "a failed connect must clear the slot so the next operation retries: {shared}"
    );

    let open = js
        .split("async function openAutonomi")
        .nth(1)
        .and_then(|rest| rest.split("\nasync function drainRest").next())
        .expect("openAutonomi is gone");
    assert!(
        open.contains("openFile(address, { onProgress: progress, signal })"),
        "the per-operation abort lives at openFile: {open}"
    );
    assert!(
        open.contains("invalidateSharedClient()"),
        "a dead shared client must be replaceable, or every later open falls back to the daemon: {open}"
    );
    // A stop is never what spends the reconnect: the abort check throws before
    // the invalidate can run.
    let stop_at = open
        .find("if (signal && signal.aborted) throw err;")
        .expect("the catch does not rethrow a stop");
    let invalidate_at = open
        .find("invalidateSharedClient()")
        .expect("the retry does not invalidate");
    assert!(
        stop_at < invalidate_at,
        "Stop must be answered before the reconnect, or stopping starts a connect: {open}"
    );
}

#[test]
fn stopping_a_browser_fetch_never_falls_back_to_the_daemon() {
    // Stop aborts the WebRTC fetch, and the SDK can surface that abort with
    // its own error wording — while an unrecognised fetch failure is exactly
    // what routes to the daemon fallback. Left untranslated, Stop therefore
    // started a *second* download with no Stop of its own. Two things must
    // hold: the catch around the browser path decides "stopped" from our own
    // signal, whatever was thrown, and it rethrows a stop before the fallback
    // can run. (The fetch helper this used to sit in, `obtain`, was folded
    // into `runOpen` when the open path learned to stream; the guard moved
    // with it and did not weaken.)
    let js = js_code(&server().get("/assets/app.js").body);

    let open = js
        .split("const runOpen = async")
        .nth(1)
        .expect("runOpen is gone");
    assert!(
        open.contains("controller.signal.aborted"),
        "a stop must be recognised by our own signal, not the SDK's wording: {open}"
    );
    assert!(
        open.contains("throw new DownloadCancelled()"),
        "an abort is not recast into a stop: {open}"
    );

    let recast_at = open
        .find("controller && controller.signal.aborted")
        .expect("the catch does not decide stopped from our signal");
    let fallback_at = open
        .find("fetchViaDaemon")
        .expect("the daemon fallback is gone");
    assert!(
        recast_at < fallback_at,
        "the stop check must come before the daemon fallback, or Stop starts the fallback: {open}"
    );
    let stop_at = open
        .find("err.name === 'DownloadCancelled'")
        .expect("runOpen does not recognise a stop");
    assert!(
        stop_at < fallback_at,
        "the stop check must come before the daemon fallback, or Stop starts the fallback: {open}"
    );
}

#[test]
fn a_media_row_opens_no_progress_tab_and_reports_on_its_own_line() {
    // Audio and video play in this page's player dialog, so for a row that says
    // so the progress tab was pure choreography: it appeared for the loading
    // phases, closed again, and a small window reappeared in the main tab — two
    // surfaces taking turns over one click. The row's own status line already
    // carries every phase (a `toTab` of a null tab is a no-op), so a known-media
    // row opens no tab at all. The decision has to be made *before any await* —
    // a `window.open` after an await has no user gesture and is blocked — which
    // is exactly why it comes from the row's stored mime rather than from the
    // plan, whose first byte is already past a lookup.
    let js = js_code(&server().get("/assets/app.js").body);
    let open = js
        .split("const runOpen = async")
        .nth(1)
        .expect("runOpen is gone");

    assert!(
        open.contains("const tab = likelyMedia ? null : openProgressTab();"),
        "the tab must be gated on the row's stored media type: {open}"
    );
    let gate_at = open
        .find("isMediaMime(result.mime_type)")
        .expect("the media decision is not made from the row");
    let open_tab_at = open
        .find("openProgressTab()")
        .expect("the tab open is gone");
    assert!(
        gate_at < open_tab_at,
        "the decision must precede the open, or it belongs to a moment too late for a popup: {open}"
    );

    // The blocked-popup warning must not fire for a deliberate skip: a tab we
    // chose not to open is not a tab the browser refused.
    assert!(
        open.contains("if (!tab && !likelyMedia)"),
        "the popup warning must be gated on having actually tried: {open}"
    );

    // A stored mime that turns out to need a display is dropped at the hand-off
    // below — the one place where a skipped tab is distinguishable from a
    // blocked one — so "Allow pop-ups … try Open again" can never deadlock: the
    // next click opens the tab normally. A popup the browser blocked teaches the
    // row nothing, so it must not flip.
    let flip_at = open
        .find("trustStoredMime = false")
        .expect("a row whose mime turned out wrong is never corrected");
    let retry_at = open
        .find("try Open again")
        .expect("the null-tab hand-off message is gone");
    assert!(
        flip_at < retry_at,
        "the belief must be dropped where the missing tab is reported: {open}"
    );
}

#[test]
fn a_failed_media_open_retries_once_and_the_fallback_names_it() {
    // The media route is a *second* network reader on the same connection —
    // random access, a different wasm path from the sequential open that already
    // fetched the plan's prefix — and this network fails transiently per attempt.
    // One bad lookup here used to surrender the whole route: a 52 MB video fell
    // through to a full download and reported only the size limit, and an mp3
    // fell through to the viewer and reported a popup block that never happened.
    // Both endings hid the thing the click actually wanted — playing the file.
    let js = js_code(&server().get("/assets/app.js").body);

    let player = js
        .split("async function startPlayer")
        .nth(1)
        .and_then(|rest| rest.split("\nui.mediaSave.addEventListener").next())
        .expect("startPlayer is gone");
    assert!(
        player.contains("invalidateSharedClient()"),
        "a failed media open must be replaceable, or one transient lookup ends playback: {player}"
    );
    assert!(
        player.contains("if (err.name === 'DownloadCancelled' || (signal && signal.aborted))"),
        "a stop must never be spent on a reconnect — nor invalidating the \
         shared client the rest of the page rides: {player}"
    );
    let first_open = player
        .find("source = await client.createMediaSource")
        .expect("the media open is gone");
    let logged = player
        .find("console.error('the media reader could not be opened'")
        .expect("the real failure must be logged — the fallback messages describe the route, never the cause");
    let retry_open = player
        .rfind("source = await client.createMediaSource")
        .expect("the retry does not exist");
    assert!(
        first_open < logged && logged < retry_open,
        "the order is open, log, retry — and the second failure propagates: {player}"
    );
    assert!(
        player.contains("console.error('the media worker did not take the session'"),
        "a post-reader failure (the type ack, the session URL) is logged too, and is not retried: {player}"
    );

    // The endings must carry the failure forward, or the fallback reports only
    // its own reason and hides what broke.
    assert!(
        js.contains("playerFailed && 'the player could not open'"),
        "the download reason must include the player failure: {js}"
    );
    let open = js
        .split("const runOpen = async")
        .nth(1)
        .expect("runOpen is gone");
    assert!(
        open.contains("downloadWhy(plan.renderable, opened.totalBytes, playerFailed)"),
        "the fresh-open download must pass the failure on: {open}"
    );
    assert!(
        open.contains("The player could not open this file, so it was fetched for a tab instead."),
        "the null-tab hand-off must name the player failure, not blame pop-ups: {open}"
    );
    // `startPlayer`'s own logging cannot cover everything that can reach this
    // fall-through — a throw from the shared connect, or from the retry's
    // connect or second attempt, bypasses it — so the catch itself logs.
    // Without this the failure behind every fallback was invisible, and the
    // messages, which describe the route taken, were all anyone had.
    assert!(
        open.contains("console.error('the player could not open', err)"),
        "a player failure must reach the console from the fall-through too: {open}"
    );
}

#[test]
fn the_progress_tab_neither_promises_a_display_nor_explains_where_progress_will_be() {
    // Two things this page must not say:
    //
    // "This tab will show the file" promises a display the tab cannot guarantee for
    // any type the server serves as an attachment, and it cannot know that beforehand.
    //
    // "Progress appears here as it goes" is true and useless — the tab *is* where
    // progress appears, and telling the reader so spends its most prominent line on
    // navigation instead of on the fetch they are waiting for.
    let page = server().get("/view/progress").body;
    assert!(
        !page.contains("This tab will show the file"),
        "the opening line promises a display the tab cannot guarantee: {page}"
    );
    assert!(
        !page.contains("Progress appears here"),
        "an explainer about the page is not progress: {page}"
    );
    // What replaces it states what the user is waiting for, rather than narrating
    // the layout back at them.
    assert!(
        page.contains("Fetching from Autonomi\u{2026}"),
        "the headline no longer says what is being waited for: {page}"
    );
}

#[test]
fn the_progress_headline_is_waited_for_progress_and_never_a_finished_state() {
    // From the user's side they are waiting for a fetch. So the headline states
    // that, always, and the *step* moves underneath it — which makes it structurally
    // impossible for the big line to end up reading like something already
    // completed while the transfer is still running.
    let js = js_code(&server().get("/assets/app.js").body);
    let page = server().get("/view/progress").body;

    // Steps land on the detail line.
    assert!(
        page.contains("sub.textContent = clamp(data.message)"),
        "a step must go beneath the headline: {page}"
    );
    assert!(
        !page.contains("phaseText.textContent = clamp(data.message)"),
        "a step overwriting the headline is how a wait reads as done: {page}"
    );
    // And the headline only changes when there is nothing left to wait for.
    assert!(
        page.contains("phaseText.textContent = 'Ready.'"),
        "a completed fetch must stop reading as in progress: {page}"
    );
    assert!(
        page.matches("phaseText.textContent =").count() == 2,
        "only the terminal states may rewrite the headline: {page}"
    );

    // The labels the page may show beneath it are all phrased as the wait itself.
    // "Downloading done" said the download was finished while it was not, which is
    // the specific confusion this pins.
    let labels = js
        .split("const PHASE_LABELS = {")
        .nth(1)
        .and_then(|rest| rest.split("\n};").next())
        .expect("PHASE_LABELS is gone");
    for forbidden in ["downloaded", "done", "failed "] {
        assert!(
            !labels.contains(forbidden),
            "`{forbidden}` in a label reads as a finished state: {labels}"
        );
    }
}

#[test]
fn a_daemon_fetch_that_cannot_be_displayed_is_saved_rather_than_opened() {
    // The browser path asks `/api/view/plan` and, on "not renderable", saves and
    // closes the tab. The daemon path skipped that entirely and navigated the tab
    // to a viewer URL serving `Content-Disposition: attachment` — so the browser
    // downloaded the file and left the tab claiming it was opening it.
    //
    // Both verdicts come from `av_ingest::mime::is_browser_renderable`, so the two
    // transports cannot disagree about the same file.
    let s = server();
    let body = js_code(&s.get("/assets/app.js").body);
    let daemon = body
        .split("view = await fetchViaDaemon")
        .nth(1)
        .and_then(|rest| rest.split("\n      } catch (err)").next())
        .expect("daemon branch moved");
    assert!(
        daemon.contains("view.renderable === false"),
        "the daemon path must ask whether the file can be displayed: {daemon}"
    );
    assert!(
        daemon.contains("downloadUrl(view.url"),
        "an undisplayable file must be saved, not navigated to: {daemon}"
    );

    // And the server sends the verdict at all.
    let done = format!(
        "{}",
        serde_json::to_string(&av_cli::web::fetchjob::FetchEvent::Done {
            url: "/v/t".into(),
            mime_type: "application/octet-stream".into(),
            filename: "blob.bin".into(),
            bytes: 3,
            expires_in: 120,
            transport: "daemon".into(),
            renderable: false,
        })
        .expect("serialize")
    );
    assert!(
        done.contains(r#""renderable":false"#),
        "the page cannot choose without it: {done}"
    );
}

#[test]
fn a_record_is_streamed_rather_than_downloaded_so_chunks_can_be_counted() {
    // `download()` waits for the whole file and reports once, at the end, with
    // `phase: "complete"` and the final byte total. It therefore says nothing for
    // the entire duration of the transfer — which is the whole point of the
    // progress tab.
    //
    // try.autonomi.com shows per-chunk progress because it does *not* call
    // `download()`: it opens a reader and consumes a stream. A reader also knows
    // `size` from the DataMap before any chunk moves, so "N chunks, X of Y" is
    // counted rather than guessed.
    let js = js_code(&server().get("/assets/app.js").body);
    // The open path was split (`openAutonomi` opens, `drainRest` counts, the
    // worker pipe streams to disk) but every assertion below still holds of
    // the whole region between the open and the row wiring.
    let body = js
        .split("async function openAutonomi")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction wireFetch").next())
        .expect("openAutonomi is gone");

    assert!(
        !body.contains(".download("),
        "download() cannot report progress while it runs: {body}"
    );
    assert!(
        body.contains("client.openFile("),
        "the reader is what knows the size up front: {body}"
    );
    assert!(
        body.contains(".stream(") && body.contains("getReader()"),
        "the file must be consumed chunk by chunk: {body}"
    );
    // Counted as they arrive, and checked against the size the DataMap promised,
    // so a short stream is an error rather than a silently truncated file.
    assert!(
        body.contains("chunks += 1"),
        "chunks are never counted: {body}"
    );
    assert!(
        body.contains("received !== totalBytes"),
        "a short stream must not pass for a complete file: {body}"
    );
}

#[test]
fn byte_progress_is_recognised_by_its_counts_not_by_its_phase() {
    // The SDK's reporter holds one `current` object and does `current = details`
    // on every report that carries details. A byte update passing
    // `{ completed, total }` without repeating `phase` therefore **erases** the
    // phase — so gating on `phase === 'downloading'` drops the numbers from
    // precisely the events that have them, and the transfer shows nothing until
    // it is over.
    let js = js_code(&server().get("/assets/app.js").body);

    let line = js
        .split("function progressLine")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("progressLine is gone");
    assert!(
        !line.contains("event.phase === 'downloading'"),
        "the phase cannot be trusted to identify a byte event: {line}"
    );
    assert!(
        line.contains("event.completed") && line.contains("event.total"),
        "the counts themselves are the reliable signal: {line}"
    );

    // And the routing decision has to use the same test, or the numbers reach the
    // page but land on the wrong channel.
    let report = js
        .split("function reportSdkProgress")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("reportSdkProgress is gone");
    assert!(
        !report.contains("event.phase === 'downloading'"),
        "routing on the phase drops byte counts: {report}"
    );
}

#[test]
fn a_progress_update_is_queued_rather_than_awaited_so_a_lost_handshake_cannot_silence_it() {
    // The failure this pins is total and silent: `toTab` used to `await` a promise
    // resolved by the page's `ready` message, so a `ready` that never arrived left
    // every update pending forever. No error, no log, no event handler firing — the
    // tab just sat on its opening sentence for the whole fetch. That is
    // indistinguishable from a fetch that reports nothing at all, which is exactly
    // the confusion it caused.
    let js = js_code(&server().get("/assets/app.js").body);

    let to_tab = js
        .split("function toTab(")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("toTab is gone");
    assert!(
        !to_tab.contains("await"),
        "an awaited handshake can discard every update silently: {to_tab}"
    );
    assert!(
        to_tab.contains("__avQueue.push(message)"),
        "an update sent before the page is listening must be held, not dropped: {to_tab}"
    );

    // Bounded, because the download reports faster than a page can be expected to
    // load, and an unbounded queue is just a slower leak.
    assert!(
        js.contains("PROGRESS_QUEUE_MAX"),
        "the queue has no ceiling: {js}"
    );

    // And there is a failsafe, so a page that never answers costs the pre-ready
    // events rather than all of them.
    let opener = js
        .split("function openProgressTab(")
        .nth(1)
        .and_then(|rest| rest.split("/* Sever the opener").next())
        .expect("openProgressTab is gone");
    assert!(
        opener.contains("setTimeout(flush, PROGRESS_HANDSHAKE_MS)"),
        "a lost `ready` leaves the queue growing forever: {opener}"
    );
    // Order is the thing a progress log is made of, so a flush drains in order.
    assert!(
        opener.contains("tab.__avQueue.splice(0)"),
        "the flush must drain in arrival order: {opener}"
    );
}

#[test]
fn a_quiet_progress_page_says_it_is_connected_rather_than_looking_frozen() {
    // "Nothing to report yet" and "the opener is not talking to me" look the same
    // on a progress page, and telling them apart is the only thing that makes a
    // broken channel diagnosable instead of mysterious.
    let page = server().get("/view/progress").body;
    assert!(
        page.contains("Waiting for the first update"),
        "an idle page never distinguishes itself from a dead channel: {page}"
    );
    assert!(
        page.contains("if (!heard)"),
        "the idle notice is unconditional, so it would claim to be waiting while it is fed: {page}"
    );
}

#[test]
fn the_vendored_sdk_is_cacheable_because_its_url_carries_the_version() {
    // 5.4 MB of the ~5.6 MB vendored SDK is `ant_core_bg.wasm`, and a browser caches
    // *compiled* WASM per URL. With no `Cache-Control`, the compile is redone on
    // every load: the first stretch of every first fetch spent tens of seconds
    // compiling, and two tabs opening at once competed for the same CPU doing it
    // twice. The version is in the path, so nothing here can change meaning in
    // place and a year-long immutable cache is safe.
    let sdk = server().get("/assets/vendor/autonomi-sdk/0.1.2/wasm/ant_core_bg.wasm");
    assert_eq!(sdk.status, 200);
    assert!(
        sdk.headers.contains("immutable"),
        "the wasm will be recompiled on every load: {}",
        sdk.headers
    );

    // The deliberately opposite half of the split. `app.js` is the code being
    // actively edited, and a stale one is a *silent* wrong-UI bug rather than a
    // slow load — a whole class of confusing reports. So it keeps `no-store`.
    let app = server().get("/assets/app.js");
    assert!(
        app.headers.contains("cache-control: no-store"),
        "a cached app.js is what made this look like a progress bug: {}",
        app.headers
    );
}

#[test]
fn hovering_an_autonomi_location_starts_the_wasm_compile_before_the_click() {
    // Compiling and instantiating 5.4 MB of WASM is the longest single stretch of a
    // first fetch — and it happens *inside* `AutonomiClient.connect`, along with the
    // seed dial, not in `loadSdk()` (which only imports the package). Triggered by
    // the click they read as "broken"; triggered by hover they overlap the user's
    // decision. So the warm must reach the shared connect, or every click still
    // pays the compile. Both promises are cached — `loadSdk` including its failure,
    // `getSharedClient` by clearing its slot — so warming is free when the click
    // never comes and duplicates nothing when it does. Touch fires no pointerenter,
    // which is why focus is wired too.
    let js = js_code(&server().get("/assets/app.js").body);
    let warm = js
        .split("const warm = () => {")
        .nth(1)
        .and_then(|rest| rest.split("\n    loc.addEventListener('click'").next())
        .expect("the warm handler is gone");
    assert!(
        warm.contains("loadSdk()"),
        "warming must reuse the cached promise rather than import again: {warm}"
    );
    assert!(
        warm.contains("getSharedClient()"),
        "the compile and the seed dial live in the connect — the warm must start those too: {warm}"
    );
    assert!(
        js.contains("loc.addEventListener('pointerenter', warm, { once: true })"),
        "pointer intent is not wired: {js}"
    );
    assert!(
        js.contains("loc.addEventListener('focus', warm, { once: true })"),
        "touch devices fire no pointerenter, so focus must be wired too: {js}"
    );
}

#[test]
fn the_sdk_never_names_the_record_at_the_user() {
    // The SDK narrates itself in words built from what the network answered, which
    // includes `opened public-file-3f9a2c71`. That filename is *always* synthetic —
    // the app refuses to trust it for anything, including naming a download — so
    // showing it beside our own counts read as though the app had picked a file off
    // the network. A phase we have a word for is ours; anything else is "Fetching…".
    let js = js_code(&server().get("/assets/app.js").body);
    let line = js
        .split("function progressLine")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("progressLine is gone");
    assert!(
        !line.contains("event.message"),
        "the SDK's own wording is back in the headline: {line}"
    );
    assert!(
        line.contains("PHASE_LABELS[event.phase] || 'Fetching\u{2026}'"),
        "an unknown phase must degrade to our own neutral wording: {line}"
    );
}

#[test]
fn one_counter_shows_progress_because_two_sources_would_flicker() {
    // The SDK reports byte counts, and we independently count chunks read against
    // the size the DataMap promised. Sending both to the same read-out made the line
    // alternate "Downloading — 1.2 MB of 8.0 MB" and "12 chunks · 1.4 MB of 8.0 MB"
    // as each fired, which reads as a broken counter. The SDK's counts now drive the
    // headline only.
    let js = js_code(&server().get("/assets/app.js").body);
    let report = js
        .split("function reportSdkProgress")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("reportSdkProgress is gone");
    assert!(
        !report.contains("type: 'live'"),
        "the SDK's byte counts must not share our counter: {report}"
    );
    assert!(
        report.contains("type: 'phase'"),
        "the SDK drives the headline: {report}"
    );

    // And our own counts go on the channel the ant CLI already uses, so both
    // transports render through one element instead of each having its own.
    // Bounded inside `hooksFor`: the media player's Save has its own onChunk
    // writing to the dialog's status line, which is a different read-out for
    // a different operation on purpose — the row's counter is the one this
    // pin is about.
    let chunk = js
        .split("const hooksFor = (tab) => ({")
        .nth(1)
        .and_then(|rest| {
            rest.split("onChunk: ({ chunks, bytes, totalBytes })")
                .nth(1)
        })
        .and_then(|rest| rest.split("\n        },").next())
        .expect("onChunk is gone");
    assert!(
        chunk.contains("type: 'chunk'"),
        "the browser path needs its own read-out, which is the flicker: {chunk}"
    );
    assert!(
        chunk.contains("expected_bytes: totalBytes"),
        "the size is known up front from the DataMap and should be shown: {chunk}"
    );
}

#[test]
fn a_fetch_shows_a_spinner_that_a_failed_fetch_stops() {
    // The wait for a ~6s seed lookup reads as "broken" when nothing moves, which is
    // what the spinner is for. Decorative, so `aria-hidden` with the headline
    // carrying the meaning.
    let page = server().get("/view/progress").body;
    assert!(
        page.contains("class=\"spin\" id=\"spin\" aria-hidden=\"true\""),
        "the spinner must be decorative: {page}"
    );
    assert!(
        page.contains("@keyframes spin"),
        "the spinner does not animate: {page}"
    );
    // It must stop. `.spin` sets `display`, and the user-agent's `[hidden]` rule
    // lives in a different origin's stylesheet, so any author `display` rule
    // silently outranks it — `!important` is what actually hides it.
    assert!(
        page.contains(".spin[hidden] { display: none !important; }"),
        "a failed fetch would leave a spinner turning forever: {page}"
    );
    assert!(
        page.contains("[hidden] { display: none !important; }"),
        "the blanket guard the other panels rely on is gone: {page}"
    );
    // And it is stopped on both outcomes, not just hidden by the stylesheet alone.
    assert_eq!(
        page.matches("spin.hidden = true").count(),
        2,
        "both done and error must stop the spinner: {page}"
    );
}

#[test]
fn an_sdk_operation_boundary_is_not_reported_as_the_users_download_finishing() {
    // `#openReader` reports `phase: "complete"` the instant the reader object exists:
    //
    //     report(`Opened ${reader.name}`, { phase: "complete" });
    //     return reader;
    //
    // — before a *single* byte has moved. That is an operation boundary, not the
    // file arriving. Rendering it verbatim said "Fetch complete" for the whole of a
    // large transfer, which is exactly backwards for someone waiting for the resource
    // to appear, and it also surfaced for a moment on small ones.
    let js = js_code(&server().get("/assets/app.js").body);

    // No terminal label of our own: whether the bytes are here is ours to decide.
    let labels = js
        .split("const PHASE_LABELS = {")
        .nth(1)
        .and_then(|rest| rest.split("\n};").next())
        .expect("PHASE_LABELS is gone");
    assert!(
        !labels.contains("complete"),
        "a `complete` label renders the SDK's operation boundary: {labels}"
    );
    assert!(
        !labels.contains("failed"),
        "the SDK's failure boundary is not our terminal state either: {labels}"
    );

    // And it cannot sneak through the handler by another route.
    let report = js
        .split("function reportSdkProgress")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("reportSdkProgress is gone");
    assert!(
        report.contains("event.phase === 'complete'"),
        "the operation boundary must be filtered, not merely unlabelled: {report}"
    );
    assert!(
        report.find("event.phase === 'complete'") < report.find("type: 'phase'"),
        "the filter has to run before anything is sent: {report}"
    );

    // What replaces it: *we* say the transfer started, at the moment it does —
    // after the reader exists and before the size is read — and only then is the
    // line true. Anchored as three ordered positions rather than a line window
    // after one call site: `openFile` now has two attempts (the shared client's
    // retry) and the announcement belongs after both of them.
    let open_at = js
        .find("await client.openFile")
        .expect("openFile call is gone");
    let announce_at = js
        .find("if (onStep) onStep(PHASE_LABELS.downloading)")
        .expect("the transfer is never announced by us");
    let size_at = js
        .find("const totalBytes = Number(reader.size)")
        .expect("the size read is gone");
    assert!(
        open_at < announce_at && announce_at < size_at,
        "the announcement must follow the open and precede the size: open={open_at} announce={announce_at} size={size_at}"
    );

    // Terminal states remain the page's own two, so a user still knows when the
    // wait ends — the headline says so and the detail line names what happens next.
    let page = server().get("/view/progress").body;
    assert!(
        page.contains("phaseText.textContent = 'Ready.'")
            && page.contains("sub.textContent = 'Opening the file"),
        "the hand-off must be announced: {page}"
    );
}

// --- installer endpoints ----------------------------------------------------
//
// These use the `ANTA_VISTA_*_INSTALL_CMD` seam to point the install endpoints
// at stub scripts inside a temp HOME, so nothing here reaches the network and
// no test can touch a real x0x or ant install. PATH is pinned to /usr/bin:/bin
// so a developer who happens to have x0x installed cannot short-circuit the
// stub tests with `already_installed`.

/// Write an executable shell stub and return its path.
#[cfg(unix)]
fn write_stub(dir: &std::path::Path, name: &str, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).expect("create stub dir");
    let path = dir.join(name);
    std::fs::write(&path, script).expect("write stub");
    let mut perm = std::fs::metadata(&path)
        .expect("stub metadata")
        .permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&path, perm).expect("chmod stub");
    path
}

/// Environment that isolates an install test: no real installer, no real
/// binary on PATH, everything inside the server's temp HOME.
#[cfg(unix)]
fn install_env(var: &'static str, stub: &std::path::Path) -> Vec<(&'static str, String)> {
    vec![
        ("PATH", "/usr/bin:/bin".to_string()),
        (var, stub.to_string_lossy().into_owned()),
    ]
}

#[cfg(unix)]
#[test]
fn install_x0x_reports_the_log_and_a_hint_when_the_binary_never_appears() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stub = write_stub(
        tmp.path(),
        "install-x0x.sh",
        "#!/bin/sh\necho 'downloading x0x…'\necho 'connection refused' >&2\nexit 1\n",
    );
    let env = install_env("ANTA_VISTA_X0X_INSTALL_CMD", &stub);
    let s = TestServer::spawn_with(
        false,
        &env.iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect::<Vec<_>>(),
    );

    let res = s.post("/api/deps/x0x/install", "{}");
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    // Transport-level ok: the request ran. The outcome is in the other fields.
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["installed"], false, "{body}");
    let log = body["log"].as_str().unwrap_or_default();
    assert!(
        log.contains("downloading x0x"),
        "log must carry stdout: {log}"
    );
    assert!(
        log.contains("connection refused"),
        "log must carry stderr: {log}"
    );
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("without leaving a working"),
        "a failed verification must say so: {hint}"
    );
}

#[cfg(unix)]
#[test]
fn install_x0x_short_circuits_when_the_binary_is_already_in_the_installer_home() {
    // The Porteus case: the binary sits in ~/.local/bin, not on PATH. The
    // endpoint must recognise it and refuse to re-run the script.
    let tmp = tempfile::tempdir().expect("tempdir");
    let ran_marker = tmp.path().join("installer-ran");
    let stub = write_stub(
        tmp.path(),
        "install-x0x.sh",
        &format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran_marker.display()),
    );
    let env = install_env("ANTA_VISTA_X0X_INSTALL_CMD", &stub);
    let s = TestServer::spawn_with(
        false,
        &env.iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect::<Vec<_>>(),
    );
    write_stub(
        &s.home().join(".local").join("bin"),
        "x0x",
        "#!/bin/sh\nexit 0\n",
    );

    let res = s.post("/api/deps/x0x/install", "{}");
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["installed"], true, "{body}");
    assert_eq!(body["already_installed"], true, "{body}");
    assert!(
        !ran_marker.exists(),
        "an already-installed binary must not re-run the installer"
    );
    // needs_install follows the binary, so the setup banner can clear.
    let deps = s.json("/api/deps");
    assert_eq!(deps["needs_install"], false, "{deps}");
}

#[cfg(unix)]
#[test]
fn install_x0x_that_leaves_a_binary_reports_installed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Success, but the daemon is never started: installed=true, running=false,
    // and the hint says the daemon is not answering rather than pretending all
    // is well. (install_x0x_with sleeps briefly after success before the
    // re-check, so this test takes a few seconds by design.)
    let stub = write_stub(
        tmp.path(),
        "install-x0x.sh",
        "#!/bin/sh\n\
         mkdir -p \"$HOME/.local/bin\"\n\
         printf '#!/bin/sh\\nexit 0\\n' > \"$HOME/.local/bin/x0x\"\n\
         chmod +x \"$HOME/.local/bin/x0x\"\n\
         echo 'x0x installed'\n\
         exit 0\n",
    );
    let env = install_env("ANTA_VISTA_X0X_INSTALL_CMD", &stub);
    let s = TestServer::spawn_with(
        false,
        &env.iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect::<Vec<_>>(),
    );

    let res = s.post("/api/deps/x0x/install", "{}");
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["installed"], true, "binary was created: {body}");
    let log = body["log"].as_str().unwrap_or_default();
    assert!(log.contains("x0x installed"), "{log}");
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("daemon is not answering"),
        "installed but not running must say so: {hint}"
    );
    let deps = s.json("/api/deps");
    assert_eq!(deps["needs_install"], false, "{deps}");
    assert_eq!(deps["deps"]["x0x_installed"], true, "{deps}");
}

#[cfg(unix)]
#[test]
fn install_ant_reports_the_log_and_a_hint_when_the_binary_never_appears() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stub = write_stub(
        tmp.path(),
        "install-ant.sh",
        "#!/bin/sh\necho 'downloading ant…'\necho 'HTTP 503' >&2\nexit 1\n",
    );
    let env = install_env("ANTA_VISTA_ANT_INSTALL_CMD", &stub);
    let s = TestServer::spawn_with(
        false,
        &env.iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect::<Vec<_>>(),
    );

    let res = s.post("/api/deps/ant/install", "{}");
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(
        body["ok"], true,
        "transport ok even when the install fails: {body}"
    );
    assert_eq!(body["installed"], false, "{body}");
    let log = body["log"].as_str().unwrap_or_default();
    assert!(log.contains("downloading ant"), "{log}");
    assert!(log.contains("HTTP 503"), "{log}");
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(hint.contains("without leaving a working"), "{hint}");
}

#[cfg(unix)]
#[test]
fn install_ant_that_leaves_a_binary_reports_installed_and_path_advice() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let stub = write_stub(
        tmp.path(),
        "install-ant.sh",
        "#!/bin/sh\n\
         mkdir -p \"$HOME/.local/bin\"\n\
         printf '#!/bin/sh\\nexit 0\\n' > \"$HOME/.local/bin/ant\"\n\
         chmod +x \"$HOME/.local/bin/ant\"\n\
         echo 'ant installed'\n\
         exit 0\n",
    );
    let env = install_env("ANTA_VISTA_ANT_INSTALL_CMD", &stub);
    let s = TestServer::spawn_with(
        false,
        &env.iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect::<Vec<_>>(),
    );

    let res = s.post("/api/deps/ant/install", "{}");
    assert_eq!(res.status, 200, "{}", res.body);
    let body = res.json();
    assert_eq!(body["installed"], true, "binary was created: {body}");
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("PATH"),
        "a successful install must surface the PATH advice: {hint}"
    );
}

#[test]
fn the_install_dialog_stays_open_and_reports_the_outcome_in_place() {
    // The old flow closed the dialog on submit (method="dialog") and wrote the
    // outcome into ui.banner, which the next refreshDeps wiped — a successful
    // install looked like nothing happened. The run must happen while the
    // dialog is open, and the result must land inside it.
    let markup = server().get("/").body;
    assert!(
        markup.contains(r#"id="install-hint""#) && markup.contains(r#"id="install-output""#),
        "the dialog needs a place for the hint and the installer log: {markup}"
    );
    assert!(
        markup.contains(r#"<button type="button" id="install-run""#),
        "Run must be type=button so method=dialog does not close on click: {markup}"
    );

    let js = js_code(&server().get("/assets/app.js").body);
    assert!(
        js.contains("ui.dialogRun.addEventListener('click'"),
        "the install POST is driven by the Run button's click, not by close"
    );
    assert!(
        !js.contains("returnValue !== 'run'"),
        "the old close-then-install flow must be gone"
    );
    assert!(
        js.contains("if (ui.dialog.dataset.ran === '1') refreshDeps();"),
        "closing after a run refreshes deps; the banner and drawer read from them"
    );
    assert!(
        !js.contains("setStatus(\n      ui.banner,"),
        "the outcome must not go back into the banner the poll wipes"
    );
}

#[test]
fn an_install_triggered_by_an_index_offers_to_finish_that_index() {
    // A no_download_backend failure strands the user's link behind the
    // installer: after a successful ant install the dialog must offer to
    // finish the job it interrupted, not strand them at a lone Cancel.
    let markup = server().get("/").body;
    assert!(
        markup.contains(r#"<button type="button" id="install-index""#)
            && markup.contains(">Index link<"),
        "the dialog needs an Index link button: {markup}"
    );
    assert!(
        markup.contains(r#"id="install-close""#),
        "the dismiss button needs an id so its label can change: {markup}"
    );

    let js = js_code(&server().get("/assets/app.js").body);

    // The pending link travels from the failed index into the dialog...
    let run_index = js.find("async function runIndex(").expect("runIndex body");
    assert!(
        js[run_index..].contains("{ uri, tags }"),
        "the no_download_backend path must carry the link it could not index"
    );
    // ...and comes back out through the same index route, not a diverging copy.
    let index_btn = js
        .find("ui.dialogIndex.addEventListener('click'")
        .expect("Index link handler");
    let after_index_btn = js[index_btn..]
        .find("}")
        .map(|i| index_btn + i)
        .unwrap_or(js.len());
    assert!(
        js[index_btn..after_index_btn].contains("runIndex(uri, tags)"),
        "the Index link must retry through runIndex"
    );

    // It appears only on success, and only for an ant install that has a link
    // pending — a banner-triggered x0x install must not offer it.
    let run = js
        .find("ui.dialogRun.addEventListener('click'")
        .expect("install run handler");
    assert!(
        js[run..].contains("if (key === 'ant' && ui.dialog.dataset.pendingUri)"),
        "the Index link is revealed by the ant success path only"
    );

    // Once the consent phase is over, "Cancel" is the wrong word for leaving.
    assert!(
        js[run..].contains("ui.dialogClose.textContent = 'Close'"),
        "after a run, Cancel becomes Close"
    );
    let open = js
        .find("function openInstall(")
        .expect("openInstall definition");
    assert!(
        js[open..run].contains("ui.dialogClose.textContent = 'Cancel'"),
        "opening the dialog again restores Cancel for the consent phase"
    );
}

#[test]
fn the_row_spinner_marks_a_fetch_that_has_not_landed_yet() {
    // The status line beside it can sit on one sentence for the seconds an
    // Autonomi lookup takes, and a line of text alone reads as nothing
    // happening — the progress tab carries its own copy of this spinner for
    // the same reason. Decorative (`aria-hidden`; the status line carries the
    // meaning), shown and hidden exactly once per open from `openView` — not
    // from setBusy, which blinks off during the plan phase while bytes are
    // very much still to come.
    let s = server();

    // Every row template carries it, starting hidden.
    assert!(
        s.get("/")
            .body
            .contains(r#"<span class="row-spin" hidden aria-hidden="true"></span>"#),
        "the row template must carry a spinner that starts hidden"
    );

    // An author `display` rule — which is exactly why the global [hidden]
    // guard has to stay `!important` — plus the animation, and the file's
    // existing global reduced-motion rule taking it back.
    let css = &s.get("/assets/modern.css").body;
    let rule = css_rule(css, ".row-spin").expect("modern.css needs a .row-spin rule");
    assert!(
        rule.contains("display: inline-block") && rule.contains("animation: av-row-spin"),
        ".row-spin must set both display and animation, or it is either invisible \
         while shown or never moves; got: {rule}"
    );
    let guard = css_rule(css, "[hidden]").expect("modern.css needs a [hidden] rule");
    assert!(
        guard.contains("display: none") && guard.contains("!important"),
        "the [hidden] guard is what hides .row-spin against its own display rule; \
         got: {guard}"
    );
    assert!(
        css.contains(
            "@media (prefers-reduced-motion: reduce) {\n  * {\n    animation: none !important;"
        ),
        "the global reduced-motion rule already stops every animation; a second \
         block for the row spinner would be redundant — unless this one is gone"
    );

    // One show before the open, one hide once it settles, both in openView —
    // the inFlight promise is already exactly "an open is running".
    let js = js_code(&s.get("/assets/app.js").body);
    let view = js
        .split("const openView = (statusEl)")
        .nth(1)
        .and_then(|rest| {
            rest.split("const loc = node.querySelector('.result-loc')")
                .next()
        })
        .expect("openView is gone");
    assert_eq!(
        view.matches("rowSpin.hidden = false").count(),
        1,
        "the spinner must be shown from exactly one site"
    );
    assert_eq!(
        view.matches("rowSpin.hidden = true").count(),
        1,
        "and hidden from exactly one site"
    );
    let show = view
        .find("rowSpin.hidden = false")
        .expect("show site missing");
    let settle = view
        .find("inFlight = runOpen(statusEl).finally(() => {")
        .expect("openView no longer settles through runOpen's finally");
    assert!(
        show < settle,
        "the spinner must start with the open it belongs to: {view}"
    );
    assert!(
        view.contains("inFlight = null;\n      rowSpin.hidden = true;"),
        "the hide must be inside the same finally that clears inFlight, or a \
         later open inherits a spinner from the last one: {view}"
    );
}

#[test]
fn the_media_dialog_can_send_playback_to_another_tab() {
    // The tab is opened *synchronously in the click* (a popup after an await
    // has no gesture and browsers block it), the dialog closes only once the
    // tab exists (a blocked popup leaves the player right here with the
    // reason on its status line — and nothing detached yet, so the restore is
    // free), and the opener is severed by hand — `noopener` would return null
    // and lose the handle we just asked for. The session is handed over
    // between the popup check and the close: flagged, detached from the
    // dialog's element, armed with the leave-prompt and the goodbye channel —
    // all before the close that spares it in teardown.
    let s = server();
    let js = js_code(&s.get("/assets/app.js").body);

    let handler = js
        .split("ui.mediaOpenTab.addEventListener('click'")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction wireFetch").next())
        .expect("the open-in-another-tab handler is gone");
    let open = handler
        .find("window.open('/media-view?'")
        .expect("the handler must open the player page");
    let blocked = handler
        .find("if (!tab)")
        .expect("the popup must be checked");
    let handed = handler
        .find("handedSessions.set(mediaSessionId(session.source.url)")
        .expect("the session must be recorded for the tab");
    let detach = handler
        .find("ui.mediaPlayer.pause();")
        .expect("the dialog's element must let go of the stream");
    let sever = handler
        .find("tab.opener = null")
        .expect("the opener must be severed by hand");
    let close = handler
        .find("ui.mediaDialog.close()")
        .expect("the dialog must close after the tab exists");
    assert!(
        open < blocked && blocked < handed && handed < detach && detach < sever && sever < close,
        "open, popup check, handoff, detach, sever, close — in that order; got: {handler}"
    );
    assert!(
        handler[blocked..handed].contains("return;"),
        "a blocked popup must leave this dialog open, nothing detached: {handler}"
    );
    assert!(
        handler.contains("if (!session || session.handedOff || ui.mediaOpenTab.disabled) return;"),
        "the button must do nothing without a session, and never hand a \
         session twice — two consumers on one reader interleave badly: {handler}"
    );
    assert!(
        handler.contains("url: session.source.url"),
        "the tab rides this page's live stream, so the stream URL must travel: {handler}"
    );
    assert!(
        handler.contains("session.handedOff = true;"),
        "the handoff must be marked, or teardown closes the reader under the tab: {handler}"
    );
    assert!(
        handler.contains("armHandoff()"),
        "the new handoff must be armed: {handler}"
    );
    assert!(
        !handler.contains("dropHandoff"),
        "handing a second tab must not retire the first — several tabs play \
         at once, each on its own session: {handler}"
    );

    // Like Save, it lights up only when a session lands.
    let player = js
        .split("async function startPlayer")
        .nth(1)
        .and_then(|rest| rest.split("\nui.mediaSave.addEventListener").next())
        .expect("startPlayer is gone");
    assert!(
        player.contains("ui.mediaOpenTab.disabled = false;"),
        "the tab button must be enabled with the session it opens: {player}"
    );
    // Last click wins the dialog: a superseded loser closes its own reader
    // and reports `SupersededOpen` *before* teardown — after it, the loser
    // would have torn down the winner it raced — and `runOpen` stops on
    // that report instead of falling through to viewer/save. A stop wired
    // through the same signal aborts the open without retrying or
    // invalidating the shared client.
    let generation = player
        .find("const generation = ++playerGeneration;")
        .expect("startPlayer must claim a generation");
    let aborted = player
        .find("if (signal && signal.aborted) {")
        .expect("startPlayer must honour the stop before claiming the dialog");
    let superseded = player
        .find("if (generation !== playerGeneration)")
        .expect("a superseded start must be detected");
    let teardown_claim = player
        .find("teardownPlayer();")
        .expect("startPlayer no longer claims the dialog's session");
    assert!(
        generation < aborted && aborted < superseded && superseded < teardown_claim,
        "claim, stop check, supersede check, teardown — in that order: {player}"
    );
    assert!(
        player.contains("SupersededOpen"),
        "a superseded loser must name itself: {player}"
    );
    assert!(
        player.matches("...(signal ? { signal } : {}),").count() >= 2,
        "both media-source attempts must ride the stop signal: {player}"
    );
    assert!(
        player.contains("err.name === 'DownloadCancelled' || (signal && signal.aborted)"),
        "a stop must not retry or invalidate the shared client: {player}"
    );
    let run_open = js
        .split("const runOpen = async")
        .nth(1)
        .and_then(|rest| rest.split("\nconst openView").next())
        .expect("runOpen is gone");
    assert!(
        run_open.contains("signal: controller.signal,"),
        "the media open must be abortable from the row's Stop: {run_open}"
    );
    assert!(
        run_open.contains("err.name === 'SupersededOpen'")
            && run_open.contains("'Another file is already playing"),
        "runOpen must stop on a superseded media open without racing the \
         winner through viewer or save: {run_open}"
    );
}

#[test]
fn the_media_player_page_rides_the_hosts_transfer_and_says_who_stops_it() {
    // `/media-view` is a handoff consumer: the dialog hands it the live
    // worker session by URL, the worker routes the tab's range requests back
    // to the Anta-Vista tab's MediaBridge, and the page just plays — no SDK,
    // no connect, no second transfer. The flip side it states itself: the
    // Anta-Vista tab must stay open, and when it closes anyway the worker's
    // `av-media-host-gone` names the cause on screen (the media element's
    // own error cannot). The goodbye rides BroadcastChannel, autoplay
    // refusals arm one click anywhere, and the server reflects *nothing*
    // from the query: every value is read by the page's own script, so a
    // hostile query must not change a byte.
    let s = server();
    let hostile = s.get(
        "/media-view?address=0000000000000000000000000000000000000000000000000000000000000000\
         &type=video/mp4&name=%3Cscript%3Ealert(1)%3C%2Fscript%3E&url=%2Fevil",
    );
    assert_eq!(hostile.status, 200, "the player page is missing");
    let page = s.get("/media-view").body;
    assert_eq!(
        hostile.body, page,
        "a query must not change one byte of this page — it reflects nothing"
    );
    assert!(
        !page.contains("/assets/app.js"),
        "the player is standalone: no app.js, no app shell"
    );
    for needle in [
        // The flow itself: block check, the handed stream, the worker
        // keepalive, the goodbye, and the precise death notices — the host
        // gone, the session dropped from the Anta-Vista tab, the worker's
        // own realm restarted underneath the playback.
        "'/api/blocked?location='",
        "'av-ping'",
        "'av-media-host-gone'",
        "'BroadcastChannel'",
        "channel.postMessage({ type: 'closed', sid })",
        "msg.type === 'dropped'",
        "SESSION_DROPPED",
        "data.label === 'av-stream worker started'",
        "WORKER_RESTARTED",
        "preciseNotice",
        "player.src = streamUrl",
        // Validation before use: a bare address, a media type, and the
        // stream URL — same origin, our worker's route, nothing else.
        r"/^[0-9a-f]{64}$/u.test(address)",
        r"/^(audio|video)\//i.test(type)",
        "u.pathname.startsWith('/__autonomi_stream/')",
        // The once-only reload that gets a stray page under worker control.
        "'av-media-view-reloaded'",
        // Autoplay refusals arm one gesture anywhere, not a hunt for the
        // video's own control.
        "'pointerdown'",
        "'touchstart'",
        "'keydown'",
        "'pointerup'",
        // Its own spinner, its own player, and the honest dependency note.
        r#"id="player""#,
        "class=\"spin\"",
        "keep that tab open",
    ] {
        assert!(page.contains(needle), "the player page is missing {needle}");
    }
    // No self-sufficient SDK path remains: the host tab feeds this page now.
    for absent in [
        "AutonomiClient",
        "createMediaSource",
        "__AV_SDK_BASE__",
        "av-media-type",
        "import(",
    ] {
        assert!(
            !page.contains(absent),
            "the handoff page must not contain {absent}"
        );
    }
}

#[test]
fn the_worker_lets_its_own_player_page_load_controlled() {
    // The media route only exists for a *controlled* document — an
    // uncontrolled `/media-view` would fall through to the server's 404 and
    // the SDK bridge would give up waiting for control after ten seconds. So
    // the worker answers that one navigation with the request itself, byte
    // for byte (Host guard and all), and nothing else about the page comes
    // near the worker: its script and assets pass to the network as usual.
    let js = js_code(&server().get("/autonomi-stream-sw.js").body);
    let fetch_start = js
        .find("addEventListener('fetch'")
        .expect("the worker's fetch listener is gone");
    let listener = &js[fetch_start..];
    let passthrough = listener
        .find("url.pathname === '/media-view'")
        .expect("the player page's navigation arm is gone");
    let marker = listener
        .find("STREAM_MARKER")
        .expect("the media route arm is gone");
    assert!(
        passthrough < marker,
        "the player-page arm must come before the media arm it exists to feed"
    );
    assert!(
        listener.contains("event.respondWith(fetch(event.request));"),
        "the arm must answer with the request itself so the document lands \
         controlled"
    );
}

#[test]
fn the_handoff_rides_the_dialogs_session_and_lets_go_when_the_tab_closes() {
    // The tab plays the dialog's live session, not a second transfer: the
    // teardown spares a handed session (the tab is feeding from it), a block
    // or remove still retires it, and the tab's own goodbye — BroadcastChannel
    // on its pagehide — releases the reader and the keepalive. The
    // leave-prompt rides only while a handoff is live, never at rest.
    // Several sessions may be handed at once — each tab keeps its own — so
    // retiring one must leave the others playing.
    let js = js_code(&server().get("/assets/app.js").body);

    // Closing the dialog clears the dialog's own state and leaves the handed
    // session — and its keepalive — alone.
    let teardown = js
        .split("function teardownPlayer")
        .nth(1)
        .and_then(|rest| rest.split("\nui.mediaDialog.addEventListener").next())
        .expect("teardownPlayer is gone");
    let spare = teardown
        .find("if (session && session.handedOff) return;")
        .expect("a handed session must be spared when the dialog closes");
    let remove = teardown
        .find("keepaliveRemove('media')")
        .expect("teardown must still release the keepalive for a plain session");
    assert!(
        spare < remove,
        "the spare must come before the keepalive is released, or a closed \
         dialog kills the tab's stream: {teardown}"
    );
    assert!(
        !teardown[spare..remove].contains("source.close()"),
        "the spare branch must not close the reader the tab is feeding from: {teardown}"
    );
    assert!(
        teardown.contains("if (!handedSessions.size) keepaliveRemove('media');"),
        "the keepalive must survive the dialog while any tab is fed: {teardown}"
    );

    // Retiring one handoff announces the drop to its tab first (so it stops
    // with a named reason), retires the session in the worker — failing any
    // in-flight range at once — closes that reader, and releases the
    // prompt/keepalive only when no other handoff is live.
    let drop = js
        .split("function dropHandoff")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction armHandoff").next())
        .expect("dropHandoff is gone");
    for needle in [
        "handedSessions.get(sid)",
        "handedSessions.delete(sid)",
        "type: 'dropped'",
        "'av-media-drop'",
        "handed.source.close()",
        "if (!handedSessions.size && beforeUnloadWhileHandoff)",
        "removeEventListener('beforeunload'",
        "if (!mediaSession && !handedSessions.size) keepaliveRemove('media');",
    ] {
        assert!(
            drop.contains(needle),
            "dropHandoff is missing {needle}: {drop}"
        );
    }
    let announced = drop
        .find("type: 'dropped'")
        .expect("the drop must be announced to the tab");
    let retired = drop
        .find("'av-media-drop'")
        .expect("the drop must reach the worker");
    let closed = drop
        .find("handed.source.close()")
        .expect("the reader must still close");
    assert!(
        announced < retired && retired < closed,
        "announce, retire in the worker, then close the reader — the other \
         way leaves in-flight ranges waiting on a closed stream: {drop}"
    );

    // The goodbye channel hears each live tab's sid and retires only that
    // one, and the leave-prompt is armed here — nowhere else — so it exists
    // exactly while a handoff does.
    let arm = js
        .split("function armHandoff")
        .nth(1)
        .and_then(|rest| rest.split("\nfunction teardownPlayer").next())
        .expect("armHandoff is gone");
    for needle in [
        "new BroadcastChannel(HANDOFF_CHANNEL)",
        "handedSessions.has(msg.sid)",
        "dropHandoff(msg.sid)",
        "addEventListener('beforeunload'",
        "event.preventDefault()",
    ] {
        assert!(
            arm.contains(needle),
            "armHandoff is missing {needle}: {arm}"
        );
    }

    // A block or remove is the user saying this file stops now, wherever it
    // is playing — every handed tab playing it, included.
    let close_if = js
        .split("function closePlayerIf")
        .nth(1)
        .and_then(|rest| rest.split("\nlet playerGeneration").next())
        .expect("closePlayerIf is gone");
    assert!(
        close_if.contains("Array.from(handedSessions)")
            && close_if.contains("handed.location === location) dropHandoff(sid)"),
        "a block or remove must retire every tab playing that location: {close_if}"
    );
}

#[test]
fn the_worker_serves_the_player_tab_from_the_host_and_says_when_it_is_gone() {
    // A handed-off session's bytes come from the page that owns the reader —
    // the host — not from the tab asking: the tab has no bridge of its own.
    // A host that is gone fails at once, with no 45 s range timeout, and the
    // reason is posted to the player tab, whose own media error cannot name
    // it. A session the page retired (`av-media-drop`) 404s at the next
    // request instead of streaming from a closed reader.
    let js = js_code(&server().get("/autonomi-stream-sw.js").body);

    // The host is recorded from the teach, whose sender owns the session.
    let teach = js
        .split("msg.type === 'av-media-type'")
        .nth(1)
        .and_then(|rest| rest.split("msg.type === 'av-media-drop'").next())
        .expect("the teach handler is gone");
    assert!(
        teach.contains("mediaHosts.set(sessionId, event.source.id)"),
        "the teach's sender is the host page: {teach}"
    );

    // The drop arm fails any in-flight range at once (a request already
    // waiting on the host must not sit out the range timeout against a
    // reader that is about to close), then forgets the session's type and
    // host and retires it.
    let drop = js
        .split("msg.type === 'av-media-drop'")
        .nth(1)
        .and_then(|rest| rest.split("addEventListener('fetch'").next())
        .expect("the drop arm is gone");
    for needle in [
        "failPendingRanges(sessionId,",
        "mediaTypes.delete(sessionId)",
        "mediaHosts.delete(sessionId)",
        "retiredSessions.set(sessionId, true)",
    ] {
        assert!(
            drop.contains(needle),
            "the drop arm is missing {needle}: {drop}"
        );
    }
    let failed = drop
        .find("failPendingRanges(sessionId,")
        .expect("in-flight ranges must be failed");
    let forgotten = drop
        .find("mediaTypes.delete(sessionId)")
        .expect("the session's type must be forgotten");
    assert!(
        failed < forgotten,
        "in-flight ranges fail before the session is forgotten: {drop}"
    );

    // A retired session is refused at the route, before any body is built.
    let stream = js
        .split("async function handleStreamRequest")
        .nth(1)
        .expect("the media route is gone");
    let retired = stream
        .find("retiredSessions.has(sessionId)")
        .expect("a retired session must be refused at the route");
    let serve = stream
        .find("return new Response(body,")
        .expect("the media route no longer streams");
    assert!(
        retired < serve,
        "the refusal must come before any body is built"
    );

    // Range requests prefer the host, fall back to the requester only when
    // no host is recorded, and say so to the tab that asked when it is gone.
    let ranges = js
        .split("async function requestRangeFromPage")
        .nth(1)
        .expect("range requests no longer come from a page");
    let host = ranges
        .find("mediaHosts.get(sessionId)")
        .expect("the host must be looked up");
    let fallback = ranges
        .find("client = clientId ? await self.clients.get(clientId)")
        .expect("the non-host fallback is gone");
    assert!(
        host < fallback,
        "the host path must win over the requester fallback: {ranges}"
    );
    for needle in [
        "'av-media-host-gone'",
        "The Anta-Vista tab that was feeding this media has closed",
    ] {
        assert!(
            ranges.contains(needle),
            "the host-gone path is missing {needle}: {ranges}"
        );
    }
    // Every in-flight request registers its rejection so the drop arm can
    // settle it at once; success, failure, timeout and the drop all funnel
    // through one guarded settle, so a late answer after a fail cannot
    // settle twice.
    for needle in [
        "pendingRanges.set(sessionId, new Set())",
        "pendingRanges.get(sessionId).add(settleReject)",
        "const settle = (fn, value) => {",
        "if (settled) return;",
    ] {
        assert!(
            ranges.contains(needle),
            "range requests must be retire-able, not just timed: {ranges}"
        );
    }
    let registered = ranges
        .find("pendingRanges.get(sessionId).add(settleReject)")
        .expect("the request must register for retirement");
    let posted = ranges
        .find("client.postMessage(")
        .expect("the request must still be posted");
    assert!(
        registered < posted,
        "registration must precede the post — a drop landing in between \
         would otherwise find nothing to fail: {ranges}"
    );
}

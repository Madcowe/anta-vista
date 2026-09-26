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

        let body = body.unwrap_or("");
        let request = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {host}:{}\r\n\
             Connection: close\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {body}",
            self.port,
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("write request");
        stream.flush().ok();

        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");

        let (head, body) = response
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("malformed HTTP response: {response:?}"));
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("status line");

        HttpResponse {
            status,
            headers: head.to_lowercase(),
            body: body.to_string(),
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

struct HttpResponse {
    status: u16,
    headers: String,
    body: String,
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
fn download_is_an_explicit_not_implemented() {
    let res = server().get("/api/download?location=autonomi%3A%2F%2Fabc");
    assert_eq!(res.status, 501, "download must not pretend to work");
    assert_eq!(res.json()["error"], "not_implemented");
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

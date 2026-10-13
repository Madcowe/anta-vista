//! Web interface for Anta-Vista.
//!
//! `av web` (and bare `av`) serve a browser UI backed by the same engine the
//! CLI uses: the local embedding index, the x0x gossip network, and the Autonomi
//! download path. Everything here is synchronous and thread-per-request, in
//! keeping with the rest of the workspace — there is no async runtime.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::cmd::{CliError, CliResult};
use crate::startup::StartupState;
use tiny_http::Server;

pub mod fetchjob;
pub mod handlers;
pub mod render;
pub mod server;
pub mod state;
pub mod viewer;

pub use state::AppState;

/// Default listen port. Port `0` is also accepted and asks the OS for a free one.
pub const DEFAULT_PORT: u16 = 8787;

/// Runtime options for the web server.
#[derive(Debug, Clone)]
pub struct WebOptions {
    /// Interface to bind. Anything other than loopback is opt-in and warned about.
    pub host: String,
    /// Port to bind; `0` picks an unused port.
    pub port: u16,
    /// Whether to launch the system browser once bound.
    pub open_browser: bool,
    /// Stop after this many seconds. `None` runs until interrupted.
    pub run_for: Option<u64>,
}

/// Serve the web interface until interrupted.
///
/// `announced` is true when the user reached this by running bare `av`, in
/// which case a short notice goes to stderr first. That matters because bare
/// `av` is now a long-running server: an agent or script that invoked it
/// without a subcommand would otherwise sit waiting for a prompt that never
/// arrives.
pub fn run(
    cli: &crate::Cli,
    startup: StartupState,
    options: WebOptions,
    announced: bool,
) -> CliResult<()> {
    if announced {
        eprintln!(
            "anta-vista: no subcommand given, starting the web interface.\n\
             \x20            Use 'av search', 'av index', ... for the command line, or\n\
             \x20            'av --help' for the full list. Press Ctrl-C to stop the server."
        );
    }

    let host: IpAddr = options
        .host
        .parse()
        .map_err(|e| CliError::Other(format!("invalid --host {:?}: {e}", options.host)))?;

    if !host.is_loopback() {
        eprintln!(
            "anta-vista: warning: listening on {host}, which is reachable from other machines.\n\
             \x20            The web interface has no authentication; use --host 127.0.0.1\n\
             \x20            to keep it local."
        );
    }

    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("failed to determine database path".to_string()))?;

    // Create the database and apply migrations once, up front and single
    // threaded. Requests each open their own connection, and a burst of them
    // arriving at a brand-new database would otherwise race on the migration
    // bookkeeping. Doing it here means the workers only ever read.
    av_store::open(&db_path)
        .map_err(|e| CliError::Database(format!("failed to open {}: {e}", db_path.display())))?;

    // Give a fresh install its starter resources: pre-baked rows whose
    // embeddings were computed at build time, so this is a few local INSERTs —
    // no network, no model, no wait before the bind. A failure is logged and
    // skipped: a nicety must not keep the interface from starting.
    if !crate::starters::seeding_disabled() {
        match crate::starters::ensure_seeded(&db_path) {
            Ok(report) if report.seeded > 0 || report.adopted > 0 => {
                tracing::info!("starter resources ready: {report:?}");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("starter seeding failed: {e}"),
        }
    }

    // Bind before loading anything expensive, so a port conflict is reported
    // immediately rather than after a model download.
    //
    // A conflict with *another copy of this program* is not an error: the
    // browser's close button does not stop the server, so the natural second
    // start — another double-click of the binary, or `av web` while one runs —
    // would otherwise die here, with the bind error printed where an icon
    // launch has nowhere to print it. Reuse the instance that is already up:
    // say where it is, open it the way a fresh launch would, and exit 0.
    let addr = SocketAddr::new(host, options.port);
    let port = options.port;
    let server = match Server::http(addr) {
        Ok(server) => server,
        // tiny_http hands back a boxed error, so the `AddrInUse` kind has to
        // be recovered by downcast before it can be matched.
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
                && already_running_at(host, port) =>
        {
            let url = format!("http://{host}:{port}/");
            println!("anta-vista web interface is already running at {url}");
            if options.open_browser {
                open_browser(&url);
            }
            return Ok(());
        }
        Err(e) => {
            return Err(CliError::Other(format!("failed to bind {addr}: {e}")));
        }
    };
    let bound_port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(options.port);
    let url = format!("http://{host}:{bound_port}/");

    let state = Arc::new(AppState {
        cli: cli.clone(),
        startup: RwLock::new(startup),
        db_path,
        provider: state::EmbeddingCache::new(),
        bound_port,
        bound_host: host.to_string(),
        started_at: state::now_secs(),
        services_lock: Mutex::new(()),
    });

    println!("anta-vista web interface: {url}");
    if !state.provider.is_loaded() {
        println!("  (embedding model loads on first search)");
    }
    match options.run_for {
        Some(secs) => println!("  stopping automatically in {secs}s"),
        None => println!("  press Ctrl-C to stop"),
    }

    if options.open_browser {
        open_browser(&url);
    }

    // Bring the network services up in the background: binding must not wait on
    // an ~8s x0x start, and opening the UI should be enough to become a peer.
    // This never installs anything; a missing x0x just leaves the banner up.
    let boot_state = Arc::clone(&state);
    std::thread::spawn(move || {
        let report = boot_state.ensure_services();
        tracing::debug!("boot service ensure: {}", report.detail);
    });

    // `server` is moved into the worker pool; from here on the process blocks.
    server::serve(state, server, options.run_for)
        .map_err(|e| CliError::Other(format!("web server failed: {e}")))
}

/// Probe a busy port for another copy of this program.
///
/// The caller only reaches this after a bind failed with `AddrInUse`, so the
/// question is whether the squatter is an anta-vista web server we can reuse
/// or something else entirely — in which case the caller reports the bind
/// error as before. A hung anta-vista answers nothing and also falls through
/// to the error, which is the honest answer: there is a process on the port
/// but it is not usable.
fn already_running_at(host: IpAddr, port: u16) -> bool {
    let url = format!("http://{host}:{port}/api/ping");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(800))
        .build();
    match agent.get(&url).call() {
        Ok(resp) => resp
            .into_json::<serde_json::Value>()
            .map(|v| v["ok"] == true && v["service"] == "anta-vista")
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Best-effort launch of the system browser.
///
/// Failure is not an error: the URL is always printed, so a headless box or a
/// machine with no opener still works.
fn open_browser(url: &str) {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "windows") {
        // `start` is a cmd builtin, and the empty string is the window title,
        // which `start` would otherwise consume as the target.
        ("cmd", vec!["/C", "start", "", url])
    } else if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else {
        ("xdg-open", vec![url])
    };

    match std::process::Command::new(program)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            // Reap the child so it cannot become a zombie, but do it on a
            // detached thread: `xdg-open` in particular can block for as long as
            // the browser it launches, and startup must not wait on that.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(_) => {
            eprintln!("anta-vista: could not open a browser automatically; visit {url}");
        }
    }
}

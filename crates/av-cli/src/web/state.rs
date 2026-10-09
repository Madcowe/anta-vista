use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use av_embed::MiniLmProvider;
use av_net_x0x::X0xConfig;
use rusqlite::Connection;

use crate::Cli;
use crate::startup::{ServiceReport, StartupState, ping_antd, ping_x0x_daemon};

/// Lazily-loaded embedding model, shared by every worker thread.
///
/// Loading MiniLM means reading a ~22 MB model off disk (and downloading it on
/// a cold cache), which is far too slow to do before binding the socket: the
/// setup UI has to come up even when no model is available yet. So the load is
/// deferred to the first request that needs it, and the outcome — success *or*
/// failure — is cached so a broken cache is not retried on every keystroke.
pub struct EmbeddingCache(OnceLock<Result<Arc<MiniLmProvider>, String>>);

impl EmbeddingCache {
    pub fn new() -> Self {
        Self(OnceLock::new())
    }

    /// Get the shared provider, loading it on first use.
    pub fn get(&self) -> Result<&Arc<MiniLmProvider>, String> {
        match self.0.get_or_init(|| {
            MiniLmProvider::new()
                .map(Arc::new)
                .map_err(|e| e.to_string())
        }) {
            Ok(p) => Ok(p),
            Err(e) => Err(e.clone()),
        }
    }

    /// Whether the model is already loaded, without triggering a load.
    pub fn is_loaded(&self) -> bool {
        matches!(self.0.get(), Some(Ok(_)))
    }
}

impl Default for EmbeddingCache {
    fn default() -> Self {
        Self::new()
    }
}

/// State shared by all HTTP worker threads.
pub struct AppState {
    /// The parsed command line. Search reuses this so that `--config`,
    /// `--timeout` and the non-interactive flag behave exactly as they do for
    /// the equivalent CLI invocation.
    pub cli: Cli,
    /// Probed at boot, but re-probed whenever the UI starts or installs a
    /// dependency. Without that, starting x0x from the setup banner would leave
    /// searches stuck on the "x0x was not running at boot" path until restart.
    pub startup: RwLock<StartupState>,
    pub db_path: PathBuf,
    pub provider: EmbeddingCache,
    /// The port actually bound, which differs from the requested port when
    /// `port = 0` asked the OS to pick a free one.
    pub bound_port: u16,
    pub bound_host: String,
    pub started_at: u64,
    /// Serialises [`AppState::ensure_services`] so a boot-time start, a search
    /// and a drawer click cannot all spawn `x0x start` at once. The loser waits,
    /// then finds the services already up and returns immediately.
    pub services_lock: Mutex<()>,
}

impl AppState {
    /// Open a fresh SQLite connection.
    ///
    /// `Connection` is not `Sync`, so it cannot be shared across the worker
    /// pool; opening per request is cheap and correct because the database runs
    /// in WAL mode, which supports a writer (the listener) alongside readers.
    pub fn conn(&self) -> Result<Connection, String> {
        av_store::open(&self.db_path).map_err(|e| e.to_string())
    }

    /// Re-probe the daemons the UI is able to start, so a dependency started
    /// from the setup banner is picked up by the very next request.
    ///
    /// Only the network daemons are re-checked. `minilm_loaded` and
    /// `listener_running` describe work this process owns or already knows
    /// about, and the embedding cache is the authority on the former.
    pub fn refresh_network_deps(&self) {
        let x0x_config = match X0xConfig::from_data_dir() {
            Ok(cfg) if ping_x0x_daemon(&cfg) => Some(cfg),
            _ => None,
        };
        let antd_running = ping_antd();

        if let Ok(mut startup) = self.startup.write() {
            startup.x0x_config = x0x_config;
            startup.antd_running = antd_running;
        }
    }

    /// Bring x0x and the background listener up on demand, then refresh the
    /// network snapshot so the very next search fans out.
    ///
    /// Serialised by `services_lock`: the first caller does the work, everyone
    /// else waits and then finds the services already running.
    pub fn ensure_services(&self) -> ServiceReport {
        let _guard = self.services_lock.lock();
        let report = crate::startup::ensure_services();
        self.refresh_network_deps();
        report
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

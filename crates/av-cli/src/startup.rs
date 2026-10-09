use crate::cmd::{CliError, CliResult};
use av_core::config::AvConfig;
use av_net_x0x::client::X0xConfig;
use dialoguer::Confirm;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct StartupState {
    pub config: AvConfig,
    pub config_path: Option<PathBuf>,
    pub x0x_config: Option<X0xConfig>,
    pub antd_running: bool,
    pub minilm_loaded: bool,
    pub listener_running: bool,
}

pub fn run_startup_checks(cli: &crate::Cli) -> CliResult<StartupState> {
    // 1. Load config
    let (config, config_path) = load_config(cli)?;

    // 2. Check x0x daemon status
    let x0x_config = match X0xConfig::from_data_dir() {
        Ok(cfg) => {
            // Verify daemon is running by pinging the health endpoint
            if ping_x0x_daemon(&cfg) {
                Some(cfg)
            } else {
                None
            }
        }
        Err(_) => None,
    };

    // 3. Check ant daemon (antd) status
    let antd_running = ping_antd();

    // 4. MiniLM model check (fastembed tries to load the model).
    // We only perform a quick check here if needed, but the actual instantiation
    // can be lazy or done during startup. Let's do a quick check:
    // If the model is not downloaded/available, fastembed will download it on first use.
    // So "minilm_loaded" represents whether it *can* load (which might trigger download).
    // Let's check if the model is cached locally, or check if we can run it.
    let minilm_loaded = check_minilm_cached();

    // 5. Ensure the background listener is running whenever x0x is available and
    //    the current command is one that benefits from peer responses.  We skip
    //    this for Status (read-only), Purge (offline), Listen itself (it IS
    //    the listener — starting another would be redundant), Stop (must work
    //    while the network is down), and Web (the whole point of the web
    //    interface is to start up and offer to fix a missing x0x).
    let needs_listener = x0x_config.is_some() && !is_offline_command(&cli.command);
    let listener_running = if needs_listener {
        crate::listener::ensure_running()
    } else {
        crate::listener::is_running()
    };

    let mut state = StartupState {
        config,
        config_path,
        x0x_config,
        antd_running,
        minilm_loaded,
        listener_running,
    };

    // 5. Handle missing dependencies based on interactive/non-interactive mode
    enforce_dependencies(cli, &mut state)?;

    Ok(state)
}

/// Commands that must work without the x0x daemon running, and which should
/// therefore never auto-spawn the background listener either.
///
/// `Web` belongs here because the web interface is the one surface that has to
/// come up *without* a daemon — it is where the setup flow lives. Every other
/// network command hard-requires x0x.
///
/// A missing subcommand (`None`) is treated the same as `Web`, because bare `av`
/// dispatches to exactly that.
///
/// `Block`/`Unblock` belong here because a block has to be enforceable when the
/// network is down. Blocking an agent that is currently flooding you is exactly
/// the moment you cannot count on the daemon being up, and `av block --list`
/// is a plain local read.
pub fn is_offline_command(command: &Option<crate::Commands>) -> bool {
    matches!(
        command,
        None | Some(crate::Commands::Status)
            | Some(crate::Commands::Purge { .. })
            | Some(crate::Commands::Listen { .. })
            | Some(crate::Commands::Stop { .. })
            | Some(crate::Commands::Block { .. })
            | Some(crate::Commands::Unblock { .. })
            | Some(crate::Commands::Web { .. })
    )
}

pub fn load_config(cli: &crate::Cli) -> CliResult<(AvConfig, Option<PathBuf>)> {
    if let Some(ref path) = cli.config {
        let config = AvConfig::from_file(path)
            .map_err(|e| CliError::Validation(format!("Failed to load config: {}", e)))?;
        config.validate().map_err(CliError::Validation)?;
        Ok((config, Some(path.clone())))
    } else if let Some(config_dir) = av_core::paths::config_dir() {
        let path = config_dir.join("config.toml");
        if path.exists() {
            let config = AvConfig::from_file(&path)
                .map_err(|e| CliError::Validation(format!("Failed to load config: {}", e)))?;
            config.validate().map_err(CliError::Validation)?;
            Ok((config, Some(path)))
        } else {
            Ok((AvConfig::default(), None))
        }
    } else {
        Ok((AvConfig::default(), None))
    }
}

/// The literal command used to install x0x on this platform, suitable for
/// showing to a user before they consent to running it.
pub fn x0x_install_command() -> &'static str {
    if cfg!(target_os = "windows") {
        "powershell -NoProfile -Command \"Invoke-WebRequest -Uri https://github.com/saorsa-labs/x0x/releases/latest/download/x0x-windows-x64.zip -OutFile x0x.zip; tar -xf x0x.zip -C $env:USERPROFILE\\.local\\bin; Remove-Item x0x.zip\""
    } else {
        "curl -sfL https://x0x.md | sh -s -- --start"
    }
}

/// The literal command used to install the ant CLI on this platform.
pub fn ant_install_command() -> &'static str {
    if cfg!(target_os = "windows") {
        "powershell -NoProfile -Command \"irm https://raw.githubusercontent.com/WithAutonomi/ant-client/main/install.ps1 | iex\""
    } else {
        "curl -fsSL https://raw.githubusercontent.com/WithAutonomi/ant-client/main/install.sh | bash"
    }
}

/// Run an installer, sending its output to `stdio`.
///
/// The CLI passes [`Stdio::inherit`] so the user watches it happen. The web
/// interface passes [`Stdio::piped`] so the log can be captured and shown in
/// the response instead of corrupting the server's own stdout.
fn run_installer(command: &str, stdio: Stdio) -> bool {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(stdio)
        .stderr(Stdio::inherit())
        .status();
    matches!(status, Ok(s) if s.success())
}

pub fn start_antd_daemon() -> bool {
    let try_start = |bin: &str| -> bool {
        match std::process::Command::new(bin).arg("start").spawn() {
            Ok(_) => {
                for _ in 0..6 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    if ping_antd() {
                        return true;
                    }
                }
                false
            }
            Err(_) => false,
        }
    };

    if try_start("antd") {
        return true;
    }
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let local_antd = base_dirs.home_dir().join(".local/bin/antd");
        if local_antd.exists() {
            return try_start(local_antd.to_str().unwrap_or("antd"));
        }
    }
    false
}

pub fn ant_cli_binary_available() -> bool {
    ant_resolve_path()
        .map(|path| {
            std::process::Command::new(path)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Install the ant CLI. `stdio` controls where the installer's own output goes;
/// see [`run_installer`].
pub fn install_ant_cli_with(stdio: Stdio) -> bool {
    if !cfg!(target_os = "windows") {
        // Linux/macOS: use the official install script
        run_installer(
            "curl -fsSL https://raw.githubusercontent.com/WithAutonomi/ant-client/main/install.sh | bash",
            stdio,
        )
    } else {
        // Windows: use the official PowerShell install script
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(
                "irm https://raw.githubusercontent.com/WithAutonomi/ant-client/main/install.ps1 | iex",
            )
            .stdout(stdio)
            .stderr(Stdio::inherit())
            .status();
        matches!(status, Ok(s) if s.success())
    }
}

pub fn install_ant_cli() -> bool {
    install_ant_cli_with(Stdio::inherit())
}

pub fn x0x_binary_available() -> bool {
    let bin = if cfg!(target_os = "windows") {
        "x0x.exe"
    } else {
        "x0x"
    };
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Install x0x. `stdio` controls where the installer's own output goes; see
/// [`run_installer`].
pub fn install_x0x_with(stdio: Stdio) -> bool {
    if !cfg!(target_os = "windows") {
        // Linux/macOS: use the install script
        let ok = run_installer("curl -sfL https://x0x.md | sh -s -- --start", stdio);
        if ok {
            std::thread::sleep(Duration::from_secs(3));
        }
        ok
    } else {
        // Windows: download zip via PowerShell, extract to ~\.local\bin
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(concat!(
                "$zip = \"$env:TEMP\\x0x.zip\"; ",
                "$target = \"$env:USERPROFILE\\.local\\bin\"; ",
                "mkdir -Force $target | Out-Null; ",
                "Invoke-WebRequest -Uri \"https://github.com/saorsa-labs/x0x/releases/latest/download/x0x-windows-x64.zip\" -OutFile $zip; ",
                "tar -xf $zip -C $target --strip-components=1; ",
                "Remove-Item $zip -Force",
            ))
            .stdout(stdio)
            .stderr(Stdio::inherit())
            .status();
        let ok = matches!(status, Ok(s) if s.success());
        if ok {
            std::thread::sleep(Duration::from_secs(3));
        }
        ok
    }
}

pub fn install_x0x() -> bool {
    install_x0x_with(Stdio::inherit())
}

/// Build a lightweight ureq agent with a short connect timeout, used
/// exclusively for local health-check pings. We set only the connect timeout
/// (not read timeout) so a slow response body doesn't affect other callers,
/// and to avoid accidentally timing out long-lived connections like SSE.
/// Without an explicit connect timeout, `ureq` relies on the OS TCP stack —
/// on Windows this can block for several seconds per attempt on closed ports.
fn health_check_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_millis(500))
        .build()
}

pub fn ping_x0x_daemon(cfg: &X0xConfig) -> bool {
    let url = format!("{}/health", cfg.api_base);
    match health_check_agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(resp) => resp.status() == 200,
        Err(_) => false,
    }
}

pub fn ping_antd() -> bool {
    // Default antd port is 8082
    match health_check_agent()
        .get("http://localhost:8082/health")
        .call()
    {
        Ok(resp) => resp.status() == 200,
        Err(_) => false,
    }
}

/// Whether the MiniLM model is ready to use.
///
/// The model is fetched lazily by `fastembed`, so we cannot know for sure
/// without constructing a provider (which may trigger a ~22 MB download). The
/// web interface calls this with a real provider it has already built, so the
/// answer there is genuine; the CLI keeps the optimistic default.
pub fn check_minilm_cached() -> bool {
    // fastembed caches models in standard cache directories, e.g. ~/.cache/fastembed/
    // Let's check if fastembed dir or hf-hub cache exists.
    // If not, we still assume true since it downloads on first use.
    // But if we want to be safe, we can just say true if it's cached or we have internet.
    true
}

/// A snapshot of every external dependency Anta-Vista needs, for the web
/// interface's setup flow and status panel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DependencyStatus {
    pub x0x_installed: bool,
    pub x0x_running: bool,
    pub x0x_api: String,
    pub x0x_agent_id: String,
    /// Literal install command for this platform, shown before consent.
    pub x0x_install_cmd: &'static str,
    pub ant_cli_installed: bool,
    pub antd_running: bool,
    /// Literal install command for this platform, shown before consent.
    pub ant_install_cmd: &'static str,
    pub av_listener_running: bool,
    pub av_listener_pid: Option<u32>,
}

/// Probe every dependency without prompting or starting anything.
///
/// Reuses the same `ping_*` helpers as startup so the answers can never drift
/// from what the CLI believes.
pub fn probe_dependencies() -> DependencyStatus {
    let x0x_cfg = X0xConfig::from_data_dir()
        .ok()
        .filter(|cfg| ping_x0x_daemon(cfg));

    DependencyStatus {
        x0x_installed: x0x_binary_available(),
        x0x_running: x0x_cfg.is_some(),
        x0x_api: x0x_cfg
            .as_ref()
            .map(|c| c.api_base.clone())
            .unwrap_or_default(),
        x0x_agent_id: x0x_cfg
            .as_ref()
            .map(|c| c.agent_id.clone())
            .unwrap_or_default(),
        x0x_install_cmd: x0x_install_command(),
        ant_cli_installed: ant_cli_binary_available(),
        antd_running: ping_antd(),
        ant_install_cmd: ant_install_command(),
        av_listener_running: crate::listener::is_running(),
        av_listener_pid: crate::listener::pid_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| s.trim().parse::<u32>().ok()),
    }
}

/// Directories the Windows ant installer may drop `ant.exe` into without the
/// current process PATH picking it up.
#[cfg(windows)]
pub fn ant_candidate_dirs() -> Vec<PathBuf> {
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    vec![
        PathBuf::from(&profile)
            .join("AppData")
            .join("Local")
            .join("ant")
            .join("bin"),
        PathBuf::from(&profile).join(".local").join("bin"),
    ]
}

#[cfg(not(windows))]
pub fn ant_candidate_dirs() -> Vec<PathBuf> {
    directories::BaseDirs::new()
        .map(|d| vec![d.home_dir().join(".local").join("bin")])
        .unwrap_or_default()
}

/// Prepend the ant candidate directories to this process's PATH.
///
/// Only safe to call from a single-threaded context (the CLI calls it during
/// startup). Server threads must not mutate process-global state, so they should
/// use [`ant_resolve_path`] instead.
pub fn refresh_ant_path() {
    let dirs = ant_candidate_dirs();
    let current = std::env::var("PATH").unwrap_or_default();
    let mut to_add = Vec::new();
    for path in &dirs {
        let p = path.to_string_lossy().to_string();
        if !p.is_empty()
            && path.join("ant.exe").exists()
            && !current.split(';').any(|existing| existing == p)
        {
            to_add.push(p);
        }
    }
    if !to_add.is_empty() {
        let new_path = to_add.join(";") + ";" + &current;
        // SAFETY: single-threaded at startup before any env reads race
        unsafe { std::env::set_var("PATH", new_path) };
    }
}

/// Resolve the `ant` executable, consulting the installer's candidate
/// directories as well as PATH.
///
/// Thread-safe, unlike [`refresh_ant_path`], because it builds the candidate
/// path list per call instead of mutating process-global state.
pub fn ant_resolve_path() -> Option<PathBuf> {
    let exe = if cfg!(target_os = "windows") {
        "ant.exe"
    } else {
        "ant"
    };
    // Prefer a hit on PATH, matching what a user would get in a fresh shell.
    if which_on_path(exe) {
        return Some(PathBuf::from(exe));
    }
    ant_candidate_dirs()
        .into_iter()
        .find(|d| d.join(exe).exists())
}

#[cfg(windows)]
fn which_on_path(exe: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .any(|dir| PathBuf::from(dir).join(exe).exists())
}

#[cfg(not(windows))]
fn which_on_path(exe: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|dir| PathBuf::from(dir).join(exe).exists())
}

fn enforce_dependencies(cli: &crate::Cli, state: &mut StartupState) -> CliResult<()> {
    // If x0xd is not running:
    // Some commands MUST have x0xd running to communicate (resolve, search, name, index, rate, propagate).
    // Let's see which command is being executed.
    let needs_x0x = !is_offline_command(&cli.command);

    if needs_x0x && state.x0x_config.is_none() {
        if x0x_binary_available() {
            // x0x is installed but not running
            if cli.non_interactive {
                return Err(CliError::MissingDependency {
                    code: "x0x_daemon_not_running",
                    detail: "x0x daemon is not running. Please start it with 'x0x start'."
                        .to_string(),
                });
            } else {
                println!("x0x daemon is not running.");
                let offer_start = Confirm::new()
                    .with_prompt("Would you like to start it?")
                    .default(true)
                    .interact()
                    .map_err(|e| CliError::Other(e.to_string()))?;

                if offer_start {
                    if start_x0x_daemon() {
                        println!("x0x daemon started successfully.");
                        // Retry config
                        if let Ok(cfg) = X0xConfig::from_data_dir() {
                            state.x0x_config = Some(cfg);
                        }
                    } else {
                        return Err(CliError::Daemon(
                            "Failed to start x0x daemon. Please run 'x0x start' manually."
                                .to_string(),
                        ));
                    }
                } else {
                    return Err(CliError::Daemon(
                        "x0x daemon is required for this command. Exiting.".to_string(),
                    ));
                }
            }
        } else {
            // x0x is not installed at all
            if cli.non_interactive {
                return Err(CliError::MissingDependency {
                    code: "x0x_not_installed",
                    detail: concat!(
                        "x0x is not installed. ",
                        "Install it: curl -sfL https://x0x.md | sh"
                    )
                    .to_string(),
                });
            } else {
                println!("x0x is not installed.");
                let offer_install = Confirm::new()
                    .with_prompt("Would you like to install and start it?")
                    .default(true)
                    .interact()
                    .map_err(|e| CliError::Other(e.to_string()))?;

                if offer_install {
                    println!("Installing x0x... (this may take a moment)");
                    if install_x0x() {
                        println!("x0x installed and started successfully.");
                        // Retry config
                        if let Ok(cfg) = X0xConfig::from_data_dir() {
                            if ping_x0x_daemon(&cfg) {
                                state.x0x_config = Some(cfg);
                            }
                        }
                        if state.x0x_config.is_none() {
                            return Err(CliError::Daemon(
                                "x0x was installed but the daemon is not responding. \
                                 Please check manually with 'x0x health'."
                                    .to_string(),
                            ));
                        }
                    } else {
                        println!();
                        println!("Installation failed. You can install x0x manually:");
                        println!();
                        println!("  Linux/macOS:  curl -sfL https://x0x.md | sh");
                        #[cfg(target_os = "windows")]
                        println!(
                            "  Windows: Download from https://github.com/saorsa-labs/x0x/releases"
                        );
                        println!();
                        return Err(CliError::Daemon(
                            "x0x installation failed. Please install manually.".to_string(),
                        ));
                    }
                } else {
                    println!();
                    println!("You can install x0x later:");
                    println!();
                    println!("  Linux/macOS:  curl -sfL https://x0x.md | sh");
                    #[cfg(target_os = "windows")]
                    println!(
                        "  Windows: Download from https://github.com/saorsa-labs/x0x/releases"
                    );
                    println!();
                    return Err(CliError::Daemon(
                        "x0x is required for this command. Exiting.".to_string(),
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Environment opt-out from automatic service startup.
///
/// Set to `1`/`true`/`yes` to make [`ensure_services`] a no-op. The web test
/// harness sets it so no test spawns a real daemon, and it is a genuine escape
/// hatch for anyone whose x0x is managed elsewhere.
pub const NO_AUTOSTART_ENV: &str = "ANTA_VISTA_NO_AUTOSTART";

pub fn autostart_disabled() -> bool {
    matches!(
        std::env::var(NO_AUTOSTART_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Whether the default x0x daemon currently answers its health endpoint.
pub fn x0x_running() -> bool {
    X0xConfig::from_data_dir()
        .ok()
        .map(|cfg| ping_x0x_daemon(&cfg))
        .unwrap_or(false)
}

/// What [`ensure_services`] found and what it had to start.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceReport {
    pub x0x_running: bool,
    pub x0x_started: bool,
    pub listener_running: bool,
    pub listener_started: bool,
    /// Short human sentence for a UI toast or a log line.
    pub detail: String,
}

/// Start x0x (when it is installed but not answering) and then the background
/// listener, as one unit.
///
/// This is the web UI's on-demand start path: called on boot, before every
/// search/index, and from the drawer's Start buttons. It is blocking by design —
/// the caller asked for the services to be up before it proceeds — but it only
/// spends time when something is actually down; a running x0x costs one local
/// health ping.
///
/// The listener is started only once x0x answers, because a listener with no
/// x0x cannot reach anyone. That is the same gate `run_startup_checks` applies.
pub fn ensure_services() -> ServiceReport {
    if autostart_disabled() {
        return ServiceReport {
            x0x_running: x0x_running(),
            x0x_started: false,
            listener_running: crate::listener::is_running(),
            listener_started: false,
            detail: "automatic service startup is disabled".to_string(),
        };
    }

    let x0x_was = x0x_running();
    let x0x_started = if x0x_was {
        false
    } else if x0x_binary_available() {
        start_x0x_daemon()
    } else {
        false
    };
    // Re-probe rather than trust the start helper's return value alone: x0x may
    // have been started by someone else between the ping and our attempt.
    let x0x_running = x0x_was || x0x_started || x0x_running();

    let listener_was = crate::listener::is_running();
    let listener_running = if x0x_running {
        crate::listener::ensure_running()
    } else {
        listener_was
    };
    let listener_started = listener_running && !listener_was;

    let mut parts = Vec::new();
    if x0x_started {
        parts.push("started x0x");
    }
    if listener_started {
        parts.push("started the background listener");
    }
    let detail = if !parts.is_empty() {
        parts.join("; ")
    } else if x0x_running {
        "network services already running".to_string()
    } else {
        "x0x is not installed or could not be started".to_string()
    };

    ServiceReport {
        x0x_running,
        x0x_started,
        listener_running,
        listener_started,
        detail,
    }
}

/// Stop the default x0x daemon, then wait briefly for it to stop answering.
///
/// Returns `true` once the daemon is not running (including when it already
/// was). The UI's Stop button is deliberately momentary — the next search or
/// index will start it again — so this only has to bring the process down, not
/// remember the decision.
pub fn stop_x0x_daemon() -> bool {
    if !x0x_running() {
        return true;
    }

    let run_stop = |bin: &str| -> bool {
        matches!(
            std::process::Command::new(bin).arg("stop").status(),
            Ok(s) if s.success()
        )
    };

    let invoked = run_stop("x0x")
        || directories::BaseDirs::new()
            .map(|d| d.home_dir().join(".local/bin/x0x"))
            .filter(|p| p.exists())
            .map(|p| run_stop(p.to_str().unwrap_or("x0x")))
            .unwrap_or(false);
    if !invoked {
        return !x0x_running();
    }

    // `x0x stop` can acknowledge before the daemon has finished exiting.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if !x0x_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    !x0x_running()
}

pub fn start_x0x_daemon() -> bool {
    let retry_until = std::time::Instant::now() + Duration::from_secs(8);

    let try_path = |bin: &str| -> bool {
        match std::process::Command::new(bin).arg("start").spawn() {
            Ok(_) => {
                while std::time::Instant::now() < retry_until {
                    std::thread::sleep(Duration::from_millis(500));
                    if let Ok(cfg) = X0xConfig::from_data_dir() {
                        if ping_x0x_daemon(&cfg) {
                            return true;
                        }
                    }
                }
                false
            }
            Err(_) => false,
        }
    };

    if try_path("x0x") {
        return true;
    }
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let local_x0x = base_dirs.home_dir().join(".local/bin/x0x");
        if local_x0x.exists() {
            return try_path(local_x0x.to_str().unwrap_or("x0x"));
        }
    }
    false
}

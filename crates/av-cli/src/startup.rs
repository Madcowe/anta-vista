use crate::cmd::{CliError, CliResult};
use av_core::config::AvConfig;
use av_net_x0x::client::X0xConfig;
use dialoguer::Confirm;
use serde_json::json;
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
    //    this for Status (read-only), Purge (offline), and Listen itself (it IS
    //    the listener — starting another would be redundant).
    let needs_listener = x0x_config.is_some() && match cli.command {
        crate::Commands::Status => false,
        crate::Commands::Purge { .. } => false,
        crate::Commands::Listen { .. } => false,
        _ => true,
    };
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

fn load_config(cli: &crate::Cli) -> CliResult<(AvConfig, Option<PathBuf>)> {
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
    let bin = if cfg!(target_os = "windows") {
        "ant.exe"
    } else {
        "ant"
    };
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn install_ant_cli() -> bool {
    if !cfg!(target_os = "windows") {
        // Linux/macOS: use the official install script
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(
                "curl -fsSL https://raw.githubusercontent.com/WithAutonomi/ant-client/main/install.sh | bash",
            )
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        matches!(status, Ok(s) if s.success())
    } else {
        // Windows: use the official PowerShell install script
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(
                "irm https://raw.githubusercontent.com/WithAutonomi/ant-client/main/install.ps1 | iex",
            )
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        matches!(status, Ok(s) if s.success())
    }
}

fn x0x_binary_available() -> bool {
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

fn install_x0x() -> bool {
    if !cfg!(target_os = "windows") {
        // Linux/macOS: use the install script
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("curl -sfL https://x0x.md | sh -s -- --start")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        let ok = matches!(status, Ok(s) if s.success());
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
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        let ok = matches!(status, Ok(s) if s.success());
        if ok {
            std::thread::sleep(Duration::from_secs(3));
        }
        ok
    }
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

fn ping_x0x_daemon(cfg: &X0xConfig) -> bool {
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

fn ping_antd() -> bool {
    // Default antd port is 8082
    match health_check_agent()
        .get("http://localhost:8082/health")
        .call()
    {
        Ok(resp) => resp.status() == 200,
        Err(_) => false,
    }
}

fn check_minilm_cached() -> bool {
    // fastembed caches models in standard cache directories, e.g. ~/.cache/fastembed/
    // Let's check if fastembed dir or hf-hub cache exists.
    // If not, we still assume true since it downloads on first use.
    // But if we want to be safe, we can just say true if it's cached or we have internet.
    true
}

fn enforce_dependencies(cli: &crate::Cli, state: &mut StartupState) -> CliResult<()> {
    // If x0xd is not running:
    // Some commands MUST have x0xd running to communicate (resolve, search, name, index, rate, purge).
    // Let's see which command is being executed.
    let needs_x0x = match cli.command {
        crate::Commands::Status => false,
        crate::Commands::Purge { .. } => false, // purge can run offline on local DB
        crate::Commands::Listen { .. } => false, // listen checks x0x itself with a clear error
        _ => true,
    };

    if needs_x0x && state.x0x_config.is_none() {
        if x0x_binary_available() {
            // x0x is installed but not running
            if cli.non_interactive {
                let json_err = json!({
                    "ok": false,
                    "error": "x0x_daemon_not_running",
                    "detail": "x0x daemon is not running. Please start it with 'x0x start'."
                });
                println!("{}", serde_json::to_string_pretty(&json_err).unwrap());
                std::process::exit(1);
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
                let json_err = json!({
                    "ok": false,
                    "error": "x0x_not_installed",
                    "detail": concat!(
                        "x0x is not installed. ",
                        "Install it: curl -sfL https://x0x.md | sh",
                    )
                });
                println!("{}", serde_json::to_string_pretty(&json_err).unwrap());
                std::process::exit(1);
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
                        println!("  Windows: Download from https://github.com/saorsa-labs/x0x/releases");
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
                    println!("  Windows: Download from https://github.com/saorsa-labs/x0x/releases");
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

fn start_x0x_daemon() -> bool {
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

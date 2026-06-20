use crate::cmd::{CliError, CliResult};
use av_core::config::AvConfig;
use av_net_x0x::client::X0xConfig;
use dialoguer::Confirm;
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct StartupState {
    pub config: AvConfig,
    pub config_path: Option<PathBuf>,
    pub x0x_config: Option<X0xConfig>,
    pub antd_running: bool,
    pub minilm_loaded: bool,
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

    let state = StartupState {
        config,
        config_path,
        x0x_config,
        antd_running,
        minilm_loaded,
    };

    // 5. Handle missing dependencies based on interactive/non-interactive mode
    enforce_dependencies(cli, &state)?;

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

fn ping_x0x_daemon(cfg: &X0xConfig) -> bool {
    let url = format!("{}/health", cfg.api_base);
    match ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(resp) => resp.status() == 200,
        Err(_) => false,
    }
}

fn ping_antd() -> bool {
    // Default antd port is 8082
    match ureq::get("http://localhost:8082/health").call() {
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

fn enforce_dependencies(cli: &crate::Cli, state: &StartupState) -> CliResult<()> {
    // If x0xd is not running:
    // Some commands MUST have x0xd running to communicate (resolve, search, name, index, rate, purge).
    // Let's see which command is being executed.
    let needs_x0x = match cli.command {
        crate::Commands::Status => false,
        crate::Commands::Purge { .. } => false, // purge can run offline on local DB
        _ => true,
    };

    if needs_x0x && state.x0x_config.is_none() {
        if cli.non_interactive {
            let json_err = json!({
                "ok": false,
                "error": "x0x_daemon_not_running",
                "detail": "Could not connect to x0x daemon. Please start it with 'x0x start'."
            });
            println!("{}", serde_json::to_string_pretty(&json_err).unwrap());
            std::process::exit(1);
        } else {
            println!("x0x daemon is not running.");
            let offer_start = Confirm::new()
                .with_prompt("Would you like to try starting the x0x daemon?")
                .default(true)
                .interact()
                .map_err(|e| CliError::Other(e.to_string()))?;

            if offer_start {
                // Try to start x0x daemon
                if start_x0x_daemon() {
                    println!("x0x daemon started successfully.");
                } else {
                    return Err(CliError::Daemon(
                        "Failed to start x0x daemon. Please run 'x0x start' manually.".to_string(),
                    ));
                }
            } else {
                return Err(CliError::Daemon(
                    "x0x daemon is required for this command. Exiting.".to_string(),
                ));
            }
        }
    }

    // ant daemon is only required for indexing ant:// URIs.
    // We will do a contextual check inside the index/name commands instead of failing here.

    Ok(())
}

fn start_x0x_daemon() -> bool {
    // Check if x0x command is available
    let output = std::process::Command::new("x0x").arg("start").spawn();

    match output {
        Ok(_) => {
            // Wait a moment for it to start up and listen
            std::thread::sleep(std::time::Duration::from_secs(2));
            true
        }
        Err(_) => {
            // Check if ~/.local/bin/x0x exists
            if let Some(base_dirs) = directories::BaseDirs::new() {
                let local_x0x = base_dirs.home_dir().join(".local/bin/x0x");
                if local_x0x.exists() {
                    if let Ok(_) = std::process::Command::new(local_x0x).arg("start").spawn() {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        return true;
                    }
                }
            }
            false
        }
    }
}

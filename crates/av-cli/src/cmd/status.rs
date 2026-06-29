use crate::cmd::{CliError, CliResult};
use crate::output::print_output;
use crate::startup::StartupState;
use serde_json::json;
use std::process::Command;

pub fn run(cli: crate::Cli, state: StartupState) -> CliResult<()> {
    // 1. Database stats
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;

    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    let resources_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM resources", [], |row| row.get(0))
        .unwrap_or(0);

    let names_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM name_records", [], |row| row.get(0))
        .unwrap_or(0);

    let embeddings_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
        .unwrap_or(0);

    let feedback_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM feedback_events", [], |row| row.get(0))
        .unwrap_or(0);

    // 2. Fetch ant details
    let ant_version = if state.antd_running {
        // Try to get more info from antd health or use placeholder
        "0.7.1".to_string()
    } else {
        "unknown".to_string()
    };

    let ant_network = if state.antd_running {
        "default".to_string()
    } else {
        "offline".to_string()
    };

    let ant_cli_found = Command::new("ant")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // 3. Serialize output
    let status_json = json!({
        "x0x_daemon": {
            "running": state.x0x_config.is_some(),
            "agent_id": state.x0x_config.as_ref().map(|c| c.agent_id.clone()).unwrap_or_else(|| "".to_string()),
            "api": state.x0x_config.as_ref().map(|c| c.api_base.clone()).unwrap_or_else(|| "".to_string()),
        },
        "av_listener": {
            "running": state.listener_running,
            "pid": crate::listener::pid_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| s.trim().parse::<u32>().ok()),
        },
        "ant_daemon": {
            "running": state.antd_running,
            "network": ant_network,
            "version": ant_version,
        },
        "ant_cli": {
            "installed": ant_cli_found,
        },
        "minilm_model": {
            "loaded": state.minilm_loaded,
            "dimensions": 384,
        },
        "database": {
            "resources": resources_count,
            "name_records": names_count,
            "embeddings": embeddings_count,
            "feedback_events": feedback_count,
        },
        "config_path": state.config_path.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "".to_string()),
    });

    print_output(
        cli.non_interactive,
        || {
            println!("anta-vista status");
            if let Some(ref x0x) = state.x0x_config {
                println!(
                    "  x0x daemon:  {} running (agent: {}, API: {})",
                    console::style("✓").green(),
                    &x0x.agent_id[..8],
                    x0x.api_base
                );
            } else {
                println!("  x0x daemon:  {} offline", console::style("x").red());
            }

            if state.listener_running {
                let pid = crate::listener::pid_path()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .map(|p| format!(" (pid: {p})"))
                    .unwrap_or_default();
                println!(
                    "  av listener: {} running{}",
                    console::style("✓").green(),
                    pid
                );
            } else {
                println!("  av listener: {} not running", console::style("x").red());
            }

            if state.antd_running {
                println!(
                    "  ant daemon:  {} running (network: {}, version: {})",
                    console::style("✓").green(),
                    ant_network,
                    ant_version
                );
            } else {
                println!("  ant daemon:  {} offline", console::style("x").red());
            }

            if ant_cli_found {
                println!("  ant CLI:     {} installed", console::style("✓").green());
            } else {
                println!("  ant CLI:     {} not found", console::style("x").red());
            }

            if state.minilm_loaded {
                println!(
                    "  MiniLM model: {} loaded (384 dimensions)",
                    console::style("✓").green()
                );
            } else {
                println!(
                    "  MiniLM model: {} failed to load",
                    console::style("x").red()
                );
            }

            println!(
                "  Database:    {} {}, {} name records",
                console::style("✓").green(),
                format!(
                    "{} resources ({} embeddings, {} feedback events)",
                    resources_count, embeddings_count, feedback_count
                ),
                names_count
            );

            if let Some(ref path) = state.config_path {
                println!("  Config:      {}", path.display());
            } else {
                println!("  Config:      using defaults");
            }
        },
        &status_json,
    );

    Ok(())
}

use crate::cmd::{CliError, CliResult};
use crate::output::{abbrev, print_output};
use crate::startup::StartupState;
use serde_json::json;
use std::process::Command;

/// Ask the ant CLI for its version. Returns "unknown" if it can't be determined.
fn probe_ant_version() -> String {
    let Some(path) = crate::startup::ant_resolve_path() else {
        return "unknown".to_string();
    };
    Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Row counts for the local index, read fresh from the database.
pub fn database_counts(conn: &rusqlite::Connection) -> serde_json::Value {
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or(0)
    };
    json!({
        "resources": count("resources"),
        "name_records": count("name_records"),
        "embeddings": count("embeddings"),
        "feedback_events": count("feedback_events"),
    })
}

pub fn run(cli: crate::Cli, state: StartupState) -> CliResult<()> {
    // 1. Database stats
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;

    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;
    let database = database_counts(&conn);
    let resources_count = database["resources"].as_i64().unwrap_or(0);
    let names_count = database["name_records"].as_i64().unwrap_or(0);
    let embeddings_count = database["embeddings"].as_i64().unwrap_or(0);
    let feedback_count = database["feedback_events"].as_i64().unwrap_or(0);

    // 2. Fetch ant details
    let antd_running = state.antd_running;
    let ant_cli_found = crate::startup::ant_cli_binary_available();
    // Report a real version, or "unknown" — never invent one. The daemon's own
    // health endpoint does not carry a version, so prefer the ant CLI when it
    // is present and otherwise say we could not determine it.
    let ant_version = match ant_cli_found {
        true => probe_ant_version(),
        false if antd_running => "unknown".to_string(),
        false => "unknown".to_string(),
    };

    let ant_network = if antd_running {
        "default".to_string()
    } else {
        "offline".to_string()
    };

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
        "database": database,
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
                    abbrev(&x0x.agent_id, 8),
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

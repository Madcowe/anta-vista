use crate::cmd::{CliError, CliResult};
use crate::output::print_output;
use crate::startup::StartupState;
use dialoguer::Confirm;
use serde_json::json;

pub fn run(
    cli: crate::Cli,
    _state: StartupState,
    resource: Option<String>,
    name: Option<String>,
    all: bool,
    cache: bool,
    duplicates: bool,
    no_confirm: bool,
) -> CliResult<()> {
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let mut conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    let mut deleted_resources = 0usize;
    let mut deleted_embeddings = 0usize;
    let mut deleted_name_records = 0usize;
    let mut deleted_feedback_events = 0usize;
    let mut deleted_peer_cache = 0usize;
    let mut deleted_query_cache = 0usize;

    if all {
        if !no_confirm && !cli.non_interactive {
            let confirm = Confirm::new()
                .with_prompt("Are you sure you want to purge the entire local database?")
                .default(false)
                .interact()
                .map_err(|e| CliError::Other(e.to_string()))?;
            if !confirm {
                return Err(CliError::Validation("Purge aborted".to_string()));
            }
        }

        let tx = conn
            .transaction()
            .map_err(|e| CliError::Database(e.to_string()))?;
        deleted_feedback_events = tx
            .execute("DELETE FROM feedback_events", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
        deleted_embeddings = tx
            .execute("DELETE FROM embeddings", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
        deleted_resources = tx
            .execute("DELETE FROM resources", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
        deleted_name_records = tx
            .execute("DELETE FROM name_records", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
        deleted_peer_cache = tx
            .execute("DELETE FROM peer_cache", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
        deleted_query_cache = tx
            .execute("DELETE FROM query_cache", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
        tx.commit().map_err(|e| CliError::Database(e.to_string()))?;
    } else if cache {
        deleted_peer_cache = conn
            .execute("DELETE FROM peer_cache", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
        deleted_query_cache = conn
            .execute("DELETE FROM query_cache", [])
            .map_err(|e| CliError::Database(e.to_string()))?;
    } else if duplicates {
        // Find locations that appear more than once, keep the newest
        let mut stmt = conn
            .prepare("SELECT location, COUNT(*) as cnt FROM resources GROUP BY location HAVING cnt > 1")
            .map_err(|e| CliError::Database(e.to_string()))?;
        let dup_locations: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| CliError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        if dup_locations.is_empty() {
            println!("  No duplicate resources found.");
        } else {
            let total = dup_locations.len();
            let msg = format!(
                "Found {} location(s) with duplicates. Keep the newest entry per location and remove the rest?",
                total
            );
            if !no_confirm && !cli.non_interactive {
                if !Confirm::new()
                    .with_prompt(&msg)
                    .default(false)
                    .interact()
                    .map_err(|e| CliError::Other(e.to_string()))?
                {
                    return Err(CliError::Validation("Deduplication aborted".to_string()));
                }
            }

            let mut kept = 0usize;
            let mut removed = 0usize;
            for loc in &dup_locations {
                // Get all resource IDs for this location, newest first
                let mut stmt2 = conn
                    .prepare(
                        "SELECT id, created_at FROM resources WHERE location = ?1 \
                         ORDER BY created_at DESC",
                    )
                    .map_err(|e| CliError::Database(e.to_string()))?;
                let rows: Vec<(String, i64)> = stmt2
                    .query_map(rusqlite::params![loc], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(|e| CliError::Database(e.to_string()))?
                    .filter_map(|r| r.ok())
                    .collect();

                if rows.len() > 1 {
                    kept += 1;
                    for (rid, _) in &rows[1..] {
                        // CASCADE will remove associated embeddings and feedback
                        conn.execute("DELETE FROM resources WHERE id = ?1", rusqlite::params![rid])
                            .map_err(|e| CliError::Database(e.to_string()))?;
                        removed += 1;
                    }
                    deleted_resources = removed;
                }
            }

            if removed > 0 {
                println!(
                    "  {} Deduplication complete: kept {} and removed {} resource(s)",
                    console::style("✓").green(),
                    kept,
                    removed,
                );
            }
        }
    } else {
        if let Some(ref r_id) = resource {
            let tx = conn
                .transaction()
                .map_err(|e| CliError::Database(e.to_string()))?;
            deleted_feedback_events = tx
                .execute(
                    "DELETE FROM feedback_events WHERE resource_id = ?1",
                    rusqlite::params![r_id],
                )
                .map_err(|e| CliError::Database(e.to_string()))?;
            deleted_embeddings = tx
                .execute(
                    "DELETE FROM embeddings WHERE resource_id = ?1",
                    rusqlite::params![r_id],
                )
                .map_err(|e| CliError::Database(e.to_string()))?;
            deleted_resources = tx
                .execute(
                    "DELETE FROM resources WHERE id = ?1",
                    rusqlite::params![r_id],
                )
                .map_err(|e| CliError::Database(e.to_string()))?;
            tx.commit().map_err(|e| CliError::Database(e.to_string()))?;
        }

        if let Some(ref n) = name {
            let normalized = av_core::types::normalize_name(n);
            deleted_name_records = conn
                .execute(
                    "DELETE FROM name_records WHERE normalized_name = ?1",
                    rusqlite::params![normalized],
                )
                .map_err(|e| CliError::Database(e.to_string()))?;
        }

        if resource.is_none() && name.is_none() {
            return Err(CliError::Validation(
                "Specify --resource, --name, --duplicates, --all, or --cache to purge"
                    .to_string(),
            ));
        }
    }

    let warning = "Previously broadcast data cannot be recalled from the gossip network";
    let output_json = json!({
        "ok": true,
        "deleted": {
            "resources": deleted_resources,
            "embeddings": deleted_embeddings,
            "name_records": deleted_name_records,
            "feedback_events": deleted_feedback_events,
            "peer_cache": deleted_peer_cache,
            "query_cache": deleted_query_cache,
        },
        "warning": warning,
    });

    print_output(
        cli.non_interactive,
        || {
            println!("{} Local purge complete.", console::style("✓").green());
            println!(
                "  deleted: {} resources, {} embeddings, {} name records, {} feedback events",
                deleted_resources,
                deleted_embeddings,
                deleted_name_records,
                deleted_feedback_events,
            );
            if deleted_peer_cache > 0 || deleted_query_cache > 0 {
                println!(
                    "  cache: {} peers, {} queries",
                    deleted_peer_cache, deleted_query_cache,
                );
            }
            println!("  warning: {}", warning);
        },
        &output_json,
    );

    Ok(())
}

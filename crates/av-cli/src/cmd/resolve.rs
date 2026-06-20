use crate::cmd::{CliError, CliResult};
use crate::network::execute_resolve;
use crate::output::print_output;
use crate::startup::StartupState;
use av_core::paths::db_path;
use serde_json::json;

pub fn run(
    cli: crate::Cli,
    state: StartupState,
    name: String,
    record_type: String,
    scheme: Option<String>,
    limit: usize,
) -> CliResult<()> {
    let db_path = db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;

    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    // Execute resolve across tiers
    let resolve_res = execute_resolve(
        &cli,
        &state,
        &conn,
        &name,
        &record_type,
        scheme.clone(),
        limit,
    )?;

    // Cache received network records locally
    for (_peer, payload) in &resolve_res.network_results {
        for record in &payload.results {
            // Validate signature first (if possible)
            // For MVP/simplicity we assume valid if serialize/deserialize matches,
            // or we insert and let the store handle unique constraints.
            let _ = av_store::repo::names::insert(&conn, record);
        }
    }

    // Re-run lookup locally to merge and get final scored and sorted results
    let mut scheme_filter = av_index::filter::SchemeFilter::default();
    if let Some(s) = scheme {
        scheme_filter = av_index::filter::SchemeFilter::new(vec![s]);
    }
    let merged_results = av_index::naming::lookup_name(&conn, &name, &scheme_filter, now_secs())
        .map_err(|e| CliError::Database(e.to_string()))?;

    let winners = merged_results.into_iter().take(limit).collect::<Vec<_>>();

    // Format output
    let winner = winners.first().cloned();
    let alternates = if winners.len() > 1 {
        winners
            .iter()
            .skip(1)
            .cloned()
            .map(|w| {
                json!({
                    "record_id": w.record.record_id,
                    "target": w.record.target,
                    "record_type": format!("{:?}", w.record.record_type),
                    "score": w.score,
                })
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };

    let resolve_json = json!({
        "name": name,
        "normalized_name": av_core::types::normalize_name(&name),
        "winner": winner.as_ref().map(|w| json!({
            "record_id": w.record.record_id,
            "target": w.record.target,
            "record_type": format!("{:?}", w.record.record_type),
            "score": w.score,
        })),
        "alternates": alternates,
        "scoring": {
            "mode": "name_v1",
            "weights": {
                "trust": 0.50,
                "agreement": 0.30,
                "recency": 0.10,
                "ttl_validity": 0.10,
            }
        }
    });

    print_output(
        cli.non_interactive,
        || {
            if let Some(ref w) = winner {
                println!(
                    "Resolved {} → {} (score: {:.3})",
                    console::style(&name).cyan().bold(),
                    console::style(&w.record.target).green().bold(),
                    w.score
                );
                if !alternates.is_empty() {
                    println!("\nAlternates:");
                    for alt in &alternates {
                        println!(
                            "  {} (score: {:.3})",
                            alt["target"].as_str().unwrap_or(""),
                            alt["score"].as_f64().unwrap_or(0.0)
                        );
                    }
                }
            } else {
                println!("No records found for name: {}", name);
            }
        },
        &resolve_json,
    );

    Ok(())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

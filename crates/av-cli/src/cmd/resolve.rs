use crate::cmd::{CliError, CliResult};
use crate::network::execute_resolve;
use crate::output::print_output;
use crate::startup::StartupState;
use av_core::constants::WEIGHT_RELEVANCE;
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

    // Cache received network records locally.
    //
    // `execute_resolve` already drops whole responses from blocked agents, so
    // anything reaching here came from a peer we are willing to hear from. The
    // per-record check is for individual records the operator has blocked
    // without blocking the peer.
    for (peer, payload) in &resolve_res.network_results {
        for record in &payload.results {
            // Fail closed. Returning `false` here on a database error would be
            // indistinguishable from "not blocked", and the effect is to cache
            // every record in the response — so a transient read failure would
            // quietly reintroduce names the operator suppressed.
            let record_blocked = av_store::repo::blocks::is_blocked(
                &conn,
                av_store::repo::blocks::BlockKind::Name,
                &record.record_id,
            )
            .map_err(|e| CliError::Database(e.to_string()))?;
            if record_blocked {
                tracing::debug!(
                    record_id = %record.record_id,
                    peer = %crate::output::abbrev(peer, 12),
                    "skipping cached network name record: locally blocked"
                );
                continue;
            }

            // `peer` is the id the x0x daemon authenticated — `network.rs` only
            // ever populates `network_results` from `msg.sender` /
            // `event.origin` after an `accept_from` gate. It is deliberately not
            // `record.by_agent_id`, which the sending peer chose and could set
            // to any value, including an origin that is not blocked.
            //
            // Validate signature first (if possible)
            // For MVP/simplicity we assume valid if serialize/deserialize matches,
            // or we insert and let the store handle unique constraints.
            if let Err(e) =
                av_store::repo::names::insert_with_origin(&conn, record, Some(peer.as_str()))
            {
                tracing::debug!(
                    record_id = %record.record_id,
                    peer = %crate::output::abbrev(peer, 12),
                    "could not cache network name record: {e}"
                );
            }
        }
    }

    // Re-run lookup locally to merge and get final scored and sorted results
    let mut scheme_filter = av_index::filter::SchemeFilter::default();
    if let Some(s) = scheme {
        scheme_filter = av_index::filter::SchemeFilter::new(vec![s]);
    }
    let merged_results = av_index::naming::lookup_name(&conn, &name, &scheme_filter, now_secs())
        .map_err(|e| CliError::Database(e.to_string()))?;

    // Apply relevance boost from feedback
    let normalized = av_core::types::normalize_name(&name);
    let mut merged_results: Vec<av_index::naming::NameResult> = merged_results
        .into_iter()
        .map(|mut nr| {
            let rel =
                av_store::repo::relevance::name_get_score(&conn, &normalized, &nr.record.record_id)
                    .ok()
                    .flatten()
                    .unwrap_or(0.5);
            nr.score = nr.score * (1.0 - WEIGHT_RELEVANCE) + rel * WEIGHT_RELEVANCE;
            nr
        })
        .collect();

    // Re-sort after relevance boost (scores may shift)
    merged_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let winners = merged_results.into_iter().take(limit).collect::<Vec<_>>();

    // Format output
    let winner = winners.first().cloned();
    let alternates = if winners.len() > 1 {
        winners
            .iter()
            .skip(1)
            .cloned()
            .map(|w| {
                let target = w
                    .record
                    .target_canonical
                    .as_deref()
                    .unwrap_or(&w.record.target);
                json!({
                    "record_id": w.record.record_id,
                    "target": target,
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
        "winner": winner.as_ref().map(|w| {
            let target = w.record.target_canonical.as_deref().unwrap_or(&w.record.target);
            json!({
            "record_id": w.record.record_id,
            "target": target,
            "record_type": format!("{:?}", w.record.record_type),
            "score": w.score,
        }) }),
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
                let target = w
                    .record
                    .target_canonical
                    .as_deref()
                    .unwrap_or(&w.record.target);
                println!(
                    "Resolved {} → {} (score: {:.3})",
                    console::style(&name).cyan().bold(),
                    console::style(target).green().bold(),
                    w.score
                );
                // The resolved URI is the *target*, not the identity of the
                // claim. A name block keys on the record id, so print it —
                // otherwise the only identifier on screen cannot be used with
                // `av block --name`, and blocking the target URI silently
                // blocks nothing.
                println!(
                    "  record_id: {}",
                    console::style(&w.record.record_id).cyan()
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
                    // Alternates are separate claims by other agents. If one of
                    // them is the bad record, its id has to be visible too.
                    for alt in &alternates {
                        if let Some(id) = alt["record_id"].as_str() {
                            println!("    record_id: {}", console::style(id).cyan());
                        }
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

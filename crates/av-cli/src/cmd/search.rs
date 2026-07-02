use crate::cmd::{CliError, CliResult};
use crate::network::execute_search;
use crate::output::print_output;
use crate::startup::StartupState;
use av_embed::minilm::MiniLmProvider;
use av_query::cluster::{cluster_responses, needs_clustering};
use serde_json::json;

pub fn run(
    cli: crate::Cli,
    state: StartupState,
    query: String,
    scheme: Option<String>,
    kind: Option<String>,
    mime: Option<String>,
    limit: usize,
) -> CliResult<()> {
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    // Load real embedding model
    let provider = MiniLmProvider::new().map_err(|e| CliError::Model(e.to_string()))?;

    let res = execute_search(
        &cli, &state, &conn, &provider, &query, scheme, kind, mime, limit,
    )?;

    // Cluster network results and merge with local
    let clustered = cluster_responses(&res.network_results);
    let use_clustering = needs_clustering(state.x0x_config.as_ref().map(|_| 1).unwrap_or(0), 3);

    // Merge local and clustered network results, sorted by score descending
    let mut all_results: Vec<serde_json::Value> = Vec::new();

    for r in &res.local_results {
        all_results.push(json!({
            "resource_id": r.resource.id,
            "location": r.resource.location,
            "description": r.resource.description_text,
            "mime_type": r.resource.mime_type,
            "kind": format!("{:?}", r.resource.kind),
            "score": r.score,
            "score_breakdown": {
                "semantic": r.semantic_score,
                "agreement": r.agreement_score,
                "feedback": r.feedback_score,
                "trust": r.trust_score,
                "relevance": r.relevance_score,
            },
            "source": "local",
        }));
    }

    for c in &clustered {
        if res
            .local_results
            .iter()
            .any(|lr| lr.resource.id == c.result.resource_id)
        {
            continue;
        }
        all_results.push(json!({
            "resource_id": c.result.resource_id,
            "location": c.result.location,
            "description": c.result.description_text,
            "mime_type": c.result.mime_type,
            "score": c.avg_score,
            "agreement_count": c.agreement_count,
            "source": "network",
        }));
    }

    all_results.sort_by(|a, b| {
        let a_score = a["score"].as_f64().unwrap_or(0.0);
        let b_score = b["score"].as_f64().unwrap_or(0.0);
        b_score
            .partial_cmp(&a_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let output_json = json!({
        "query": query,
        "results": all_results,
        "total": all_results.len(),
        "clustering_active": use_clustering,
    });

    print_output(
        cli.non_interactive,
        || {
            if all_results.is_empty() {
                println!("No results found for: {}", console::style(&query).cyan());
                return;
            }

            println!(
                "Search results for: {}\n",
                console::style(&query).cyan().bold()
            );

            for (rank, result) in all_results.iter().enumerate() {
                let score = result["score"].as_f64().unwrap_or(0.0);
                let location = result["location"].as_str().unwrap_or("");
                let description = result["description"].as_str().unwrap_or("");
                let resource_id = result["resource_id"].as_str().unwrap_or("");
                let source = result["source"].as_str().unwrap_or("");

                match source {
                    "local" => {
                        let mime_type = result["mime_type"].as_str().unwrap_or("");
                        println!(
                            "  {}. [{}] {} (score: {:.3})",
                            rank + 1,
                            console::style("local").dim(),
                            console::style(location).green(),
                            score,
                        );
                        println!("     {} [{}]", description, mime_type);
                        println!("     resource_id: {}", &resource_id[..resource_id.len().min(16)]);
                    }
                    "network" => {
                        let agreement_count = result["agreement_count"].as_u64().unwrap_or(0);
                        println!(
                            "  {}. [{}] {} (score: {:.3}, agreed by {} peers)",
                            rank + 1,
                            console::style("network").blue(),
                            console::style(location).green(),
                            score,
                            agreement_count,
                        );
                        println!("     {}", description);
                        println!(
                            "     resource_id: {}",
                            &resource_id[..resource_id.len().min(16)]
                        );
                    }
                    _ => {}
                }
                println!();
            }
        },
        &output_json,
    );

    // Interactive relevance feedback + content propagation
    if !cli.non_interactive && !all_results.is_empty() {
        use std::io::Write;
        print!("\nWhich result was most relevant? (1-{}, or Enter for none): ", all_results.len());
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let input = input.trim();

        if let Ok(n) = input.parse::<usize>() {
            if n >= 1 && n <= all_results.len() {
                let idx = n - 1;
                if let Some(rid) = all_results[idx]["resource_id"].as_str() {
                    // Propagate network results into local index
                    if av_store::repo::resources::get(&conn, rid)
                        .ok()
                        .flatten()
                        .is_none()
                    {
                        let description = all_results[idx]["description"]
                            .as_str()
                            .unwrap_or("");
                        let location = all_results[idx]["location"]
                            .as_str()
                            .unwrap_or("");
                        let mime_type = all_results[idx]["mime_type"]
                            .as_str()
                            .unwrap_or("application/octet-stream");

                        if !description.is_empty() {
                            if let Err(e) =
                                crate::cmd::propagate::propagate_resource(
                                    &conn, &provider, rid, location, description, mime_type,
                                )
                            {
                                tracing::warn!("Failed to propagate resource: {e}");
                            }
                        }
                    }

                    let normalized = query.trim().to_lowercase();
                    match av_store::repo::relevance::upsert(&conn, &normalized, rid, 1.0) {
                        Ok(()) => {
                            println!(
                                "  {} Marked result {} as relevant for this query",
                                console::style("✓").green(),
                                n,
                            );
                        }
                        Err(e) => {
                            println!(
                                "  {} Failed to store relevance: {}",
                                console::style("✗").red(),
                                e,
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

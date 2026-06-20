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

    // Build the output results list: local results first, then clustered network
    // Build JSON results array
    let mut results_json = Vec::new();

    for r in &res.local_results {
        results_json.push(json!({
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
            },
            "source": "local",
        }));
    }

    for c in &clustered {
        // Skip if already in local results
        if res
            .local_results
            .iter()
            .any(|lr| lr.resource.id == c.result.resource_id)
        {
            continue;
        }
        results_json.push(json!({
            "resource_id": c.result.resource_id,
            "location": c.result.location,
            "description": c.result.description_text,
            "score": c.avg_score,
            "agreement_count": c.agreement_count,
            "source": "network",
        }));
    }

    let output_json = json!({
        "query": query,
        "results": results_json,
        "total": results_json.len(),
        "clustering_active": use_clustering,
    });

    print_output(
        cli.non_interactive,
        || {
            if results_json.is_empty() {
                println!("No results found for: {}", console::style(&query).cyan());
                return;
            }

            println!(
                "Search results for: {}\n",
                console::style(&query).cyan().bold()
            );

            let mut rank = 1usize;

            for r in &res.local_results {
                println!(
                    "  {}. [{}] {} (score: {:.3})",
                    rank,
                    console::style("local").dim(),
                    console::style(&r.resource.location).green(),
                    r.score,
                );
                println!(
                    "     {} [{}]",
                    r.resource.description_text, r.resource.mime_type
                );
                println!("     resource_id: {}", &r.resource.id[..16]);
                println!();
                rank += 1;
            }

            for c in &clustered {
                if res
                    .local_results
                    .iter()
                    .any(|lr| lr.resource.id == c.result.resource_id)
                {
                    continue;
                }
                println!(
                    "  {}. [{}] {} (score: {:.3}, agreed by {} peers)",
                    rank,
                    console::style("network").blue(),
                    console::style(&c.result.location).green(),
                    c.avg_score,
                    c.agreement_count,
                );
                println!("     {}", c.result.description_text);
                println!(
                    "     resource_id: {}",
                    &c.result.resource_id[..16.min(c.result.resource_id.len())]
                );
                println!();
                rank += 1;
            }
        },
        &output_json,
    );

    Ok(())
}

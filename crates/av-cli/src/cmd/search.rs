use rusqlite::Connection;
use std::sync::Arc;

use crate::cmd::{CliError, CliResult};
use crate::network::execute_search;
use crate::output::print_output;
use crate::startup::StartupState;
use av_core::constants::WEIGHT_RELEVANCE;
use av_embed::minilm::MiniLmProvider;
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_query::cluster::cluster_responses;
use serde_json::json;

/// What the interactive rating prompt says about sharing a network link,
/// shown whenever the result list holds one.
///
/// The browser confirms before recommending a network result for exactly
/// this reason — "Share this link?" / "Recommending it may share the link
/// with other people using this network" — because a rating that ranks a
/// network result up propagates it into the local index, where `av listen`
/// serves it on to peers. The terminal's single prompt covers every result
/// at once and cannot ask per-result, so the caveat goes up once, in
/// yellow, above the question — and only when there is a network result
/// that could be shared; with local results only, a rating shares nothing,
/// and a warning that fires anyway teaches the user to ignore warnings.
pub const RATE_SHARING_NOTE: &str = "Note: recommending a network result may share the link \
                                     with other people using this network.";

/// Merged, ranked, de-duplicated search results ready for presentation.
///
/// Shared by the CLI and the web interface so both surfaces rank, name-match
/// and de-duplicate in exactly the same way. The JSON shape here is a public
/// contract for `--non-interactive` callers, so fields are only ever added.
pub struct MergedResults {
    pub results: Vec<serde_json::Value>,
    /// Whether network responses were clustered. Clustering only has anything
    /// to do when peers actually replied, so this tracks that rather than
    /// whether a daemon merely exists.
    pub clustering_active: bool,
}

/// Run a full search and merge local, network and name hits into one ranked list.
///
/// This is the single implementation of Anta-Vista search ranking; the CLI and
/// the web UI are both thin presentation layers over it.
/// Query filters and display options for one search.
///
/// Grouped because the flat argument list had grown past the point of being
/// readable, and because `limit` and `show_peers` are display concerns that
/// `collect_results` has no business knowing about individually.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub scheme: Option<String>,
    pub kind: Option<String>,
    pub mime: Option<String>,
    pub limit: usize,
    /// Print the peer ids behind each network result. Hidden by default.
    pub show_peers: bool,
}

pub fn collect_results(
    cli: &crate::Cli,
    state: &StartupState,
    conn: &Connection,
    provider: &MiniLmProvider,
    query: &str,
    opts: &SearchOptions,
) -> CliResult<MergedResults> {
    let SearchOptions {
        scheme,
        kind,
        mime,
        limit,
        ..
    } = opts.clone();
    // Normalise once, here, so every consumer agrees on the canonical spelling:
    // `av search --scheme ant`, `?scheme=ant` from the browser, the local
    // `SchemeFilter`, the name filter, and the `allowed_schemes` the wire
    // payload carries must all match `location_scheme` as *stored* — which is
    // canonical `autonomi`, per `normalize_uri`. Filtering on the raw alias
    // matched nothing at all.
    let scheme = scheme.map(|s| av_core::types::normalize_scheme(&s));
    let name_scheme_filter = scheme
        .clone()
        .map(|s| av_index::filter::SchemeFilter::new(vec![s]));

    let res = execute_search(cli, state, conn, provider, query, scheme, kind, mime, limit)?;

    // Cluster network results and merge with local
    let clustered = cluster_responses(&res.network_results);
    let clustering_active = !res.network_results.is_empty();

    // Local and name hits are already filtered by `av-index` (which is also
    // what keeps `av listen` from serving them). Network hits never pass
    // through the index — they arrive as wire payloads and are clustered — so
    // this is the only place they can be dropped.
    let blocked_resources =
        av_store::repo::blocks::blocked_ids(conn, av_store::repo::blocks::BlockKind::Resource)
            .unwrap_or_default();
    let blocked_names =
        av_store::repo::blocks::blocked_ids(conn, av_store::repo::blocks::BlockKind::Name)
            .unwrap_or_default();

    // Merge local and clustered network results, sorted by score descending
    let mut all_results: Vec<serde_json::Value> = Vec::new();

    for r in &res.local_results {
        let location = r
            .resource
            .location_canonical
            .as_deref()
            .unwrap_or(&r.resource.location);
        all_results.push(json!({
            "resource_id": r.resource.id,
            "location": location,
            "description": r.resource.description_text,
            "mime_type": r.resource.mime_type,
            // The name the author published, which `location` cannot show: an
            // Autonomi record's canonical location is the address alone, so
            // `autonomi://<hex>/welcome.md` reaches the page as `autonomi://<hex>`
            // and the only surviving copy of the name is this column.
            "filename": r.resource.filename,
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
        if blocked_resources.contains(&c.result.resource_id) {
            continue;
        }
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
            // The peers that actually returned this, so an operator who does
            // not want the result can block whoever sent it rather than only
            // the resource. These are transport-authenticated origins.
            "agent_ids": c.agent_ids,
            "source": "network",
        }));
    }

    // Append name matches with proper scoring + relevance boost
    // Try the full query first, then each significant token (>=2 chars)
    let normalized_query = query.trim().to_lowercase();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let sf = name_scheme_filter.unwrap_or_default();
    let mut name_candidates: Vec<String> = Vec::new();
    name_candidates.push(query.to_string());
    for token in query.split_whitespace().filter(|t| t.len() >= 2) {
        if !name_candidates.iter().any(|c| c == token) {
            name_candidates.push(token.to_string());
        }
    }
    for candidate in &name_candidates {
        if let Ok(name_results) = av_index::naming::lookup_name(conn, candidate, &sf, now) {
            for nr in name_results {
                if blocked_names.contains(&nr.record.record_id) {
                    continue;
                }
                let name_location = nr
                    .record
                    .target_canonical
                    .as_deref()
                    .unwrap_or(&nr.record.target);
                if all_results
                    .iter()
                    .any(|r| r["location"].as_str() == Some(name_location))
                {
                    continue;
                }
                let rel = av_store::repo::relevance::name_get_score(
                    conn,
                    &normalized_query,
                    &nr.record.record_id,
                )
                .ok()
                .flatten()
                .unwrap_or(0.5);
                let adjusted_score = nr.score * (1.0 - WEIGHT_RELEVANCE) + rel * WEIGHT_RELEVANCE;
                all_results.push(json!({
                    "resource_id": format!("name:{}", nr.record.record_id),
                    "location": name_location,
                    "description": nr.record.original_name,
                    "mime_type": "text/plain",
                    "score": adjusted_score,
                    "source": "name",
                    "name_record": serde_json::to_value(&nr.record).unwrap_or_default(),
                }));
            }
        }
    }

    all_results.sort_by(|a, b| {
        let a_score = a["score"].as_f64().unwrap_or(0.0);
        let b_score = b["score"].as_f64().unwrap_or(0.0);
        b_score
            .partial_cmp(&a_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(MergedResults {
        results: all_results,
        clustering_active,
    })
}

pub fn run(
    cli: crate::Cli,
    state: StartupState,
    query: String,
    opts: SearchOptions,
) -> CliResult<()> {
    let show_peers = opts.show_peers;
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    // Load real embedding model
    let provider = MiniLmProvider::new().map_err(|e| CliError::Model(e.to_string()))?;

    let merged = collect_results(&cli, &state, &conn, &provider, &query, &opts)?;
    let all_results = merged.results;

    let output_json = json!({
        "query": query,
        "results": all_results,
        "total": all_results.len(),
        "clustering_active": merged.clustering_active,
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
                        // The stored name annotates the identifier rather than
                        // replacing it: the location is what every command takes,
                        // and for Autonomi it is the address, which reads as noise
                        // on its own. Only local rows have one — a peer does not
                        // send a filename, and inventing one would be a lie.
                        let filename = result["filename"].as_str().unwrap_or("");
                        let identified = if filename.is_empty() {
                            console::style(location).green().to_string()
                        } else {
                            format!(
                                "{} \u{b7} {}",
                                console::style(location).green(),
                                console::style(filename).dim()
                            )
                        };
                        println!(
                            "  {}. [{}] {} (score: {:.3})",
                            rank + 1,
                            console::style("local").dim(),
                            identified,
                            score,
                        );
                        println!("     {} [{}]", description, mime_type);
                        // The full id, not a truncated one: an abbreviated hash
                        // cannot be pasted into `av block`, and a block stored
                        // against a truncated id matches nothing while looking
                        // like it worked.
                        println!("     resource_id: {}", resource_id);
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
                        println!("     resource_id: {}", resource_id);
                        // The peer count is already in the header line above,
                        // and printing every id is unusable at any real cluster
                        // size (a 50-agent cluster is ~3.2k characters on one
                        // line). Ids are transport-authenticated origins, so
                        // they stay available for anyone who needs to block a
                        // specific peer, behind an explicit flag and in the
                        // `--non-interactive` JSON, which always carries them.
                        if show_peers
                            && let Some(agents) = result["agent_ids"].as_array()
                            && !agents.is_empty()
                        {
                            let ids: Vec<&str> = agents.iter().filter_map(|a| a.as_str()).collect();
                            println!("     peers:       {}", ids.join(", "));
                        }
                    }
                    "name" => {
                        // A name result's `resource_id` is the synthetic
                        // `name:<record-id>` used for ranking, not a resource
                        // id, and `location` is the *target* URI — which is not
                        // what a name block keys on either. Print the record id
                        // under its own name so neither is mistaken for it.
                        let record_id = result["name_record"]["record_id"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        println!(
                            "  {}. [{}] {} → {} (score: {:.3})",
                            rank + 1,
                            console::style("name").magenta(),
                            console::style(description).cyan().bold(),
                            console::style(location).green(),
                            score,
                        );
                        println!("     record_id:   {}", record_id);
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
        // The GUI confirms before recommending a network link for exactly
        // this reason; one prompt cannot ask per-result, so the caveat goes
        // up once — and only when a network result is there to share.
        if all_results
            .iter()
            .any(|r| r["source"].as_str() == Some("network"))
        {
            println!("\n{}", console::style(RATE_SHARING_NOTE).yellow());
        }
        print!(
            "Which result was most relevant? (1-{}, or Enter for none): ",
            all_results.len()
        );
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let input = input.trim();

        if let Ok(n) = input.parse::<usize>() {
            if n >= 1 && n <= all_results.len() {
                let idx = n - 1;
                if let Some(rid) = all_results[idx]["resource_id"].as_str() {
                    let source = all_results[idx]["source"].as_str().unwrap_or("");

                    if source == "name" {
                        // Name result: re-broadcast the name claim to the network
                        if let Ok(record) = serde_json::from_value::<av_core::types::NameRecord>(
                            all_results[idx]["name_record"].clone(),
                        ) {
                            // Store relevance first (record is used by value in publish_name_claim)
                            let normalized = query.trim().to_lowercase();
                            if let Err(e) = av_store::repo::relevance::name_upsert(
                                &conn,
                                &normalized,
                                &record.record_id,
                                1.0,
                            ) {
                                tracing::warn!("Failed to store name relevance: {e}");
                            }
                            let local_agent_id = state
                                .x0x_config
                                .as_ref()
                                .map(|c| c.agent_id.as_str())
                                .unwrap_or("local");
                            if record.by_agent_id != local_agent_id {
                                if let Some(ref x0x_cfg) = state.x0x_config {
                                    let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));
                                    let dispatcher = MessageDispatcher::new(net_client);
                                    let _ = dispatcher.subscribe_all();
                                    if let Err(e) = dispatcher.publish_name_claim(record) {
                                        tracing::warn!("Failed to broadcast name claim: {}", e);
                                    }
                                }
                            }
                            println!(
                                "  {} Marked name result {} as relevant — propagated to network",
                                console::style("✓").green(),
                                n,
                            );
                        }
                    } else {
                        // Resource result: propagate into local index
                        if av_store::repo::blocks::is_blocked(
                            &conn,
                            av_store::repo::blocks::BlockKind::Resource,
                            rid,
                        )
                        .unwrap_or(false)
                        {
                            println!(
                                "  {} Result {} is blocked; not propagating it back",
                                console::style("✗").red(),
                                n,
                            );
                        } else if av_store::repo::resources::get(&conn, rid)
                            .ok()
                            .flatten()
                            .is_none()
                        {
                            let description =
                                all_results[idx]["description"].as_str().unwrap_or("");
                            let location = all_results[idx]["location"].as_str().unwrap_or("");
                            let mime_type = all_results[idx]["mime_type"]
                                .as_str()
                                .unwrap_or("application/octet-stream");

                            if !description.is_empty() {
                                if let Err(e) = crate::cmd::propagate::propagate_resource(
                                    &conn,
                                    &provider,
                                    rid,
                                    location,
                                    description,
                                    mime_type,
                                ) {
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
    }

    Ok(())
}

/// Record that a result was relevant for `query`.
///
/// This is what a positive rating does beyond storing the feedback row: it
/// boosts the exact-query relevance score (which feeds the `relevance` ranking
/// weight on subsequent searches) and, for a resource we do not already hold,
/// keeps a local metadata copy so `av listen` can serve it to peers.
///
/// Shared by the CLI's interactive prompt and the web interface's rating
/// buttons so both reinforce results identically.
pub fn reinforce(
    conn: &Connection,
    provider: &MiniLmProvider,
    state: &StartupState,
    query: &str,
    result: &serde_json::Value,
) -> CliResult<()> {
    let Some(resource_id) = result["resource_id"].as_str() else {
        return Err(CliError::Validation("result has no resource_id".into()));
    };
    let normalized = query.trim().to_lowercase();

    if result["source"].as_str() == Some("name") {
        let record =
            serde_json::from_value::<av_core::types::NameRecord>(result["name_record"].clone())
                .map_err(|e| CliError::Validation(format!("malformed name_record: {e}")))?;

        // A block imposed after this result was rendered must not be undone by
        // the operator's own next action, which would re-broadcast the very
        // record that was just blocked.
        if av_store::repo::blocks::is_blocked(
            conn,
            av_store::repo::blocks::BlockKind::Name,
            &record.record_id,
        )
        .unwrap_or(false)
        {
            return Err(CliError::Validation(format!(
                "name record {} is blocked; unblock it before reinforcing",
                record.record_id
            )));
        }

        // Relevance first: `publish_name_claim` consumes the record by value.
        av_store::repo::relevance::name_upsert(conn, &normalized, &record.record_id, 1.0)
            .map_err(|e| CliError::Database(e.to_string()))?;

        let local_agent_id = state
            .x0x_config
            .as_ref()
            .map(|c| c.agent_id.as_str())
            .unwrap_or("local");
        if record.by_agent_id != local_agent_id {
            if let Some(ref x0x_cfg) = state.x0x_config {
                let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));
                let dispatcher = MessageDispatcher::new(net_client);
                let _ = dispatcher.subscribe_all();
                dispatcher
                    .publish_name_claim(record)
                    .map_err(|e| CliError::Network(e.to_string()))?;
            }
        }
        return Ok(());
    }

    // Resource result: keep a metadata copy if we only heard about it from a peer.
    // This stores the description we were already given — it does not download
    // the underlying bytes.
    //
    // Refuse a blocked resource first: otherwise a block imposed after this
    // result was rendered would be silently reversed by reinforcing it, which
    // re-inserts the resource and re-publishes nothing but does re-advertise
    // its description to peers via the listener.
    if av_store::repo::blocks::is_blocked(
        conn,
        av_store::repo::blocks::BlockKind::Resource,
        resource_id,
    )
    .unwrap_or(false)
    {
        return Err(CliError::Validation(format!(
            "resource {resource_id} is blocked; unblock it before reinforcing"
        )));
    }

    if av_store::repo::resources::get(conn, resource_id)
        .ok()
        .flatten()
        .is_none()
    {
        let description = result["description"].as_str().unwrap_or("");
        let location = result["location"].as_str().unwrap_or("");
        let mime_type = result["mime_type"]
            .as_str()
            .unwrap_or("application/octet-stream");

        if description.is_empty() {
            // `relevance_judgments.resource_id` references `resources(id)`, so
            // a relevance row cannot be written for something we do not hold.
            // Without a description there is nothing to propagate either, so
            // fail with the reason rather than letting the insert trip the
            // foreign key and report an opaque database error.
            return Err(CliError::Validation(format!(
                "cannot record relevance for {resource_id}: it is not in the local \
                 index and carries no description to store it from. Add the link first."
            )));
        }

        crate::cmd::propagate::propagate_resource(
            conn,
            provider,
            resource_id,
            location,
            description,
            mime_type,
        )?;
    }

    av_store::repo::relevance::upsert(conn, &normalized, resource_id, 1.0)
        .map_err(|e| CliError::Database(e.to_string()))?;
    Ok(())
}

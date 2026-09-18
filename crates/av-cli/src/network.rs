use crate::cmd::{CliError, CliResult};
use crate::startup::StartupState;
use av_core::types::{MessageKind, NameRecord};
use av_embed::minilm::MiniLmProvider;
use av_index::index::LocalIndex;
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::{NameResponsePayload, ResourceResult, ResponsePayload};
use av_store::repo::peers;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct SearchResultWrapper {
    pub local_results: Vec<av_index::search::SearchResult>,
    pub network_results: Vec<(String, ResponsePayload)>, // (agent_id, payload)
}

#[allow(dead_code)]
pub struct ResolveResultWrapper {
    pub local_results: Vec<av_index::naming::NameResult>,
    pub network_results: Vec<(String, NameResponsePayload)>, // (agent_id, payload)
}

pub fn execute_search(
    cli: &crate::Cli,
    state: &StartupState,
    conn: &Connection,
    provider: &MiniLmProvider,
    query: &str,
    scheme: Option<String>,
    kind: Option<String>,
    mime: Option<String>,
    limit: usize,
) -> CliResult<SearchResultWrapper> {
    // 1. Local search
    let index = LocalIndex::new(conn, provider);
    let mut query_filter = av_index::filter::QueryFilter::default();
    if let Some(s) = scheme.clone() {
        query_filter.scheme = av_index::filter::SchemeFilter::new(vec![s]);
    }
    if let Some(k) = kind {
        let rk = match k.to_lowercase().as_str() {
            "text" => av_core::types::ResourceKind::Text,
            "image" => av_core::types::ResourceKind::Image,
            "audio" => av_core::types::ResourceKind::Audio,
            "file" => av_core::types::ResourceKind::File,
            "pdf" => av_core::types::ResourceKind::Pdf,
            other => av_core::types::ResourceKind::Other(other.to_owned()),
        };
        query_filter.kind = av_index::filter::KindFilter::new(vec![rk]);
    }
    if let Some(m) = mime {
        query_filter.mime = av_index::filter::MimeFilter::new(vec![m]);
    }

    let local_results = index
        .search(query, limit, &query_filter)
        .map_err(|e| CliError::Database(e.to_string()))?;

    let mut network_results = Vec::new();

    // 2. If x0x daemon is running, check direct peers and gossip
    if let Some(ref x0x_cfg) = state.x0x_config {
        let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));
        let dispatcher = MessageDispatcher::new(net_client.clone());

        // Subscribe to all anta-vista gossip topics BEFORE opening the SSE listener
        // so the daemon is already a member of those topics when we start listening.
        dispatcher
            .subscribe_all()
            .map_err(|e| CliError::Network(e.to_string()))?;

        // Start background listeners
        let gossip_rx =
            av_net_x0x::listener::start_listener(x0x_cfg.api_base.clone(), x0x_cfg.token.clone())
                .map_err(|e| CliError::Network(e.to_string()))?;

        let direct_rx = av_net_x0x::direct_listener::start_direct_listener(
            x0x_cfg.api_base.clone(),
            x0x_cfg.token.clone(),
        )
        .map_err(|e| CliError::Network(e.to_string()))?;

        // Determine allowed schemes
        let allowed_schemes = if let Some(s) = scheme {
            vec![s]
        } else {
            vec![]
        };

        // Query peers via direct messaging (parallel connect + send).
        //
        // Candidate set = recent peer_cache entries ∪ x0x-discovered agents,
        // deduped, excluding ourselves.  The x0x daemon can deliver direct
        // messages to remote agents through relay/coordinator routing even when
        // a direct P2P connection isn't established, so we send to every
        // candidate rather than gating on connect_agent success.
        // Register a fresh query_id per candidate up front (so late-arriving,
        // relay-routed responses still match), then fire direct sends from
        // detached threads so the wait loop below overlaps their routing time.
        let mut query_ids: HashSet<String> = HashSet::new();
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(peer_list) = peers::list_recent(conn, 10) {
            let recent: Vec<_> = peer_list
                .into_iter()
                .filter(|p| p.last_seen_at >= now_secs() - 3600)
                .collect();
            candidates.extend(recent.into_iter().map(|p| p.peer_id));
        }
        candidates.extend(net_client.discover_agent_ids(10));
        let self_id = x0x_cfg.agent_id.clone();
        let candidates: Vec<String> = candidates
            .into_iter()
            .filter(|id| id != &self_id)
            .fold(Vec::new(), |mut acc, id| {
                if !acc.contains(&id) {
                    acc.push(id);
                }
                acc
            });

        for peer_id in &candidates {
            let query_id = uuid::Uuid::new_v4().to_string();
            query_ids.insert(query_id.clone());
            let net_client = net_client.clone();
            let to_agent = peer_id.clone();
            let query = query.to_string();
            let allowed_schemes = allowed_schemes.clone();
            let query_timeout = cli.timeout;
            let limit_u32 = limit as u32;
            std::thread::Builder::new()
                .name(format!("av-search-direct-{to_agent}"))
                .spawn(move || {
                    let dispatcher = MessageDispatcher::new(net_client.clone());
                    // Best-effort connect warm-up; the daemon may still route
                    // via relay/gossip inbox even if this errors.
                    let _ = dispatcher.connect_agent(&to_agent);
                    tracing::debug!(
                        peer_id=%to_agent, query_id=%query_id,
                        "sending direct query"
                    );
                    let _ = dispatcher.send_direct_query_with_id(
                        &query_id,
                        &to_agent,
                        &query,
                        limit_u32,
                        query_timeout,
                        allowed_schemes,
                    );
                })
                .ok();
        }

        // Gossip query
        let gossip_query_id = dispatcher
            .publish_query(query, limit as u32, cli.timeout, allowed_schemes)
            .map_err(|e| CliError::Network(e.to_string()))?;
        query_ids.insert(gossip_query_id);

        // Wait loop
        let start_time = Instant::now();
        let timeout_duration = Duration::from_millis(cli.timeout);

        while start_time.elapsed() < timeout_duration {
            // Check direct channel
            if let Ok(Ok(msg)) = direct_rx.recv_timeout(Duration::from_millis(10)) {
                if msg.envelope.kind == MessageKind::Response {
                    if let Ok(resp) =
                        serde_json::from_value::<ResponsePayload>(msg.envelope.payload.clone())
                    {
                        if query_ids.contains(&resp.query_id) {
                            network_results.push((msg.sender.clone(), resp.clone()));
                            let _ = peers::upsert(conn, &msg.sender, serde_json::json!({}), now_secs());
                            if cli.stream {
                                print_progressive_search_results(&msg.sender, &resp.results);
                            }
                        }
                    }
                }
            }

            // Check gossip channel
            if let Ok(Ok(event)) = gossip_rx.recv_timeout(Duration::from_millis(10)) {
                if event.envelope.kind == MessageKind::Response {
                    if let Ok(resp) =
                        serde_json::from_value::<ResponsePayload>(event.envelope.payload.clone())
                    {
                        if query_ids.contains(&resp.query_id) {
                            network_results.push((event.origin.clone(), resp.clone()));
                            let _ = peers::upsert(conn, &event.origin, serde_json::json!({}), now_secs());
                            if cli.stream {
                                print_progressive_search_results(&event.origin, &resp.results);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(SearchResultWrapper {
        local_results,
        network_results,
    })
}

pub fn execute_resolve(
    cli: &crate::Cli,
    state: &StartupState,
    conn: &Connection,
    name: &str,
    record_type: &str,
    scheme: Option<String>,
    limit: usize,
) -> CliResult<ResolveResultWrapper> {
    // 1. Local resolve
    let mock_provider = av_embed::mock::MockEmbeddingProvider::new();
    let index = LocalIndex::new(conn, &mock_provider);
    let mut scheme_filter = av_index::filter::SchemeFilter::default();
    if let Some(s) = scheme {
        scheme_filter = av_index::filter::SchemeFilter::new(vec![s]);
    }

    let local_results = index
        .resolve_name(name, &scheme_filter)
        .map_err(|e| CliError::Database(e.to_string()))?;

    let mut network_results = Vec::new();

    // 2. Network resolve
    if let Some(ref x0x_cfg) = state.x0x_config {
        let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));
        let dispatcher = MessageDispatcher::new(net_client.clone());

        // Subscribe to all anta-vista gossip topics BEFORE opening the SSE listener.
        dispatcher
            .subscribe_all()
            .map_err(|e| CliError::Network(e.to_string()))?;

        // Start background listeners
        let gossip_rx =
            av_net_x0x::listener::start_listener(x0x_cfg.api_base.clone(), x0x_cfg.token.clone())
                .map_err(|e| CliError::Network(e.to_string()))?;

        let direct_rx = av_net_x0x::direct_listener::start_direct_listener(
            x0x_cfg.api_base.clone(),
            x0x_cfg.token.clone(),
        )
        .map_err(|e| CliError::Network(e.to_string()))?;

        // Query direct peers: register a query_id per candidate up front, fire
        // sends from detached threads (overlapping the wait loop below).
        let mut query_ids: HashSet<String> = HashSet::new();
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(peer_list) = peers::list_recent(conn, 10) {
            let recent: Vec<_> = peer_list
                .into_iter()
                .filter(|p| p.last_seen_at >= now_secs() - 3600)
                .collect();
            candidates.extend(recent.into_iter().map(|p| p.peer_id));
        }
        candidates.extend(net_client.discover_agent_ids(10));
        let self_id = x0x_cfg.agent_id.clone();
        let candidates: Vec<String> = candidates
            .into_iter()
            .filter(|id| id != &self_id)
            .fold(Vec::new(), |mut acc, id| {
                if !acc.contains(&id) {
                    acc.push(id);
                }
                acc
            });

        for peer_id in &candidates {
            let query_id = uuid::Uuid::new_v4().to_string();
            query_ids.insert(query_id.clone());
            let net_client = net_client.clone();
            let to_agent = peer_id.clone();
            let name = name.to_string();
            let record_type = record_type.to_string();
            let query_timeout = cli.timeout;
            let limit_u32 = limit as u32;
            std::thread::Builder::new()
                .name(format!("av-resolve-direct-{to_agent}"))
                .spawn(move || {
                    let dispatcher = MessageDispatcher::new(net_client.clone());
                    let _ = dispatcher.connect_agent(&to_agent);
                    tracing::debug!(
                        peer_id=%to_agent, query_id=%query_id,
                        "sending direct name query"
                    );
                    let _ = dispatcher.send_direct_name_query_with_id(
                        &query_id,
                        &to_agent,
                        &name,
                        Some(&record_type),
                        limit_u32,
                        query_timeout,
                    );
                })
                .ok();
        }

        // Gossip query
        let gossip_query_id = dispatcher
            .publish_name_query(name, Some(record_type), limit as u32, cli.timeout)
            .map_err(|e| CliError::Network(e.to_string()))?;
        query_ids.insert(gossip_query_id);

        // Wait loop
        let start_time = Instant::now();
        let timeout_duration = Duration::from_millis(cli.timeout);

        while start_time.elapsed() < timeout_duration {
            // Check direct channel
            if let Ok(Ok(msg)) = direct_rx.recv_timeout(Duration::from_millis(10)) {
                if msg.envelope.kind == MessageKind::NameResponse {
                    if let Ok(resp) =
                        serde_json::from_value::<NameResponsePayload>(msg.envelope.payload.clone())
                    {
                        if query_ids.contains(&resp.query_id) {
                            network_results.push((msg.sender.clone(), resp.clone()));
                            let _ = peers::upsert(conn, &msg.sender, serde_json::json!({}), now_secs());
                            if cli.stream {
                                print_progressive_name_results(&msg.sender, &resp.results);
                            }
                        }
                    }
                }
            }

            // Check gossip channel
            if let Ok(Ok(event)) = gossip_rx.recv_timeout(Duration::from_millis(10)) {
                if event.envelope.kind == MessageKind::NameResponse {
                    if let Ok(resp) = serde_json::from_value::<NameResponsePayload>(
                        event.envelope.payload.clone(),
                    ) {
                        if query_ids.contains(&resp.query_id) {
                            network_results.push((event.origin.clone(), resp.clone()));
                            let _ = peers::upsert(conn, &event.origin, serde_json::json!({}), now_secs());
                            if cli.stream {
                                print_progressive_name_results(&event.origin, &resp.results);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(ResolveResultWrapper {
        local_results,
        network_results,
    })
}

fn print_progressive_search_results(sender: &str, results: &[ResourceResult]) {
    for res in results {
        println!(
            "{} [Peer {}] {} - {} (score: {:.3})",
            console::style("→").cyan(),
            &sender[..8],
            res.location,
            res.description_text,
            res.score
        );
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn print_progressive_name_results(sender: &str, results: &[NameRecord]) {
    for rec in results {
        println!(
            "{} [Peer {}] {} → {} (TTL: {}s)",
            console::style("→").cyan(),
            &sender[..8],
            rec.original_name,
            rec.target,
            rec.ttl_secs
        );
    }
}

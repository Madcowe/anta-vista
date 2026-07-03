use crate::cmd::{CliError, CliResult};
use crate::startup::StartupState;
use av_core::types::{MessageKind, NameRecord};
use av_embed::minilm::MiniLmProvider;
use av_index::index::LocalIndex;
use av_net_x0x::client::{NetworkClient, X0xNetClient};
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::{NameResponsePayload, ResourceResult, ResponsePayload};
use av_store::repo::peers;
use rusqlite::Connection;
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

        // Query direct peers (parallel connect + sequential send)
        if let Ok(peer_list) = peers::list_recent(conn, 10) {
            let recent: Vec<_> = peer_list
                .into_iter()
                .filter(|p| p.last_seen_at >= now_secs() - 3600)
                .collect();

            // Connect to all recent peers in parallel — a thread per peer.
            let connected: Vec<String> = std::thread::scope(|s| {
                let mut handles = Vec::with_capacity(recent.len());
                for peer in &recent {
                    handles.push(s.spawn(|| {
                        net_client
                            .connect_agent(&peer.peer_id)
                            .ok()
                            .map(|_| peer.peer_id.clone())
                    }));
                }
                handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
            });

            // Send direct queries sequentially — these should be fast when
            // the daemon already has a connection to the peer.
            for peer_id in &connected {
                let _ = dispatcher.send_direct_query(
                    peer_id,
                    query,
                    limit as u32,
                    cli.timeout,
                    allowed_schemes.clone(),
                );
            }
        }

        // Gossip query
        let query_id = dispatcher
            .publish_query(query, limit as u32, cli.timeout, allowed_schemes)
            .map_err(|e| CliError::Network(e.to_string()))?;

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
                        if resp.query_id == query_id {
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
                        if resp.query_id == query_id {
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

        // Query direct peers (parallel connect + sequential send)
        if let Ok(peer_list) = peers::list_recent(conn, 10) {
            let recent: Vec<_> = peer_list
                .into_iter()
                .filter(|p| p.last_seen_at >= now_secs() - 3600)
                .collect();

            // Connect to all recent peers in parallel — a thread per peer.
            let connected: Vec<String> = std::thread::scope(|s| {
                let mut handles = Vec::with_capacity(recent.len());
                for peer in &recent {
                    handles.push(s.spawn(|| {
                        net_client
                            .connect_agent(&peer.peer_id)
                            .ok()
                            .map(|_| peer.peer_id.clone())
                    }));
                }
                handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
            });

            // Send direct queries sequentially.
            for peer_id in &connected {
                let _ = dispatcher.send_direct_name_query(
                    peer_id,
                    name,
                    Some(record_type),
                    limit as u32,
                    cli.timeout,
                );
            }
        }

        // Gossip query
        let query_id = dispatcher
            .publish_name_query(name, Some(record_type), limit as u32, cli.timeout)
            .map_err(|e| CliError::Network(e.to_string()))?;

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
                        if resp.query_id == query_id {
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
                        if resp.query_id == query_id {
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

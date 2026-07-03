use std::sync::Arc;
use std::time::Duration;

use crate::cmd::{CliError, CliResult};
use crate::startup::StartupState;
use av_core::types::MessageKind;
use av_embed::mock::MockEmbeddingProvider;
use av_embed::minilm::MiniLmProvider;
use av_index::index::LocalIndex;
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::{QueryPayload, NameQueryPayload, ResourceResult};
use av_store::repo::peers;

/// Run the anta-vista peer listener daemon.
///
/// This is the missing half of the gossip protocol: it subscribes to all
/// anta-vista topics, listens on the SSE stream, and responds to incoming
/// NameQuery and Query messages with records from the local database.
///
/// Without this running on a machine, other peers can broadcast queries all
/// day but will never get a response from that machine's local records.
///
/// Run this in the background on every machine that wants to share its index:
///
///   av listen &
///   av listen --timeout 0   # run forever (default)
///
pub fn run(state: StartupState, run_for_secs: Option<u64>) -> CliResult<()> {
    let x0x_cfg = state.x0x_config.ok_or_else(|| {
        CliError::Daemon("x0x daemon is not running. Start it with 'x0x start'.".to_string())
    })?;

    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = open_db(&db_path)?;

    // Load the embedding model once at startup — it's needed to respond to search queries.
    // Name queries don't need it so we tolerate model load failure gracefully.
    let embed_provider = MiniLmProvider::new().ok();

    let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));
    let dispatcher = MessageDispatcher::new(net_client.clone());

    // Subscribe to every anta-vista topic so the daemon routes messages to us.
    dispatcher
        .subscribe_all()
        .map_err(|e| CliError::Network(format!("subscribe_all: {e}")))?;

    // Open the SSE listener for gossip messages.
    let gossip_rx =
        av_net_x0x::listener::start_listener(x0x_cfg.api_base.clone(), x0x_cfg.token.clone())
            .map_err(|e| CliError::Network(format!("gossip listener: {e}")))?;

    // Open the SSE listener for direct messages.
    let direct_rx = av_net_x0x::direct_listener::start_direct_listener(
        x0x_cfg.api_base.clone(),
        x0x_cfg.token.clone(),
    )
    .map_err(|e| CliError::Network(format!("direct listener: {e}")))?;

    let deadline = run_for_secs.map(|s| {
        std::time::Instant::now() + Duration::from_secs(s)
    });

    println!(
        "av listen: active on agent {} — responding to network queries",
        &x0x_cfg.agent_id[..12.min(x0x_cfg.agent_id.len())]
    );
    if let Some(secs) = run_for_secs {
        println!("  (will stop after {secs}s)");
    } else {
        println!("  Press Ctrl-C to stop.");
    }

    // Register our PID so other av commands and `av status` can detect us.
    crate::listener::write_pid(std::process::id());

    loop {
        // Honour optional runtime limit.
        if let Some(dl) = deadline {
            if std::time::Instant::now() >= dl {
                break;
            }
        }

        // --- gossip channel ---
        if let Ok(Ok(event)) = gossip_rx.recv_timeout(Duration::from_millis(20)) {
            // Ignore our own messages.
            if event.origin == x0x_cfg.agent_id {
                continue;
            }

            let _ = peers::upsert(&conn, &event.origin, serde_json::json!({}), now_secs());

            match event.envelope.kind {
                MessageKind::Query => {
                    if let Ok(q) =
                        serde_json::from_value::<QueryPayload>(event.envelope.payload.clone())
                    {
                        tracing::debug!(query_id = %q.query_id, from = %event.origin, "received gossip Query");
                        respond_to_search_query(&dispatcher, &db_path, &embed_provider, &q)?;
                    }
                }
                MessageKind::NameQuery => {
                    if let Ok(q) =
                        serde_json::from_value::<NameQueryPayload>(event.envelope.payload.clone())
                    {
                        tracing::debug!(query_id = %q.query_id, name = %q.name, from = %event.origin, "received gossip NameQuery");
                        respond_to_name_query(&dispatcher, &db_path, &q)?;
                    }
                }
                _ => {}
            }
        }

        // --- direct channel ---
        if let Ok(Ok(msg)) = direct_rx.recv_timeout(Duration::from_millis(20)) {
            if msg.sender == x0x_cfg.agent_id {
                continue;
            }

            let _ = peers::upsert(&conn, &msg.sender, serde_json::json!({}), now_secs());

            match msg.envelope.kind {
                MessageKind::Query => {
                    if let Ok(q) =
                        serde_json::from_value::<QueryPayload>(msg.envelope.payload.clone())
                    {
                        tracing::debug!(query_id = %q.query_id, from = %msg.sender, "received direct Query");
                        respond_to_search_query_direct(&dispatcher, &db_path, &embed_provider, &q, &msg.sender)?;
                    }
                }
                MessageKind::NameQuery => {
                    if let Ok(q) =
                        serde_json::from_value::<NameQueryPayload>(msg.envelope.payload.clone())
                    {
                        tracing::debug!(query_id = %q.query_id, name = %q.name, from = %msg.sender, "received direct NameQuery");
                        respond_to_name_query_direct(&dispatcher, &db_path, &q, &msg.sender)?;
                    }
                }
                _ => {}
            }
        }
    }

    // Clean up PID file on graceful exit (run_for timeout).
    crate::listener::clear_pid();
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn open_db(db_path: &std::path::Path) -> CliResult<rusqlite::Connection> {
    av_store::open(db_path).map_err(|e| CliError::Database(e.to_string()))
}

fn respond_to_search_query(
    dispatcher: &MessageDispatcher,
    db_path: &std::path::Path,
    embed: &Option<MiniLmProvider>,
    q: &QueryPayload,
) -> CliResult<()> {
    let conn = open_db(db_path)?;
    let mock = MockEmbeddingProvider::new();
    let provider: &dyn av_embed::provider::EmbeddingProvider = match embed {
        Some(p) => p,
        None => &mock,
    };
    let index = LocalIndex::new(&conn, provider);

    let mut filter = av_index::filter::QueryFilter::default();
    if !q.allowed_schemes.is_empty() {
        filter.scheme = av_index::filter::SchemeFilter::new(q.allowed_schemes.clone());
    }

    let results = index
        .search(&q.query_text, q.max_results as usize, &filter)
        .unwrap_or_default();

    if results.is_empty() {
        return Ok(());
    }

    let network_results: Vec<ResourceResult> = results
        .into_iter()
        .map(|r| ResourceResult {
            resource_id: r.resource.id.clone(),
            description_text: r.resource.description_text.clone(),
            location: r.resource.location.clone(),
            location_scheme: r.resource.location_scheme.clone(),
            mime_type: r.resource.mime_type.clone(),
            score: r.score,
        })
        .collect();

    dispatcher
        .publish_response(&q.query_id, network_results)
        .map_err(|e| CliError::Network(e.to_string()))
}

fn respond_to_search_query_direct(
    dispatcher: &MessageDispatcher,
    db_path: &std::path::Path,
    embed: &Option<MiniLmProvider>,
    q: &QueryPayload,
    to_agent_id: &str,
) -> CliResult<()> {
    let conn = open_db(db_path)?;
    let mock = MockEmbeddingProvider::new();
    let provider: &dyn av_embed::provider::EmbeddingProvider = match embed {
        Some(p) => p,
        None => &mock,
    };
    let index = LocalIndex::new(&conn, provider);

    let mut filter = av_index::filter::QueryFilter::default();
    if !q.allowed_schemes.is_empty() {
        filter.scheme = av_index::filter::SchemeFilter::new(q.allowed_schemes.clone());
    }

    let results = index
        .search(&q.query_text, q.max_results as usize, &filter)
        .unwrap_or_default();

    if results.is_empty() {
        return Ok(());
    }

    let network_results: Vec<ResourceResult> = results
        .into_iter()
        .map(|r| ResourceResult {
            resource_id: r.resource.id.clone(),
            description_text: r.resource.description_text.clone(),
            location: r.resource.location.clone(),
            location_scheme: r.resource.location_scheme.clone(),
            mime_type: r.resource.mime_type.clone(),
            score: r.score,
        })
        .collect();

    dispatcher
        .send_direct_response(to_agent_id, &q.query_id, network_results)
        .map_err(|e| CliError::Network(e.to_string()))
}

fn respond_to_name_query(
    dispatcher: &MessageDispatcher,
    db_path: &std::path::Path,
    q: &NameQueryPayload,
) -> CliResult<()> {
    let conn = open_db(db_path)?;

    let scheme_filter = av_index::filter::SchemeFilter::default();
    let now = now_secs();

    let results = av_index::naming::lookup_name(&conn, &q.name, &scheme_filter, now)
        .unwrap_or_default();

    if results.is_empty() {
        return Ok(());
    }

    let records: Vec<av_core::types::NameRecord> =
        results.into_iter().take(q.max_results as usize).map(|r| r.record).collect();

    dispatcher
        .publish_name_response(&q.query_id, &q.normalized_name, records)
        .map_err(|e| CliError::Network(e.to_string()))
}

fn respond_to_name_query_direct(
    dispatcher: &MessageDispatcher,
    db_path: &std::path::Path,
    q: &NameQueryPayload,
    to_agent_id: &str,
) -> CliResult<()> {
    let conn = open_db(db_path)?;

    let scheme_filter = av_index::filter::SchemeFilter::default();
    let now = now_secs();

    let results = av_index::naming::lookup_name(&conn, &q.name, &scheme_filter, now)
        .unwrap_or_default();

    if results.is_empty() {
        return Ok(());
    }

    let records: Vec<av_core::types::NameRecord> =
        results.into_iter().take(q.max_results as usize).map(|r| r.record).collect();

    dispatcher
        .send_direct_name_response(to_agent_id, &q.query_id, &q.normalized_name, records)
        .map_err(|e| CliError::Network(e.to_string()))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

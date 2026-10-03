use std::sync::Arc;
use std::time::Duration;

use crate::cmd::{CliError, CliResult};
use crate::startup::StartupState;
use av_core::types::MessageKind;
use av_embed::minilm::MiniLmProvider;
use av_embed::mock::MockEmbeddingProvider;
use av_index::index::LocalIndex;
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::{NameQueryPayload, QueryPayload, ResourceResult};
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

    let deadline = run_for_secs.map(|s| std::time::Instant::now() + Duration::from_secs(s));

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
    // Ensure the PID file is removed on every exit path (timeout, Ctrl-C, or
    // an error bubbling up from the loop) so we never leave a stale PID behind.
    let _pid_guard = crate::listener::PidFileGuard::new();

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
                        respond_to_search_query_direct(
                            &dispatcher,
                            &db_path,
                            &embed_provider,
                            &q,
                            &msg.sender,
                        )?;
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

    // Clean up happens automatically via the PID guard on any exit path.
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn open_db(db_path: &std::path::Path) -> CliResult<rusqlite::Connection> {
    av_store::open(db_path).map_err(|e| CliError::Database(e.to_string()))
}

/// Map index hits to wire results, dropping blocked resources.
///
/// `av_index::search::search_top_k` already filters these, so in practice the
/// blocked branch finds nothing. It exists because this is the responder: the
/// promise that a blocked resource never leaves this node should not depend on
/// a filter inside a different crate staying intact, and because it is the only
/// place that can *report* that something was withheld. Same reasoning as the
/// loopback host-header guard in the web server — keep it in front of the send.
///
/// Returns the publishable results and the ids that were withheld.
fn to_publishable_resources(
    conn: &rusqlite::Connection,
    hits: Vec<av_index::search::SearchResult>,
) -> (Vec<ResourceResult>, Vec<String>) {
    // Fails closed, like the two `av-index` filters. This is the last gate
    // before the send, so defaulting to "nothing blocked" here would publish
    // whatever the previous layer failed to filter.
    let blocked = match av_store::repo::blocks::blocked_ids(
        conn,
        av_store::repo::blocks::BlockKind::Resource,
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("av listen: refusing to serve results, could not read the block list: {e}");
            // Withhold everything rather than guess.
            return (
                Vec::new(),
                hits.into_iter().map(|h| h.resource.id).collect(),
            );
        }
    };

    let mut kept = Vec::with_capacity(hits.len());
    let mut withheld = Vec::new();
    for r in hits {
        if blocked.contains(&r.resource.id) {
            withheld.push(r.resource.id);
            continue;
        }
        kept.push(ResourceResult {
            resource_id: r.resource.id,
            description_text: r.resource.description_text,
            location: r.resource.location,
            location_scheme: r.resource.location_scheme,
            mime_type: r.resource.mime_type,
            score: r.score,
        });
    }
    (kept, withheld)
}

/// Drop blocked name records from a response about to be published, for the
/// same reason as [`to_publishable_resources`].
fn to_publishable_names(
    conn: &rusqlite::Connection,
    hits: Vec<av_index::naming::NameResult>,
) -> (Vec<av_core::types::NameRecord>, Vec<String>) {
    // Recorded up front so the fail-closed path below can name everything it is
    // withholding without having to keep the moved-out `hits` around.
    let all_ids: Vec<String> = hits.iter().map(|h| h.record.record_id.clone()).collect();
    let refuse = |reason: &str| -> (Vec<av_core::types::NameRecord>, Vec<String>) {
        eprintln!("av listen: refusing to serve names, {reason}");
        (Vec::new(), all_ids.clone())
    };

    // The same policy `lookup_name` applied, via the same loader, so this gate
    // and the lookup the responder already did cannot disagree about what is
    // blocked. It covers all three kinds: a blocked claim by record id, a claim
    // whose target resource is blocked, and a claim a blocked peer published.
    //
    // Fails closed for the same reason as `to_publishable_resources`: an
    // unreadable block list is not evidence that nothing is blocked.
    let policy = match av_index::naming::NameBlockPolicy::load(conn) {
        Ok(p) => p,
        Err(e) => return refuse(&format!("could not read the block list: {e}")),
    };

    let candidates: Vec<av_core::types::NameRecord> = hits.into_iter().map(|h| h.record).collect();
    let kept = match av_index::naming::retain_unblocked(conn, candidates, &policy) {
        Ok(k) => k,
        Err(e) => return refuse(&format!("could not apply block policy: {e}")),
    };

    let kept_ids: std::collections::HashSet<String> =
        kept.iter().map(|r| r.record_id.clone()).collect();
    let withheld: Vec<String> = all_ids
        .into_iter()
        .filter(|id| !kept_ids.contains(id))
        .collect();

    (kept, withheld)
}

/// Note that a response was trimmed by local block policy.
fn note_withheld(query_id: &str, kind: &str, withheld: &[String]) {
    if !withheld.is_empty() {
        tracing::debug!(
            query_id = %crate::output::abbrev(query_id, 8),
            count = withheld.len(),
            "withheld locally blocked {kind} from response"
        );
    }
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

    let (network_results, withheld) = to_publishable_resources(&conn, results);
    note_withheld(&q.query_id, "resources", &withheld);

    if network_results.is_empty() {
        return Ok(());
    }

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

    let (network_results, withheld) = to_publishable_resources(&conn, results);
    note_withheld(&q.query_id, "resources", &withheld);

    if network_results.is_empty() {
        return Ok(());
    }

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

    let results =
        av_index::naming::lookup_name(&conn, &q.name, &scheme_filter, now).unwrap_or_default();

    // Truncate before filtering, so an unblocked query answers with exactly the
    // same number of records it always did.
    let capped = results.into_iter().take(q.max_results as usize).collect();
    let (records, withheld) = to_publishable_names(&conn, capped);
    note_withheld(&q.query_id, "name records", &withheld);

    if records.is_empty() {
        return Ok(());
    }

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

    let results =
        av_index::naming::lookup_name(&conn, &q.name, &scheme_filter, now).unwrap_or_default();

    let capped = results.into_iter().take(q.max_results as usize).collect();
    let (records, withheld) = to_publishable_names(&conn, capped);
    note_withheld(&q.query_id, "name records", &withheld);

    if records.is_empty() {
        return Ok(());
    }

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

// --- Block policy on the responder path ------------------------------------
//
// `to_publishable_resources` / `to_publishable_names` are the last gate before
// a result leaves this node, so they are tested directly. The search and naming
// layers also filter blocked subjects, and these tests deliberately block a
// result that *did* come back from that layer — a resource blocked with
// `--keep-local` still has its embedding, so the only thing standing between
// it and a peer is this function.

#[cfg(test)]
mod tests {
    use super::*;
    use av_index::naming::NameResult;
    use av_index::search::SearchResult;
    use av_store::repo::blocks::{self, BlockKind};

    fn stub_resource(id: &str) -> SearchResult {
        SearchResult {
            resource: av_core::types::ResourceDescriptor {
                id: id.to_string(),
                kind: av_core::types::ResourceKind::Text,
                location: format!("autonomi://{id}"),
                location_scheme: Some("autonomi".into()),
                location_canonical: Some(format!("autonomi://{id}")),
                mime_type: "text/plain".into(),
                filename: None,
                metadata_json: serde_json::json!({}),
                description_text: "a description".into(),
                created_at: 1_700_000_000,
            },
            semantic_score: 1.0,
            agreement_score: 0.0,
            feedback_score: 0.0,
            trust_score: 0.0,
            relevance_score: 0.0,
            score: 1.0,
        }
    }

    fn block(conn: &rusqlite::Connection, kind: BlockKind, id: &str) {
        blocks::add(conn, kind, id, Some("test"), 1_700_000_000, "test").expect("add block");
    }

    #[test]
    fn blocked_resource_is_never_published_to_a_peer() {
        let conn = av_store::open_in_memory().expect("open db");
        let hits = vec![stub_resource("aa"), stub_resource("bb")];
        block(&conn, BlockKind::Resource, "bb");

        let (publishable, withheld) = to_publishable_resources(&conn, hits);
        let ids: Vec<&str> = publishable.iter().map(|r| r.resource_id.as_str()).collect();

        assert_eq!(
            ids,
            vec!["aa"],
            "the blocked resource must not be published"
        );
        assert_eq!(
            withheld,
            vec!["bb".to_string()],
            "and the responder says so"
        );
    }

    #[test]
    fn nothing_is_published_when_every_hit_is_blocked() {
        let conn = av_store::open_in_memory().expect("open db");
        let hits = vec![stub_resource("aa"), stub_resource("bb")];
        block(&conn, BlockKind::Resource, "aa");
        block(&conn, BlockKind::Resource, "bb");

        let (publishable, withheld) = to_publishable_resources(&conn, hits);
        assert!(publishable.is_empty());
        assert_eq!(withheld.len(), 2);
    }

    #[test]
    fn unblocked_resources_pass_through_untouched() {
        let conn = av_store::open_in_memory().expect("open db");
        let hits = vec![stub_resource("aa")];
        let (publishable, withheld) = to_publishable_resources(&conn, hits);
        assert_eq!(publishable.len(), 1);
        assert!(withheld.is_empty());
    }

    #[test]
    fn a_blocked_name_record_is_never_published_to_a_peer() {
        let conn = av_store::open_in_memory().expect("open db");
        let make = |id: &str| NameResult {
            record: av_core::types::NameRecord {
                schema_version: 1,
                record_id: id.to_string(),
                normalized_name: "contested".into(),
                original_name: "Contested".into(),
                record_type: av_core::types::NameRecordType::Uri,
                target: "autonomi://aa".into(),
                target_scheme: Some("autonomi".into()),
                target_canonical: Some("autonomi://aa".into()),
                ttl_secs: 3600,
                by_agent_id: "agent-abc".into(),
                origin_agent_id: None,
                timestamp: 1_700_000_000,
                signature: vec![],
            },
            score: 1.0,
            trust_score: 1.0,
            agreement_score: 0.0,
            recency_score: 0.0,
            ttl_valid: true,
        };
        block(&conn, BlockKind::Name, "nr-bad");

        let (publishable, withheld) =
            to_publishable_names(&conn, vec![make("nr-good"), make("nr-bad")]);
        let ids: Vec<&str> = publishable.iter().map(|r| r.record_id.as_str()).collect();

        assert_eq!(
            ids,
            vec!["nr-good"],
            "only the blocked name claim is withheld"
        );
        assert_eq!(withheld, vec!["nr-bad".to_string()]);
    }

    #[test]
    fn a_resource_block_does_not_withhold_a_name_record() {
        // The three kinds are independent policies. Sharing a string between a
        // resource id and a name record id must not couple them, so a resource
        // block is never allowed to suppress a name response.
        let conn = av_store::open_in_memory().expect("open db");
        let make = |id: &str| NameResult {
            record: av_core::types::NameRecord {
                schema_version: 1,
                record_id: id.to_string(),
                normalized_name: "shared".into(),
                original_name: "Shared".into(),
                record_type: av_core::types::NameRecordType::Uri,
                target: "autonomi://aa".into(),
                target_scheme: Some("autonomi".into()),
                target_canonical: Some("autonomi://aa".into()),
                ttl_secs: 3600,
                by_agent_id: "agent-abc".into(),
                origin_agent_id: None,
                timestamp: 1_700_000_000,
                signature: vec![],
            },
            score: 1.0,
            trust_score: 1.0,
            agreement_score: 0.0,
            recency_score: 0.0,
            ttl_valid: true,
        };
        block(&conn, BlockKind::Resource, "shared");

        let (publishable, withheld) = to_publishable_names(&conn, vec![make("shared")]);
        assert_eq!(
            publishable.len(),
            1,
            "a resource block must not touch name records"
        );
        assert!(withheld.is_empty());
    }

    fn stub_name_from(id: &str, claimed_by: &str, origin: Option<&str>) -> NameResult {
        NameResult {
            record: av_core::types::NameRecord {
                schema_version: 1,
                record_id: id.to_string(),
                normalized_name: "svc".into(),
                original_name: "Svc".into(),
                record_type: av_core::types::NameRecordType::Uri,
                target: "autonomi://aa".into(),
                target_scheme: Some("autonomi".into()),
                target_canonical: Some("autonomi://aa".into()),
                ttl_secs: 3600,
                by_agent_id: claimed_by.to_string(),
                origin_agent_id: origin.map(str::to_string),
                timestamp: 1_700_000_000,
                signature: vec![],
            },
            score: 1.0,
            trust_score: 1.0,
            agreement_score: 0.0,
            recency_score: 0.0,
            ttl_valid: true,
        }
    }

    fn agent_id(seed: char) -> String {
        seed.to_string().repeat(64)
    }

    #[test]
    fn a_blocked_peers_names_are_never_published_to_a_peer() {
        let conn = av_store::open_in_memory().expect("open db");
        let hits = vec![
            stub_name_from("nr-bad", &agent_id('b'), Some(&agent_id('b'))),
            stub_name_from("nr-good", &agent_id('g'), Some(&agent_id('g'))),
        ];
        block(&conn, BlockKind::Agent, &agent_id('b'));

        let (publishable, withheld) = to_publishable_names(&conn, hits);
        let ids: Vec<&str> = publishable.iter().map(|r| r.record_id.as_str()).collect();

        assert_eq!(
            ids,
            vec!["nr-good"],
            "a blocked peer's claim must not be re-served to another peer"
        );
        assert_eq!(
            withheld,
            vec!["nr-bad".to_string()],
            "the withheld claim is reported, not silently dropped"
        );
    }

    #[test]
    fn publishing_names_fails_closed_when_the_block_list_is_unreadable() {
        let conn = av_store::open_in_memory().expect("open db");
        let hits = vec![stub_name_from("nr-1", "someone", None)];
        // Sanity: the same input is served while the database is intact.
        assert_eq!(to_publishable_names(&conn, hits.clone()).0.len(), 1);

        conn.execute_batch("DROP TABLE blocks")
            .expect("drop blocks");

        let (publishable, withheld) = to_publishable_names(&conn, hits);
        assert!(
            publishable.is_empty(),
            "a failed block read must not be read as 'nothing is blocked'"
        );
        assert_eq!(
            withheld.len(),
            1,
            "everything is withheld, and the withheld list says so"
        );
    }
}

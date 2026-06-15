use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use std::thread;

use av_core::types::{NameRecord, NameRecordType, normalize_name, MessageKind};
use av_net_x0x::{
    client::{X0xConfig, X0xNetClient},
    dispatcher::MessageDispatcher,
    listener::{start_listener, IncomingEvent},
    direct_listener::{start_direct_listener, DirectMessage},
    payloads::{QueryPayload, NameQueryPayload, ResourceResult},
};
use av_embed::{EmbeddingProvider, MockEmbeddingProvider, MiniLmProvider, provider::profile_id};
use av_index::{LocalIndex, QueryFilter, SchemeFilter};
use rusqlite::Connection;

use crate::cli::Cli;

pub fn run_seed(args: Cli, config: X0xConfig) {
    tracing::info!("Initializing seed database and index...");

    // 1. Open database
    let conn = av_store::open_in_memory().expect("Failed to open SQLite database");

    // 2. Initialize embedding provider
    let provider: Arc<dyn EmbeddingProvider> = if args.real_model {
        tracing::info!("Loading real MiniLM model (this might download model weights)...");
        Arc::new(MiniLmProvider::new().expect("Failed to load MiniLM model"))
    } else {
        tracing::info!("Using deterministic MockEmbeddingProvider");
        Arc::new(MockEmbeddingProvider::new())
    };

    // 3. Populate sample resources
    init_seed_db(&conn, provider.as_ref(), &config.agent_id);
    let shared_conn = Arc::new(Mutex::new(conn));

    // 4. Connect dispatcher
    let client = Arc::new(X0xNetClient::new(config.clone()));
    let dispatcher = Arc::new(MessageDispatcher::new(client));

    // 5. Subscribe to gossip topics
    dispatcher.subscribe_all().expect("Failed to subscribe to gossip topics");
    tracing::info!("Subscribed to gossip topics.");

    announce_identity(&config);

    // 6. Broadcast our NameClaims
    broadcast_claims(&dispatcher, &shared_conn);

    // Spawn a thread to periodically broadcast name claims so the probe can autodetect us
    let dispatcher_clone = dispatcher.clone();
    let conn_clone = shared_conn.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(10));
        broadcast_claims(&dispatcher_clone, &conn_clone);
    });

    // 7. Start listeners
    let gossip_rx = start_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start gossip SSE listener");
    let direct_rx = start_direct_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start direct message SSE listener");

    tracing::info!("Seed node running. Listening for queries...");

    // Setup channels/threads for message processing
    let handler_dispatcher = dispatcher.clone();
    let handler_conn = shared_conn.clone();
    let handler_provider = provider.clone();

    // Spawn Gossip Processor Thread
    let gossip_handle = thread::spawn(move || {
        for msg in gossip_rx {
            if let Ok(event) = msg {
                if let Err(e) = handle_gossip_event(event, &handler_dispatcher, &handler_conn, handler_provider.as_ref()) {
                    tracing::error!("Error handling gossip event: {:?}", e);
                }
            }
        }
    });

    // Spawn Direct Message Processor Thread
    let direct_dispatcher = dispatcher.clone();
    let direct_conn = shared_conn.clone();
    let direct_provider = provider.clone();
    let direct_handle = thread::spawn(move || {
        for msg in direct_rx {
            if let Ok(event) = msg {
                if let Err(e) = handle_direct_message(event, &direct_dispatcher, &direct_conn, direct_provider.as_ref()) {
                    tracing::error!("Error handling direct message: {:?}", e);
                }
            }
        }
    });

    // Keep main thread alive or wait for Ctrl+C
    let running = Arc::new(Mutex::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        tracing::info!("Received shutdown signal. Exiting seed...");
        let mut running_guard = r.lock().unwrap();
        *running_guard = false;
    }).expect("Error setting Ctrl-C handler");

    while *running.lock().unwrap() {
        thread::sleep(Duration::from_millis(500));
    }

    // Wait for threads to close (or force exit since we are terminating)
    drop(gossip_handle);
    drop(direct_handle);
}

fn init_seed_db(conn: &Connection, provider: &dyn EmbeddingProvider, agent_id: &str) {
    let pid = profile_id(provider.profile());
    av_store::repo::embeddings::insert_profile(conn, &pid, provider.profile()).expect("insert profile");

    let samples: &[(&str, &[u8], &str)] = &[
        (
            "fish.jpg",
            &[
                0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
                0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
            ],
            "https://example.com/fish.jpg",
        ),
        (
            "rust_guide.txt",
            b"The Rust programming language provides memory safety without garbage collection.",
            "https://example.com/rust_guide.txt",
        ),
        (
            "cheesy.mp3",
            &[0x49, 0x44, 0x33, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "https://example.com/cheesy.mp3",
        ),
        (
            "autonomi_index.txt",
            b"Autonomi network distributed storage system index file.",
            "ant://56fd2c26139fa0e078838d963c6b14054ad913f2fdff0d5b88039292dbe41f03",
        ),
        (
            "report.pdf",
            b"%PDF-1.4\n1 0 obj\n<</Type /Catalog>>\nendobj\n%%EOF",
            "https://example.com/report.pdf",
        ),
    ];

    for (filename, bytes, location) in samples {
        let resource = av_ingest::ingest_bytes(bytes, Some(filename), location).expect("ingest failed");
        av_store::repo::resources::insert(conn, &resource).expect("store resource");
        let embedding = provider.embed_resource(&resource.description_text, &resource.id).expect("embed failed");
        av_store::repo::embeddings::insert(conn, &embedding).expect("store embedding");
    }

    // Name claim 1: seed.av
    let seed_record = NameRecord {
        schema_version: 1,
        record_id: "seed-name-record".to_string(),
        normalized_name: normalize_name("seed.av"),
        original_name: "seed.av".to_string(),
        record_type: NameRecordType::Uri,
        target: format!("ant://{}", agent_id),
        target_scheme: Some("ant".to_string()),
        target_canonical: Some(format!("ant://{}", agent_id)),
        ttl_secs: 3600,
        by_agent_id: agent_id.to_string(),
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
        signature: vec![],
    };
    av_store::repo::names::insert(conn, &seed_record).expect("store seed name");

    // Name claim 2: alias.av (to test scheme alias normalization autonomi:// -> ant://)
    let alias_record = NameRecord {
        schema_version: 1,
        record_id: "alias-name-record".to_string(),
        normalized_name: normalize_name("alias.av"),
        original_name: "alias.av".to_string(),
        record_type: NameRecordType::Uri,
        target: format!("autonomi://{}", agent_id),
        target_scheme: Some("autonomi".to_string()),
        target_canonical: Some(format!("ant://{}", agent_id)),
        ttl_secs: 3600,
        by_agent_id: agent_id.to_string(),
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
        signature: vec![],
    };
    av_store::repo::names::insert(conn, &alias_record).expect("store alias name");
}

fn broadcast_claims(dispatcher: &MessageDispatcher, conn: &Mutex<Connection>) {
    let conn_guard = conn.lock().unwrap();
    if let Ok(records) = av_store::repo::names::get_by_normalized_name(&conn_guard, "seed.av") {
        for record in records {
            tracing::debug!("Broadcasting NameClaim for seed.av");
            let _ = dispatcher.publish_name_claim(record);
        }
    }
    if let Ok(records) = av_store::repo::names::get_by_normalized_name(&conn_guard, "alias.av") {
        for record in records {
            tracing::debug!("Broadcasting NameClaim for alias.av");
            let _ = dispatcher.publish_name_claim(record);
        }
    }
}

fn announce_identity(config: &X0xConfig) {
    let result = ureq::post(&format!("{}/announce", config.api_base))
        .set("Authorization", &format!("Bearer {}", config.token))
        .send_json(serde_json::json!({}));

    match result {
        Ok(_) => tracing::info!("Announced seed identity to x0x discovery."),
        Err(e) => tracing::warn!("Could not announce seed identity to x0x discovery: {}", e),
    }
}

fn handle_gossip_event(
    event: IncomingEvent,
    dispatcher: &MessageDispatcher,
    conn: &Mutex<Connection>,
    provider: &dyn EmbeddingProvider,
) -> Result<(), Box<dyn std::error::Error>> {
    // Deduplicate: x0x fans out one published message to every active
    // subscription on this daemon.  validate_incoming checks the message_id
    // against the dispatcher's dedup cache and returns Err(Duplicate) on
    // repeat deliveries, so we only process each query once.
    if let Err(e) = dispatcher.validate_incoming(&event.envelope, event.raw_size) {
        tracing::debug!("Dropping gossip event ({}): {:?}", event.envelope.message_id, e);
        return Ok(());
    }

    match event.envelope.kind {
        MessageKind::Query => {
            let payload: QueryPayload = serde_json::from_value(event.envelope.payload)?;
            tracing::info!("Received gossip search query: \"{}\"", payload.query_text);

            let conn_guard = conn.lock().unwrap();
            let index = LocalIndex::new(&conn_guard, provider);

            let mut filter = QueryFilter::default();
            if !payload.allowed_schemes.is_empty() {
                filter.scheme = SchemeFilter::new(payload.allowed_schemes.iter().map(|s| s.as_str()));
            }

            let search_results = index.search(&payload.query_text, payload.max_results as usize, &filter)?;
            let results: Vec<ResourceResult> = search_results
                .into_iter()
                .map(|r| ResourceResult {
                    resource_id: r.resource.id,
                    description_text: r.resource.description_text,
                    location: r.resource.location,
                    location_scheme: r.resource.location_scheme,
                    mime_type: r.resource.mime_type,
                    score: r.score,
                })
                .collect();

            tracing::info!("Found {} matches. Publishing gossip response...", results.len());
            dispatcher.publish_response(&payload.query_id, results)?;
        }
        MessageKind::NameQuery => {
            let payload: NameQueryPayload = serde_json::from_value(event.envelope.payload)?;
            tracing::info!("Received gossip name query for: \"{}\"", payload.name);

            let conn_guard = conn.lock().unwrap();
            let index = LocalIndex::new(&conn_guard, provider);

            let results = index.resolve_name(&payload.normalized_name, &SchemeFilter::default())?;
            let name_records: Vec<NameRecord> = results.into_iter().map(|r| r.record).collect();

            tracing::info!("Found {} matching name records. Publishing gossip name response...", name_records.len());
            dispatcher.publish_name_response(&payload.query_id, &payload.normalized_name, name_records.clone())?;

            // Re-broadcast NameClaim for any matching records so the probe can
            // observe a fresh NameClaim in response to its query (N1 test).
            for record in name_records {
                let _ = dispatcher.publish_name_claim(record);
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_direct_message(
    msg: DirectMessage,
    dispatcher: &MessageDispatcher,
    conn: &Mutex<Connection>,
    provider: &dyn EmbeddingProvider,
) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure we have a bidirectional QUIC link back to the sender so we can
    // reply via /direct/send.  connect_agent is idempotent if the link is
    // already established.
    if let Err(e) = dispatcher.connect_agent(&msg.sender) {
        tracing::warn!(
            "Could not establish return QUIC link to {}: {:?}",
            msg.sender,
            e
        );
    }

    match msg.envelope.kind {
        MessageKind::Query => {
            let payload: QueryPayload = serde_json::from_value(msg.envelope.payload)?;
            tracing::info!("Received direct search query from {}: \"{}\"", msg.sender, payload.query_text);

            let conn_guard = conn.lock().unwrap();
            let index = LocalIndex::new(&conn_guard, provider);

            let mut filter = QueryFilter::default();
            if !payload.allowed_schemes.is_empty() {
                filter.scheme = SchemeFilter::new(payload.allowed_schemes.iter().map(|s| s.as_str()));
            }

            let search_results = index.search(&payload.query_text, payload.max_results as usize, &filter)?;
            let results: Vec<ResourceResult> = search_results
                .into_iter()
                .map(|r| ResourceResult {
                    resource_id: r.resource.id,
                    description_text: r.resource.description_text,
                    location: r.resource.location,
                    location_scheme: r.resource.location_scheme,
                    mime_type: r.resource.mime_type,
                    score: r.score,
                })
                .collect();

            tracing::info!("Found {} matches. Sending direct response to {}...", results.len(), msg.sender);
            dispatcher.send_direct_response(&msg.sender, &payload.query_id, results)?;
        }
        MessageKind::NameQuery => {
            let payload: NameQueryPayload = serde_json::from_value(msg.envelope.payload)?;
            tracing::info!("Received direct name query from {} for: \"{}\"", msg.sender, payload.name);

            let conn_guard = conn.lock().unwrap();
            let index = LocalIndex::new(&conn_guard, provider);

            let results = index.resolve_name(&payload.normalized_name, &SchemeFilter::default())?;
            let name_records: Vec<NameRecord> = results.into_iter().map(|r| r.record).collect();

            tracing::info!("Found {} matching name records. Sending direct name response to {}...", name_records.len(), msg.sender);
            dispatcher.send_direct_name_response(&msg.sender, &payload.query_id, &payload.normalized_name, name_records)?;
        }
        _ => {}
    }
    Ok(())
}

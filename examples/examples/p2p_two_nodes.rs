//! P2P two-node demo (requires a running x0x daemon).
//!
//! This example shows how an anta-vista node connects to x0x, subscribes to
//! all av.* topics, and broadcasts a search query and a name claim. In a real
//! two-node setup a second node would receive these and respond.
//!
//! # Prerequisites
//!
//! 1. Install x0x:
//!    curl -sfL https://github.com/saorsa-labs/x0x/releases/latest/download/x0x-linux-x64-gnu.tar.gz | tar xz
//!    cp x0x-linux-x64-gnu/x0xd ~/.local/bin/
//!    cp x0x-linux-x64-gnu/x0x  ~/.local/bin/
//!
//! 2. Start a daemon instance:
//!    x0x start
//!    x0x health    # verify it is running
//!
//! 3. Run this example:
//!    cargo run --example p2p_two_nodes -p anta-vista-examples
//!
//! # What it demonstrates
//!
//! - Connecting to the local x0x daemon via auto-detected config
//! - Subscribing to all av.* gossip topics via MessageDispatcher
//! - Broadcasting a search query (av.query.v1)
//! - Broadcasting a name claim (av.name.claim.v1)
//! - Graceful exit with a clear message when no daemon is running

use av_core::types::{NameRecord, NameRecordType, normalize_name};
use av_net_x0x::{
    client::{X0xConfig, X0xNetClient},
    dispatcher::MessageDispatcher,
};
use std::sync::Arc;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  anta-vista — P2P two-node demo");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("  This example requires a running x0x daemon.");
    println!("  See the file header for setup instructions.");
    println!();

    // Try to auto-detect a running x0x daemon
    let config = match X0xConfig::from_data_dir() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  ✗ Could not connect to x0x daemon: {e}");
            eprintln!();
            eprintln!("  Start a daemon with: x0x start");
            eprintln!("  Then re-run this example.");
            std::process::exit(1);
        }
    };

    let short_id = &config.agent_id[..16.min(config.agent_id.len())];
    println!("  ✓ Connected to x0x daemon");
    println!("  ✓ Agent ID: {short_id}...");
    println!();

    let client = Arc::new(X0xNetClient::new(config.clone()));
    let dispatcher = MessageDispatcher::new(client);

    // Subscribe to all anta-vista gossip topics
    match dispatcher.subscribe_all() {
        Ok(_) => println!("  ✓ Subscribed to all av.* gossip topics"),
        Err(e) => {
            eprintln!("  ✗ Subscribe failed: {e}");
            std::process::exit(1);
        }
    }

    // Broadcast a search query via gossip
    println!("\n  📡 Broadcasting search query: \"distributed storage\"");
    match dispatcher.publish_query("distributed storage", 10, 1200, vec![]) {
        Ok(query_id) => println!("     query_id = {query_id}"),
        Err(e) => eprintln!("     failed: {e}"),
    }

    // Build and broadcast a name claim
    let target = format!("ant://{}", "a".repeat(64));
    let record = NameRecord {
        schema_version: 1,
        record_id: "demo-record-001".to_string(),
        normalized_name: normalize_name("my-service"),
        original_name: "my-service".to_string(),
        record_type: NameRecordType::Uri,
        target_canonical: Some(target.clone()),
        target_scheme: Some("ant".to_string()),
        target,
        ttl_secs: 3600,
        by_agent_id: config.agent_id.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        signature: vec![],
    };

    println!("\n  📛 Publishing name claim: \"my-service\"");
    match dispatcher.publish_name_claim(record) {
        Ok(_) => println!("     Published to av.name.claim.v1"),
        Err(e) => eprintln!("     failed: {e}"),
    }

    println!();
    println!("  All messages published. In a real deployment:");
    println!("  • Other peers subscribed to av.query.v1 receive the query");
    println!("  • They respond via av.response.v1 or direct messaging");
    println!("  • Name claims propagate via av.name.claim.v1 gossip");
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
}

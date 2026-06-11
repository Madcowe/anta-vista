use std::time::{Duration, Instant};
use std::sync::Arc;

use av_core::types::MessageKind;
use av_net_x0x::{
    client::{X0xConfig, X0xNetClient},
    dispatcher::MessageDispatcher,
    listener::start_listener,
    direct_listener::start_direct_listener,
    payloads::NameClaimPayload,
};

use crate::cli::{Cli, OutputFormat};
use crate::output::{print_json_line, print_markdown_summary, TestStatus};
use crate::tests::helpers::MessageHub;
use crate::tests::run_all_tests;

pub fn run_probe(mut args: Cli, config: X0xConfig) {
    // 1. Peer Autodetection
    if args.peer.is_none() {
        tracing::info!("Peer ID not provided. Commencing autodetection sequence...");
        let autodetect_timeout = Duration::from_secs(args.wait * 2);
        match autodetect_peer(&config, autodetect_timeout) {
            Some(peer_id) => {
                args.peer = Some(peer_id);
            }
            None => {
                tracing::error!("Failed to autodetect seed node within timeout.");
                std::process::exit(1);
            }
        }
    }

    let peer_id = args.peer.clone().unwrap();

    // 2. Open DB and initialize connection
    let conn = av_store::open_in_memory().expect("Failed to open local database");

    // 3. Connect dispatcher
    let client = Arc::new(X0xNetClient::new(config.clone()));
    let dispatcher = MessageDispatcher::new(client);
    dispatcher.subscribe_all().expect("Failed to subscribe to gossip topics");

    // 4. Start listeners and construct MessageHub
    let gossip_rx = start_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start gossip listener");
    let direct_rx = start_direct_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start direct listener");
    
    let hub = MessageHub { gossip_rx, direct_rx };

    tracing::info!("Commencing test suite against peer: {}...", peer_id);

    // 5. Execute tests
    let results = run_all_tests(&args, &dispatcher, &hub, &conn, &config);

    // 6. Output Results
    if args.output == OutputFormat::Json {
        for res in &results {
            print_json_line(res);
        }
    }

    // Always output the Markdown summary table for the user
    print_markdown_summary(&config.agent_id, args.peer.as_deref(), &results);

    // Exit code based on failures
    let failed = results.iter().any(|r| r.status == TestStatus::Fail);
    if failed {
        std::process::exit(2);
    }
}

fn autodetect_peer(config: &X0xConfig, wait_timeout: Duration) -> Option<String> {
    let gossip_rx = match start_listener(config.api_base.clone(), config.token.clone()) {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!("Could not start gossip listener for autodetection: {:?}", e);
            return None;
        }
    };

    let start = Instant::now();
    while start.elapsed() < wait_timeout {
        let remaining = wait_timeout.checked_sub(start.elapsed()).unwrap_or(Duration::ZERO);
        if remaining == Duration::ZERO {
            break;
        }
        match gossip_rx.recv_timeout(remaining) {
            Ok(Ok(event)) => {
                if event.envelope.kind == MessageKind::NameClaim {
                    if let Ok(claim) = serde_json::from_value::<NameClaimPayload>(event.envelope.payload) {
                        if claim.record.normalized_name == "seed.av" {
                            let peer_id = event.envelope.from_agent_id;
                            tracing::info!("Autodetected seed node Agent ID: {}", peer_id);
                            return Some(peer_id);
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("Error receiving gossip event during autodetection: {:?}", e);
            }
            Err(_) => break,
        }
    }
    None
}

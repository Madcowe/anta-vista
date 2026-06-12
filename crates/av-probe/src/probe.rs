use std::time::{Duration, Instant};
use std::sync::{Arc, Mutex};

use av_core::types::MessageKind;
use av_net_x0x::{
    client::{X0xConfig, X0xNetClient},
    dispatcher::MessageDispatcher,
    listener::{start_listener, IncomingEvent},
    direct_listener::{start_direct_listener, DirectMessage},
    payloads::NameClaimPayload,
};

use crate::cli::{Cli, OutputFormat};
use crate::output::{print_json_line, print_markdown_summary, TestStatus};
use crate::tests::helpers::{MessageHub, LoopbackClientWrapper, LoopbackMessage};
use crate::tests::run_all_tests;

pub fn run_probe(mut args: Cli, config: X0xConfig) {
    // 1. Check if the loopback server is running and connect to it
    let mut loopback_stream = None;
    if let Ok(stream) = std::net::TcpStream::connect("127.0.0.1:12709") {
        tracing::info!("Detected local seed node via loopback TCP server. Enabling single-agent loopback mode.");
        loopback_stream = Some(stream);
        if args.peer.is_none() || args.peer.as_ref() == Some(&config.agent_id) {
            args.peer = Some(config.agent_id.clone());
        }
    }

    // 2. Peer Autodetection (only if loopback mode is not active)
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

    // 3. Open DB and initialize connection
    let conn = av_store::open_in_memory().expect("Failed to open local database");

    // 4. Connect dispatcher
    let client = Arc::new(X0xNetClient::new(config.clone()));
    let loopback_stream_arc = Arc::new(Mutex::new(loopback_stream));
    let wrapper = Arc::new(LoopbackClientWrapper {
        real_client: client,
        loopback_stream: loopback_stream_arc.clone(),
    });
    let dispatcher = MessageDispatcher::new(wrapper);
    dispatcher.subscribe_all().expect("Failed to subscribe to gossip topics");

    // Establish direct connection to seed peer (only if loopback mode is not active)
    if loopback_stream_arc.lock().unwrap().is_none() {
        tracing::info!("Establishing peer connection to seed node {}...", peer_id);
        if let Err(e) = dispatcher.connect_agent(&peer_id) {
            tracing::warn!("Failed to initiate connection to seed node: {:?}", e);
        } else {
            tracing::info!("Connection request sent. Waiting 5 seconds for QUIC link to establish...");
            std::thread::sleep(Duration::from_secs(5));
        }
    }

    // 5. Start listeners and construct MessageHub
    let gossip_rx = start_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start gossip listener");
    let direct_rx = start_direct_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start direct listener");

    // Unified receivers
    let (unified_gossip_tx, unified_gossip_rx) = std::sync::mpsc::channel();
    let (unified_direct_tx, unified_direct_rx) = std::sync::mpsc::channel();

    // Forward real gossip to unified
    let tx = unified_gossip_tx.clone();
    std::thread::spawn(move || {
        for msg in gossip_rx {
            let _ = tx.send(msg);
        }
    });

    // If loopback stream is present, read from TCP and forward to unified
    let loopback_stream_for_read = loopback_stream_arc.lock().unwrap().as_ref().and_then(|s| s.try_clone().ok());
    if let Some(stream) = loopback_stream_for_read {
        let tx_gossip = unified_gossip_tx.clone();
        let tx_direct = unified_direct_tx.clone();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stream);
            use std::io::BufRead;
            let mut line = String::new();
            let mut seen_ids = std::collections::HashSet::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if let Ok(msg) = serde_json::from_str::<LoopbackMessage>(&line) {
                    let msg_id = match &msg {
                        LoopbackMessage::Gossip { envelope, .. } => envelope.message_id.clone(),
                        LoopbackMessage::Direct { envelope, .. } => envelope.message_id.clone(),
                    };
                    if seen_ids.insert(msg_id) {
                        match msg {
                            LoopbackMessage::Gossip { topic, envelope } => {
                                let event = IncomingEvent {
                                    topic,
                                    origin: "loopback".to_string(),
                                    envelope,
                                    raw_size: 0,
                                };
                                let _ = tx_gossip.send(Ok(event));
                            }
                            LoopbackMessage::Direct { to_agent_id: _, envelope } => {
                                let msg = DirectMessage {
                                    sender: envelope.from_agent_id.clone(),
                                    machine_id: "loopback".to_string(),
                                    envelope,
                                    received_at: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs() as i64,
                                };
                                let _ = tx_direct.send(Ok(msg));
                            }
                        }
                    }
                }
                line.clear();
            }
        });
    }

    // Forward real direct to unified
    let tx = unified_direct_tx.clone();
    std::thread::spawn(move || {
        for msg in direct_rx {
            let _ = tx.send(msg);
        }
    });
    
    let hub = MessageHub { gossip_rx: unified_gossip_rx, direct_rx: unified_direct_rx };

    tracing::info!("Commencing test suite against peer: {}...", peer_id);

    // 6. Execute tests
    let results = run_all_tests(&args, &dispatcher, &hub, &conn, &config);

    // 7. Output Results
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

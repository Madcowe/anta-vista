use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use av_core::constants::TOPIC_NAME_CLAIM;
use av_core::types::MessageKind;
use av_net_x0x::{
    client::{NetworkClient, X0xConfig, X0xNetClient},
    direct_listener::start_direct_listener,
    dispatcher::MessageDispatcher,
    listener::start_listener,
    payloads::{NameClaimPayload, NameResponsePayload},
};

use crate::cli::{Cli, OutputFormat};
use crate::output::{TestStatus, print_json_line, print_markdown_summary};
use crate::tests::helpers::MessageHub;
use crate::tests::run_all_tests;

pub fn run_probe(mut args: Cli, config: X0xConfig) {
    tracing::info!("Transport mode: X0X GOSSIP (real p2p network)");

    // ── 1. Peer autodetection ────────────────

    if args.peer.is_none() {
        tracing::info!("Peer ID not provided. Commencing autodetection via gossip...");
        let autodetect_timeout = Duration::from_secs(args.wait * 2);
        match autodetect_peer(&config, autodetect_timeout) {
            Some(peer_id) => {
                args.peer = Some(peer_id);
            }
            None => {
                tracing::error!("Failed to autodetect seed node within timeout.");
                tracing::error!("Hint: Make sure the seed node is running on a different machine.");
                std::process::exit(1);
            }
        }
    }

    let peer_id = args.peer.clone().unwrap();

    // ── 2. Fail-fast guard for single-daemon ────────────

    if peer_id == config.agent_id {
        tracing::error!("FATAL: Peer agent_id ({}) matches local agent_id.", peer_id);
        tracing::error!(
            "A single x0x daemon cannot deliver gossip messages to itself. \
             You must run the seed node and probe node on different machines (with separate x0x daemons)."
        );
        std::process::exit(1);
    }

    // ── 3. Open DB and connect dispatcher ────────────────────────────────

    let conn = av_store::open_in_memory().expect("Failed to open local database");

    let client = Arc::new(X0xNetClient::new(config.clone()));
    let dispatcher = MessageDispatcher::new(client);
    dispatcher
        .subscribe_all()
        .expect("Failed to subscribe to gossip topics");

    // Establish direct connection to seed peer
    tracing::info!("Establishing peer connection to seed node {}...", peer_id);
    if let Err(e) = dispatcher.connect_agent(&peer_id) {
        tracing::warn!("Failed to initiate connection to seed node: {:?}", e);
    } else {
        tracing::info!("Connection request sent. Waiting 5 seconds for QUIC link to establish...");
        std::thread::sleep(Duration::from_secs(5));
    }

    // ── 4. Start SSE listeners and wire up MessageHub ────────────────────

    let gossip_rx = start_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start gossip listener");
    let direct_rx = start_direct_listener(config.api_base.clone(), config.token.clone())
        .expect("Failed to start direct listener");

    let hub = MessageHub {
        gossip_rx,
        direct_rx,
    };

    tracing::info!("Commencing test suite against peer: {}...", peer_id);

    // ── 5. Execute tests ─────────────────────────────────────────────────

    let results = run_all_tests(&args, &dispatcher, &hub, &conn, &config);

    // ── 6. Output results ────────────────────────────────────────────────

    if args.output == OutputFormat::Json {
        for res in &results {
            print_json_line(res);
        }
    }

    print_markdown_summary(&config.agent_id, args.peer.as_deref(), &results);

    let failed = results.iter().any(|r| r.status == TestStatus::Fail);
    if failed {
        std::process::exit(2);
    }
}

// ── Peer autodetection ───────────────────────────────────────────────────────

fn autodetect_peer(config: &X0xConfig, wait_timeout: Duration) -> Option<String> {
    let client = Arc::new(X0xNetClient::new(config.clone()));
    let dispatcher = MessageDispatcher::new(client.clone());

    if let Err(e) = client.subscribe(TOPIC_NAME_CLAIM) {
        tracing::error!(
            "Could not subscribe to name claims for autodetection: {:?}",
            e
        );
        return None;
    }

    let gossip_rx = match start_listener(config.api_base.clone(), config.token.clone()) {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!("Could not start gossip listener for autodetection: {:?}", e);
            return None;
        }
    };

    let direct_rx = match start_direct_listener(config.api_base.clone(), config.token.clone()) {
        Ok(rx) => Some(rx),
        Err(e) => {
            tracing::warn!(
                "Could not start direct listener for seed verification: {:?}",
                e
            );
            None
        }
    };

    let start = Instant::now();
    let discovery_interval = Duration::from_secs(2);
    let mut next_discovery = start;
    let mut seen_candidates = HashSet::new();
    let mut last_candidate_probe: HashMap<String, Instant> = HashMap::new();

    while start.elapsed() < wait_timeout {
        let now = Instant::now();
        if now >= next_discovery {
            for candidate in discover_candidate_agents(config) {
                if candidate == config.agent_id {
                    continue;
                }
                seen_candidates.insert(candidate.clone());
                if last_candidate_probe
                    .get(&candidate)
                    .is_some_and(|last_probe| last_probe.elapsed() < Duration::from_secs(5))
                {
                    continue;
                }

                tracing::info!(
                    "Discovered candidate agent {}; probing for seed.av...",
                    candidate
                );
                if let Err(e) = dispatcher.connect_agent(&candidate) {
                    tracing::debug!(
                        "Could not initiate connection to candidate {}: {:?}",
                        candidate,
                        e
                    );
                    continue;
                }

                match dispatcher.send_direct_name_query(&candidate, "seed.av", None, 1, 2_000) {
                    Ok(_) => {
                        last_candidate_probe.insert(candidate, Instant::now());
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Could not send seed verification query to candidate {}: {:?}",
                            candidate,
                            e
                        );
                    }
                }
            }
            next_discovery = now + discovery_interval;
        }

        if let Some(rx) = &direct_rx {
            loop {
                match rx.try_recv() {
                    Ok(Ok(msg)) => {
                        if msg.envelope.kind == MessageKind::NameResponse {
                            if let Ok(resp) = serde_json::from_value::<NameResponsePayload>(
                                msg.envelope.payload.clone(),
                            ) {
                                let is_seed = resp.normalized_name == "seed.av"
                                    && resp.results.iter().any(|r| {
                                        r.normalized_name == "seed.av"
                                            && r.by_agent_id == msg.sender
                                    });
                                if is_seed {
                                    tracing::info!(
                                        "Autodetected seed node Agent ID via direct verification: {}",
                                        msg.sender
                                    );
                                    return Some(msg.sender);
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            "Error receiving direct event during autodetection: {:?}",
                            e
                        );
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        }

        let remaining = wait_timeout
            .checked_sub(start.elapsed())
            .unwrap_or(Duration::ZERO);
        if remaining == Duration::ZERO {
            break;
        }

        let wait_slice = remaining.min(Duration::from_millis(250));
        match gossip_rx.recv_timeout(wait_slice) {
            Ok(Ok(event)) => {
                if event.envelope.kind == MessageKind::NameClaim {
                    if let Ok(claim) =
                        serde_json::from_value::<NameClaimPayload>(event.envelope.payload)
                    {
                        if claim.record.normalized_name == "seed.av" {
                            let peer_id = event.envelope.from_agent_id;
                            tracing::info!(
                                "Autodetected seed node Agent ID via gossip claim: {}",
                                peer_id
                            );
                            return Some(peer_id);
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("Error receiving gossip event during autodetection: {:?}", e);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    tracing::warn!(
        "Autodetect saw {} discovered candidate(s), but none verified seed.av",
        seen_candidates.len()
    );
    None
}

fn discover_candidate_agents(config: &X0xConfig) -> HashSet<String> {
    let mut candidates = HashSet::new();
    for path in ["/agents/discovered", "/presence/online"] {
        match get_daemon_json(config, path) {
            Ok(value) => collect_agent_ids(&value, &mut candidates),
            Err(e) => tracing::debug!("Could not query x0x discovery path {}: {}", path, e),
        }
    }
    candidates
}

fn get_daemon_json(config: &X0xConfig, path: &str) -> Result<serde_json::Value, String> {
    ureq::get(&format!("{}{}", config.api_base, path))
        .set("Authorization", &format!("Bearer {}", config.token))
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())
}

fn collect_agent_ids(value: &serde_json::Value, out: &mut HashSet<String>) {
    match value {
        serde_json::Value::String(s) => {
            if is_agent_id(s) {
                out.insert(s.to_string());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_agent_ids(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "agent_id" || key.ends_with("_agent_id") {
                    collect_agent_ids(value, out);
                } else if is_agent_id(key) {
                    out.insert(key.to_string());
                } else {
                    collect_agent_ids(value, out);
                }
            }
        }
        _ => {}
    }
}

fn is_agent_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

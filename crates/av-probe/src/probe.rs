use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use av_core::constants::TOPIC_NAME_CLAIM;
use av_core::types::MessageKind;
use av_net_x0x::{
    client::{NetworkClient, X0xConfig, X0xNetClient},
    direct_listener::{start_direct_listener, DirectMessage},
    dispatcher::MessageDispatcher,
    error::NetResult,
    listener::{start_listener, IncomingEvent},
    payloads::{NameClaimPayload, NameResponsePayload},
};

use crate::cli::{Cli, OutputFormat};
use crate::output::{TestStatus, print_json_line, print_markdown_summary};
use crate::tests::helpers::MessageHub;
use crate::tests::run_all_tests;
use crate::tests::trust::reset_daemon_trust;

/// Return type for autodetection: the found peer ID together with the already-open
/// SSE receivers so they can be reused by the test suite without a gap.
struct AutodetectResult {
    peer_id: String,
    gossip_rx: Receiver<NetResult<IncomingEvent>>,
    direct_rx: Receiver<NetResult<DirectMessage>>,
}

pub fn run_probe(mut args: Cli, config: X0xConfig) {
    tracing::info!("Transport mode: X0X GOSSIP (real p2p network)");

    // ── 1. Peer autodetection ────────────────
    //
    // We open the SSE listeners *before* discovering the peer and hand them
    // directly into the MessageHub.  This way there is no window between
    // "autodetection ends" and "listeners start" during which the seed could
    // broadcast a NameClaim or response that we would silently miss.

    if args.peer.is_none() {
        tracing::info!("Peer ID not provided. Commencing autodetection via gossip...");
        let autodetect_timeout = Duration::from_secs(args.wait * 2);
        match autodetect_peer(&config, autodetect_timeout) {
            Some(result) => {
                args.peer = Some(result.peer_id.clone());

                let peer_id = result.peer_id;

                // ── 2. Fail-fast guard for single-daemon ────────────
                if peer_id == config.agent_id {
                    tracing::error!("FATAL: Peer agent_id ({}) matches local agent_id.", peer_id);
                    tracing::error!(
                        "A single x0x daemon cannot deliver gossip messages to itself. \
                         You must run the seed node and probe node on different machines \
                         (with separate x0x daemons)."
                    );
                    std::process::exit(1);
                }

                // ── 3. Open DB and connect dispatcher ──────────────
                let conn = av_store::open_in_memory().expect("Failed to open local database");

                let client = Arc::new(X0xNetClient::new(config.clone()));
                let dispatcher = MessageDispatcher::new(client);
                dispatcher
                    .subscribe_all()
                    .expect("Failed to subscribe to gossip topics");

                // Re-use the QUIC link that was opened during autodetection; wait
                // a bit more in case the handshake is still in-flight.
                tracing::info!(
                    "Ensuring QUIC link to seed node {} is ready...",
                    peer_id
                );
                if let Err(e) = dispatcher.connect_agent(&peer_id) {
                    tracing::warn!(
                        "connect_agent after autodetect returned an error (link may already be up): {:?}",
                        e
                    );
                }
                std::thread::sleep(Duration::from_secs(3));

                // Reuse the SSE receivers opened during autodetection.
                let hub = MessageHub::new(result.gossip_rx, result.direct_rx);

                tracing::info!("Commencing test suite against peer: {}...", peer_id);
                run_tests_and_exit(args, dispatcher, hub, conn, config, &peer_id);
            }
            None => {
                tracing::error!("Failed to autodetect seed node within timeout.");
                tracing::error!("Hint: Make sure the seed node is running on a different machine.");
                std::process::exit(1);
            }
        }
    } else {
        let peer_id = args.peer.clone().unwrap();

        // ── 2. Fail-fast guard for single-daemon ────────────
        if peer_id == config.agent_id {
            tracing::error!("FATAL: Peer agent_id ({}) matches local agent_id.", peer_id);
            tracing::error!(
                "A single x0x daemon cannot deliver gossip messages to itself. \
                 You must run the seed node and probe node on different machines \
                 (with separate x0x daemons)."
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
            tracing::info!(
                "Connection request sent. Waiting 5 seconds for QUIC link to establish..."
            );
            std::thread::sleep(Duration::from_secs(5));
        }

        // ── 4. Start SSE listeners and wire up MessageHub ────────────────────
        let gossip_rx = start_listener(config.api_base.clone(), config.token.clone())
            .expect("Failed to start gossip listener");
        let direct_rx = start_direct_listener(config.api_base.clone(), config.token.clone())
            .expect("Failed to start direct listener");

        let hub = MessageHub::new(gossip_rx, direct_rx);

        tracing::info!("Commencing test suite against peer: {}...", peer_id);
        run_tests_and_exit(args, dispatcher, hub, conn, config, &peer_id);
    }
}

fn run_tests_and_exit(
    args: Cli,
    dispatcher: MessageDispatcher,
    hub: MessageHub,
    conn: rusqlite::Connection,
    config: X0xConfig,
    peer_id: &str,
) {
    // Safety reset: ensure the peer isn't blocked from a previous run.
    // x0x silently drops all gossip and direct messages from blocked agents,
    // so this must happen before any test traffic is sent.
    tracing::info!("Resetting peer trust level to 'unknown' before test run...");
    reset_daemon_trust(&config, peer_id);

    // Brief warm-up: give the SSE stream 2s to settle so T1 (the first test)
    // doesn't publish before the connection is fully established.
    tracing::info!("Waiting 2s for SSE stream to settle...");
    std::thread::sleep(Duration::from_secs(2));

    let results = run_all_tests(&args, &dispatcher, &hub, &conn, &config);

    if args.output == OutputFormat::Json {
        for res in &results {
            print_json_line(res);
        }
    }

    print_markdown_summary(&config.agent_id, Some(peer_id), &results);

    let failed = results.iter().any(|r| r.status == TestStatus::Fail);
    if failed {
        std::process::exit(2);
    }
}

// ── Peer autodetection ───────────────────────────────────────────────────────

fn autodetect_peer(config: &X0xConfig, wait_timeout: Duration) -> Option<AutodetectResult> {
    let client = Arc::new(X0xNetClient::new(config.clone()));
    let dispatcher = MessageDispatcher::new(client.clone());

    if let Err(e) = client.subscribe(TOPIC_NAME_CLAIM) {
        tracing::error!(
            "Could not subscribe to name claims for autodetection: {:?}",
            e
        );
        return None;
    }

    // Open the SSE listeners now so that no events are missed between autodetection
    // completing and the test suite starting.
    let gossip_rx = match start_listener(config.api_base.clone(), config.token.clone()) {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!("Could not start gossip listener for autodetection: {:?}", e);
            return None;
        }
    };

    let direct_rx = match start_direct_listener(config.api_base.clone(), config.token.clone()) {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(
                "Could not start direct listener for autodetection: {:?}",
                e
            );
            return None;
        }
    };

    let start = Instant::now();
    let discovery_interval = Duration::from_secs(2);
    let mut next_discovery = start;
    let mut seen_candidates = HashSet::new();
    let mut last_candidate_probe: HashMap<String, Instant> = HashMap::new();

    // We need to drain both channels during autodetection.  The gossip_rx is
    // consumed here via recv_timeout; direct messages are drained via try_recv.
    // After returning we hand *both* receivers to the MessageHub so tests pick
    // up from where autodetection left off.
    //
    // To do that we collect buffered gossip events that were *not* the seed
    // claim and re-inject them into a local Vec.  The MessageHub only exposes
    // mpsc receivers so we can't push back; instead we just don't drain gossip
    // aggressively — we only block for short slices and let the channel buffer
    // everything else.

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

                // Retry the direct name query with a small backoff to give the
                // QUIC handshake time to complete before we hit /direct/send.
                let mut sent = false;
                for attempt in 0..3u32 {
                    if attempt > 0 {
                        std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
                    }
                    match dispatcher.send_direct_name_query(&candidate, "seed.av", None, 1, 2_000)
                    {
                        Ok(_) => {
                            sent = true;
                            break;
                        }
                        Err(e) => {
                            tracing::debug!(
                                "Attempt {} to send seed verification query to {}: {:?}",
                                attempt + 1,
                                candidate,
                                e
                            );
                        }
                    }
                }
                if sent {
                    last_candidate_probe.insert(candidate, Instant::now());
                }
            }
            next_discovery = now + discovery_interval;
        }

        // Drain any direct messages that arrived.
        loop {
            match direct_rx.try_recv() {
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
                                return Some(AutodetectResult {
                                    peer_id: msg.sender,
                                    gossip_rx,
                                    direct_rx,
                                });
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

        let remaining = wait_timeout
            .checked_sub(start.elapsed())
            .unwrap_or(Duration::ZERO);
        if remaining == Duration::ZERO {
            break;
        }

        // Poll the gossip channel briefly; leave most of the capacity in the
        // channel buffer so the test suite can read it later.
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
                            return Some(AutodetectResult {
                                peer_id,
                                gossip_rx,
                                direct_rx,
                            });
                        }
                    }
                }
                // Non-matching gossip events are dropped here; the test suite
                // will re-trigger the operations it needs so this is fine.
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

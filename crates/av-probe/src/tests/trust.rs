use std::time::{Instant, SystemTime, UNIX_EPOCH, Duration};
use crate::cli::Cli;
use crate::output::{TestResult, TestStatus};
use crate::tests::helpers::{MessageHub, prompt_user};
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::client::X0xConfig;
use av_core::types::{NameRecord, NameRecordType, TrustState, normalize_name};
use av_index::{LocalIndex, SchemeFilter};
use rusqlite::Connection;

pub fn test_unknown_trust(
    args: &Cli,
    _dispatcher: &MessageDispatcher,
    _hub: &MessageHub,
    conn: &Connection,
    _x0x_cfg: &X0xConfig,
) -> TestResult {
    let start = Instant::now();
    let peer_id = match &args.peer {
        Some(id) => id,
        None => {
            return TestResult {
                test_id: "T4".to_string(),
                category: "trust".to_string(),
                name: "unknown_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Skip,
                duration_ms: 0,
                details: "Peer ID not set (autodetection failed or seed not found)".to_string(),
                debug: serde_json::json!({}),
            };
        }
    };

    // 1. Clear any existing trust records for this peer in local DB
    let _ = conn.execute("DELETE FROM trust WHERE subject_agent_id = ?1", rusqlite::params![peer_id]);

    // 2. Insert mock name record for this peer
    let name_record = NameRecord {
        schema_version: 1,
        record_id: format!("trust-rec-{}", peer_id),
        normalized_name: normalize_name("trust-test.av"),
        original_name: "trust-test.av".to_string(),
        record_type: NameRecordType::Uri,
        target: "ant://trust-test-loc".to_string(),
        target_scheme: Some("ant".to_string()),
        target_canonical: Some("ant://trust-test-loc".to_string()),
        ttl_secs: 3600,
        by_agent_id: peer_id.clone(),
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
        signature: vec![],
    };
    let _ = av_store::repo::names::insert(conn, &name_record);

    // 3. Resolve name and verify that trust_score defaults to 0.5 (neutral)
    let provider = av_embed::MockEmbeddingProvider::new();
    let index = LocalIndex::new(conn, &provider);
    let name_results = index.resolve_name("trust-test.av", &SchemeFilter::default()).unwrap();

    let duration_ms = start.elapsed().as_millis() as u64;

    if let Some(res) = name_results.iter().find(|r| r.record.by_agent_id == *peer_id) {
        // trust score component should be 0.5 for unknown agent
        if (res.trust_score - 0.5).abs() < 0.001 {
            TestResult {
                test_id: "T4".to_string(),
                category: "trust".to_string(),
                name: "unknown_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Pass,
                duration_ms,
                details: format!("Neutral trust score component (0.5) verified for unknown peer {}", peer_id),
                debug: serde_json::json!({ "trust_score": res.trust_score }),
            }
        } else {
            TestResult {
                test_id: "T4".to_string(),
                category: "trust".to_string(),
                name: "unknown_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms,
                details: format!("Expected trust score component 0.5, got {}", res.trust_score),
                debug: serde_json::json!({ "trust_score": res.trust_score }),
            }
        }
    } else {
        TestResult {
            test_id: "T4".to_string(),
            category: "trust".to_string(),
            name: "unknown_trust".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: "Could not find name record in local resolution results".to_string(),
            debug: serde_json::json!({}),
        }
    }
}

pub fn test_known_trust(
    args: &Cli,
    _dispatcher: &MessageDispatcher,
    _hub: &MessageHub,
    conn: &Connection,
    x0x_cfg: &X0xConfig,
) -> TestResult {
    let start = Instant::now();
    let peer_id = match &args.peer {
        Some(id) => id,
        None => {
            return TestResult {
                test_id: "T5".to_string(),
                category: "trust".to_string(),
                name: "known_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Skip,
                duration_ms: 0,
                details: "Peer ID not set (autodetection failed or seed not found)".to_string(),
                debug: serde_json::json!({}),
            };
        }
    };

    // 1. Prompt user to trust peer
    prompt_user(peer_id, "trusted", x0x_cfg);

    // 2. Poll daemon to confirm level changed to trusted
    let mut actual_level = "unknown".to_string();
    let poll_start = Instant::now();
    while poll_start.elapsed() < Duration::from_secs(30) {
        if let Some(level) = get_daemon_trust_level(x0x_cfg, peer_id) {
            actual_level = level;
            if actual_level == "trusted" {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    if actual_level != "trusted" {
        return TestResult {
            test_id: "T5".to_string(),
            category: "trust".to_string(),
            name: "known_trust".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Timed out waiting for trust level to change to 'trusted'. Currently: {}", actual_level),
            debug: serde_json::json!({ "level": actual_level }),
        };
    }

    // 3. Update trust score in local DB: trusted maps to +1.0
    let trust_state = TrustState {
        subject_agent_id: peer_id.clone(),
        trust_score: 1.0,
        evidence_count: 1,
        last_updated_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
    };
    let _ = av_store::repo::trust::upsert(conn, &trust_state);

    // 4. Resolve name and verify that trust_score is updated to > 0.5 (specifically 1.0 normalized)
    let provider = av_embed::MockEmbeddingProvider::new();
    let index = LocalIndex::new(conn, &provider);
    let name_results = index.resolve_name("trust-test.av", &SchemeFilter::default()).unwrap();

    let duration_ms = start.elapsed().as_millis() as u64;

    if let Some(res) = name_results.iter().find(|r| r.record.by_agent_id == *peer_id) {
        // (trust_score + 1.0) / 2.0 = (1.0 + 1.0) / 2.0 = 1.0
        if (res.trust_score - 1.0).abs() < 0.001 {
            TestResult {
                test_id: "T5".to_string(),
                category: "trust".to_string(),
                name: "known_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Pass,
                duration_ms,
                details: format!("High trust score component (1.0) verified for trusted peer {}", peer_id),
                debug: serde_json::json!({ "trust_score": res.trust_score }),
            }
        } else {
            TestResult {
                test_id: "T5".to_string(),
                category: "trust".to_string(),
                name: "known_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms,
                details: format!("Expected trust score component 1.0, got {}", res.trust_score),
                debug: serde_json::json!({ "trust_score": res.trust_score }),
            }
        }
    } else {
        TestResult {
            test_id: "T5".to_string(),
            category: "trust".to_string(),
            name: "known_trust".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: "Could not find name record in local resolution results after trust update".to_string(),
            debug: serde_json::json!({}),
        }
    }
}

pub fn test_blocked_trust(
    args: &Cli,
    _dispatcher: &MessageDispatcher,
    _hub: &MessageHub,
    conn: &Connection,
    x0x_cfg: &X0xConfig,
) -> TestResult {
    let start = Instant::now();
    let peer_id = match &args.peer {
        Some(id) => id,
        None => {
            return TestResult {
                test_id: "T6".to_string(),
                category: "trust".to_string(),
                name: "blocked_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Skip,
                duration_ms: 0,
                details: "Peer ID not set (autodetection failed or seed not found)".to_string(),
                debug: serde_json::json!({}),
            };
        }
    };

    // 1. Prompt user to block peer
    prompt_user(peer_id, "blocked", x0x_cfg);

    // 2. Poll daemon to confirm level changed to blocked
    let mut actual_level = "unknown".to_string();
    let poll_start = Instant::now();
    while poll_start.elapsed() < Duration::from_secs(30) {
        if let Some(level) = get_daemon_trust_level(x0x_cfg, peer_id) {
            actual_level = level;
            if actual_level == "blocked" {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    if actual_level != "blocked" {
        return TestResult {
            test_id: "T6".to_string(),
            category: "trust".to_string(),
            name: "blocked_trust".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Timed out waiting for trust level to change to 'blocked'. Currently: {}", actual_level),
            debug: serde_json::json!({ "level": actual_level }),
        };
    }

    // 3. Update trust score in local DB: blocked maps to -1.0
    let trust_state = TrustState {
        subject_agent_id: peer_id.clone(),
        trust_score: -1.0,
        evidence_count: 1,
        last_updated_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
    };
    let _ = av_store::repo::trust::upsert(conn, &trust_state);

    // 4. Resolve name and verify that trust_score is updated to 0.0 (specifically -1.0 normalized)
    let provider = av_embed::MockEmbeddingProvider::new();
    let index = LocalIndex::new(conn, &provider);
    let name_results = index.resolve_name("trust-test.av", &SchemeFilter::default()).unwrap();

    let duration_ms = start.elapsed().as_millis() as u64;

    if let Some(res) = name_results.iter().find(|r| r.record.by_agent_id == *peer_id) {
        // (trust_score + 1.0) / 2.0 = (-1.0 + 1.0) / 2.0 = 0.0
        if (res.trust_score - 0.0).abs() < 0.001 {
            TestResult {
                test_id: "T6".to_string(),
                category: "trust".to_string(),
                name: "blocked_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Pass,
                duration_ms,
                details: format!("Blocked trust score component (0.0) verified for blocked peer {}", peer_id),
                debug: serde_json::json!({ "trust_score": res.trust_score }),
            }
        } else {
            TestResult {
                test_id: "T6".to_string(),
                category: "trust".to_string(),
                name: "blocked_trust".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms,
                details: format!("Expected trust score component 0.0, got {}", res.trust_score),
                debug: serde_json::json!({ "trust_score": res.trust_score }),
            }
        }
    } else {
        TestResult {
            test_id: "T6".to_string(),
            category: "trust".to_string(),
            name: "blocked_trust".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: "Could not find name record in local resolution results after trust update".to_string(),
            debug: serde_json::json!({}),
        }
    }
}

fn get_daemon_trust_level(config: &X0xConfig, peer_id: &str) -> Option<String> {
    let url = format!("{}/contacts", config.api_base);
    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", config.token))
        .call()
        .ok()?;
    let val: serde_json::Value = resp.into_json().ok()?;
    let arr = val["contacts"].as_array()?;
    for contact in arr {
        if contact["agent_id"].as_str() == Some(peer_id) {
            return contact["trust_level"].as_str().map(|s| s.to_string());
        }
    }
    None
}

use std::time::{Duration, Instant};
use crate::cli::Cli;
use crate::output::{TestResult, TestStatus};
use crate::tests::helpers::MessageHub;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::{NameClaimPayload, NameResponsePayload};
use av_core::types::MessageKind;

pub fn test_gossip_name_claim(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();
    let peer_id = match &args.peer {
        Some(id) => id,
        None => {
            return TestResult {
                test_id: "N1".to_string(),
                category: "naming".to_string(),
                name: "gossip_name_claim".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Skip,
                duration_ms: 0,
                details: "Peer ID not set (autodetection failed or seed not found)".to_string(),
                debug: serde_json::json!({}),
            };
        }
    };

    // Publish a name query for "seed.av" to prompt the seed to immediately
    // re-broadcast its NameClaim rather than waiting up to 10 s for the next
    // periodic tick.
    let _ = dispatcher.publish_name_query("seed.av", None, 1, args.wait * 1000);

    // Wait for the NameClaim broadcast from the seed node.
    let timeout = Duration::from_secs(args.wait);
    let matched = hub.wait_for_gossip(timeout, |event| {
        if event.envelope.kind == MessageKind::NameClaim && &event.envelope.from_agent_id == peer_id {
            if let Ok(claim) = serde_json::from_value::<NameClaimPayload>(event.envelope.payload.clone()) {
                return claim.record.normalized_name == "seed.av";
            }
        }
        false
    });

    let duration_ms = start.elapsed().as_millis() as u64;

    match matched {
        Some(event) => TestResult {
            test_id: "N1".to_string(),
            category: "naming".to_string(),
            name: "gossip_name_claim".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Pass,
            duration_ms,
            details: "Detected NameClaim broadcast from seed node".to_string(),
            debug: serde_json::json!({ "envelope": event.envelope }),
        },
        None => TestResult {
            test_id: "N1".to_string(),
            category: "naming".to_string(),
            name: "gossip_name_claim".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for NameClaim broadcast from seed node ({})", peer_id),
            debug: serde_json::json!({ "peer_id": peer_id }),
        },
    }
}

pub fn test_name_query_response(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();

    // 1. Broadcast name query
    let query_id = match dispatcher.publish_name_query("seed.av", None, 5, args.wait * 1000) {
        Ok(id) => id,
        Err(e) => {
            return TestResult {
                test_id: "N2".to_string(),
                category: "naming".to_string(),
                name: "name_query_response".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Failed to publish name query: {:?}", e),
                debug: serde_json::json!({ "error": e.to_string() }),
            };
        }
    };

    // 2. Wait for response
    let timeout = Duration::from_secs(args.wait);
    let matched = hub.wait_for_gossip(timeout, |event| {
        if event.envelope.kind == MessageKind::NameResponse {
            if let Ok(resp) = serde_json::from_value::<NameResponsePayload>(event.envelope.payload.clone()) {
                return resp.query_id == query_id;
            }
        }
        false
    });

    let duration_ms = start.elapsed().as_millis() as u64;

    match matched {
        Some(event) => TestResult {
            test_id: "N2".to_string(),
            category: "naming".to_string(),
            name: "name_query_response".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Pass,
            duration_ms,
            details: "Name query and response successful".to_string(),
            debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
        },
        None => TestResult {
            test_id: "N2".to_string(),
            category: "naming".to_string(),
            name: "name_query_response".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for name response for seed.av"),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

pub fn test_case_insensitive(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();

    // 1. Broadcast mixed-case name query
    let query_id = match dispatcher.publish_name_query("SeEd.Av", None, 5, args.wait * 1000) {
        Ok(id) => id,
        Err(e) => {
            return TestResult {
                test_id: "N3".to_string(),
                category: "naming".to_string(),
                name: "case_insensitive".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Failed to publish name query: {:?}", e),
                debug: serde_json::json!({ "error": e.to_string() }),
            };
        }
    };

    // 2. Wait for response
    let timeout = Duration::from_secs(args.wait);
    let matched = hub.wait_for_gossip(timeout, |event| {
        if event.envelope.kind == MessageKind::NameResponse {
            if let Ok(resp) = serde_json::from_value::<NameResponsePayload>(event.envelope.payload.clone()) {
                return resp.query_id == query_id;
            }
        }
        false
    });

    let duration_ms = start.elapsed().as_millis() as u64;

    match matched {
        Some(event) => {
            let payload: NameResponsePayload = serde_json::from_value(event.envelope.payload.clone()).unwrap();
            if payload.results.is_empty() {
                TestResult {
                    test_id: "N3".to_string(),
                    category: "naming".to_string(),
                    name: "case_insensitive".to_string(),
                    transport: "gossip".to_string(),
                    status: TestStatus::Fail,
                    duration_ms,
                    details: "Received empty results list for mixed-case name query".to_string(),
                    debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
                }
            } else {
                TestResult {
                    test_id: "N3".to_string(),
                    category: "naming".to_string(),
                    name: "case_insensitive".to_string(),
                    transport: "gossip".to_string(),
                    status: TestStatus::Pass,
                    duration_ms,
                    details: "Case-insensitive name query returned matching records successfully".to_string(),
                    debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
                }
            }
        }
        None => TestResult {
            test_id: "N3".to_string(),
            category: "naming".to_string(),
            name: "case_insensitive".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for name response for SeEd.Av"),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

pub fn test_scheme_alias(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();

    // 1. Broadcast query for name with scheme alias
    let query_id = match dispatcher.publish_name_query("alias.av", None, 5, args.wait * 1000) {
        Ok(id) => id,
        Err(e) => {
            return TestResult {
                test_id: "N4".to_string(),
                category: "naming".to_string(),
                name: "scheme_alias".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Failed to publish name query: {:?}", e),
                debug: serde_json::json!({ "error": e.to_string() }),
            };
        }
    };

    // 2. Wait for response
    let timeout = Duration::from_secs(args.wait);
    let matched = hub.wait_for_gossip(timeout, |event| {
        if event.envelope.kind == MessageKind::NameResponse {
            if let Ok(resp) = serde_json::from_value::<NameResponsePayload>(event.envelope.payload.clone()) {
                return resp.query_id == query_id;
            }
        }
        false
    });

    let duration_ms = start.elapsed().as_millis() as u64;

    match matched {
        Some(event) => {
            let payload: NameResponsePayload = serde_json::from_value(event.envelope.payload.clone()).unwrap();
            let is_normalized = payload.results.iter().any(|r| {
                r.target_scheme.as_deref() == Some("ant") &&
                r.target_canonical.as_ref().map(|tc| tc.starts_with("ant://")).unwrap_or(false)
            });

            if is_normalized {
                TestResult {
                    test_id: "N4".to_string(),
                    category: "naming".to_string(),
                    name: "scheme_alias".to_string(),
                    transport: "gossip".to_string(),
                    status: TestStatus::Pass,
                    duration_ms,
                    details: "autonomi:// was correctly normalized to ant:// in the target record".to_string(),
                    debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
                }
            } else {
                TestResult {
                    test_id: "N4".to_string(),
                    category: "naming".to_string(),
                    name: "scheme_alias".to_string(),
                    transport: "gossip".to_string(),
                    status: TestStatus::Fail,
                    duration_ms,
                    details: "Name target was not normalized from autonomi:// to ant://".to_string(),
                    debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
                }
            }
        }
        None => TestResult {
            test_id: "N4".to_string(),
            category: "naming".to_string(),
            name: "scheme_alias".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for name response for alias.av"),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

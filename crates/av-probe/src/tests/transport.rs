use std::time::{Duration, Instant};
use crate::cli::Cli;
use crate::output::{TestResult, TestStatus};
use crate::tests::helpers::{MessageHub, now_secs};
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::{QueryPayload, ResponsePayload};
use av_core::types::{MessageKind, MessageEnvelope};
use av_core::constants::{SCHEMA_VERSION, TOPIC_QUERY};

pub fn test_gossip_delivery(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();
    let query_text = "test gossip delivery query";
    
    // 1. Publish query
    let query_id = match dispatcher.publish_query(query_text, 1, args.wait * 1000, vec![]) {
        Ok(id) => id,
        Err(e) => {
            return TestResult {
                test_id: "T1".to_string(),
                category: "transport".to_string(),
                name: "gossip_delivery".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Failed to publish query: {:?}", e),
                debug: serde_json::json!({ "error": e.to_string() }),
            };
        }
    };

    // 2. Wait for response with matching query_id
    let timeout = Duration::from_secs(args.wait);
    let matched = hub.wait_for_gossip(timeout, |event| {
        if event.envelope.kind == MessageKind::Response {
            if let Ok(resp) = serde_json::from_value::<ResponsePayload>(event.envelope.payload.clone()) {
                return resp.query_id == query_id;
            }
        }
        false
    });

    let duration_ms = start.elapsed().as_millis() as u64;

    match matched {
        Some(event) => TestResult {
            test_id: "T1".to_string(),
            category: "transport".to_string(),
            name: "gossip_delivery".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Pass,
            duration_ms,
            details: "Gossip query and response round-trip successful".to_string(),
            debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
        },
        None => TestResult {
            test_id: "T1".to_string(),
            category: "transport".to_string(),
            name: "gossip_delivery".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for response after {}s", args.wait),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

pub fn test_direct_delivery(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();
    let peer_id = match &args.peer {
        Some(id) => id,
        None => {
            return TestResult {
                test_id: "T2".to_string(),
                category: "transport".to_string(),
                name: "direct_delivery".to_string(),
                transport: "direct".to_string(),
                status: TestStatus::Skip,
                duration_ms: 0,
                details: "Peer ID not set (autodetection failed or seed not found)".to_string(),
                debug: serde_json::json!({}),
            };
        }
    };

    // 1. Establish direct connection
    if let Err(e) = dispatcher.connect_agent(peer_id) {
        return TestResult {
            test_id: "T2".to_string(),
            category: "transport".to_string(),
            name: "direct_delivery".to_string(),
            transport: "direct".to_string(),
            status: TestStatus::Fail,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Failed to connect to agent {}: {:?}", peer_id, e),
            debug: serde_json::json!({ "error": e.to_string(), "peer_id": peer_id }),
        };
    }

    // 2. Send direct query
    let query_id = match dispatcher.send_direct_query(peer_id, "test direct delivery query", 1, args.wait * 1000, vec![]) {
        Ok(id) => id,
        Err(e) => {
            return TestResult {
                test_id: "T2".to_string(),
                category: "transport".to_string(),
                name: "direct_delivery".to_string(),
                transport: "direct".to_string(),
                status: TestStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Failed to send direct query: {:?}", e),
                debug: serde_json::json!({ "error": e.to_string() }),
            };
        }
    };

    // 3. Wait for response with matching query_id
    let timeout = Duration::from_secs(args.wait);
    let matched = hub.wait_for_direct(timeout, |msg| {
        if msg.envelope.kind == MessageKind::Response {
            if let Ok(resp) = serde_json::from_value::<ResponsePayload>(msg.envelope.payload.clone()) {
                return resp.query_id == query_id;
            }
        }
        false
    });

    let duration_ms = start.elapsed().as_millis() as u64;

    match matched {
        Some(msg) => TestResult {
            test_id: "T2".to_string(),
            category: "transport".to_string(),
            name: "direct_delivery".to_string(),
            transport: "direct".to_string(),
            status: TestStatus::Pass,
            duration_ms,
            details: "Direct query and response round-trip successful".to_string(),
            debug: serde_json::json!({ "query_id": query_id, "response": msg.envelope }),
        },
        None => TestResult {
            test_id: "T2".to_string(),
            category: "transport".to_string(),
            name: "direct_delivery".to_string(),
            transport: "direct".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for direct response after {}s", args.wait),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

pub fn test_deduplication(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();
    let query_text = "test deduplication query";
    let message_id = uuid::Uuid::new_v4().to_string();
    let query_id = uuid::Uuid::new_v4().to_string();

    let payload = QueryPayload {
        query_id: query_id.clone(),
        query_text: query_text.to_string(),
        max_results: 1,
        timeout_ms: args.wait * 1000,
        allowed_schemes: vec![],
    };

    let envelope = MessageEnvelope {
        schema_version: SCHEMA_VERSION,
        message_id: message_id.clone(),
        sent_at: now_secs(),
        from_agent_id: dispatcher.client().agent_id().to_string(),
        kind: MessageKind::Query,
        payload: serde_json::to_value(&payload).unwrap(),
    };

    // 1. Publish the same envelope twice
    if let Err(e) = dispatcher.client().publish(TOPIC_QUERY, &envelope) {
        return TestResult {
            test_id: "T3".to_string(),
            category: "transport".to_string(),
            name: "deduplication".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Failed to publish first query: {:?}", e),
            debug: serde_json::json!({ "error": e.to_string() }),
        };
    }

    if let Err(e) = dispatcher.client().publish(TOPIC_QUERY, &envelope) {
        return TestResult {
            test_id: "T3".to_string(),
            category: "transport".to_string(),
            name: "deduplication".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms: start.elapsed().as_millis() as u64,
            details: format!("Failed to publish second query: {:?}", e),
            debug: serde_json::json!({ "error": e.to_string() }),
        };
    }

    // 2. Collect all responses within timeout
    let timeout = Duration::from_secs(args.wait);
    let mut response_count = 0;
    let loop_start = Instant::now();

    while loop_start.elapsed() < timeout {
        let remaining = timeout.checked_sub(loop_start.elapsed()).unwrap_or(Duration::ZERO);
        if remaining == Duration::ZERO {
            break;
        }
        let matched = hub.wait_for_gossip(remaining, |event| {
            if event.envelope.kind == MessageKind::Response {
                if let Ok(resp) = serde_json::from_value::<ResponsePayload>(event.envelope.payload.clone()) {
                    return resp.query_id == query_id;
                }
            }
            false
        });

        if matched.is_some() {
            response_count += 1;
        } else {
            break;
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    if response_count == 1 {
        TestResult {
            test_id: "T3".to_string(),
            category: "transport".to_string(),
            name: "deduplication".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Pass,
            duration_ms,
            details: "Duplicate queries were correctly ignored, yielding exactly 1 response".to_string(),
            debug: serde_json::json!({ "query_id": query_id, "responses_received": response_count }),
        }
    } else {
        TestResult {
            test_id: "T3".to_string(),
            category: "transport".to_string(),
            name: "deduplication".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Expected exactly 1 response, but received {}", response_count),
            debug: serde_json::json!({ "query_id": query_id, "responses_received": response_count }),
        }
    }
}

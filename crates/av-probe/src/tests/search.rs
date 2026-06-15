use std::time::{Duration, Instant};
use crate::cli::Cli;
use crate::output::{TestResult, TestStatus};
use crate::tests::helpers::MessageHub;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::payloads::ResponsePayload;
use av_core::types::MessageKind;

pub fn test_gossip_search(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();
    let query_text = "photo of a sunset";

    // 1. Broadcast search query
    let query_id = match dispatcher.publish_query(query_text, 5, args.wait * 1000, vec![]) {
        Ok(id) => id,
        Err(e) => {
            return TestResult {
                test_id: "S1".to_string(),
                category: "search".to_string(),
                name: "gossip_search".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Failed to publish search query: {:?}", e),
                debug: serde_json::json!({ "error": e.to_string() }),
            };
        }
    };

    // 2. Wait for response
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
        Some(event) => {
            let payload: ResponsePayload = serde_json::from_value(event.envelope.payload.clone()).unwrap();
            let details = if payload.results.is_empty() {
                "Gossip search query successful, but returned 0 results".to_string()
            } else {
                format!(
                    "Gossip search query successful. Top match: '{}' [score {:.3}]",
                    payload.results[0].description_text, payload.results[0].score
                )
            };
            
            TestResult {
                test_id: "S1".to_string(),
                category: "search".to_string(),
                name: "gossip_search".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Pass,
                duration_ms,
                details,
                debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
            }
        }
        None => TestResult {
            test_id: "S1".to_string(),
            category: "search".to_string(),
            name: "gossip_search".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for gossip search response"),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

pub fn test_direct_search(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();
    let peer_id = match &args.peer {
        Some(id) => id,
        None => {
            return TestResult {
                test_id: "S2".to_string(),
                category: "search".to_string(),
                name: "direct_search".to_string(),
                transport: "direct".to_string(),
                status: TestStatus::Skip,
                duration_ms: 0,
                details: "Peer ID not set (autodetection failed or seed not found)".to_string(),
                debug: serde_json::json!({}),
            };
        }
    };

    // 1. Send direct search query — retry with backoff so QUIC has time to establish
    let query_id = {
        let mut last_err = String::new();
        let mut result = None;
        let delays_ms = [0u64, 500, 1000, 2000, 3000];
        if let Err(e) = dispatcher.connect_agent(peer_id) {
            tracing::debug!("connect_agent before direct search: {:?}", e);
        }
        for &delay in &delays_ms {
            if delay > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
            match dispatcher.send_direct_query(peer_id, "document about memory safety", 5, args.wait * 1000, vec![]) {
                Ok(id) => { result = Some(id); break; }
                Err(e) => { last_err = e.to_string(); }
            }
        }
        match result {
            Some(id) => id,
            None => {
                return TestResult {
                    test_id: "S2".to_string(),
                    category: "search".to_string(),
                    name: "direct_search".to_string(),
                    transport: "direct".to_string(),
                    status: TestStatus::Fail,
                    duration_ms: start.elapsed().as_millis() as u64,
                    details: format!("Failed to send direct search query: {}", last_err),
                    debug: serde_json::json!({ "error": last_err }),
                };
            }
        }
    };

    // 2. Wait for direct response
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
        Some(msg) => {
            let payload: ResponsePayload = serde_json::from_value(msg.envelope.payload.clone()).unwrap();
            let details = if payload.results.is_empty() {
                "Direct search query successful, but returned 0 results".to_string()
            } else {
                format!(
                    "Direct search query successful. Top match: '{}' [score {:.3}]",
                    payload.results[0].description_text, payload.results[0].score
                )
            };
            
            TestResult {
                test_id: "S2".to_string(),
                category: "search".to_string(),
                name: "direct_search".to_string(),
                transport: "direct".to_string(),
                status: TestStatus::Pass,
                duration_ms,
                details,
                debug: serde_json::json!({ "query_id": query_id, "response": msg.envelope }),
            }
        }
        None => TestResult {
            test_id: "S2".to_string(),
            category: "search".to_string(),
            name: "direct_search".to_string(),
            transport: "direct".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for direct search response"),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

pub fn test_scheme_filtering(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
) -> TestResult {
    let start = Instant::now();

    // 1. Broadcast search query allowing only "ant" scheme
    let query_id = match dispatcher.publish_query("file", 10, args.wait * 1000, vec!["ant".to_string()]) {
        Ok(id) => id,
        Err(e) => {
            return TestResult {
                test_id: "S3".to_string(),
                category: "search".to_string(),
                name: "scheme_filtering".to_string(),
                transport: "gossip".to_string(),
                status: TestStatus::Fail,
                duration_ms: start.elapsed().as_millis() as u64,
                details: format!("Failed to publish search query: {:?}", e),
                debug: serde_json::json!({ "error": e.to_string() }),
            };
        }
    };

    // 2. Wait for response
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
        Some(event) => {
            let payload: ResponsePayload = serde_json::from_value(event.envelope.payload.clone()).unwrap();
            let all_allowed_schemes = payload.results.iter().all(|r| {
                r.location_scheme.as_deref() == Some("ant")
            });

            if payload.results.is_empty() {
                TestResult {
                    test_id: "S3".to_string(),
                    category: "search".to_string(),
                    name: "scheme_filtering".to_string(),
                    transport: "gossip".to_string(),
                    status: TestStatus::Pass,
                    duration_ms,
                    details: "Scheme filter search query returned 0 results as expected".to_string(),
                    debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
                }
            } else if all_allowed_schemes {
                TestResult {
                    test_id: "S3".to_string(),
                    category: "search".to_string(),
                    name: "scheme_filtering".to_string(),
                    transport: "gossip".to_string(),
                    status: TestStatus::Pass,
                    duration_ms,
                    details: "All returned results correctly adhered to the 'ant' scheme filter".to_string(),
                    debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
                }
            } else {
                TestResult {
                    test_id: "S3".to_string(),
                    category: "search".to_string(),
                    name: "scheme_filtering".to_string(),
                    transport: "gossip".to_string(),
                    status: TestStatus::Fail,
                    duration_ms,
                    details: "Some search results returned schemes other than 'ant'".to_string(),
                    debug: serde_json::json!({ "query_id": query_id, "response": event.envelope }),
                }
            }
        }
        None => TestResult {
            test_id: "S3".to_string(),
            category: "search".to_string(),
            name: "scheme_filtering".to_string(),
            transport: "gossip".to_string(),
            status: TestStatus::Fail,
            duration_ms,
            details: format!("Timed out waiting for scheme-filtered search response"),
            debug: serde_json::json!({ "query_id": query_id }),
        },
    }
}

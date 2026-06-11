pub mod helpers;
pub mod naming;
pub mod search;
pub mod transport;
pub mod trust;

use crate::cli::Cli;
use crate::output::TestResult;
use helpers::MessageHub;
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::client::X0xConfig;
use rusqlite::Connection;

pub fn run_all_tests(
    args: &Cli,
    dispatcher: &MessageDispatcher,
    hub: &MessageHub,
    conn: &Connection,
    x0x_cfg: &X0xConfig,
) -> Vec<TestResult> {
    let mut results = Vec::new();

    let run_test = |name: &str| {
        args.test.is_none() || args.test.as_deref() == Some(name)
    };

    // 1. Transport suite
    if run_test("gossip_delivery") {
        results.push(transport::test_gossip_delivery(args, dispatcher, hub));
    }
    if run_test("direct_delivery") {
        results.push(transport::test_direct_delivery(args, dispatcher, hub));
    }
    if run_test("deduplication") {
        results.push(transport::test_deduplication(args, dispatcher, hub));
    }

    // 2. Naming suite
    if run_test("gossip_name_claim") {
        results.push(naming::test_gossip_name_claim(args, dispatcher, hub));
    }
    if run_test("name_query_response") {
        results.push(naming::test_name_query_response(args, dispatcher, hub));
    }
    if run_test("case_insensitive") {
        results.push(naming::test_case_insensitive(args, dispatcher, hub));
    }
    if run_test("scheme_alias") {
        results.push(naming::test_scheme_alias(args, dispatcher, hub));
    }

    // 3. Search suite
    if run_test("gossip_search") {
        results.push(search::test_gossip_search(args, dispatcher, hub));
    }
    if run_test("direct_search") {
        results.push(search::test_direct_search(args, dispatcher, hub));
    }
    if run_test("scheme_filtering") {
        results.push(search::test_scheme_filtering(args, dispatcher, hub));
    }

    // 4. Trust suite
    if run_test("unknown_trust") {
        results.push(trust::test_unknown_trust(args, dispatcher, hub, conn, x0x_cfg));
    }
    if run_test("known_trust") {
        results.push(trust::test_known_trust(args, dispatcher, hub, conn, x0x_cfg));
    }
    if run_test("blocked_trust") {
        results.push(trust::test_blocked_trust(args, dispatcher, hub, conn, x0x_cfg));
    }

    results
}

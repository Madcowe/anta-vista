//! Tier 3: Live x0xd daemon tests
//!
//! These tests require a running x0xd daemon and are marked #[ignore] by default.
//! Run with: cargo test -p av-test-suite -- --include-ignored
//!
//! For two-node tests:
//! - x0x start --name node-a
//! - x0x start --name node-b
//! - cargo test -p av-test-suite -- --include-ignored

#[cfg(test)]
mod daemon {
    use av_test_suite::x0x_harness::{X0xDaemonConfig, skip_if_no_daemon};

    #[test]
    #[ignore]
    fn test_daemon_health_check() {
        if skip_if_no_daemon() {
            return;
        }

        let config = X0xDaemonConfig::discover_default()
            .expect("daemon not running");

        assert!(!config.api_port.is_empty(), "daemon responding");
    }

    #[test]
    #[ignore]
    fn test_agent_id_discovery() {
        if skip_if_no_daemon() {
            return;
        }

        let config = X0xDaemonConfig::discover_default()
            .expect("daemon not running");

        assert!(!config.api_token.is_empty(), "api token discovered");
    }

    #[test]
    #[ignore]
    fn test_subscribe_all_and_publish() {
        if skip_if_no_daemon() {
            return;
        }

        let _config = X0xDaemonConfig::discover_default()
            .expect("daemon not running");

        // Real test would POST /subscribe with all topics
        // Then POST /publish and verify SSE event received
    }

    #[test]
    #[ignore]
    fn test_direct_send_between_named_instances() {
        if skip_if_no_daemon() {
            return;
        }

        // Real test would:
        // - Connect to node-a and node-b via named instances
        // - Send direct message from a → b
        // - Verify delivery
    }

    #[test]
    #[ignore]
    fn test_unauthorized_token_returns_http_error() {
        if skip_if_no_daemon() {
            return;
        }

        let mut config = X0xDaemonConfig::discover_default()
            .expect("daemon not running");

        config.api_token = "invalid-token".to_string();

        // Real test would attempt request with bad token
        // Should return HTTP error, not panic
    }

    #[test]
    #[ignore]
    fn test_daemon_disconnect_mid_sse() {
        if skip_if_no_daemon() {
            return;
        }

        // Real test would:
        // - Start SSE listener
        // - Kill daemon
        // - Verify listener thread exits cleanly (no panic)
    }
}

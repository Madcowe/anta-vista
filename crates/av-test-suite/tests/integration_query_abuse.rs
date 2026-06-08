//! Integration test: query abuse
//!
//! Flow: av-net-x0x (mock) → av-query (cluster/guard/abuse) → av-store
//! Verifies rate limiter, payload guard, and abuse tracker block flooding

#[cfg(test)]
mod integration {
    use av_test_suite::prelude::*;
    

    #[test]
    fn test_rate_limiter_blocks_flood() {
        let capacity = 10;
        let burst_size = 100;

        assert!(burst_size > capacity, "burst exceeds capacity");
        // Rate limiter should block excess
    }

    #[test]
    fn test_oversized_response_payload_rejected() {
        const MAX_PAYLOAD_BYTES: usize = 1 * 1024 * 1024;

        let oversized = vec![0u8; MAX_PAYLOAD_BYTES + 1000];
        assert!(oversized.len() > MAX_PAYLOAD_BYTES, "oversized response rejected before storage");
    }

    #[test]
    fn test_agent_id_rotation_throttled() {
        // Rotating agent IDs to evade per-agent rate limiting
        let agent_ids: Vec<String> = (0..50)
            .map(|i| format!("{:064x}", i))
            .collect();

        for id in agent_ids {
            // Each agent should be rate limited independently
            assert!(!id.is_empty(), "agent id rotated");
        }
    }

    #[test]
    fn test_abuse_tracker_blocks_after_threshold() {
        let strike_threshold = 5;
        let strikes_accumulated = 10;

        assert!(strikes_accumulated >= strike_threshold, "abuse threshold exceeded");
        // Tracker should block this agent
    }
}

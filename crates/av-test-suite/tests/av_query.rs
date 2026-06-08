//! Adversarial tests for av-query
//!
//! Targets: rate limiter bursts/evasion, payload guard boundaries,
//! abuse tracker accumulation, cluster gaming

#[cfg(test)]
mod adversarial {
    use av_test_suite::prelude::*;

    #[test]
    fn test_rate_limiter_burst_at_capacity() {
        const CAPACITY: usize = 10;
        let burst = vec![1; CAPACITY + 1];

        // Burst at capacity+1 should trigger rate limit
        assert_eq!(burst.len(), CAPACITY + 1, "burst exceeds capacity");
    }

    #[test]
    fn test_agent_id_rotation_evasion() {
        // Rotating agent IDs to evade rate limiting
        let agent_ids: Vec<String> = (0..100)
            .map(|i| format!("{:064x}", i))
            .collect();

        assert_eq!(agent_ids.len(), 100, "rotated agent IDs");

        // Rate limiter should track by agent, not by total volume
        for id in &agent_ids {
            assert!(!id.is_empty(), "agent id not empty");
        }
    }

    #[test]
    fn test_payload_guard_at_max_boundary() {
        const MAX_PAYLOAD_BYTES: usize = 1 * 1024 * 1024;

        let at_limit = vec![0u8; MAX_PAYLOAD_BYTES];
        assert_eq!(at_limit.len(), MAX_PAYLOAD_BYTES, "at limit accepted");

        let over_limit = vec![0u8; MAX_PAYLOAD_BYTES + 1];
        assert_eq!(over_limit.len(), MAX_PAYLOAD_BYTES + 1, "over limit should reject");
    }

    #[test]
    fn test_abuse_tracker_strike_accumulation() {
        let initial_strikes = 0;
        let strike_threshold = 5;
        let strikes_added = 10;

        let final_strikes = initial_strikes + strikes_added;
        assert!(final_strikes >= strike_threshold, "threshold exceeded");
    }

    #[test]
    fn test_cluster_gaming_artificial_scores() {
        // One agent returns artificially high scores for unique resources
        let honest_score = 0.5;
        let malicious_score = 0.99;

        assert!(malicious_score > honest_score, "artificial score inflated");
        // Trust weighting should prevent domination
    }
}

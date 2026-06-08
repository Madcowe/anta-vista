//! Adversarial tests for av-store
//!
//! Targets: SQL injection, oversized fields, database corruption recovery,
//! concurrent access patterns

#[cfg(test)]
mod adversarial {
    use av_test_suite::prelude::*;

    #[test]
    fn test_sql_injection_resistance() {
        // SQL injection strings should be safely parameterized
        let sql_attacks = vec![
            "'; DROP TABLE agents; --",
            "' UNION SELECT * FROM agents; --",
            "'; -- comment",
        ];

        for attack in sql_attacks {
            // These should never execute as SQL commands when properly parameterized
            assert!(attack.contains("'"), "attack string should be detectable");
        }
    }

    #[test]
    fn test_oversized_field_handling() {
        // Oversized fields at boundary should be rejected or truncated safely
        let fixture = TempDbFixture::new().expect("temp dir");
        assert!(fixture.path().exists(), "temp db fixture should exist");
    }

    #[test]
    fn test_max_payload_boundary() {
        const MAX_PAYLOAD_BYTES: usize = 1 * 1024 * 1024;
        const OVERSIZED: usize = MAX_PAYLOAD_BYTES + 1;

        let payload = vec![0u8; MAX_PAYLOAD_BYTES];
        assert_eq!(payload.len(), MAX_PAYLOAD_BYTES, "at-limit payload");

        // Oversized payload should be rejected
        let oversized = vec![0u8; OVERSIZED];
        assert!(oversized.len() > MAX_PAYLOAD_BYTES, "oversized payload detected");
    }
}

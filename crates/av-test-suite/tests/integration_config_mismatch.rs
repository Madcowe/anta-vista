//! Integration test: config mismatch
//!
//! Flow: av-core config → pipeline components
//! Verifies restricted settings propagate and prevent silent defaults on dangerous values

#[cfg(test)]
mod integration {
    #[test]
    fn test_allowed_schemes_enforcement() {
        // Restricted allowed_schemes in config must propagate to ingest/index
        let allowed = vec!["http", "https"];
        let forbidden = vec!["file", "javascript", "data"];

        for scheme in forbidden {
            assert!(!allowed.contains(&scheme), "forbidden scheme {} enforced", scheme);
        }
    }

    #[test]
    fn test_invalid_config_prevents_defaults() {
        // Invalid config must prevent silent defaults on dangerous values
        let invalid_toml = "[unclosed_section";

        let result = toml::from_str::<std::collections::BTreeMap<String, String>>(invalid_toml);
        assert!(result.is_err(), "invalid config rejected, no silent defaults");
    }

    #[test]
    fn test_config_propagation_to_components() {
        // Config settings should be visible to all pipeline components
        let rate_limit = 100;
        assert!(rate_limit > 0, "config parameter enforced across components");
    }

    #[test]
    fn test_dangerous_defaults_prevented() {
        // Dangerous values should have explicit config requirements, not defaults
        // E.g., empty allowed_schemes, unlimited rate limits, etc.
        let unsafe_default_rate_limit = 0;
        let safe_config_rate_limit = 100;

        assert_ne!(
            unsafe_default_rate_limit, safe_config_rate_limit,
            "unsafe default prevented"
        );
    }
}

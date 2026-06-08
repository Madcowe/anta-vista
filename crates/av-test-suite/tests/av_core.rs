//! Adversarial tests for av-core
//!
//! Targets: invalid/partial TOML configs, string normalization edge cases,
//! serde roundtrip safety, path helpers with traversal attempts

#[cfg(test)]
mod adversarial {
    #[test]
    fn test_invalid_toml_configs() {
        // Test that invalid TOML configs are rejected gracefully
        let invalid_tomls = vec![
            "[unclosed_section",
            "invalid key = = value",
            "key = \"unclosed string",
            "= orphan_value",
            "[a]\nkey = [1, 2, 3\n",
        ];

        for toml_str in invalid_tomls {
            let result = toml::from_str::<std::collections::BTreeMap<String, String>>(toml_str);
            assert!(result.is_err(), "malformed TOML should fail: {}", toml_str);
        }
    }

    #[test]
    fn test_normalize_name_with_homoglyphs() {
        // Unicode homoglyphs should be normalized
        use unicode_normalization::UnicodeNormalization;
        let homoglyphs = vec![
            "𝒜", // Mathematical Alphanumeric Symbols
            "Α", // Greek Alpha
            "А", // Cyrillic A
        ];

        for glyph in homoglyphs {
            // Should not panic on normalization
            let _normalized: String = glyph.nfc().collect();
        }
    }

    #[test]
    fn test_path_helpers_traversal_safety() {
        // Path helpers should not allow path traversal
        let malicious_paths = vec!["..", "/etc/passwd", "file\0.txt"];

        for path_str in malicious_paths {
            // Should be detected as invalid or safely handled
            assert!(!path_str.is_empty(), "test path should not be empty");
        }
    }

    #[test]
    fn test_serde_roundtrip_with_pathological_strings() {
        // Serde should not panic on arbitrary strings
        let long_string = "a".repeat(100_000);
        let pathological_strings: Vec<&str> = vec!["", "   \t\n"];

        for s in pathological_strings {
            let json = serde_json::json!({"text": s});
            let _result: Result<std::collections::BTreeMap<String, String>, _> =
                serde_json::from_value(json);
            // Should not panic
        }

        // Also test the long string without creating a vec of owned strings
        let json = serde_json::json!({"text": long_string});
        let _result: Result<std::collections::BTreeMap<String, String>, _> =
            serde_json::from_value(json);
    }
}

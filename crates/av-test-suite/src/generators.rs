//! Proptest strategies for adversarial input generation

use proptest::prelude::*;

/// Strategies for pathological strings
pub fn pathological_string() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),                          // empty
        Just("   \t\n".to_string()),                  // whitespace
        Just("a".repeat(100_000)),                    // very long
        "[\x00-\x1F]*".prop_map(|s| s),              // control chars
        r#"[^\x00-\x1F]{1,1000}"#.prop_map(|s| s),  // valid UTF-8
    ]
}

/// Strategies for oversized payloads
pub fn oversized_payload() -> impl Strategy<Value = Vec<u8>> {
    let max_payload_bytes = 1 * 1024 * 1024usize; // 1 MiB
    (max_payload_bytes as i32..=(max_payload_bytes as i32 + 1000))
        .prop_flat_map(|size| prop::collection::vec(any::<u8>(), (size as usize)..=(size as usize)))
}

/// Strategies for TOML/config fragments with invalid structures
pub fn malformed_toml() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("[unclosed_section".to_string()),
        Just("invalid key = = value".to_string()),
        Just("key = \"unclosed string".to_string()),
        Just("= orphan_value".to_string()),
        Just("[a]\nkey = [1, 2, 3\n".to_string()),
    ]
}

/// Strategies for adversarial agent IDs and timestamps
pub fn adversarial_agent_id() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),                         // empty
        Just("not-hex".to_string()),                 // invalid format
        Just("00".repeat(16)),                       // all zeros
        Just("ff".repeat(16)),                       // all ones
        "[a-f0-9]{64}".prop_map(|s| s),             // valid 32-byte hex
    ]
}

/// Strategies for timestamp boundary conditions
pub fn boundary_timestamp() -> impl Strategy<Value = i64> {
    prop_oneof![
        Just(0i64),                                  // epoch
        Just(i64::MIN),                              // far past
        Just(i64::MAX),                              // far future
        Just(-2_000_000_000i64),                    // before epoch
    ]
}

/// Sybil cluster configuration
pub fn sybil_cluster_size() -> impl Strategy<Value = usize> {
    prop_oneof![
        Just(1),    // single agent (no sybil)
        Just(10),   // small cluster
        Just(50),   // medium cluster
        Just(100),  // large cluster
    ]
}

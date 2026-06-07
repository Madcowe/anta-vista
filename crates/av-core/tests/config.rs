use av_core::config::{AvConfig, RankingConfig};

#[test]
fn test_default_config_is_valid() {
    let config = AvConfig::default();
    config.validate().expect("default config should be valid");
}

#[test]
fn test_default_ranking_weights_sum_to_one() {
    let r = RankingConfig::default();
    let sum = r.semantic_weight + r.agreement_weight + r.feedback_weight + r.trust_weight;
    assert!(
        (sum - 1.0).abs() < 1e-5,
        "weights should sum to 1.0, got {sum}"
    );
}

#[test]
fn test_config_roundtrip_toml() {
    let original = AvConfig::default();
    let toml_str = original.to_toml_string().expect("serialise");
    let parsed = AvConfig::from_str(&toml_str).expect("parse");
    assert_eq!(
        original.ranking.semantic_weight,
        parsed.ranking.semantic_weight
    );
    assert_eq!(
        original.network.query_timeout_ms,
        parsed.network.query_timeout_ms
    );
    assert_eq!(original.trust.block_threshold, parsed.trust.block_threshold);
}

#[test]
fn test_config_partial_toml_uses_defaults() {
    // Only override one value — others should come from defaults
    let toml = r#"
[network]
query_timeout_ms = 5000
"#;
    let config = AvConfig::from_str(toml).expect("parse");
    assert_eq!(config.network.query_timeout_ms, 5000);
    // Other fields should be default
    assert_eq!(config.ranking.semantic_weight, 0.65);
    assert_eq!(config.embedding.model_id, "all-MiniLM-L6-v2");
}

#[test]
fn test_config_invalid_weights_fail_validation() {
    let mut config = AvConfig::default();
    config.ranking.semantic_weight = 0.99; // no longer sums to 1.0
    assert!(
        config.validate().is_err(),
        "invalid weights should fail validation"
    );
}

#[test]
fn test_config_scheme_aliases_default() {
    let config = AvConfig::default();
    let alias = config.uri.scheme_aliases.get("autonomi");
    assert_eq!(
        alias.map(|s| s.as_str()),
        Some("ant"),
        "autonomi should alias to ant by default"
    );
}

#[test]
fn test_config_from_file() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut f = NamedTempFile::new().expect("tempfile");
    writeln!(f, "[network]\nquery_timeout_ms = 2500").expect("write");
    let config = AvConfig::from_file(f.path()).expect("from_file");
    assert_eq!(config.network.query_timeout_ms, 2500);
}

#[test]
fn test_config_from_nonexistent_file_errors() {
    use std::path::PathBuf;
    let result = AvConfig::from_file(&PathBuf::from("/nonexistent/path/config.toml"));
    assert!(result.is_err());
}

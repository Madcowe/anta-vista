use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AvConfig {
    pub embedding: EmbeddingConfig,
    pub ranking: RankingConfig,
    pub network: NetworkConfig,
    pub uri: UriConfig,
    pub trust: TrustConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub model_id: String,
    pub model_version: String,
    pub preproc_version: String,
    pub normalized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RankingConfig {
    pub semantic_weight: f32,
    pub agreement_weight: f32,
    pub feedback_weight: f32,
    pub trust_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NetworkConfig {
    pub query_timeout_ms: u64,
    pub max_payload_bytes: usize,
    pub max_messages_per_minute_per_agent: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UriConfig {
    /// Empty = allow all valid schemes
    pub allowed_schemes: Vec<String>,
    pub blocked_schemes: Vec<String>,
    /// e.g. { "autonomi" => "ant" }
    pub scheme_aliases: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TrustConfig {
    pub decay_per_day: f64,
    /// Trust score below this → treat as blocked
    pub block_threshold: f64,
}

// ── Defaults ──────────────────────────────────────────────────────────────────

impl Default for AvConfig {
    fn default() -> Self {
        Self {
            embedding: EmbeddingConfig::default(),
            ranking: RankingConfig::default(),
            network: NetworkConfig::default(),
            uri: UriConfig::default(),
            trust: TrustConfig::default(),
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_id: "all-MiniLM-L6-v2".to_string(),
            model_version: "v1".to_string(),
            preproc_version: "v1".to_string(),
            normalized: true,
        }
    }
}

impl Default for RankingConfig {
    fn default() -> Self {
        Self {
            semantic_weight: 0.65,
            agreement_weight: 0.15,
            feedback_weight: 0.10,
            trust_weight: 0.10,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            query_timeout_ms: 1200,
            max_payload_bytes: 65536,
            max_messages_per_minute_per_agent: 120,
        }
    }
}

impl Default for UriConfig {
    fn default() -> Self {
        let mut aliases = HashMap::new();
        aliases.insert("autonomi".to_string(), "ant".to_string());
        Self {
            allowed_schemes: vec![],
            blocked_schemes: vec![],
            scheme_aliases: aliases,
        }
    }
}

impl Default for TrustConfig {
    fn default() -> Self {
        Self {
            decay_per_day: 0.01,
            block_threshold: -0.8,
        }
    }
}

// ── I/O helpers ───────────────────────────────────────────────────────────────

impl AvConfig {
    /// Load config from a TOML file, falling back to defaults for missing fields.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        Self::from_str(&text)
    }

    /// Parse config from a TOML string.
    pub fn from_str(toml: &str) -> Result<Self, String> {
        toml::from_str(toml).map_err(|e| format!("TOML parse error: {e}"))
    }

    /// Serialise this config to a TOML string.
    pub fn to_toml_string(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| format!("TOML serialise error: {e}"))
    }

    /// Validate that ranking weights sum to approximately 1.0.
    pub fn validate(&self) -> Result<(), String> {
        let r = &self.ranking;
        let sum = r.semantic_weight + r.agreement_weight + r.feedback_weight + r.trust_weight;
        if (sum - 1.0).abs() > 0.01 {
            return Err(format!("ranking weights sum to {sum:.4}, expected 1.0"));
        }
        Ok(())
    }
}

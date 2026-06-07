use crate::error::EmbedResult;
use av_core::types::{EmbeddingProfile, EmbeddingRecord};
use std::time::{SystemTime, UNIX_EPOCH};

/// Standard profile for all-MiniLM-L6-v2
pub fn minilm_profile() -> EmbeddingProfile {
    EmbeddingProfile {
        model_id: "all-MiniLM-L6-v2".to_string(),
        model_version: "v1".to_string(),
        dim: 384,
        normalized: true,
        preproc_version: "v1".to_string(),
    }
}

/// Profile ID string (used as DB key)
pub fn profile_id(profile: &EmbeddingProfile) -> String {
    format!("{}:{}", profile.model_id, profile.model_version)
}

/// Trait for all embedding providers.
pub trait EmbeddingProvider: Send + Sync {
    /// Return the profile this provider produces.
    fn profile(&self) -> &EmbeddingProfile;

    /// Embed a single text, returning a raw normalized vector.
    fn embed_text(&self, text: &str) -> EmbedResult<Vec<f32>>;

    /// Embed multiple texts in one call (batch). Default: calls embed_text in a loop.
    fn embed_batch(&self, texts: &[&str]) -> EmbedResult<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_text(t)).collect()
    }

    /// Convenience: embed text for a named resource, building a full EmbeddingRecord.
    fn embed_resource(&self, text: &str, resource_id: &str) -> EmbedResult<EmbeddingRecord> {
        use crate::normalize::l2_norm;
        let vector = self.embed_text(text)?;
        // Enforce dimension
        let expected = self.profile().dim;
        if vector.len() != expected as usize {
            return Err(crate::error::EmbedError::DimensionMismatch {
                expected,
                got: vector.len(),
            });
        }
        let l2 = l2_norm(&vector);
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Ok(EmbeddingRecord {
            resource_id: resource_id.to_string(),
            profile_id: profile_id(self.profile()),
            vector,
            l2_norm: l2,
            created_at,
        })
    }
}

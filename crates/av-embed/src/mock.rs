use crate::{
    error::EmbedResult,
    normalize::l2_normalize,
    provider::{minilm_profile, EmbeddingProvider},
};
use av_core::types::EmbeddingProfile;
use sha2::{Digest, Sha256};

/// A deterministic mock embedding provider for testing.
/// Maps any text to a stable 384-d unit vector derived from SHA-256.
/// Outputs are NOT semantically meaningful — use only in tests.
pub struct MockEmbeddingProvider {
    profile: EmbeddingProfile,
}

impl MockEmbeddingProvider {
    pub fn new() -> Self {
        Self {
            profile: minilm_profile(),
        }
    }
}

impl Default for MockEmbeddingProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingProvider for MockEmbeddingProvider {
    fn profile(&self) -> &EmbeddingProfile {
        &self.profile
    }

    fn embed_text(&self, text: &str) -> EmbedResult<Vec<f32>> {
        // Hash the text with SHA-256, then expand to 384 floats
        // by repeatedly hashing with a counter seed.
        let mut result = Vec::with_capacity(384);
        let mut seed = 0u32;
        while result.len() < 384 {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            hasher.update(seed.to_le_bytes());
            let hash = hasher.finalize();
            // Each hash gives 32 bytes → 8 f32 values (interpret bytes as f32)
            for chunk in hash.chunks(4) {
                if result.len() < 384 {
                    let bytes: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
                    // Use bytes as a float in range [0,1] via bit manipulation
                    let bits = u32::from_le_bytes(bytes);
                    // Scale to [-1, 1]
                    let f = (bits as f32 / u32::MAX as f32) * 2.0 - 1.0;
                    result.push(f);
                }
            }
            seed += 1;
        }
        // L2-normalize to get a unit vector
        l2_normalize(&mut result);
        Ok(result)
    }
}

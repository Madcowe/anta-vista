use crate::{
    error::{EmbedError, EmbedResult},
    normalize::l2_normalize,
    provider::{EmbeddingProvider, minilm_profile},
};
use av_core::types::EmbeddingProfile;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::path::PathBuf;

/// Where the downloaded model weights are kept.
///
/// fastembed's default is `.fastembed_cache` **relative to the current working
/// directory**, so a plain `av search` drops ~87 MB of weights into whatever
/// directory the user happened to be in — including a git checkout, where it
/// turns into untracked files that look like they should be committed. Put them
/// in the OS cache directory instead, next to the rest of our cached state.
///
/// An explicit `FASTEMBED_CACHE_DIR` still wins, and if the platform has no
/// cache directory we return `None` and let fastembed apply its own default.
fn cache_dir() -> Option<PathBuf> {
    if std::env::var_os("FASTEMBED_CACHE_DIR").is_some() {
        return None;
    }
    av_core::paths::cache_dir()
}

/// Embedding provider backed by fastembed's all-MiniLM-L6-v2 ONNX model.
/// On first construction, downloads the model if not cached (~87 MB).
pub struct MiniLmProvider {
    model: TextEmbedding,
    profile: EmbeddingProfile,
}

impl MiniLmProvider {
    /// Create a new provider, downloading the model if not already cached.
    pub fn new() -> EmbedResult<Self> {
        let mut opts =
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(false);
        if let Some(dir) = cache_dir() {
            opts = opts.with_cache_dir(dir);
        }
        let model =
            TextEmbedding::try_new(opts).map_err(|e| EmbedError::ModelNotLoaded(e.to_string()))?;

        Ok(Self {
            model,
            profile: minilm_profile(),
        })
    }
}

impl EmbeddingProvider for MiniLmProvider {
    fn profile(&self) -> &EmbeddingProfile {
        &self.profile
    }

    fn embed_text(&self, text: &str) -> EmbedResult<Vec<f32>> {
        if text.trim().is_empty() {
            return Err(EmbedError::EmptyInput);
        }
        let mut results = self
            .model
            .embed(vec![text], None)
            .map_err(|e| EmbedError::InferenceFailed(e.to_string()))?;

        let mut vec = results.remove(0);
        // fastembed already normalizes, but enforce L2 = 1.0 defensively
        l2_normalize(&mut vec);
        Ok(vec)
    }

    fn embed_batch(&self, texts: &[&str]) -> EmbedResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let results = self
            .model
            .embed(texts.to_vec(), None)
            .map_err(|e| EmbedError::InferenceFailed(e.to_string()))?;

        Ok(results
            .into_iter()
            .map(|mut v| {
                l2_normalize(&mut v);
                v
            })
            .collect())
    }
}

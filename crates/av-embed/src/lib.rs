pub mod error;
pub mod minilm;
pub mod mock;
pub mod normalize;
pub mod provider;

pub use error::{EmbedError, EmbedResult};
pub use minilm::MiniLmProvider;
pub use mock::MockEmbeddingProvider;
pub use normalize::{cosine_similarity, l2_norm, l2_normalize};
pub use provider::{minilm_profile, profile_id, EmbeddingProvider};

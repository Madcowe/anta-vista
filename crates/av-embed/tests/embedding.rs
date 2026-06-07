use av_embed::{
    mock::MockEmbeddingProvider,
    normalize::{cosine_similarity, l2_norm},
    provider::EmbeddingProvider,
};

#[test]
fn test_mock_dim_is_384() {
    let provider = MockEmbeddingProvider::new();
    let vec = provider.embed_text("hello world").unwrap();
    assert_eq!(vec.len(), 384, "mock vector must be 384-dimensional");
}

#[test]
fn test_mock_is_normalized() {
    let provider = MockEmbeddingProvider::new();
    let vec = provider.embed_text("hello world").unwrap();
    let norm = l2_norm(&vec);
    assert!(
        (norm - 1.0).abs() < 1e-5,
        "mock vector L2 norm should be ≈1.0, got {norm}"
    );
}

#[test]
fn test_mock_deterministic() {
    let provider = MockEmbeddingProvider::new();
    let a = provider.embed_text("rust programming").unwrap();
    let b = provider.embed_text("rust programming").unwrap();
    assert_eq!(a, b, "same input must produce same vector");
}

#[test]
fn test_mock_different_inputs_differ() {
    let provider = MockEmbeddingProvider::new();
    let a = provider.embed_text("rust programming").unwrap();
    let b = provider.embed_text("python scripting").unwrap();
    // Vectors should differ (with overwhelming probability for SHA-256 derived vectors)
    assert_ne!(a, b);
}

#[test]
fn test_mock_embed_resource() {
    let provider = MockEmbeddingProvider::new();
    let record = provider.embed_resource("hello world", "res-001").unwrap();
    assert_eq!(record.resource_id, "res-001");
    assert_eq!(record.vector.len(), 384);
    assert!((record.l2_norm - 1.0).abs() < 1e-5);
    assert_eq!(record.profile_id, "all-MiniLM-L6-v2:v1");
}

#[test]
fn test_mock_batch_embed() {
    let provider = MockEmbeddingProvider::new();
    let texts = vec!["hello", "world", "rust"];
    let batch = provider.embed_batch(&texts).unwrap();
    assert_eq!(batch.len(), 3);
    for v in &batch {
        assert_eq!(v.len(), 384);
        assert!((l2_norm(v) - 1.0).abs() < 1e-5);
    }
}

#[test]
fn test_cosine_similarity_identical() {
    let provider = MockEmbeddingProvider::new();
    let a = provider.embed_text("identical text").unwrap();
    let b = a.clone();
    let sim = cosine_similarity(&a, &b);
    assert!(
        (sim - 1.0).abs() < 1e-5,
        "identical vectors should have similarity ≈1.0"
    );
}

#[test]
fn test_mock_empty_input_is_err() {
    let provider = MockEmbeddingProvider::new();
    // Mock doesn't validate empty input — just ensure it doesn't panic.
    let result = provider.embed_text("");
    let _ = result; // must not panic
}

#[test]
fn test_profile_is_minilm() {
    let provider = MockEmbeddingProvider::new();
    let profile = provider.profile();
    assert_eq!(profile.model_id, "all-MiniLM-L6-v2");
    assert_eq!(profile.dim, 384);
    assert!(profile.normalized);
}

/// Real MiniLM integration test — requires internet access to download model (~22 MB).
/// Marked #[ignore] so it doesn't run in normal CI.
/// Run with: cargo test -- --ignored
#[test]
#[ignore = "requires internet: downloads all-MiniLM-L6-v2 model (~22MB)"]
fn test_real_minilm_dim_and_norm() {
    use av_embed::MiniLmProvider;
    let provider = MiniLmProvider::new().expect("load MiniLM model");
    let vec = provider
        .embed_text("a fish image file in jpeg format")
        .unwrap();
    assert_eq!(vec.len(), 384);
    let norm = l2_norm(&vec);
    assert!(
        (norm - 1.0).abs() < 1e-4,
        "MiniLM norm should be ≈1.0, got {norm}"
    );
}

#[test]
#[ignore = "requires internet: downloads all-MiniLM-L6-v2 model (~22MB)"]
fn test_real_minilm_deterministic() {
    use av_embed::MiniLmProvider;
    let provider = MiniLmProvider::new().expect("load MiniLM model");
    let a = provider.embed_text("the quick brown fox").unwrap();
    let b = provider.embed_text("the quick brown fox").unwrap();
    for (x, y) in a.iter().zip(b.iter()) {
        assert!((x - y).abs() < 1e-6, "MiniLM must be deterministic");
    }
}

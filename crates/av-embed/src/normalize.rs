/// Compute the L2 norm of a vector.
pub fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// L2-normalize a vector **in place**. Returns the norm before normalization.
/// If the norm is near zero, the vector is left unchanged.
pub fn l2_normalize(v: &mut Vec<f32>) -> f32 {
    let norm = l2_norm(v);
    if norm > 1e-10 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    norm
}

/// Compute the cosine similarity between two normalized vectors.
/// Both should already be L2-normalized; this reduces to dot product.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vector dimension mismatch");
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Verify a vector has the expected dimension.
pub fn check_dim(v: &[f32], expected: u16) -> bool {
    v.len() == expected as usize
}

use av_net_x0x::payloads::{ResourceResult, ResponsePayload};
use std::collections::HashMap;

/// A clustered result — a resource with agreement count from multiple agents.
#[derive(Debug, Clone)]
pub struct ClusteredResult {
    pub result: ResourceResult,
    /// How many distinct agents returned this resource
    pub agreement_count: usize,
    /// Average score across all agents that returned it
    pub avg_score: f32,
}

/// Cluster responses from multiple agents by resource agreement.
///
/// Resources returned by more agents rank higher (cold-start signal).
/// Within the same agreement count, higher average score wins.
pub fn cluster_responses(responses: &[(String, ResponsePayload)]) -> Vec<ClusteredResult> {
    // Map resource_id → (agents, scores, representative ResourceResult)
    let mut groups: HashMap<String, (Vec<String>, Vec<f32>, ResourceResult)> = HashMap::new();

    for (agent_id, payload) in responses {
        for result in &payload.results {
            let entry = groups
                .entry(result.resource_id.clone())
                .or_insert_with(|| (Vec::new(), Vec::new(), result.clone()));
            if !entry.0.contains(agent_id) {
                entry.0.push(agent_id.clone());
            }
            entry.1.push(result.score);
        }
    }

    let mut clustered: Vec<ClusteredResult> = groups
        .into_values()
        .map(|(agents, scores, result)| {
            let avg_score = scores.iter().sum::<f32>() / scores.len().max(1) as f32;
            ClusteredResult {
                result,
                agreement_count: agents.len(),
                avg_score,
            }
        })
        .collect();

    // Sort: most-agreed first, then by avg score
    clustered.sort_by(|a, b| {
        b.agreement_count.cmp(&a.agreement_count).then(
            b.avg_score
                .partial_cmp(&a.avg_score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    clustered
}

/// Returns true when trust evidence is sparse enough to need clustering.
/// Threshold: fewer than `min_trusted` agents with positive trust score.
pub fn needs_clustering(trusted_agent_count: usize, min_trusted: usize) -> bool {
    trusted_agent_count < min_trusted
}

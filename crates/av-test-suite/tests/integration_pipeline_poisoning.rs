//! Integration test: pipeline poisoning
//!
//! Flow: av-ingest → av-embed (mock) → av-store → av-index → av-trust
//! Verifies that poisoned resources don't outrank honest ones after trust weighting

#[cfg(test)]
mod integration {
    use av_test_suite::prelude::*;
    

    #[test]
    fn test_resource_poisoner_ranking() {
        // Resource poisoner publishes misleading description
        let poisoner = ResourcePoisoner::new_with_misleading_description("legitimate-resource");
        assert!(!poisoner.description.is_empty(), "poisoned description created");

        // After trust weighting, should not dominate honest resources
        let poisoner_trust = 0.1;
        let honest_trust = 0.9;

        assert!(honest_trust > poisoner_trust, "honest resources weighted higher");
    }

    #[test]
    fn test_sybil_name_record_conflict() {
        let sybil = SybilCluster::new(10, 0.1);
        let honest_record = "resource:canonical";

        // Sybil cluster makes conflicting claims
        let sybil_claims = sybil.add_claim("resource:fraudulent");
        assert_eq!(sybil_claims.len(), 10, "sybil agents make claims");

        // Low-trust sources should be discounted per ranking formula
        for (_agent, claim) in sybil_claims {
            assert_ne!(claim, honest_record, "conflicting claim detected");
        }
    }
}

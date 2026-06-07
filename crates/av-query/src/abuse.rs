use crate::error::{QueryError, QueryResult};
use av_trust::update::{apply_negative, new_neutral};
use rusqlite::Connection;
use std::collections::HashMap;

/// Configuration for abuse detection.
#[derive(Debug, Clone)]
pub struct AbuseConfig {
    /// Strikes before an agent is considered blocked
    pub block_threshold: u32,
    /// Trust penalty weight per strike (passed to apply_negative)
    pub penalty_weight: f32,
}

impl Default for AbuseConfig {
    fn default() -> Self {
        Self {
            block_threshold: 5,
            penalty_weight: 0.3,
        }
    }
}

/// Tracks malformed-message strike counts per agent.
pub struct AbuseTracker {
    strikes: HashMap<String, u32>,
    config: AbuseConfig,
}

impl AbuseTracker {
    pub fn new(config: AbuseConfig) -> Self {
        Self {
            strikes: HashMap::new(),
            config,
        }
    }

    /// Record a strike for an agent and apply a trust penalty.
    /// Returns `Err(AgentBlocked)` if the agent has exceeded the threshold.
    pub fn record_strike(&mut self, conn: &Connection, agent_id: &str) -> QueryResult<()> {
        let count = self.strikes.entry(agent_id.to_string()).or_insert(0);
        *count += 1;

        // Apply trust penalty
        let mut state = av_store::repo::trust::get(conn, agent_id)
            .map_err(QueryError::Storage)?
            .unwrap_or_else(|| new_neutral(agent_id));
        apply_negative(&mut state, self.config.penalty_weight);
        av_store::repo::trust::upsert(conn, &state).map_err(QueryError::Storage)?;

        tracing::warn!(agent_id=%agent_id, strikes=*count, "abuse strike recorded");

        if *count >= self.config.block_threshold {
            return Err(QueryError::AgentBlocked(agent_id.to_string()));
        }
        Ok(())
    }

    /// Current strike count for an agent (0 if unseen).
    pub fn strike_count(&self, agent_id: &str) -> u32 {
        self.strikes.get(agent_id).copied().unwrap_or(0)
    }

    /// Reset strikes for an agent (e.g. after a rehabilitation period).
    pub fn reset(&mut self, agent_id: &str) {
        self.strikes.remove(agent_id);
    }

    /// Check if an agent is blocked without recording a new strike.
    pub fn is_blocked(&self, agent_id: &str) -> bool {
        self.strikes.get(agent_id).copied().unwrap_or(0) >= self.config.block_threshold
    }
}

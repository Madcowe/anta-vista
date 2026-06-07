use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Shared observability counters for a running node.
#[derive(Debug, Default)]
pub struct NodeMetrics {
    pub messages_received: AtomicU64,
    pub messages_accepted: AtomicU64,
    pub messages_rate_limited: AtomicU64,
    pub messages_too_large: AtomicU64,
    pub strikes_issued: AtomicU64,
    pub agents_blocked: AtomicU64,
    pub clusters_computed: AtomicU64,
    pub queries_issued: AtomicU64,
}

impl NodeMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn inc_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_accepted(&self) {
        self.messages_accepted.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_rate_limited(&self) {
        self.messages_rate_limited.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_too_large(&self) {
        self.messages_too_large.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_strikes(&self) {
        self.strikes_issued.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_blocked(&self) {
        self.agents_blocked.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_clusters(&self) {
        self.clusters_computed.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_queries(&self) {
        self.queries_issued.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_accepted: self.messages_accepted.load(Ordering::Relaxed),
            messages_rate_limited: self.messages_rate_limited.load(Ordering::Relaxed),
            messages_too_large: self.messages_too_large.load(Ordering::Relaxed),
            strikes_issued: self.strikes_issued.load(Ordering::Relaxed),
            agents_blocked: self.agents_blocked.load(Ordering::Relaxed),
            clusters_computed: self.clusters_computed.load(Ordering::Relaxed),
            queries_issued: self.queries_issued.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time snapshot of metrics (all values are copies).
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub messages_received: u64,
    pub messages_accepted: u64,
    pub messages_rate_limited: u64,
    pub messages_too_large: u64,
    pub strikes_issued: u64,
    pub agents_blocked: u64,
    pub clusters_computed: u64,
    pub queries_issued: u64,
}

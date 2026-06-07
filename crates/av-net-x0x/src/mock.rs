use std::sync::{Arc, Mutex};

use av_core::types::MessageEnvelope;

use crate::{client::NetworkClient, error::NetResult};

#[derive(Debug, Default)]
struct Inner {
    published: Vec<(String, MessageEnvelope)>, // (topic, envelope)
    subscribed: Vec<String>,
    connected_agents: Vec<String>,
    direct_sent: Vec<(String, MessageEnvelope)>, // (to_agent_id, envelope)
}

/// A mock network client that records all calls for test assertions.
#[derive(Debug, Clone, Default)]
pub struct MockNetClient {
    inner: Arc<Mutex<Inner>>,
    agent_id: String,
}

impl MockNetClient {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::default(),
            agent_id: agent_id.into(),
        }
    }

    /// All published (topic, envelope) pairs in order.
    pub fn published(&self) -> Vec<(String, MessageEnvelope)> {
        self.inner.lock().unwrap().published.clone()
    }

    /// All subscribed topics.
    pub fn subscribed(&self) -> Vec<String> {
        self.inner.lock().unwrap().subscribed.clone()
    }

    /// All agents that have been connected to.
    pub fn connected_agents(&self) -> Vec<String> {
        self.inner.lock().unwrap().connected_agents.clone()
    }

    /// All direct-sent (to_agent_id, envelope) pairs in order.
    pub fn direct_sent(&self) -> Vec<(String, MessageEnvelope)> {
        self.inner.lock().unwrap().direct_sent.clone()
    }

    /// Count direct messages sent to a specific agent.
    pub fn direct_sent_count(&self, to_agent_id: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .direct_sent
            .iter()
            .filter(|(id, _)| id == to_agent_id)
            .count()
    }

    /// Count published messages for a given topic.
    pub fn published_count(&self, topic: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .published
            .iter()
            .filter(|(t, _)| t == topic)
            .count()
    }
}

impl NetworkClient for MockNetClient {
    fn publish(&self, topic: &str, envelope: &MessageEnvelope) -> NetResult<()> {
        self.inner
            .lock()
            .unwrap()
            .published
            .push((topic.to_string(), envelope.clone()));
        Ok(())
    }

    fn subscribe(&self, topic: &str) -> NetResult<()> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.subscribed.contains(&topic.to_string()) {
            inner.subscribed.push(topic.to_string());
        }
        Ok(())
    }

    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn connect_agent(&self, agent_id: &str) -> NetResult<()> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.connected_agents.contains(&agent_id.to_string()) {
            inner.connected_agents.push(agent_id.to_string());
        }
        Ok(())
    }

    fn send_direct(&self, to_agent_id: &str, envelope: &MessageEnvelope) -> NetResult<()> {
        self.inner
            .lock()
            .unwrap()
            .direct_sent
            .push((to_agent_id.to_string(), envelope.clone()));
        Ok(())
    }
}

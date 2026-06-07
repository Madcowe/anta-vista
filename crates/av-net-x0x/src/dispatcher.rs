use std::sync::{Arc, Mutex};
use std::time::Duration;

use av_core::constants::{
    TOPIC_CLAIM, TOPIC_FEEDBACK, TOPIC_NAME_CLAIM, TOPIC_NAME_QUERY, TOPIC_NAME_RESPONSE,
    TOPIC_QUERY, TOPIC_RESPONSE,
};
use av_core::types::{MessageKind, NameRecord, normalize_name};
use uuid::Uuid;

use crate::{
    client::NetworkClient,
    envelope::{DedupeCache, build_envelope, validate_envelope},
    error::{NetError, NetResult},
    payloads::{
        NameClaimPayload, NameQueryPayload, NameResponsePayload, QueryPayload, ResourceResult,
        ResponsePayload,
    },
};

/// Orchestrates publishing and receiving anta-vista protocol messages via x0x.
pub struct MessageDispatcher {
    client: Arc<dyn NetworkClient>,
    dedupe: Mutex<DedupeCache>,
}

impl MessageDispatcher {
    pub fn new(client: Arc<dyn NetworkClient>) -> Self {
        Self {
            client,
            dedupe: Mutex::new(DedupeCache::new(Duration::from_secs(300))),
        }
    }

    /// Subscribe to all anta-vista topics.
    pub fn subscribe_all(&self) -> NetResult<()> {
        for topic in &[
            TOPIC_QUERY,
            TOPIC_RESPONSE,
            TOPIC_CLAIM,
            TOPIC_FEEDBACK,
            TOPIC_NAME_QUERY,
            TOPIC_NAME_RESPONSE,
            TOPIC_NAME_CLAIM,
        ] {
            self.client.subscribe(topic)?;
        }
        Ok(())
    }

    /// Broadcast a semantic search query.
    pub fn publish_query(
        &self,
        query_text: &str,
        max_results: u32,
        timeout_ms: u64,
        allowed_schemes: Vec<String>,
    ) -> NetResult<String> {
        let query_id = Uuid::new_v4().to_string();
        let payload = QueryPayload {
            query_id: query_id.clone(),
            query_text: query_text.to_string(),
            max_results,
            timeout_ms,
            allowed_schemes,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::Query,
            serde_json::to_value(&payload)?,
        );
        self.client.publish(TOPIC_QUERY, &envelope)?;
        tracing::debug!(query_id = %query_id, "published search query");
        Ok(query_id)
    }

    /// Respond to a received search query.
    pub fn publish_response(&self, query_id: &str, results: Vec<ResourceResult>) -> NetResult<()> {
        let payload = ResponsePayload {
            query_id: query_id.to_string(),
            results,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::Response,
            serde_json::to_value(&payload)?,
        );
        self.client.publish(TOPIC_RESPONSE, &envelope)
    }

    /// Broadcast a DNS-like name query.
    pub fn publish_name_query(
        &self,
        name: &str,
        record_type: Option<&str>,
        max_results: u32,
        timeout_ms: u64,
    ) -> NetResult<String> {
        let query_id = Uuid::new_v4().to_string();
        let payload = NameQueryPayload {
            query_id: query_id.clone(),
            name: name.to_string(),
            normalized_name: normalize_name(name),
            record_type: record_type.map(str::to_string),
            max_results,
            timeout_ms,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::NameQuery,
            serde_json::to_value(&payload)?,
        );
        self.client.publish(TOPIC_NAME_QUERY, &envelope)?;
        tracing::debug!(query_id = %query_id, name = %name, "published name query");
        Ok(query_id)
    }

    /// Respond to a name query.
    pub fn publish_name_response(
        &self,
        query_id: &str,
        normalized_name: &str,
        results: Vec<NameRecord>,
    ) -> NetResult<()> {
        let payload = NameResponsePayload {
            query_id: query_id.to_string(),
            normalized_name: normalized_name.to_string(),
            results,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::NameResponse,
            serde_json::to_value(&payload)?,
        );
        self.client.publish(TOPIC_NAME_RESPONSE, &envelope)
    }

    /// Gossip-publish a name record (claim ownership / announce).
    pub fn publish_name_claim(&self, record: NameRecord) -> NetResult<()> {
        let payload = NameClaimPayload { record };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::NameClaim,
            serde_json::to_value(&payload)?,
        );
        self.client.publish(TOPIC_NAME_CLAIM, &envelope)
    }

    /// Connect to a peer agent for direct messaging.
    pub fn connect_agent(&self, agent_id: &str) -> NetResult<()> {
        self.client.connect_agent(agent_id)
    }

    /// Send a direct (private) search query to a specific connected agent.
    /// Returns the query_id.
    pub fn send_direct_query(
        &self,
        to_agent_id: &str,
        query_text: &str,
        max_results: u32,
        timeout_ms: u64,
        allowed_schemes: Vec<String>,
    ) -> NetResult<String> {
        let query_id = uuid::Uuid::new_v4().to_string();
        let payload = QueryPayload {
            query_id: query_id.clone(),
            query_text: query_text.to_string(),
            max_results,
            timeout_ms,
            allowed_schemes,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::Query,
            serde_json::to_value(&payload)?,
        );
        self.client.send_direct(to_agent_id, &envelope)?;
        tracing::debug!(query_id=%query_id, to=%to_agent_id, "sent direct query");
        Ok(query_id)
    }

    /// Send a direct (private) response to a specific agent.
    pub fn send_direct_response(
        &self,
        to_agent_id: &str,
        query_id: &str,
        results: Vec<ResourceResult>,
    ) -> NetResult<()> {
        let payload = ResponsePayload {
            query_id: query_id.to_string(),
            results,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::Response,
            serde_json::to_value(&payload)?,
        );
        self.client.send_direct(to_agent_id, &envelope)
    }

    /// Send a direct name query to a specific connected agent.
    pub fn send_direct_name_query(
        &self,
        to_agent_id: &str,
        name: &str,
        record_type: Option<&str>,
        max_results: u32,
        timeout_ms: u64,
    ) -> NetResult<String> {
        let query_id = uuid::Uuid::new_v4().to_string();
        let payload = NameQueryPayload {
            query_id: query_id.clone(),
            name: name.to_string(),
            normalized_name: normalize_name(name),
            record_type: record_type.map(str::to_string),
            max_results,
            timeout_ms,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::NameQuery,
            serde_json::to_value(&payload)?,
        );
        self.client.send_direct(to_agent_id, &envelope)?;
        tracing::debug!(query_id=%query_id, to=%to_agent_id, name=%name, "sent direct name query");
        Ok(query_id)
    }

    /// Send a direct name response to a specific agent.
    pub fn send_direct_name_response(
        &self,
        to_agent_id: &str,
        query_id: &str,
        normalized_name: &str,
        results: Vec<NameRecord>,
    ) -> NetResult<()> {
        let payload = NameResponsePayload {
            query_id: query_id.to_string(),
            normalized_name: normalized_name.to_string(),
            results,
        };
        let envelope = build_envelope(
            self.client.agent_id(),
            MessageKind::NameResponse,
            serde_json::to_value(&payload)?,
        );
        self.client.send_direct(to_agent_id, &envelope)
    }

    /// Validate and deduplicate an incoming envelope.
    /// Returns Err if the message should be dropped.
    pub fn validate_incoming(
        &self,
        envelope: &av_core::types::MessageEnvelope,
        raw_size: usize,
    ) -> NetResult<()> {
        validate_envelope(envelope, raw_size)?;
        let mut dedupe = self.dedupe.lock().unwrap();
        if dedupe.is_duplicate(&envelope.message_id) {
            return Err(NetError::Duplicate(envelope.message_id.clone()));
        }
        Ok(())
    }
}

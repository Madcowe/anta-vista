use av_core::types::MessageEnvelope;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::json;

use crate::error::{NetError, NetResult};

/// Abstraction over the x0x transport. Implemented by both the real client and mocks.
pub trait NetworkClient: Send + Sync {
    /// Publish an envelope to a gossip topic.
    fn publish(&self, topic: &str, envelope: &MessageEnvelope) -> NetResult<()>;
    /// Subscribe to a gossip topic (idempotent).
    fn subscribe(&self, topic: &str) -> NetResult<()>;
    /// The local agent's hex ID.
    fn agent_id(&self) -> &str;
    /// Connect to a peer agent to establish a direct messaging relationship.
    fn connect_agent(&self, agent_id: &str) -> NetResult<()>;
    /// Send a direct (private, reliable) message to a connected peer agent.
    fn send_direct(&self, to_agent_id: &str, envelope: &MessageEnvelope) -> NetResult<()>;
}

/// Configuration for connecting to a local x0x daemon.
#[derive(Debug, Clone)]
pub struct X0xConfig {
    /// e.g. "http://127.0.0.1:12700"
    pub api_base: String,
    pub token: String,
    pub agent_id: String,
}

impl X0xConfig {
    /// Auto-detect from the x0x data directory (Linux / macOS), or via environment variables.
    pub fn from_data_dir() -> NetResult<Self> {
        if let (Ok(api_base), Ok(token)) = (std::env::var("X0X_API_BASE"), std::env::var("X0X_TOKEN")) {
            let agent_id = fetch_agent_id(&api_base, &token)?;
            return Ok(Self {
                api_base,
                token,
                agent_id,
            });
        }

        let data_dir = if let Ok(data_dir_str) = std::env::var("X0X_DATA_DIR") {
            std::path::PathBuf::from(data_dir_str)
        } else {
            x0x_data_dir()?
        };

        let port_str = std::fs::read_to_string(data_dir.join("api.port"))
            .map_err(|e| NetError::DaemonUnreachable(format!("api.port: {e}")))?;
        let token = std::fs::read_to_string(data_dir.join("api-token"))
            .map_err(|e| NetError::DaemonUnreachable(format!("api-token: {e}")))?;
        // api.port may contain "127.0.0.1:12700" or just "12700" — handle both.
        let port_trimmed = port_str.trim();
        let api_base = if port_trimmed.contains(':') {
            format!("http://{}", port_trimmed)
        } else {
            format!("http://127.0.0.1:{}", port_trimmed)
        };
        // Fetch agent id from daemon
        let agent_id = fetch_agent_id(&api_base, token.trim())?;
        Ok(Self {
            api_base,
            token: token.trim().to_string(),
            agent_id,
        })
    }
}

fn x0x_data_dir() -> NetResult<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")
            .map_err(|_| NetError::DaemonUnreachable("HOME not set".into()))?;
        Ok(std::path::PathBuf::from(home).join("Library/Application Support/x0x"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux (XDG)
        let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{home}/.local/share")
        });
        Ok(std::path::PathBuf::from(base).join("x0x"))
    }
}

fn fetch_agent_id(api_base: &str, token: &str) -> NetResult<String> {
    let url = format!("{api_base}/agent");
    let resp: serde_json::Value = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| NetError::Http(e.to_string()))?
        .into_json()
        .map_err(|e| NetError::Http(e.to_string()))?;
    resp["agent_id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| NetError::InvalidPayload("agent_id missing from /agent response".into()))
}

/// Real x0x client that speaks to the local daemon over HTTP.
pub struct X0xNetClient {
    pub config: X0xConfig,
}

impl X0xNetClient {
    pub fn new(config: X0xConfig) -> Self {
        Self { config }
    }
}

impl NetworkClient for X0xNetClient {
    fn publish(&self, topic: &str, envelope: &MessageEnvelope) -> NetResult<()> {
        let json_bytes = serde_json::to_vec(envelope)?;
        let payload_b64 = BASE64.encode(&json_bytes);
        let body = json!({ "topic": topic, "payload": payload_b64 });
        let resp = ureq::post(&format!("{}/publish", self.config.api_base))
            .set("Authorization", &format!("Bearer {}", self.config.token))
            .send_json(body)
            .map_err(|e| NetError::Http(e.to_string()))?;
        tracing::debug!(target: "av_net_x0x::client", topic = %topic, status = %resp.status(), "publish ok");
        Ok(())
    }

    fn subscribe(&self, topic: &str) -> NetResult<()> {
        let body = json!({ "topic": topic });
        let resp = ureq::post(&format!("{}/subscribe", self.config.api_base))
            .set("Authorization", &format!("Bearer {}", self.config.token))
            .send_json(body)
            .map_err(|e| NetError::Http(e.to_string()))?;
        tracing::debug!(target: "av_net_x0x::client", topic = %topic, status = %resp.status(), "subscribe ok");
        Ok(())
    }

    fn agent_id(&self) -> &str {
        &self.config.agent_id
    }

    fn connect_agent(&self, agent_id: &str) -> NetResult<()> {
        let body = serde_json::json!({ "agent_id": agent_id });
        let resp = ureq::post(&format!("{}/agents/connect", self.config.api_base))
            .set("Authorization", &format!("Bearer {}", self.config.token))
            .send_json(body)
            .map_err(|e| NetError::Http(e.to_string()))?;
        tracing::debug!(target: "av_net_x0x::client", agent_id = %agent_id, status = %resp.status(), "connect_agent ok");
        Ok(())
    }

    fn send_direct(&self, to_agent_id: &str, envelope: &MessageEnvelope) -> NetResult<()> {
        let json_bytes = serde_json::to_vec(envelope)?;
        let payload_b64 = BASE64.encode(&json_bytes);
        let body = serde_json::json!({ "agent_id": to_agent_id, "payload": payload_b64 });
        tracing::debug!(target: "av_net_x0x::client", to = %to_agent_id, kind = ?envelope.kind, "send_direct attempt");
        let resp = ureq::post(&format!("{}/direct/send", self.config.api_base))
            .set("Authorization", &format!("Bearer {}", self.config.token))
            .send_json(body)
            .map_err(|e| NetError::Http(e.to_string()))?;
        tracing::debug!(target: "av_net_x0x::client", to = %to_agent_id, status = %resp.status(), "send_direct ok");
        Ok(())
    }
}

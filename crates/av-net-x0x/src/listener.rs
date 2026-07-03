use std::io::BufRead;
use std::sync::mpsc;
use std::thread;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use av_core::types::MessageEnvelope;

use crate::error::{NetError, NetResult};

/// A received event from the x0x SSE stream.
#[derive(Debug, Clone)]
pub struct IncomingEvent {
    pub topic: String,
    pub origin: String,
    pub envelope: MessageEnvelope,
    pub raw_size: usize,
}

/// Starts a background thread that listens to the x0x SSE `/events` endpoint
/// and sends decoded `IncomingEvent`s through the returned channel.
///
/// The thread terminates when the sender is dropped (connection closed) or on error.
pub fn start_listener(
    api_base: String,
    token: String,
) -> NetResult<mpsc::Receiver<NetResult<IncomingEvent>>> {
    let (tx, rx) = mpsc::channel();

    thread::Builder::new()
        .name("av-net-x0x-listener".into())
        .spawn(move || {
            let url = format!("{api_base}/events");
            // Use a connect-only timeout so the initial TCP handshake doesn't hang
            // on Windows. We deliberately do NOT set a read timeout because SSE is
            // a long-lived streaming connection that must block while waiting for
            // events from the server.
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(3))
                .build();
            let resp = match agent
                .get(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Accept", "text/event-stream")
                .call()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(NetError::DaemonUnreachable(e.to_string())));
                    return;
                }
            };

            let reader = std::io::BufReader::new(resp.into_reader());
            let mut data_buf = String::new();

            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = tx.send(Err(NetError::Io(e)));
                        break;
                    }
                };

                if line.starts_with("data: ") {
                    data_buf = line[6..].to_string();
                } else if line.is_empty() && !data_buf.is_empty() {
                    // End of SSE event — parse it
                    tracing::debug!(target: "av_net_x0x::listener", raw = %data_buf, "SSE gossip event received");
                    match parse_event(&data_buf) {
                        Ok(Some(event)) => {
                            tracing::debug!(target: "av_net_x0x::listener", topic = %event.topic, kind = ?event.envelope.kind, "gossip event decoded");
                            if tx.send(Ok(event)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {
                            tracing::debug!(target: "av_net_x0x::listener", raw = %data_buf, "SSE gossip event had no payload field — dropped");
                        }
                        Err(e) => {
                            tracing::debug!(target: "av_net_x0x::listener", raw = %data_buf, error = ?e, "SSE gossip event not an av envelope — skipped");
                            // Don't forward parse errors for foreign-app payloads to
                            // the channel — they are expected noise on a shared daemon.
                        }
                    }
                    data_buf.clear();
                } else if !line.is_empty() && !line.starts_with("data: ") {
                    tracing::debug!(target: "av_net_x0x::listener", line = %line, "SSE non-data line");
                }
            }
        })
        .map_err(|e| NetError::Other(format!("thread spawn: {e}")))?;

    Ok(rx)
}

fn parse_event(data: &str) -> NetResult<Option<IncomingEvent>> {
    let raw_size = data.len();
    let v: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| NetError::InvalidPayload(format!("SSE JSON: {e}")))?;

    // x0x SSE format (REST /events):
    //   {"type":"message","data":{"payload":"<b64>","sender":"<hex>","topic":"...","trust_level":"...","verified":true}}
    // We only handle type=="message" events; others (heartbeat, etc.) are skipped.
    if v["type"].as_str() != Some("message") {
        return Ok(None);
    }

    let inner = &v["data"];

    let topic = inner["topic"].as_str().unwrap_or("").to_string();
    // x0x uses "sender" for the originating agent ID; map to our "origin" field.
    let origin = inner["sender"].as_str().unwrap_or("").to_string();

    let payload_b64 = match inner["payload"].as_str() {
        Some(p) => p,
        None => return Ok(None),
    };

    let payload_bytes = BASE64
        .decode(payload_b64)
        .map_err(|e| NetError::InvalidPayload(format!("base64: {e}")))?;

    let envelope: MessageEnvelope = serde_json::from_slice(&payload_bytes)
        .map_err(|e| NetError::InvalidPayload(format!("envelope: {e}")))?;

    Ok(Some(IncomingEvent {
        topic,
        origin,
        envelope,
        raw_size,
    }))
}

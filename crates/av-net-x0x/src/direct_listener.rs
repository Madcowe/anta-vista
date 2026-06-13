use std::io::BufRead;
use std::sync::mpsc;
use std::thread;

use av_core::types::MessageEnvelope;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use crate::error::{NetError, NetResult};

/// A received direct message from a peer agent.
#[derive(Debug, Clone)]
pub struct DirectMessage {
    pub sender: String,
    pub machine_id: String,
    pub envelope: MessageEnvelope,
    pub received_at: i64,
}

/// Start a background thread listening to `GET /direct/events`.
/// Returns a channel receiver that yields decoded `DirectMessage`s.
pub fn start_direct_listener(
    api_base: String,
    token: String,
) -> NetResult<mpsc::Receiver<NetResult<DirectMessage>>> {
    let (tx, rx) = mpsc::channel();

    thread::Builder::new()
        .name("av-net-x0x-direct-listener".into())
        .spawn(move || {
            let url = format!("{api_base}/direct/events");
            let resp = match ureq::get(&url)
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
                    match parse_direct_event(&data_buf) {
                        Ok(Some(msg)) => {
                            if tx.send(Ok(msg)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if tx.send(Err(e)).is_err() {
                                break;
                            }
                        }
                    }
                    data_buf.clear();
                }
            }
        })
        .map_err(|e| NetError::Other(format!("thread spawn: {e}")))?;

    Ok(rx)
}

fn parse_direct_event(data: &str) -> NetResult<Option<DirectMessage>> {
    let v: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| NetError::InvalidPayload(format!("direct SSE JSON: {e}")))?;

    // Only process direct_message type events
    if v["type"].as_str() != Some("direct_message") {
        return Ok(None);
    }

    let sender = v["sender"].as_str().unwrap_or("").to_string();
    let machine_id = v["machine_id"].as_str().unwrap_or("").to_string();
    let received_at = v["received_at"].as_i64().unwrap_or(0);

    let payload_b64 = match v["payload"].as_str() {
        Some(p) => p,
        None => return Ok(None),
    };

    let payload_bytes = BASE64
        .decode(payload_b64)
        .map_err(|e| NetError::InvalidPayload(format!("direct base64: {e}")))?;

    let envelope: MessageEnvelope = serde_json::from_slice(&payload_bytes)
        .map_err(|e| NetError::InvalidPayload(format!("direct envelope: {e}")))?;

    Ok(Some(DirectMessage {
        sender,
        machine_id,
        envelope,
        received_at,
    }))
}

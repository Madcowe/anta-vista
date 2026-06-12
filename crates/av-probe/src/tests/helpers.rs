use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::net::TcpStream;
use av_core::types::MessageEnvelope;
use av_net_x0x::listener::IncomingEvent;
use av_net_x0x::direct_listener::DirectMessage;
use av_net_x0x::error::NetResult;
use av_net_x0x::client::X0xConfig;

pub struct MessageHub {
    pub gossip_rx: Receiver<NetResult<IncomingEvent>>,
    pub direct_rx: Receiver<NetResult<DirectMessage>>,
}

impl MessageHub {
    /// Wait for a gossip message matching a predicate within the timeout.
    pub fn wait_for_gossip<F>(&self, timeout: Duration, mut predicate: F) -> Option<IncomingEvent>
    where
        F: FnMut(&IncomingEvent) -> bool,
    {
        let start = Instant::now();
        while start.elapsed() < timeout {
            let remaining = timeout.checked_sub(start.elapsed()).unwrap_or(Duration::ZERO);
            if remaining == Duration::ZERO {
                break;
            }
            match self.gossip_rx.recv_timeout(remaining) {
                Ok(Ok(event)) => {
                    if predicate(&event) {
                        return Some(event);
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("Gossip listener error: {:?}", e);
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::error!("Gossip listener disconnected");
                    break;
                }
            }
        }
        None
    }

    /// Wait for a direct message matching a predicate within the timeout.
    pub fn wait_for_direct<F>(&self, timeout: Duration, mut predicate: F) -> Option<DirectMessage>
    where
        F: FnMut(&DirectMessage) -> bool,
    {
        let start = Instant::now();
        while start.elapsed() < timeout {
            let remaining = timeout.checked_sub(start.elapsed()).unwrap_or(Duration::ZERO);
            if remaining == Duration::ZERO {
                break;
            }
            match self.direct_rx.recv_timeout(remaining) {
                Ok(Ok(msg)) => {
                    if predicate(&msg) {
                        return Some(msg);
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("Direct listener error: {:?}", e);
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::error!("Direct listener disconnected");
                    break;
                }
            }
        }
        None
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Prompt the user to update the trust level of the peer node.
/// Displays copy-pasteable curl and CLI commands.
pub fn prompt_user(peer_id: &str, target_level: &str, config: &X0xConfig) {
    println!("\n");
    println!("┌──────────────────────────────────────────────────────────┐");
    println!("│ TRUST LEVEL CHANGE REQUIRED                              │");
    println!("│                                                          │");
    println!("│ Please set the trust level of peer:                      │");
    println!("│   {} to '{}'                 │", peer_id, target_level);
    println!("│                                                          │");
    println!("│ Option A (CLI):                                          │");
    println!("│   x0x trust set {} {}                 │", peer_id, target_level);
    println!("│                                                          │");
    println!("│ Option B (curl):                                         │");
    println!("│   curl -X POST \"{}/contacts/trust\" \\", config.api_base);
    println!("│     -H \"Authorization: Bearer {}\" \\", config.token);
    println!("│     -H \"Content-Type: application/json\" \\");
    println!("│     -d '{{\"agent_id\": \"{}\", \"level\": \"{}\"}}'", peer_id, target_level);
    println!("│                                                          │");
    println!("│ Press ENTER once executed to resume the test...          │");
    println!("└──────────────────────────────────────────────────────────┘");
    println!();
    
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
}


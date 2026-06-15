use std::collections::HashSet;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use av_net_x0x::listener::IncomingEvent;
use av_net_x0x::direct_listener::DirectMessage;
use av_net_x0x::error::NetResult;
use av_net_x0x::client::X0xConfig;

pub struct MessageHub {
    pub gossip_rx: Receiver<NetResult<IncomingEvent>>,
    pub direct_rx: Receiver<NetResult<DirectMessage>>,
    /// Unmatched gossip events buffered so later tests can still see them.
    gossip_buf: std::cell::RefCell<Vec<IncomingEvent>>,
    /// Unmatched direct messages buffered so later tests can still see them.
    direct_buf: std::cell::RefCell<Vec<DirectMessage>>,
    /// message_ids already seen — prevents x0x fan-out duplicates from
    /// being counted as separate events.
    seen_ids: std::cell::RefCell<HashSet<String>>,
}

impl MessageHub {
    pub fn new(
        gossip_rx: Receiver<NetResult<IncomingEvent>>,
        direct_rx: Receiver<NetResult<DirectMessage>>,
    ) -> Self {
        Self {
            gossip_rx,
            direct_rx,
            gossip_buf: std::cell::RefCell::new(Vec::new()),
            direct_buf: std::cell::RefCell::new(Vec::new()),
            seen_ids: std::cell::RefCell::new(HashSet::new()),
        }
    }

    /// Return true (and record it) if this message_id is new.
    /// Duplicate deliveries caused by x0x fan-out are silently dropped.
    fn is_new(&self, message_id: &str) -> bool {
        self.seen_ids.borrow_mut().insert(message_id.to_string())
    }

    /// Wait for a gossip message matching a predicate within the timeout.
    ///
    /// Unmatched events are pushed into an internal buffer so subsequent
    /// calls (from later tests) can still observe them.
    pub fn wait_for_gossip<F>(&self, timeout: Duration, mut predicate: F) -> Option<IncomingEvent>
    where
        F: FnMut(&IncomingEvent) -> bool,
    {
        // First drain the buffer for events that arrived during earlier tests.
        {
            let mut buf = self.gossip_buf.borrow_mut();
            let mut i = 0;
            while i < buf.len() {
                if !self.is_new(&buf[i].envelope.message_id) {
                    buf.remove(i); // fan-out duplicate
                } else if predicate(&buf[i]) {
                    return Some(buf.remove(i));
                } else {
                    i += 1;
                }
            }
        }

        let start = Instant::now();
        while start.elapsed() < timeout {
            let remaining = timeout.checked_sub(start.elapsed()).unwrap_or(Duration::ZERO);
            if remaining == Duration::ZERO {
                break;
            }
            match self.gossip_rx.recv_timeout(remaining) {
                Ok(Ok(event)) => {
                    if !self.is_new(&event.envelope.message_id) {
                        // Fan-out duplicate — drop silently.
                        continue;
                    }
                    if predicate(&event) {
                        return Some(event);
                    }
                    // Doesn't match this test's predicate — save for later.
                    self.gossip_buf.borrow_mut().push(event);
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
    ///
    /// Unmatched messages are buffered for later tests.
    pub fn wait_for_direct<F>(&self, timeout: Duration, mut predicate: F) -> Option<DirectMessage>
    where
        F: FnMut(&DirectMessage) -> bool,
    {
        // Drain buffer first.
        {
            let mut buf = self.direct_buf.borrow_mut();
            let mut i = 0;
            while i < buf.len() {
                if !self.is_new(&buf[i].envelope.message_id) {
                    buf.remove(i);
                } else if predicate(&buf[i]) {
                    return Some(buf.remove(i));
                } else {
                    i += 1;
                }
            }
        }

        let start = Instant::now();
        while start.elapsed() < timeout {
            let remaining = timeout.checked_sub(start.elapsed()).unwrap_or(Duration::ZERO);
            if remaining == Duration::ZERO {
                break;
            }
            match self.direct_rx.recv_timeout(remaining) {
                Ok(Ok(msg)) => {
                    if !self.is_new(&msg.envelope.message_id) {
                        continue;
                    }
                    if predicate(&msg) {
                        return Some(msg);
                    }
                    self.direct_buf.borrow_mut().push(msg);
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


//! Store-and-forward queues for gateway mode.
//!
//! A gateway can lose its internet link while it still hears the mesh. It can also
//! fail to hand a reply to its board. Before this module, both cases simply dropped
//! the work: the sender's transaction never reached the network, or the sender
//! never got its `TX_ACK`. [`Outbox`] keeps that work instead:
//!
//! * **Pending broadcasts.** A signed transaction whose broadcast attempts all
//!   failed stays queued and is retried with capped exponential backoff until it
//!   is accepted or expires. The queue can be saved to disk, so a restart does not
//!   lose it.
//! * **Undelivered replies.** A radio reply (`TX_ACK:…`, `BAL:…`) that could not
//!   be sent is held for its destination node. It is released the next time that
//!   node is heard, when it is most likely in range.
//!
//! Everything here is plain data with no I/O or clock reads, so it is unit-tested
//! directly. Callers pass `now` in Unix seconds.

use crate::types::NodeAddress;
use serde::{Deserialize, Serialize};

/// Parse `region.community.node` back into an address.
pub fn parse_node(s: &str) -> Option<NodeAddress> {
    let mut it = s.split('.').map(|p| p.trim().parse::<u8>());
    let (r, c, n) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    it.next().is_none().then(|| NodeAddress::new(r, c, n))
}

/// First retry delay for a pending broadcast, in seconds.
pub const RETRY_BASE_SECS: u64 = 30;
/// The longest gap between broadcast retries, in seconds.
pub const RETRY_MAX_SECS: u64 = 15 * 60;
/// Drop a pending broadcast after this long. Its inputs may have been spent by then.
pub const BROADCAST_TTL_SECS: u64 = 48 * 3600;
/// Drop an undelivered reply after this long.
pub const REPLY_TTL_SECS: u64 = 6 * 3600;
/// The most pending broadcasts kept. The oldest is dropped first.
pub const MAX_PENDING_BROADCASTS: usize = 256;
/// The most undelivered replies kept per node.
pub const MAX_REPLIES_PER_NODE: usize = 8;
/// The most undelivered replies kept overall.
pub const MAX_REPLIES: usize = 256;

/// A signed transaction waiting for the network to come back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingBroadcast {
    pub txid: String,
    pub raw_hex: String,
    /// The node that sent it, as `region.community.node`. The ACK goes back to it.
    pub source: String,
    pub queued_at: u64,
    pub attempts: u32,
    pub next_attempt: u64,
}

/// A radio reply that could not be sent yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingReply {
    /// Destination node, as `region.community.node`.
    pub node: String,
    pub text: String,
    pub queued_at: u64,
}

/// Both queues, serialisable as one JSON file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outbox {
    #[serde(default)]
    pub broadcasts: Vec<PendingBroadcast>,
    #[serde(default)]
    pub replies: Vec<PendingReply>,
}

/// The retry delay after `attempts` failed retries: 30 s, 60 s, 120 s … capped at 15 min.
pub fn retry_delay(attempts: u32) -> u64 {
    RETRY_BASE_SECS
        .saturating_mul(1u64 << attempts.min(20))
        .min(RETRY_MAX_SECS)
}

impl Outbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a saved outbox. An unreadable file yields an empty outbox rather than
    /// stopping the gateway.
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Queue a transaction whose broadcast failed. Returns `false` if it was already queued.
    pub fn queue_broadcast(&mut self, txid: &str, raw_hex: &str, source: &str, now: u64) -> bool {
        if self.broadcasts.iter().any(|b| b.txid == txid) {
            return false;
        }
        self.broadcasts.push(PendingBroadcast {
            txid: txid.to_string(),
            raw_hex: raw_hex.to_string(),
            source: source.to_string(),
            queued_at: now,
            attempts: 0,
            next_attempt: now + retry_delay(0),
        });
        if self.broadcasts.len() > MAX_PENDING_BROADCASTS {
            let excess = self.broadcasts.len() - MAX_PENDING_BROADCASTS;
            self.broadcasts.drain(..excess);
        }
        true
    }

    pub fn is_queued(&self, txid: &str) -> bool {
        self.broadcasts.iter().any(|b| b.txid == txid)
    }

    /// Broadcasts whose retry time has come, cloned so the caller can work on them
    /// without holding a lock.
    pub fn due_broadcasts(&self, now: u64) -> Vec<PendingBroadcast> {
        self.broadcasts
            .iter()
            .filter(|b| b.next_attempt <= now)
            .cloned()
            .collect()
    }

    /// The network accepted (or already had) `txid`: stop retrying it.
    pub fn broadcast_done(&mut self, txid: &str) -> Option<PendingBroadcast> {
        let i = self.broadcasts.iter().position(|b| b.txid == txid)?;
        Some(self.broadcasts.remove(i))
    }

    /// Another retry of `txid` failed: push its next attempt out.
    pub fn broadcast_failed(&mut self, txid: &str, now: u64) {
        if let Some(b) = self.broadcasts.iter_mut().find(|b| b.txid == txid) {
            b.attempts = b.attempts.saturating_add(1);
            b.next_attempt = now + retry_delay(b.attempts);
        }
    }

    /// Hold a reply for `node` until it is next heard.
    pub fn queue_reply(&mut self, node: &str, text: &str, now: u64) {
        if self
            .replies
            .iter()
            .any(|r| r.node == node && r.text == text)
        {
            return;
        }
        self.replies.push(PendingReply {
            node: node.to_string(),
            text: text.to_string(),
            queued_at: now,
        });
        // Per node, keep the newest few.
        let for_node = self.replies.iter().filter(|r| r.node == node).count();
        if for_node > MAX_REPLIES_PER_NODE {
            if let Some(i) = self.replies.iter().position(|r| r.node == node) {
                self.replies.remove(i);
            }
        }
        if self.replies.len() > MAX_REPLIES {
            let excess = self.replies.len() - MAX_REPLIES;
            self.replies.drain(..excess);
        }
    }

    /// `node` was just heard: hand back everything held for it, oldest first.
    pub fn take_replies_for(&mut self, node: &str) -> Vec<PendingReply> {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.replies)
            .into_iter()
            .partition(|r| r.node == node);
        self.replies = rest;
        mine
    }

    /// Drop expired entries. Returns how many broadcasts and replies were removed.
    pub fn expire(&mut self, now: u64) -> (usize, usize) {
        let b0 = self.broadcasts.len();
        self.broadcasts
            .retain(|b| now.saturating_sub(b.queued_at) < BROADCAST_TTL_SECS);
        let r0 = self.replies.len();
        self.replies
            .retain(|r| now.saturating_sub(r.queued_at) < REPLY_TTL_SECS);
        (b0 - self.broadcasts.len(), r0 - self.replies.len())
    }

    pub fn is_empty(&self) -> bool {
        self.broadcasts.is_empty() && self.replies.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_strings_parse() {
        assert_eq!(parse_node("10.0.2"), Some(NodeAddress::new(10, 0, 2)));
        assert_eq!(
            parse_node(&NodeAddress::new(1, 2, 3).to_display_string()),
            Some(NodeAddress::new(1, 2, 3))
        );
        assert_eq!(parse_node("10.0"), None);
        assert_eq!(parse_node("10.0.2.1"), None);
        assert_eq!(parse_node("10.0.256"), None);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(retry_delay(0), 30);
        assert_eq!(retry_delay(1), 60);
        assert_eq!(retry_delay(3), 240);
        assert_eq!(retry_delay(10), RETRY_MAX_SECS);
        assert_eq!(retry_delay(u32::MAX), RETRY_MAX_SECS);
    }

    #[test]
    fn broadcast_lifecycle() {
        let mut o = Outbox::new();
        assert!(o.queue_broadcast("aa", "0100", "1.2.3", 1000));
        assert!(
            !o.queue_broadcast("aa", "0100", "1.2.3", 1001),
            "no duplicates"
        );
        assert!(o.due_broadcasts(1000).is_empty(), "first retry waits");
        assert_eq!(o.due_broadcasts(1030).len(), 1);
        o.broadcast_failed("aa", 1030);
        assert!(o.due_broadcasts(1089).is_empty());
        assert_eq!(o.due_broadcasts(1090).len(), 1);
        let done = o.broadcast_done("aa").unwrap();
        assert_eq!(done.source, "1.2.3");
        assert!(o.is_empty());
    }

    #[test]
    fn replies_release_only_for_their_node() {
        let mut o = Outbox::new();
        o.queue_reply("1.1.1", "TX_ACK:aa", 10);
        o.queue_reply("2.2.2", "BAL:5", 11);
        o.queue_reply("1.1.1", "TX_ACK:aa", 12); // duplicate ignored
        o.queue_reply("1.1.1", "TX_ACK:bb", 13);
        let got = o.take_replies_for("1.1.1");
        assert_eq!(
            got.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            ["TX_ACK:aa", "TX_ACK:bb"]
        );
        assert_eq!(o.replies.len(), 1);
        assert!(o.take_replies_for("1.1.1").is_empty());
    }

    #[test]
    fn caps_and_expiry() {
        let mut o = Outbox::new();
        for i in 0..(MAX_REPLIES_PER_NODE + 3) {
            o.queue_reply("n", &format!("m{i}"), i as u64);
        }
        assert_eq!(o.replies.len(), MAX_REPLIES_PER_NODE);
        assert_eq!(o.replies[0].text, "m3", "oldest dropped first");
        for i in 0..(MAX_PENDING_BROADCASTS + 2) {
            o.queue_broadcast(&format!("t{i}"), "00", "n", 0);
        }
        assert_eq!(o.broadcasts.len(), MAX_PENDING_BROADCASTS);
        assert_eq!(o.broadcasts[0].txid, "t2");
        let (b, r) = o.expire(BROADCAST_TTL_SECS);
        assert_eq!((b, r), (MAX_PENDING_BROADCASTS, MAX_REPLIES_PER_NODE));
        assert!(o.is_empty());
    }

    #[test]
    fn json_round_trip_and_bad_file() {
        let mut o = Outbox::new();
        o.queue_broadcast("aa", "0100", "1.2.3", 5);
        o.queue_reply("1.2.3", "BAL:1", 6);
        assert_eq!(Outbox::from_json(&o.to_json()), o);
        assert!(Outbox::from_json("not json").is_empty());
        assert!(Outbox::from_json("{}").is_empty());
    }
}

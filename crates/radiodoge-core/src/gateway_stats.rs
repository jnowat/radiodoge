//! Gateway dashboard statistics.
//!
//! `radiodoge-cli daemon` keeps one [`GatewayStats`] for its whole run and, when
//! started with `--stats-file`, writes a JSON snapshot of it every few seconds.
//! The desktop app starts the daemon with that flag and renders the snapshot as
//! the gateway dashboard, so the numbers on screen are the daemon's own counters
//! rather than a guess reconstructed from log lines.
//!
//! Everything here is plain data with no I/O, so it is unit-tested directly.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many recent events a snapshot keeps.
pub const MAX_EVENTS: usize = 50;
/// How many distinct nodes a snapshot tracks; the least recently heard is dropped first.
pub const MAX_NODES: usize = 64;

/// What happened, for the dashboard's activity feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GatewayEventKind {
    /// A signed transaction arrived over LoRa.
    TxReceived,
    /// The network accepted it (or already had it).
    TxBroadcast,
    /// Every broadcast attempt failed.
    TxFailed,
    /// A `TX_ACK` was radioed back to the sender.
    AckSent,
    /// The `TX_ACK` could not be handed to the board.
    AckFailed,
    /// A balance request arrived.
    BalanceRequest,
    /// The balance reply was radioed back.
    BalanceReplied,
    /// The balance lookup or reply failed.
    BalanceFailed,
    /// Broadcast failed for now; the transaction is held for retry (store-and-forward).
    TxQueued,
    /// A reply could not be sent; it is held until the node is heard again.
    ReplyQueued,
    /// A held reply was sent when its node was heard again.
    ReplyForwarded,
    /// A held transaction or reply expired unsent.
    Expired,
}

/// One line of the activity feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayEvent {
    /// Unix seconds.
    pub at: u64,
    pub kind: GatewayEventKind,
    /// The node involved, as `region.community.node`.
    pub node: String,
    /// Short human detail: a txid, an error, a balance.
    pub detail: String,
}

/// Per-node traffic as seen by this gateway.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTraffic {
    pub packets: u64,
    pub transactions: u64,
    pub last_seen: u64,
    pub last_rssi: i16,
    /// Mesh hops on the most recent packet (0 = heard directly).
    pub last_hops: u8,
}

/// Counters and recent activity for one daemon run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStats {
    /// Unix seconds when the daemon connected.
    pub started_at: u64,
    /// Unix seconds of the snapshot.
    pub updated_at: u64,
    pub port: String,
    pub gateway_address: Option<String>,
    pub firmware: Option<String>,

    pub packets_heard: u64,
    /// Packets that reached this gateway through at least one relay.
    pub packets_relayed_in: u64,
    pub tx_received: u64,
    pub tx_broadcast: u64,
    /// Repeats of a transaction already broadcast this run (answered with a re-ACK).
    pub tx_duplicates: u64,
    pub tx_failed: u64,
    pub acks_sent: u64,
    pub acks_failed: u64,
    pub balance_requests: u64,
    pub balance_replies: u64,
    pub balance_failures: u64,
    /// Store-and-forward: how many times work was queued, and what is waiting now.
    #[serde(default)]
    pub tx_queued: u64,
    #[serde(default)]
    pub replies_forwarded: u64,
    #[serde(default)]
    pub outbox_broadcasts: u64,
    #[serde(default)]
    pub outbox_replies: u64,

    pub nodes: BTreeMap<String, NodeTraffic>,
    /// Newest first.
    pub events: Vec<GatewayEvent>,
}

impl GatewayStats {
    pub fn new(port: &str, now: u64) -> Self {
        Self {
            port: port.to_string(),
            started_at: now,
            updated_at: now,
            ..Self::default()
        }
    }

    /// Count any packet heard from `node`.
    pub fn record_packet(&mut self, node: &str, rssi: i16, hops: u8, now: u64) {
        self.packets_heard += 1;
        if hops > 0 {
            self.packets_relayed_in += 1;
        }
        let entry = self.nodes.entry(node.to_string()).or_default();
        entry.packets += 1;
        entry.last_seen = now;
        entry.last_rssi = rssi;
        entry.last_hops = hops;
        self.trim_nodes();
        self.updated_at = now;
    }

    /// Record an event and bump the counter that goes with it.
    pub fn record(
        &mut self,
        kind: GatewayEventKind,
        node: &str,
        detail: impl Into<String>,
        now: u64,
    ) {
        use GatewayEventKind::*;
        match kind {
            TxReceived => {
                self.tx_received += 1;
                if let Some(n) = self.nodes.get_mut(node) {
                    n.transactions += 1;
                }
            }
            TxBroadcast => self.tx_broadcast += 1,
            TxFailed => self.tx_failed += 1,
            AckSent => self.acks_sent += 1,
            AckFailed => self.acks_failed += 1,
            BalanceRequest => self.balance_requests += 1,
            BalanceReplied => self.balance_replies += 1,
            BalanceFailed => self.balance_failures += 1,
            TxQueued => self.tx_queued += 1,
            ReplyForwarded => self.replies_forwarded += 1,
            ReplyQueued | Expired => {}
        }
        self.events.insert(
            0,
            GatewayEvent {
                at: now,
                kind,
                node: node.to_string(),
                detail: detail.into(),
            },
        );
        self.events.truncate(MAX_EVENTS);
        self.updated_at = now;
    }

    /// A repeat transaction that was answered from the session cache.
    pub fn record_duplicate(&mut self, now: u64) {
        self.tx_duplicates += 1;
        self.updated_at = now;
    }

    /// Share of received transactions the network accepted, 0–100. `None` before any arrive.
    pub fn success_rate(&self) -> Option<f64> {
        let done = self.tx_broadcast + self.tx_failed;
        (done > 0).then(|| self.tx_broadcast as f64 * 100.0 / done as f64)
    }

    fn trim_nodes(&mut self) {
        while self.nodes.len() > MAX_NODES {
            let oldest = self
                .nodes
                .iter()
                .min_by_key(|(_, t)| t.last_seen)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.nodes.remove(&k);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use GatewayEventKind::*;

    #[test]
    fn counts_packets_per_node_and_relays() {
        let mut s = GatewayStats::new("COM5", 100);
        s.record_packet("10.0.2", -80, 0, 101);
        s.record_packet("10.0.2", -85, 2, 105);
        s.record_packet("10.0.3", -100, 0, 106);
        assert_eq!(s.packets_heard, 3);
        assert_eq!(s.packets_relayed_in, 1);
        let n = &s.nodes["10.0.2"];
        assert_eq!(
            (n.packets, n.last_seen, n.last_rssi, n.last_hops),
            (2, 105, -85, 2)
        );
        assert_eq!(s.updated_at, 106);
    }

    #[test]
    fn events_bump_counters_newest_first_and_cap() {
        let mut s = GatewayStats::new("COM5", 0);
        s.record_packet("10.0.2", -80, 0, 1);
        s.record(TxReceived, "10.0.2", "abc", 2);
        s.record(TxBroadcast, "10.0.2", "abc", 3);
        s.record(AckSent, "10.0.2", "abc", 3);
        s.record(TxReceived, "10.0.2", "def", 4);
        s.record(TxFailed, "10.0.2", "def", 5);
        s.record_duplicate(6);
        assert_eq!(
            (
                s.tx_received,
                s.tx_broadcast,
                s.tx_failed,
                s.acks_sent,
                s.tx_duplicates
            ),
            (2, 1, 1, 1, 1)
        );
        assert_eq!(s.nodes["10.0.2"].transactions, 2);
        assert_eq!(s.events[0].kind, TxFailed);
        assert_eq!(s.success_rate(), Some(50.0));
        for i in 0..100 {
            s.record(BalanceRequest, "10.0.9", "", 10 + i);
        }
        assert_eq!(s.events.len(), MAX_EVENTS);
        assert_eq!(s.balance_requests, 100);
    }

    #[test]
    fn success_rate_is_none_before_any_tx() {
        assert_eq!(GatewayStats::new("p", 0).success_rate(), None);
    }

    #[test]
    fn node_table_drops_least_recently_heard() {
        let mut s = GatewayStats::new("p", 0);
        for i in 0..(MAX_NODES as u64 + 5) {
            s.record_packet(&format!("1.0.{}", i), -90, 0, i + 1);
        }
        assert_eq!(s.nodes.len(), MAX_NODES);
        assert!(!s.nodes.contains_key("1.0.0"));
        assert!(s.nodes.contains_key(&format!("1.0.{}", MAX_NODES + 4)));
    }

    #[test]
    fn snapshot_json_is_camel_case_and_round_trips() {
        let mut s = GatewayStats::new("COM5", 7);
        s.record(BalanceReplied, "10.0.2", "1.5 DOGE", 8);
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"startedAt\":7") && j.contains("\"balanceReplied\""));
        let back: GatewayStats = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s);
    }
}

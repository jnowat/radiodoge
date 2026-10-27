//! Multi-gateway dashboard: merge several gateways' stats snapshots into one view.
//!
//! Each `radiodoge-cli daemon` writes its own `--stats-file`. Point several of them
//! at a shared folder (or copy the files over) and the app aggregates them here.
//! Pure data: no I/O, so it is fully unit-tested.

use crate::gateway_stats::{GatewayEvent, GatewayStats};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Events kept in the merged feed.
pub const MAX_FLEET_EVENTS: usize = 50;
/// A snapshot older than this (seconds) counts as offline.
pub const STALE_AFTER_SECS: u64 = 30;

/// One gateway's contribution, labelled for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetMember {
    pub label: String,
    pub stats: GatewayStats,
}

/// A node as heard across the fleet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetNode {
    pub packets: u64,
    pub transactions: u64,
    pub last_seen: u64,
    /// Gateways that heard this node, by label.
    pub heard_by: Vec<String>,
    /// The gateway with the strongest last RSSI (best uplink), and that RSSI.
    pub best_gateway: String,
    pub best_rssi: i16,
}

/// A feed event tagged with the gateway that logged it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetEvent {
    pub gateway: String,
    #[serde(flatten)]
    pub event: GatewayEvent,
}

/// Fleet-wide totals.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetSummary {
    pub gateways: usize,
    pub online: usize,
    pub packets_heard: u64,
    pub tx_received: u64,
    pub tx_broadcast: u64,
    pub tx_duplicates: u64,
    pub tx_failed: u64,
    pub acks_sent: u64,
    pub balance_replies: u64,
    pub outbox_broadcasts: u64,
    pub outbox_replies: u64,
    pub nodes: BTreeMap<String, FleetNode>,
    pub events: Vec<FleetEvent>,
}

impl FleetSummary {
    pub fn success_rate(&self) -> Option<f64> {
        let done = self.tx_broadcast + self.tx_failed;
        (done > 0).then(|| self.tx_broadcast as f64 * 100.0 / done as f64)
    }
}

/// Whether a snapshot is fresh enough to count as online at `now`.
pub fn is_online(stats: &GatewayStats, now: u64) -> bool {
    now.saturating_sub(stats.updated_at) <= STALE_AFTER_SECS
}

/// Merge members into one summary. Counters add up; a node heard by several
/// gateways is listed once with every gateway that heard it.
pub fn aggregate(members: &[FleetMember], now: u64) -> FleetSummary {
    let mut out = FleetSummary {
        gateways: members.len(),
        ..Default::default()
    };
    for m in members {
        let s = &m.stats;
        if is_online(s, now) {
            out.online += 1;
        }
        out.packets_heard += s.packets_heard;
        out.tx_received += s.tx_received;
        out.tx_broadcast += s.tx_broadcast;
        out.tx_duplicates += s.tx_duplicates;
        out.tx_failed += s.tx_failed;
        out.acks_sent += s.acks_sent;
        out.balance_replies += s.balance_replies;
        out.outbox_broadcasts += s.outbox_broadcasts;
        out.outbox_replies += s.outbox_replies;
        for (addr, t) in &s.nodes {
            let n = out.nodes.entry(addr.clone()).or_default();
            let first = n.heard_by.is_empty();
            n.packets += t.packets;
            n.transactions += t.transactions;
            n.last_seen = n.last_seen.max(t.last_seen);
            if !n.heard_by.contains(&m.label) {
                n.heard_by.push(m.label.clone());
            }
            // RSSI 0 means "unknown" in the firmware; never prefer it.
            let better = t.last_rssi != 0 && (n.best_rssi == 0 || t.last_rssi > n.best_rssi);
            if first || better {
                n.best_gateway = m.label.clone();
                n.best_rssi = t.last_rssi;
            }
        }
        out.events.extend(s.events.iter().map(|e| FleetEvent {
            gateway: m.label.clone(),
            event: e.clone(),
        }));
    }
    out.events.sort_by_key(|e| std::cmp::Reverse(e.event.at));
    out.events.truncate(MAX_FLEET_EVENTS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway_stats::GatewayEventKind::*;

    fn gw(label: &str, updated: u64) -> FleetMember {
        let mut s = GatewayStats::new("COM3", 0);
        s.updated_at = updated;
        FleetMember {
            label: label.into(),
            stats: s,
        }
    }

    #[test]
    fn sums_counters_and_counts_online() {
        let mut a = gw("north", 100);
        let mut b = gw("south", 10);
        a.stats.record_packet("1.1.1", -80, 0, 100);
        a.stats.record(TxBroadcast, "1.1.1", "", 100);
        b.stats.record(TxFailed, "1.1.2", "", 10);
        b.stats.updated_at = 10;
        let f = aggregate(&[a, b], 100);
        assert_eq!((f.gateways, f.online), (2, 1));
        assert_eq!(f.packets_heard, 1);
        assert_eq!(f.success_rate(), Some(50.0));
    }

    #[test]
    fn merges_nodes_and_picks_strongest_gateway() {
        let mut a = gw("north", 100);
        let mut b = gw("south", 100);
        a.stats.record_packet("1.1.1", -110, 0, 50);
        b.stats.record_packet("1.1.1", -70, 1, 90);
        b.stats.record_packet("1.1.1", -70, 1, 90);
        let f = aggregate(&[a, b], 100);
        let n = &f.nodes["1.1.1"];
        assert_eq!(n.packets, 3);
        assert_eq!(n.last_seen, 90);
        assert_eq!(n.heard_by, vec!["north", "south"]);
        assert_eq!((n.best_gateway.as_str(), n.best_rssi), ("south", -70));
    }

    #[test]
    fn unknown_rssi_never_wins() {
        let mut a = gw("a", 100);
        let mut b = gw("b", 100);
        a.stats.record_packet("2.2.2", -100, 0, 1);
        b.stats.record_packet("2.2.2", 0, 0, 2);
        let f = aggregate(&[a, b], 100);
        assert_eq!(f.nodes["2.2.2"].best_gateway, "a");
    }

    #[test]
    fn feed_is_merged_newest_first_tagged_and_capped() {
        let mut a = gw("a", 100);
        let mut b = gw("b", 100);
        a.stats.record(AckSent, "1.1.1", "", 5);
        b.stats.record(AckSent, "1.1.1", "", 9);
        let f = aggregate(&[a.clone(), b], 100);
        assert_eq!(f.events[0].gateway, "b");
        assert_eq!(f.events[1].gateway, "a");
        let many: Vec<FleetMember> = (0..30)
            .map(|i| {
                let mut m = gw(&format!("g{i}"), 100);
                m.stats.record(AckSent, "x", "", i);
                m.stats.record(AckSent, "x", "", i);
                m
            })
            .collect();
        assert_eq!(aggregate(&many, 100).events.len(), MAX_FLEET_EVENTS);
    }

    #[test]
    fn event_json_is_flat() {
        let mut a = gw("a", 1);
        a.stats.record(AckSent, "1.1.1", "d", 1);
        let v = serde_json::to_value(&aggregate(&[a], 1).events[0]).unwrap();
        assert_eq!(v["gateway"], "a");
        assert_eq!(v["kind"], "ackSent");
        assert_eq!(v["node"], "1.1.1");
    }
}

//! Multi-channel scanning: hop the board across frequency/spreading-factor
//! combinations, listen for a while on each, and report what was heard.
//!
//! This module is the pure part — the channel plans, the radio-parameter
//! encoding shared with the LoRa settings screen, and result bookkeeping. The
//! Tauri command drives the board.
//!
//! Note: the board only decodes RadioDoge frames. A Meshtastic preset in the
//! plan finds RadioDoge nodes configured on that preset's channel; it does not
//! decode Meshtastic traffic (that needs the Meshtastic encoding work).

use crate::types::{IncomingPacket, LoraSettings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Longest dwell per channel the app allows, in seconds.
pub const MAX_DWELL_SECS: u64 = 120;
/// Most channels in one scan.
pub const MAX_CHANNELS: usize = 24;

/// One stop on the scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanChannel {
    pub label: String,
    pub frequency_mhz: f32,
    pub spreading_factor: u8,
    pub bandwidth_khz: f32,
}

impl ScanChannel {
    fn new(
        label: impl Into<String>,
        frequency_mhz: f32,
        spreading_factor: u8,
        bandwidth_khz: f32,
    ) -> Self {
        Self {
            label: label.into(),
            frequency_mhz,
            spreading_factor,
            bandwidth_khz,
        }
    }

    /// Whether the firmware will accept it (same ranges as `loRaParamsAreValid`).
    pub fn is_valid(&self) -> bool {
        (7..=12).contains(&self.spreading_factor)
            && matches!(self.bandwidth_khz as u32, 125 | 250 | 500)
            && (150.0..=960.0).contains(&self.frequency_mhz)
    }

    /// `base` with this channel's frequency, SF and bandwidth.
    pub fn apply_to(&self, base: &LoraSettings) -> LoraSettings {
        LoraSettings {
            frequency_mhz: self.frequency_mhz,
            spreading_factor: self.spreading_factor,
            bandwidth_khz: self.bandwidth_khz,
            ..base.clone()
        }
    }
}

/// Built-in channel plans.
pub fn plan(region: &str) -> Vec<ScanChannel> {
    let (base, mesh_freq, name) = match region {
        "eu868" => (868.1, 869.525, "EU868"),
        _ => (915.0, 906.875, "US915"),
    };
    let mut v: Vec<ScanChannel> = (7..=12)
        .map(|sf| ScanChannel::new(format!("{name} SF{sf}"), base, sf, 125.0))
        .collect();
    v.push(ScanChannel::new(
        format!("{name} Meshtastic LongFast"),
        mesh_freq,
        11,
        250.0,
    ));
    v
}

/// The `CMD_SET_LORA_PARAMS` fields for `s`: (sf, bw_idx, cr denominator, freq kHz, power dBm).
pub fn lora_param_fields(s: &LoraSettings) -> (u8, u8, u8, u32, u8) {
    let freq_khz = (s.frequency_mhz * 1000.0).round() as u32;
    let bw_idx = match s.bandwidth_khz as u32 {
        250 => 1,
        500 => 2,
        _ => 0,
    };
    let cr = s
        .coding_rate
        .trim_start_matches("4/")
        .parse::<u8>()
        .unwrap_or(5);
    let power = s.power_dbm.clamp(2, 22) as u8;
    (s.spreading_factor, bw_idx, cr, freq_khz, power)
}

/// What one channel produced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResult {
    pub packets: u64,
    /// Nodes heard, with their strongest RSSI.
    pub nodes: BTreeMap<String, i16>,
    pub best_rssi: Option<i16>,
}

impl ChannelResult {
    /// Count an over-the-air packet. Serial replies from the board itself
    /// (RSSI 0, our own address) are ignored by the caller via `is_air_packet`.
    pub fn record(&mut self, p: &IncomingPacket) {
        self.packets += 1;
        let addr = format!(
            "{}.{}.{}",
            p.source.region, p.source.community, p.source.node
        );
        let e = self.nodes.entry(addr).or_insert(p.rssi);
        *e = (*e).max(p.rssi);
        self.best_rssi = Some(self.best_rssi.map_or(p.rssi, |b| b.max(p.rssi)));
    }
}

/// Packets that came over the air (not the board answering a serial command).
pub fn is_air_packet(p: &IncomingPacket) -> bool {
    use crate::radio::*;
    const AIR: &[u8] = &[
        CMD_PING,
        CMD_MESSAGE,
        CMD_BROADCAST,
        CMD_MULTIPART,
        CMD_DOGE_TX,
        CMD_RECEIVED_ACK,
        CMD_RECEIVED_PING,
    ];
    AIR.contains(&p.command) && p.rssi != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NodeAddress;

    fn pkt(cmd: u8, node: u8, rssi: i16) -> IncomingPacket {
        IncomingPacket {
            timestamp: 0,
            source: NodeAddress::new(1, 2, node),
            destination: NodeAddress::broadcast(),
            command: cmd,
            payload_hex: String::new(),
            decoded: None,
            rssi,
            hops: 0,
            snr: None,
        }
    }

    #[test]
    fn plans_are_valid_and_bounded() {
        for r in ["us915", "eu868"] {
            let p = plan(r);
            assert_eq!(p.len(), 7);
            assert!(p.len() <= MAX_CHANNELS);
            assert!(p.iter().all(ScanChannel::is_valid));
        }
        assert_eq!(plan("eu868")[0].frequency_mhz, 868.1);
    }

    #[test]
    fn rejects_out_of_range_channels() {
        assert!(!ScanChannel::new("x", 915.0, 6, 125.0).is_valid());
        assert!(!ScanChannel::new("x", 915.0, 9, 200.0).is_valid());
        assert!(!ScanChannel::new("x", 1000.0, 9, 125.0).is_valid());
    }

    #[test]
    fn apply_keeps_power_cr_and_address() {
        let base = LoraSettings {
            power_dbm: 14,
            coding_rate: "4/7".into(),
            ..Default::default()
        };
        let s = ScanChannel::new("x", 906.875, 11, 250.0).apply_to(&base);
        assert_eq!(lora_param_fields(&s), (11, 1, 7, 906_875, 14));
        assert_eq!(s.node_address, base.node_address);
    }

    #[test]
    fn records_air_packets_and_strongest_rssi() {
        let mut r = ChannelResult::default();
        for p in [
            pkt(crate::radio::CMD_PING, 5, -110),
            pkt(crate::radio::CMD_PING, 5, -90),
        ] {
            assert!(is_air_packet(&p));
            r.record(&p);
        }
        assert_eq!(r.packets, 2);
        assert_eq!(r.nodes["1.2.5"], -90);
        assert_eq!(r.best_rssi, Some(-90));
        assert!(!is_air_packet(&pkt(crate::radio::CMD_GET_SETTINGS, 5, -90)));
        assert!(!is_air_packet(&pkt(crate::radio::CMD_PING, 5, 0)));
    }
}

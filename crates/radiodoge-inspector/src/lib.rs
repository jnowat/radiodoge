//! # radiodoge-inspector
//!
//! Decode RadioDoge packets from a hex dump, natively or in the browser via
//! WebAssembly (`www/index.html`). Uses the same framing and payload decoders
//! as the desktop app and CLI (`radiodoge_core::radio`), so a capture is cut
//! into packets exactly the way the serial read loop would cut it.

use radiodoge_core::radio::{self, *};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// One packet found in the input.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// Byte offset of the packet in the input.
    pub offset: usize,
    pub length: usize,
    pub command: u8,
    pub command_name: &'static str,
    /// `None` for the firmware's legacy replies, which have no desktop header.
    pub flags: Option<u8>,
    pub hops: Option<u8>,
    pub multipart: bool,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub payload_hex: String,
    pub decoded: Option<String>,
}

/// Bytes that could not start a packet and were skipped.
#[derive(Debug, Serialize, PartialEq)]
pub struct Noise {
    pub offset: usize,
    pub length: usize,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub frames: Vec<Frame>,
    pub noise: Vec<Noise>,
    /// Bytes at the end that start a packet but are incomplete.
    pub trailing: usize,
    pub error: Option<String>,
}

/// Name of a command byte.
pub fn command_name(cmd: u8) -> &'static str {
    match cmd {
        CMD_GET_NODE_ADDR => "GET_NODE_ADDR",
        CMD_SET_NODE_ADDRS => "SET_NODE_ADDRS",
        CMD_PING => "PING",
        CMD_MESSAGE => "MESSAGE",
        CMD_BROADCAST => "BROADCAST",
        CMD_MULTIPART => "MULTIPART",
        CMD_DOGE_TX => "DOGE_TX",
        CMD_REQUEST_BALANCE => "REQUEST_BALANCE",
        CMD_GET_FIRMWARE_VERSION => "GET_FIRMWARE_VERSION",
        CMD_SET_LORA_PARAMS => "SET_LORA_PARAMS",
        CMD_GET_SETTINGS => "GET_SETTINGS",
        CMD_SET_GATEWAY => "SET_GATEWAY",
        CMD_WIFI_TOGGLE => "WIFI_TOGGLE",
        CMD_ADDR_CONFLICT => "ADDR_CONFLICT",
        CMD_GET_BATTERY => "GET_BATTERY",
        CMD_GET_MAC => "GET_MAC",
        CMD_BLE_TOGGLE => "BLE_TOGGLE",
        CMD_RECEIVED_ACK => "RECEIVED_ACK",
        CMD_RECEIVED_ACK_SIGNAL => "RECEIVED_ACK_SIGNAL",
        CMD_RECEIVED_PING => "RECEIVED_PING",
        LEGACY_HARDWARE_INFO => "LEGACY_HARDWARE_INFO",
        LEGACY_RESULT_CODE => "LEGACY_RESULT_CODE",
        _ => "UNKNOWN",
    }
}

/// Parse a hex dump. Accepts whitespace, commas, colons, dashes and `0x`
/// prefixes between bytes (`02 00 0a…`, `0x02,0x00`, `02:00:0A`).
pub fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = input
        .replace("0x", " ")
        .replace("0X", " ")
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, ',' | ':' | '-'))
        .collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err(format!("odd number of hex digits ({})", cleaned.len()));
    }
    hex::decode(&cleaned).map_err(|e| format!("not valid hex: {}", e))
}

fn addr(b: &[u8]) -> String {
    let s = format!("{}.{}.{}", b[0], b[1], b[2]);
    if b == [0xFF, 0xFF, 0xFF] { format!("{} (broadcast)", s) } else { s }
}

/// Cut `bytes` into packets and decode each one.
pub fn inspect(bytes: &[u8]) -> Report {
    let mut r = Report::default();
    let mut pos = 0;
    while pos < bytes.len() {
        let skip = resync_offset(&bytes[pos..]);
        if skip > 0 {
            r.noise.push(Noise { offset: pos, length: skip });
            pos += skip;
            continue;
        }
        let buf = &bytes[pos..];
        let cmd = buf[0];
        let n = match frame_packet_len(cmd, buf) {
            Some(n) if n > 0 => n,
            _ => {
                r.trailing = buf.len();
                break;
            }
        };
        let pkt = &buf[..n];
        let frame = if n < SINGLE_HDR_LEN {
            Frame {
                offset: pos, length: n, command: cmd, command_name: command_name(cmd),
                flags: None, hops: None, multipart: false, source: None, destination: None,
                payload_hex: hex::encode(&pkt[1..]), decoded: None,
            }
        } else {
            let payload = &pkt[SINGLE_HDR_LEN..];
            Frame {
                offset: pos, length: n, command: cmd, command_name: command_name(cmd),
                flags: Some(pkt[1]),
                hops: Some(hops_from_flags(pkt[1])),
                multipart: is_multipart_flags(pkt[1]),
                source: Some(addr(&pkt[2..5])),
                destination: Some(addr(&pkt[5..8])),
                payload_hex: hex::encode(payload),
                decoded: radio::decode_payload(cmd, payload),
            }
        };
        r.frames.push(frame);
        pos += n;
    }
    r
}

/// Browser entry point: hex dump in, JSON [`Report`] out.
#[wasm_bindgen]
pub fn inspect_hex(input: &str) -> String {
    let report = match parse_hex(input) {
        Ok(bytes) => inspect(&bytes),
        Err(e) => Report { error: Some(e), ..Default::default() },
    };
    serde_json::to_string(&report).unwrap_or_else(|e| format!("{{\"error\":\"{}\"}}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiodoge_core::types::NodeAddress;

    #[test]
    fn parses_hex_formats() {
        assert_eq!(parse_hex("02 00 0a").unwrap(), vec![2, 0, 10]);
        assert_eq!(parse_hex("0x02,0x00:0A-ff").unwrap(), vec![2, 0, 10, 255]);
        assert!(parse_hex("abc").is_err());
        assert!(parse_hex("zz").is_err());
    }

    #[test]
    fn decodes_a_stream_with_noise() {
        let me = NodeAddress::new(10, 0, 1);
        let mut buf = b"\xEE\xEE".to_vec();
        buf.extend(build_ping(&me, &NodeAddress::new(10, 0, 2)));
        buf.extend(build_broadcast(&me, "much wow"));
        let r = inspect(&buf);
        assert_eq!(r.noise, vec![Noise { offset: 0, length: 2 }]);
        assert_eq!(r.frames.len(), 2);
        assert_eq!(r.frames[0].command_name, "PING");
        assert_eq!(r.frames[0].source.as_deref(), Some("10.0.1"));
        assert_eq!(r.frames[0].destination.as_deref(), Some("10.0.2"));
        assert_eq!(r.frames[1].command_name, "MESSAGE");
        assert_eq!(r.frames[1].destination.as_deref(), Some("255.255.255 (broadcast)"));
        assert!(r.frames[1].decoded.as_deref().unwrap_or("").contains("much wow"));
    }

    #[test]
    fn legacy_and_truncated() {
        let r = inspect(&[0xFE, 1, 0x06, 0x02, 0x00, 0x0A]);
        assert_eq!(r.frames[0].command_name, "LEGACY_RESULT_CODE");
        assert_eq!(r.frames[0].source, None);
        assert_eq!(r.trailing, 3);
    }

    #[test]
    fn json_entry_point() {
        let j = inspect_hex("nothex!");
        assert!(j.contains("\"error\""));
        let j = inspect_hex("02 00 0a 00 01 0a 00 02");
        assert!(j.contains("\"commandName\":\"PING\""));
    }
}

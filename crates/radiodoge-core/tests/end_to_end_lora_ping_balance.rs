//! End-to-end tests for PING and REQUEST_BALANCE over a mocked serial/LoRa path.
//!
//! Same board framing model as `end_to_end_lora_tx.rs`: host bytes → board
//! `HostSerialRead` → on-air packet → gateway host serial ingest. No hardware.

use radiodoge_core::radio::{
    self, frame_packet_len, ingest_packet, resync_offset, MultipartReassembler, CMD_PING,
    CMD_REQUEST_BALANCE, MULTIPART_CHUNK_LEN, MULTIPART_EXTRA_LEN, SINGLE_HDR_LEN,
};
use radiodoge_core::types::NodeAddress;

struct BoardFrame {
    cmd: u8,
    flags: u8,
    hdr_rest: [u8; 6],
    extra: Vec<u8>,
}

fn is_desktop_command_byte(b: u8) -> bool {
    matches!(
        b,
        0x10 | 0x11 | 0x20 | 0x21 | 0x22 | 0x23 | 0x24 | 0x26 | 0x27 | 0x28
    )
}

fn desktop_command_length(b: u8) -> Option<usize> {
    match b {
        0x20 | 0x22 | 0x26 | 0x27 => Some(8),
        0x23 | 0x24 | 0x28 => Some(9),
        0x21 => Some(16),
        _ => None,
    }
}

fn board_read_and_transmit(stream: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut pos = 0usize;
    let mut on_air = Vec::new();
    while pos < stream.len() {
        if stream.len() - pos < 2 {
            return Err(format!("truncated header at offset {}", pos));
        }
        let cmd = stream[pos];
        let flags = stream[pos + 1];
        pos += 2;
        if !is_desktop_command_byte(cmd) {
            return Err(format!("cmd 0x{:02X} is not a desktop command", cmd));
        }
        if stream.len() - pos < 6 {
            return Err("truncated desktop header".to_string());
        }
        let mut hdr_rest = [0u8; 6];
        hdr_rest.copy_from_slice(&stream[pos..pos + 6]);
        pos += 6;
        let extra: Vec<u8> = if (flags & 0x0F) == radio::FLAG_MULTIPART {
            if stream.len() - pos < MULTIPART_EXTRA_LEN {
                return Err("truncated multipart header".to_string());
            }
            let mut e = stream[pos..pos + MULTIPART_EXTRA_LEN].to_vec();
            pos += MULTIPART_EXTRA_LEN;
            let chunk_len = e[MULTIPART_EXTRA_LEN - 1] as usize;
            if chunk_len > MULTIPART_CHUNK_LEN {
                return Err(format!("chunk length {} out of range", chunk_len));
            }
            if stream.len() - pos < chunk_len {
                return Err("truncated multipart chunk".to_string());
            }
            e.extend_from_slice(&stream[pos..pos + chunk_len]);
            pos += chunk_len;
            e
        } else if let Some(total) = desktop_command_length(cmd) {
            let want = total - SINGLE_HDR_LEN;
            if stream.len() - pos < want {
                return Err("truncated fixed-length payload".to_string());
            }
            let e = stream[pos..pos + want].to_vec();
            pos += want;
            e
        } else {
            let e = stream[pos..].to_vec();
            pos = stream.len();
            e
        };
        on_air.push(
            BoardFrame {
                cmd,
                flags,
                hdr_rest,
                extra,
            }
            .to_air(),
        );
    }
    Ok(on_air)
}

impl BoardFrame {
    fn to_air(&self) -> Vec<u8> {
        let mut ota = vec![self.cmd, self.flags];
        ota.extend_from_slice(&self.hdr_rest);
        ota.extend_from_slice(&self.extra);
        ota
    }
}

fn host_read_stream(stream: &[u8]) -> Vec<radiodoge_core::types::IncomingPacket> {
    let mut acc = stream.to_vec();
    let mut rx = MultipartReassembler::new();
    let mut out = Vec::new();
    while !acc.is_empty() {
        let skip = resync_offset(&acc);
        if skip > 0 {
            acc.drain(..skip);
            continue;
        }
        let len = match frame_packet_len(acc[0], &acc) {
            Some(n) => n,
            None => break,
        };
        if let Some(pkt) = ingest_packet(&acc[..len], -70, &mut rx, 0) {
            out.push(pkt);
        }
        acc.drain(..len);
    }
    out
}

/// REQUEST_BALANCE (0x11) travels host → board → air → gateway host with the
/// Dogecoin address intact.
#[test]
fn balance_request_survives_host_to_board_to_air_to_gateway() {
    let sender = NodeAddress::new(10, 0, 1);
    let gateway = NodeAddress::new(10, 0, 9);
    let doge = "DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L";

    let frame = radio::build_request_balance_to(&sender, &gateway, doge);
    assert_eq!(frame[0], CMD_REQUEST_BALANCE);

    let on_air = board_read_and_transmit(&frame).expect("board frames balance request");
    assert_eq!(on_air.len(), 1);
    assert_eq!(on_air[0], frame);

    let packets = host_read_stream(&on_air.concat());
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].command, CMD_REQUEST_BALANCE);
    assert_eq!(packets[0].source, sender);
    assert_eq!(packets[0].destination, gateway);
    let recovered = String::from_utf8(hex::decode(&packets[0].payload_hex).unwrap()).unwrap();
    assert_eq!(recovered, doge);
}

/// Gateway BAL reply (CMD_MESSAGE "BAL:{koinus}") reaches the sender through
/// the same reverse serial framing, including a legacy board result code prefix.
#[test]
fn balance_reply_reaches_sender_through_serial_framing() {
    let gateway = NodeAddress::new(10, 0, 9);
    let sender = NodeAddress::new(10, 0, 1);
    let koinus: u64 = 230_000_000; // 2.3 DOGE

    let reply = radio::build_message(&gateway, &sender, &format!("BAL:{}", koinus));
    let mut stream = vec![radio::LEGACY_RESULT_CODE, 0x01, 0x06];
    stream.extend_from_slice(&reply);

    let packets = host_read_stream(&stream);
    assert_eq!(packets.len(), 1);
    assert_eq!(
        packets[0].decoded.as_deref(),
        Some("💰 Balance: 2.30000000 DOGE")
    );
}

/// Over-the-air PING (and its received-ping echo) survive gateway host framing
/// when two boards exchange them with no gaps on the serial byte stream.
#[test]
fn ping_and_received_ping_survive_gateway_serial_stream() {
    let a = NodeAddress::new(10, 0, 1);
    let b = NodeAddress::new(10, 0, 2);

    let ping = radio::build_ping(&a, &b);
    assert_eq!(ping[0], CMD_PING);
    assert_eq!(ping.len(), SINGLE_HDR_LEN);

    // What the listening board writes to its host after hearing a PING over LoRa
    // is modeled as the same on-air bytes (firmware relays mesh packets up).
    let packets = host_read_stream(&ping);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].command, CMD_PING);
    assert_eq!(packets[0].source, a);
    assert_eq!(packets[0].destination, b);

    // A second PING glued back-to-back must still split cleanly.
    let mut stream = ping.clone();
    stream.extend_from_slice(&radio::build_ping(&b, &a));
    let two = host_read_stream(&stream);
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].source, a);
    assert_eq!(two[1].source, b);
}

/// Balance request and ping interleaved on one gateway stream stay separate.
#[test]
fn interleaved_ping_and_balance_request_stay_separate() {
    let sender = NodeAddress::new(10, 0, 1);
    let gateway = NodeAddress::new(10, 0, 9);
    let peer = NodeAddress::new(10, 0, 2);
    let doge = "DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L";

    let bal = radio::build_request_balance_to(&sender, &gateway, doge);
    let on_air = board_read_and_transmit(&bal).unwrap();
    let ping = radio::build_ping(&peer, &gateway);

    // Variable-length REQUEST_BALANCE must be last on the stream (no quiet-line
    // timer in this mock), otherwise its payload would swallow the next frame.
    let mut stream = Vec::new();
    stream.extend_from_slice(&ping);
    stream.extend_from_slice(&radio::build_ping(&gateway, &peer));
    stream.extend_from_slice(&on_air[0]);

    let packets = host_read_stream(&stream);
    assert_eq!(packets.len(), 3);
    assert_eq!(packets[0].command, CMD_PING);
    assert_eq!(packets[1].command, CMD_PING);
    assert_eq!(packets[2].command, CMD_REQUEST_BALANCE);
    assert_eq!(
        String::from_utf8(hex::decode(&packets[2].payload_hex).unwrap()).unwrap(),
        doge
    );
}

/// FW13+ board-initiated ACK_SIGNAL (discover reply path) reaches the host
/// with RSSI + SNR intact — same serial ingest as live boards, no hardware.
#[test]
fn discover_ack_signal_snr_survives_host_ingest() {
    use radiodoge_core::radio::{CMD_RECEIVED_ACK, CMD_RECEIVED_ACK_SIGNAL};

    // cmd + flags + region/community/node + dest(4) + rssi i16 BE + snr i8 = 11
    let pkt = [
        CMD_RECEIVED_ACK_SIGNAL,
        0,
        10,
        0,
        2,
        0xFF,
        0xFF,
        0xFF,
        0xFF,
        0xB5,
        0x07,
    ];
    assert_eq!(pkt.len(), 11);

    let packets = host_read_stream(&pkt);
    assert_eq!(packets.len(), 1, "ACK_SIGNAL should ingest as one packet");
    let p = &packets[0];
    assert_eq!(
        p.command, CMD_RECEIVED_ACK,
        "ACK_SIGNAL folds into RECEIVED_ACK"
    );
    assert_eq!(p.rssi, -75);
    assert_eq!(p.snr, Some(7));
    assert_eq!(p.source.node, 2);
}

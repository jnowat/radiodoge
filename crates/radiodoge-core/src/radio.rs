//! RadioDoge LoRa packet protocol implementation.
//!
//! This module encodes and decodes the binary packet format used between
//! the desktop app and the Heltec ESP32 LoRa module over serial.
//!
//! Packet format (derived from RadioDogeSharp & serdog source):
//!
//! Single packet:
//!   [0]  Command byte (see CMD_* constants)
//!   [1]  Flags byte (0x00 for standard, 0x01 for multipart)
//!   [2]  Source region
//!   [3]  Source community
//!   [4]  Source node
//!   [5]  Destination region
//!   [6]  Destination community
//!   [7]  Destination node
//!   [8+] Payload (up to MAX_SINGLE_PAYLOAD_LEN bytes)
//!
//! Multipart packet (for payloads > MAX_SINGLE_PAYLOAD_LEN):
//!   [0..8]   Same header as single packet (flags byte = 0x01)
//!   [8]  Total parts (1 byte)
//!   [9]  Part index (0-based, 1 byte)
//!   [10] Part ID high byte
//!   [11] Part ID low byte
//!   [12+] Payload chunk

use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::{IncomingPacket, NodeAddress};

/// Maximum payload bytes in a single (non-multipart) packet
pub const MAX_SINGLE_PAYLOAD_LEN: usize = 192;

/// Maximum parts in a multipart message
pub const MAX_MULTIPART_PARTS: u8 = 20;

// Command bytes (matching RadioDogeSharp's SerialCommandType enum)
pub const CMD_GET_NODE_ADDR: u8 = 0x00;
pub const CMD_SET_NODE_ADDRS: u8 = 0x01;
pub const CMD_PING: u8 = 0x02;
pub const CMD_MESSAGE: u8 = 0x03;
pub const CMD_BROADCAST: u8 = 0x04;
pub const CMD_MULTIPART: u8 = 0x05;
pub const CMD_DOGE_TX: u8 = 0x10;     // Dogecoin transaction
pub const CMD_REQUEST_BALANCE: u8 = 0x11; // Balance request
pub const CMD_GET_FIRMWARE_VERSION: u8 = 0x20; // Query firmware version string
pub const CMD_SET_LORA_PARAMS: u8 = 0x21;      // Set SF/BW/CR/frequency/TX-power (v0.3.3)
pub const CMD_GET_SETTINGS: u8 = 0x22;         // v0.3.6: Query board live state (addr + gateway_mode)
pub const CMD_SET_GATEWAY: u8 = 0x23;          // v0.3.6: Set/persist gateway_mode on board
pub const CMD_WIFI_TOGGLE: u8 = 0x24;          // v0.3.7: Enable/disable WiFi radio (persists to NVS)
pub const CMD_ADDR_CONFLICT: u8 = 0x25;        // v0.3.7: Board-initiated: duplicate node address detected
pub const CMD_GET_BATTERY: u8 = 0x26;          // v0.3.8: Query battery voltage (2-byte u16 mV in payload)
pub const CMD_GET_MAC: u8 = 0x27;              // v0.3.8: Query board MAC address (6 bytes in payload)
pub const CMD_BLE_TOGGLE: u8 = 0x28;          // v0.3.16: Enable/disable BLE advertising (persists to NVS)
pub const CMD_RECEIVED_ACK: u8 = 0x29;        // v0.4.0: Board-initiated: over-the-air ACK received from remote node
pub const CMD_RECEIVED_PING: u8 = 0x2A;       // v0.4.0: Board-initiated: over-the-air Ping received from remote node

/// Single packet header length in bytes
pub const SINGLE_HDR_LEN: usize = 8;
/// Multipart packet header length (single header + 4 multipart bytes)
pub const MULTIPART_HDR_LEN: usize = 12;

/// Flags byte: standard single packet
pub const FLAG_STANDARD: u8 = 0x00;
/// Flags byte: this packet is part of a multipart sequence
pub const FLAG_MULTIPART: u8 = 0x01;

/// v0.4.0 — Maximum number of mesh hops a packet may be relayed before it is
/// dropped. Bounds rebroadcast storms independently of the dedup table.
pub const MAX_MESH_HOPS: u8 = 8;

/// The flags byte packs the mesh hop count into its upper nibble and the
/// single/multipart flag into its lower nibble, so hop tracking is backward
/// compatible: freshly built packets have hops = 0 (upper nibble clear).
///
/// Extract the hop count (0–15) from a flags byte.
pub fn hops_from_flags(flags: u8) -> u8 {
    (flags >> 4) & 0x0F
}

/// Combine a base flags value (lower nibble, e.g. [`FLAG_MULTIPART`]) with a hop
/// count (upper nibble).
pub fn flags_with_hops(base: u8, hops: u8) -> u8 {
    (base & 0x0F) | ((hops.min(0x0F)) << 4)
}

/// `true` if a flags byte marks the packet as part of a multipart sequence.
///
/// Only the low nibble carries the multipart bit; the high nibble is the mesh
/// hop count, so a relayed multipart frame still reports `true`.
pub fn is_multipart_flags(flags: u8) -> bool {
    (flags & 0x0F) == FLAG_MULTIPART
}

/// Produce the relayed form of a packet: the same bytes with the header's hop
/// count incremented by one.
///
/// Returns `None` if the packet is too short to have a header, or if it has
/// already reached [`MAX_MESH_HOPS`] (in which case a relay must drop it rather
/// than forward it, preventing mesh storms).
pub fn relay_packet(pkt: &[u8]) -> Option<Vec<u8>> {
    if pkt.len() < SINGLE_HDR_LEN {
        return None;
    }
    let hops = hops_from_flags(pkt[1]);
    if hops >= MAX_MESH_HOPS {
        return None;
    }
    let mut relayed = pkt.to_vec();
    relayed[1] = flags_with_hops(pkt[1], hops + 1);
    Some(relayed)
}

/// Build a GET_NODE_ADDR command to query the Heltec device's address.
pub fn build_get_node_addr(src: &NodeAddress) -> Vec<u8> {
    let broadcast = NodeAddress::broadcast();
    build_header(CMD_GET_NODE_ADDR, FLAG_STANDARD, src, &broadcast)
}

/// Build a GET_FIRMWARE_VERSION command (CMD 0x20).
/// The device should respond with a packet whose payload is the version string.
pub fn build_get_firmware_version(src: &NodeAddress) -> Vec<u8> {
    build_header(CMD_GET_FIRMWARE_VERSION, FLAG_STANDARD, src, &NodeAddress::broadcast())
}

/// Build a PING packet to check if the device is alive.
pub fn build_ping(src: &NodeAddress, dst: &NodeAddress) -> Vec<u8> {
    build_header(CMD_PING, FLAG_STANDARD, src, dst)
}

/// Build a text MESSAGE packet.
///
/// `text` longer than [`MAX_SINGLE_PAYLOAD_LEN`] is truncated on a UTF-8
/// character boundary, so the payload is always valid UTF-8 for the receiver to
/// decode (a plain byte-slice cut can land mid-sequence and produce mojibake).
pub fn build_message(src: &NodeAddress, dst: &NodeAddress, text: &str) -> Vec<u8> {
    let mut packet = build_header(CMD_MESSAGE, FLAG_STANDARD, src, dst);
    packet.extend_from_slice(truncate_on_char_boundary(text, MAX_SINGLE_PAYLOAD_LEN).as_bytes());
    packet
}

/// Longest prefix of `text` that fits in `max_bytes` without splitting a
/// multi-byte UTF-8 character.
fn truncate_on_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Build a BROADCAST packet (sent to all nodes).
pub fn build_broadcast(src: &NodeAddress, text: &str) -> Vec<u8> {
    build_message(src, &NodeAddress::broadcast(), text)
}

/// Build a Dogecoin transaction packet.
/// If payload exceeds MAX_SINGLE_PAYLOAD_LEN, use build_multipart_packets instead.
pub fn build_doge_tx(src: &NodeAddress, dst: &NodeAddress, tx_payload: &[u8]) -> Vec<u8> {
    let mut packet = build_header(CMD_DOGE_TX, FLAG_STANDARD, src, dst);
    let clamped = &tx_payload[..tx_payload.len().min(MAX_SINGLE_PAYLOAD_LEN)];
    packet.extend_from_slice(clamped);
    packet
}

/// Build a GET_SETTINGS command (CMD 0x22) — v0.3.6.
/// Board replies with a 5-byte payload:
/// `[region, community, node, gateway_mode, wifi_enabled]` (13 bytes total).
/// The `wifi_enabled` byte was added in v0.3.7; parsers accept a 4-byte payload
/// from older firmware and default `wifi_enabled` to true.
pub fn build_get_settings(src: &NodeAddress) -> Vec<u8> {
    build_header(CMD_GET_SETTINGS, FLAG_STANDARD, src, &NodeAddress::broadcast())
}

/// Build a SET_GATEWAY command (CMD 0x23) — v0.3.6.
/// Payload byte: 1 = enable gateway mode, 0 = disable.
pub fn build_set_gateway(src: &NodeAddress, enable: bool) -> Vec<u8> {
    let mut packet = build_header(CMD_SET_GATEWAY, FLAG_STANDARD, src, &NodeAddress::broadcast());
    packet.push(if enable { 1u8 } else { 0u8 });
    packet
}

/// Build a WIFI_TOGGLE command (CMD 0x24) — v0.3.7.
/// Payload byte: 1 = enable WiFi radio, 0 = disable (power saving).
pub fn build_wifi_toggle(src: &NodeAddress, enable: bool) -> Vec<u8> {
    let mut packet = build_header(CMD_WIFI_TOGGLE, FLAG_STANDARD, src, &NodeAddress::broadcast());
    packet.push(if enable { 1u8 } else { 0u8 });
    packet
}

/// Build a GET_BATTERY command (CMD 0x26) — v0.3.8.
/// Board replies with 2-byte big-endian u16 (battery voltage in mV).
pub fn build_get_battery(src: &NodeAddress) -> Vec<u8> {
    build_header(CMD_GET_BATTERY, FLAG_STANDARD, src, &NodeAddress::broadcast())
}

/// Build a GET_MAC command (CMD 0x27) — v0.3.8.
/// Board replies with 6 raw MAC address bytes.
pub fn build_get_mac(src: &NodeAddress) -> Vec<u8> {
    build_header(CMD_GET_MAC, FLAG_STANDARD, src, &NodeAddress::broadcast())
}

/// Build a BLE_TOGGLE command (CMD 0x28) — v0.3.16.
/// Payload byte: 1 = enable BLE advertising, 0 = disable (power saving).
/// Board persists setting to NVS.
pub fn build_ble_toggle(src: &NodeAddress, enable: bool) -> Vec<u8> {
    let mut packet = build_header(CMD_BLE_TOGGLE, FLAG_STANDARD, src, &NodeAddress::broadcast());
    packet.push(if enable { 1u8 } else { 0u8 });
    packet
}

/// Build a CMD_REQUEST_BALANCE packet.
/// The gateway that receives this will query Blockbook for `doge_address` and reply with
/// a CMD_MESSAGE containing `"BAL:{koinus}"` addressed back to `src`.
pub fn build_request_balance(src: &NodeAddress, doge_address: &str) -> Vec<u8> {
    let mut packet = build_header(CMD_REQUEST_BALANCE, FLAG_STANDARD, src, &NodeAddress::broadcast());
    let addr_bytes = doge_address.as_bytes();
    packet.extend_from_slice(&addr_bytes[..addr_bytes.len().min(MAX_SINGLE_PAYLOAD_LEN)]);
    packet
}

/// Return the exact total byte count for commands with fixed-size replies.
/// Returns None for variable-length commands (e.g., CMD_MESSAGE, CMD_GET_FIRMWARE_VERSION).
/// Used by the serial read loop to advance the accumulator precisely, avoiding
/// packet-boundary drift when multiple responses arrive in the same read.
pub fn exact_packet_len(cmd: u8) -> Option<usize> {
    match cmd {
        CMD_GET_NODE_ADDR   => Some(8),  // header only
        CMD_PING            => Some(8),  // header only (ACK)
        // CMD_REQUEST_BALANCE has a variable-length payload (Dogecoin address) — handled as variable
        CMD_SET_LORA_PARAMS => Some(8),  // header only (ACK)
        CMD_ADDR_CONFLICT   => Some(8),  // header only
        CMD_SET_NODE_ADDRS  => Some(8),  // header only (ACK)
        CMD_SET_GATEWAY     => Some(9),  // header + 1 byte (gateway_mode)
        CMD_WIFI_TOGGLE     => Some(9),  // header + 1 byte (wifi_enabled)
        CMD_GET_SETTINGS    => Some(13), // header + 5 bytes [region, community, node, gw, wifi]
        CMD_GET_BATTERY     => Some(10), // header + 2 bytes (voltage_mv big-endian)
        CMD_GET_MAC         => Some(14), // header + 6 bytes (MAC address)
        CMD_BLE_TOGGLE      => Some(9),  // header + 1 byte (ble_enabled)
        CMD_RECEIVED_ACK    => Some(8),  // header only; src = remote node that sent the ACK
        CMD_RECEIVED_PING   => Some(8),  // header only; src = remote node that sent the Ping
        _ => None,                       // variable length (0x03 MSG, 0x20 FW version, etc.)
    }
}

/// Check that a payload can actually reach the board over the host serial link.
///
/// The Heltec firmware reads the host header as `[command, payload_size]` — it
/// treats byte 1 as a length, while the host writes its flags byte there. That
/// works only while flags are `0x00`: [`FLAG_MULTIPART`] (`0x01`) makes the
/// board read one payload byte, swallowing the first source-address byte and
/// misaligning every byte after it.
///
/// So a host→board packet must fit in a single frame. Multipart is real
/// protocol — the firmware speaks it over the air, and
/// [`try_build_multipart_packets`] encodes it correctly — but it cannot
/// currently be handed to the board over serial. Sending it anyway produced no
/// error and a corrupted transmission, which for a signed transaction is a
/// silent loss, so callers must check before sending.
///
/// Returns `Err` with a user-facing message when `payload_len` is too large.
pub fn check_host_payload_fits(payload_len: usize) -> Result<(), String> {
    if payload_len <= MAX_SINGLE_PAYLOAD_LEN {
        return Ok(());
    }
    Err(format!(
        "Payload is {} bytes; the board's serial protocol accepts at most {} per packet. \
         Multipart transfer to the board is not supported by the current firmware, so this \
         cannot be sent over LoRa. A signed transaction exceeds the limit whenever it has a \
         change output or more than one input — broadcast it over the internet instead \
         (`radiodoge-cli broadcast`, or the Wallet tab's direct broadcast).",
        payload_len, MAX_SINGLE_PAYLOAD_LEN
    ))
}

/// `true` if `byte` can legitimately start a packet.
///
/// The framing loops use this to resynchronise: the firmware also emits plain
/// `Serial.println` debug text, and any byte that cannot begin a packet is
/// dropped until the stream lines up again. Every transport must agree on this
/// set — a command missing here is silently discarded as noise, which then
/// shifts every packet behind it by one byte.
///
/// The trailing values (`0x3F`, `0x62`, `0x64`, `0x68`, `0x6D`, `0xFE`) are
/// firmware-side message IDs that predate the desktop command range.
pub fn is_known_command(byte: u8) -> bool {
    matches!(
        byte,
        CMD_GET_NODE_ADDR
            | CMD_SET_NODE_ADDRS
            | CMD_PING
            | CMD_MESSAGE
            | CMD_BROADCAST
            | CMD_MULTIPART
            | CMD_DOGE_TX
            | CMD_REQUEST_BALANCE
            | CMD_GET_FIRMWARE_VERSION
            | CMD_SET_LORA_PARAMS
            | CMD_GET_SETTINGS
            | CMD_SET_GATEWAY
            | CMD_WIFI_TOGGLE
            | CMD_ADDR_CONFLICT
            | CMD_GET_BATTERY
            | CMD_GET_MAC
            | CMD_BLE_TOGGLE
            | CMD_RECEIVED_ACK
            | CMD_RECEIVED_PING
            | 0x3F
            | 0x62
            | 0x64
            | 0x68
            | 0x6D
            | 0xFE
    )
}

/// Decide how many bytes of `buf` belong to the packet that starts at `buf[0]`.
///
/// This is the single framing rule shared by every transport that has to cut a
/// byte stream into packets: the desktop serial read loop and the Android
/// USB/BLE bridge. Keeping it in one place is what stops the two paths from
/// drifting apart.
///
/// Returns `None` when `buf` does not yet hold a complete packet — the caller
/// must keep the bytes buffered and retry after the next read. It never returns
/// a length longer than `buf`, so `&buf[..n]` is always a valid slice.
///
/// Rules:
/// - Fixed-length commands ([`exact_packet_len`]) need exactly that many bytes.
/// - `CMD_GET_FIRMWARE_VERSION` carries a NUL-terminated string; the packet ends
///   at the NUL. Without one we wait, unless the payload has already reached
///   [`MAX_SINGLE_PAYLOAD_LEN`] (a board that never sends the terminator must not
///   wedge the stream forever).
/// - Other variable-length commands have no in-band length, so all buffered
///   payload bytes are taken, capped at [`MAX_SINGLE_PAYLOAD_LEN`].
pub fn frame_packet_len(cmd: u8, buf: &[u8]) -> Option<usize> {
    if buf.len() < SINGLE_HDR_LEN {
        return None;
    }

    // A multipart frame is identified by its flags byte, not its command byte —
    // it reuses the carried command (e.g. CMD_DOGE_TX). It has a 12-byte header
    // and, like other variable-length frames, no in-band chunk length.
    if is_multipart_flags(buf[1]) {
        if buf.len() < MULTIPART_HDR_LEN {
            return None;
        }
        let chunk = &buf[MULTIPART_HDR_LEN..];
        return Some(MULTIPART_HDR_LEN + chunk.len().min(MULTIPART_CHUNK_LEN));
    }

    if let Some(n) = exact_packet_len(cmd) {
        // A fixed-size reply split across two reads must wait for its tail,
        // not be emitted short.
        return if buf.len() >= n { Some(n) } else { None };
    }

    let after_hdr = &buf[SINGLE_HDR_LEN..];

    if cmd == CMD_GET_FIRMWARE_VERSION {
        return match after_hdr.iter().position(|&b| b == 0) {
            Some(i) => Some(SINGLE_HDR_LEN + i + 1), // include the NUL
            None if after_hdr.len() >= MAX_SINGLE_PAYLOAD_LEN => {
                Some(SINGLE_HDR_LEN + MAX_SINGLE_PAYLOAD_LEN)
            }
            None => None, // terminator still in flight
        };
    }

    Some(SINGLE_HDR_LEN + after_hdr.len().min(MAX_SINGLE_PAYLOAD_LEN))
}

/// Build a SET_LORA_PARAMS packet (CMD 0x21) — v0.3.3 additive.
///
/// Payload format (8 bytes):
///   [0]  Spreading factor (7–12)
///   [1]  Bandwidth index (0=125kHz, 1=250kHz, 2=500kHz)
///   [2]  Coding rate denominator (5–8, meaning 4/5 .. 4/8)
///   [3]  Frequency high byte  (freq_khz >> 8)
///   [4]  Frequency low byte   (freq_khz & 0xFF)  — freq in kHz, e.g. 915000
///   [5]  TX power in dBm (2–22)
///   [6-7] Reserved (0x00)
pub fn build_set_lora_params(
    src: &NodeAddress,
    sf: u8,
    bw_idx: u8,
    cr: u8,
    freq_khz: u32,
    tx_power: u8,
) -> Vec<u8> {
    let mut packet = build_header(CMD_SET_LORA_PARAMS, FLAG_STANDARD, src, &NodeAddress::broadcast());
    let freq_hi = ((freq_khz >> 8) & 0xFF) as u8;
    let freq_lo = (freq_khz & 0xFF) as u8;
    packet.extend_from_slice(&[sf, bw_idx, cr, freq_hi, freq_lo, tx_power, 0x00, 0x00]);
    packet
}

/// Build a SET_NODE_ADDRS packet to configure the device's LoRa address.
pub fn build_set_node_addr(src: &NodeAddress, new_addr: &NodeAddress) -> Vec<u8> {
    let mut packet = build_header(CMD_SET_NODE_ADDRS, FLAG_STANDARD, src, &NodeAddress::broadcast());
    // Payload: the new address bytes
    packet.push(new_addr.region);
    packet.push(new_addr.community);
    packet.push(new_addr.node);
    packet
}

/// Payload bytes carried by one multipart part (the 4 extra multipart header
/// bytes come out of the single-packet payload budget).
pub const MULTIPART_CHUNK_LEN: usize = MAX_SINGLE_PAYLOAD_LEN - (MULTIPART_HDR_LEN - SINGLE_HDR_LEN);

/// The largest payload that can be expressed as a multipart sequence.
pub const MAX_MULTIPART_PAYLOAD_LEN: usize = MULTIPART_CHUNK_LEN * MAX_MULTIPART_PARTS as usize;

/// Split a large payload into multipart packets (for payloads > MAX_SINGLE_PAYLOAD_LEN).
///
/// Each part has a 12-byte header:
///   [0..8]  Standard header (with FLAG_MULTIPART)
///   [8]     Total parts count
///   [9]     This part's index (0-based)
///   [10-11] Unique session ID (random u16, same for all parts)
///
/// Returns an empty vec if `payload` exceeds [`MAX_MULTIPART_PAYLOAD_LEN`].
/// Prefer [`try_build_multipart_packets`], which reports that as an error
/// instead of leaving the caller to notice nothing was sent.
pub fn build_multipart_packets(
    src: &NodeAddress,
    dst: &NodeAddress,
    cmd: u8,
    payload: &[u8],
) -> Vec<Vec<u8>> {
    try_build_multipart_packets(src, dst, cmd, payload).unwrap_or_default()
}

/// Split a large payload into multipart packets, erroring when it does not fit.
///
/// A payload needing more than [`MAX_MULTIPART_PARTS`] parts cannot be encoded:
/// the part index and count are one byte each in a fixed-size sequence the
/// receiver reassembles. Earlier revisions silently kept the first 20 parts and
/// stamped them with a truncated total, so the receiver reassembled a corrupt
/// payload while the sender reported success — for a signed transaction that is
/// a silent loss, so it is now a hard error.
pub fn try_build_multipart_packets(
    src: &NodeAddress,
    dst: &NodeAddress,
    cmd: u8,
    payload: &[u8],
) -> Result<Vec<Vec<u8>>, String> {
    if payload.len() > MAX_MULTIPART_PAYLOAD_LEN {
        return Err(format!(
            "payload of {} bytes needs {} multipart parts, but the protocol allows at most {} ({} bytes)",
            payload.len(),
            payload.len().div_ceil(MULTIPART_CHUNK_LEN),
            MAX_MULTIPART_PARTS,
            MAX_MULTIPART_PAYLOAD_LEN,
        ));
    }

    let chunks: Vec<&[u8]> = payload.chunks(MULTIPART_CHUNK_LEN).collect();
    let total_parts = chunks.len();
    let session_id = rand::random::<u16>();

    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(idx, chunk)| {
            let mut packet = build_header(cmd, FLAG_MULTIPART, src, dst);
            packet.push(total_parts as u8);
            packet.push(idx as u8);
            packet.extend_from_slice(&session_id.to_be_bytes());
            packet.extend_from_slice(chunk);
            packet
        })
        .collect())
}

// ─── Multipart reassembly ────────────────────────────────────────────────────

/// One parsed multipart frame, as produced by [`parse_multipart`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartPart<'a> {
    /// The command the reassembled payload carries (e.g. [`CMD_DOGE_TX`]).
    pub command: u8,
    pub source: NodeAddress,
    pub destination: NodeAddress,
    /// Mesh hop count from the flags byte's upper nibble.
    pub hops: u8,
    /// How many parts make up the complete payload.
    pub total_parts: u8,
    /// This part's 0-based position.
    pub index: u8,
    /// Random id shared by every part of one payload.
    pub session_id: u16,
    pub chunk: &'a [u8],
}

/// Parse a multipart frame. Returns `None` if `buf` is not a well-formed
/// multipart packet — including structurally impossible part counts/indices,
/// which are rejected here so the reassembler never has to defend against them.
pub fn parse_multipart(buf: &[u8]) -> Option<MultipartPart<'_>> {
    if buf.len() < MULTIPART_HDR_LEN || !is_multipart_flags(buf[1]) {
        return None;
    }
    let total_parts = buf[8];
    let index = buf[9];
    if total_parts == 0 || total_parts > MAX_MULTIPART_PARTS || index >= total_parts {
        return None;
    }
    Some(MultipartPart {
        command: buf[0],
        source: NodeAddress::new(buf[2], buf[3], buf[4]),
        destination: NodeAddress::new(buf[5], buf[6], buf[7]),
        hops: hops_from_flags(buf[1]),
        total_parts,
        index,
        session_id: u16::from_be_bytes([buf[10], buf[11]]),
        chunk: &buf[MULTIPART_HDR_LEN..],
    })
}

/// How long an incomplete multipart session is kept before being discarded.
/// Matches the firmware's `MULTIPART_TIMEOUT_MS` (30 s) so both ends give up together.
pub const MULTIPART_SESSION_TIMEOUT_SECS: u64 = 30;

/// Maximum multipart payloads being assembled at once. Beyond this the oldest
/// session is evicted, bounding memory against a node that starts many
/// sequences and finishes none.
pub const MAX_CONCURRENT_MULTIPART_SESSIONS: usize = 8;

#[derive(Debug)]
struct MultipartSession {
    command: u8,
    destination: NodeAddress,
    hops: u8,
    total_parts: u8,
    /// One slot per part; `None` until that part arrives.
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
    started_at: u64,
}

/// Reassembles multipart payloads split by [`try_build_multipart_packets`].
///
/// Parts may arrive out of order, duplicated, or interleaved with other
/// sequences — sessions are keyed by `(source, session_id)`. Incomplete
/// sessions are dropped after [`MULTIPART_SESSION_TIMEOUT_SECS`], and at most
/// [`MAX_CONCURRENT_MULTIPART_SESSIONS`] are tracked at once, so a peer that
/// never finishes a sequence cannot grow memory without bound.
///
/// Time is passed in rather than read from the clock, which keeps the type
/// deterministic and lets the tests drive expiry directly.
///
/// ```
/// # use radiodoge_core::radio::*;
/// # use radiodoge_core::types::NodeAddress;
/// let (src, dst) = (NodeAddress::new(10, 0, 1), NodeAddress::new(10, 0, 2));
/// let payload = vec![7u8; 400];
/// let packets = try_build_multipart_packets(&src, &dst, CMD_DOGE_TX, &payload).unwrap();
///
/// let mut rx = MultipartReassembler::new();
/// let mut done = None;
/// for pkt in &packets {
///     done = rx.push(&parse_multipart(pkt).unwrap(), 0);
/// }
/// assert_eq!(done.unwrap().payload, payload);
/// ```
#[derive(Debug, Default)]
pub struct MultipartReassembler {
    sessions: Vec<((NodeAddress, u16), MultipartSession)>,
}

/// A fully reassembled multipart payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledPayload {
    pub command: u8,
    pub source: NodeAddress,
    pub destination: NodeAddress,
    pub hops: u8,
    pub payload: Vec<u8>,
}

impl MultipartReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of sequences currently being assembled (for diagnostics/tests).
    pub fn pending_sessions(&self) -> usize {
        self.sessions.len()
    }

    /// Absorb one part. Returns the complete payload once the final missing
    /// part arrives, otherwise `None`.
    ///
    /// `now_secs` is a monotonic-ish wall clock in seconds, used only for
    /// session expiry.
    pub fn push(&mut self, part: &MultipartPart<'_>, now_secs: u64) -> Option<AssembledPayload> {
        self.expire(now_secs);

        let key = (part.source.clone(), part.session_id);

        let slot = self.sessions.iter().position(|(k, _)| *k == key);
        let slot = match slot {
            Some(i) => {
                // A sender reusing a session id with a different shape means the
                // previous sequence was abandoned; start over rather than mixing
                // parts of two payloads into one buffer.
                let stale = {
                    let s = &self.sessions[i].1;
                    s.total_parts != part.total_parts || s.command != part.command
                };
                if stale {
                    self.sessions[i].1 = MultipartSession::new(part, now_secs);
                }
                i
            }
            None => {
                if self.sessions.len() >= MAX_CONCURRENT_MULTIPART_SESSIONS {
                    // Evict the least recently started session.
                    if let Some(oldest) = self
                        .sessions
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, (_, s))| s.started_at)
                        .map(|(i, _)| i)
                    {
                        self.sessions.remove(oldest);
                    }
                }
                self.sessions
                    .push((key, MultipartSession::new(part, now_secs)));
                self.sessions.len() - 1
            }
        };

        let session = &mut self.sessions[slot].1;

        // A duplicate part is ignored rather than counted twice; a retransmit
        // must not make the session look complete when parts are still missing.
        let cell = &mut session.parts[part.index as usize];
        if cell.is_none() {
            *cell = Some(part.chunk.to_vec());
            session.received += 1;
        }
        // Track the highest hop count seen — the payload travelled at least that far.
        session.hops = session.hops.max(part.hops);

        if session.received < session.total_parts as usize {
            return None;
        }

        let (_, session) = self.sessions.remove(slot);
        let mut payload = Vec::new();
        for chunk in session.parts.into_iter() {
            payload.extend_from_slice(&chunk.expect("all parts present when received == total"));
        }
        Some(AssembledPayload {
            command: session.command,
            source: part.source.clone(),
            destination: session.destination,
            hops: session.hops,
            payload,
        })
    }

    /// Drop sessions that have been incomplete for too long.
    fn expire(&mut self, now_secs: u64) {
        self.sessions.retain(|(_, s)| {
            now_secs.saturating_sub(s.started_at) < MULTIPART_SESSION_TIMEOUT_SECS
        });
    }
}

impl MultipartSession {
    fn new(part: &MultipartPart<'_>, now_secs: u64) -> Self {
        MultipartSession {
            command: part.command,
            destination: part.destination.clone(),
            hops: part.hops,
            total_parts: part.total_parts,
            parts: vec![None; part.total_parts as usize],
            received: 0,
            started_at: now_secs,
        }
    }
}

/// Parse an incoming byte buffer into an IncomingPacket.
///
/// Returns None if the buffer is too short or malformed.
pub fn parse_incoming(buf: &[u8], rssi: i16) -> Option<IncomingPacket> {
    if buf.len() < SINGLE_HDR_LEN {
        return None;
    }

    let command = buf[0];
    let hops = hops_from_flags(buf[1]);

    let source = NodeAddress::new(buf[2], buf[3], buf[4]);
    let destination = NodeAddress::new(buf[5], buf[6], buf[7]);

    let payload = &buf[SINGLE_HDR_LEN..];
    let payload_hex = hex::encode(payload);

    // Try to decode known payload formats
    let decoded = decode_payload(command, payload);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Some(IncomingPacket {
        timestamp,
        source,
        destination,
        command,
        payload_hex,
        decoded,
        rssi,
        hops,
    })
}

/// Turn one framed packet into an [`IncomingPacket`], transparently reassembling
/// multipart sequences.
///
/// This is what both framing loops call, so the desktop serial path and the
/// Android USB/BLE bridge treat multipart identically and every downstream
/// consumer — GUI, packet log, gateway daemon — sees a single complete packet
/// rather than fragments it would have to stitch together itself.
///
/// Returns `None` for a fragment that does not yet complete a payload.
pub fn ingest_packet(
    buf: &[u8],
    rssi: i16,
    reassembler: &mut MultipartReassembler,
    now_secs: u64,
) -> Option<IncomingPacket> {
    if buf.len() >= SINGLE_HDR_LEN && is_multipart_flags(buf[1]) {
        // A malformed multipart frame is dropped rather than surfaced as a
        // bogus single packet with multipart header bytes in its payload.
        let part = parse_multipart(buf)?;
        let done = reassembler.push(&part, now_secs)?;
        return Some(assembled_to_packet(done, rssi));
    }
    parse_incoming(buf, rssi)
}

/// Build the `IncomingPacket` a completed multipart payload represents, as if it
/// had arrived as one packet.
fn assembled_to_packet(a: AssembledPayload, rssi: i16) -> IncomingPacket {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    IncomingPacket {
        timestamp,
        source: a.source,
        destination: a.destination,
        command: a.command,
        payload_hex: hex::encode(&a.payload),
        decoded: decode_payload(a.command, &a.payload),
        rssi,
        hops: a.hops,
    }
}

/// Attempt to decode a packet payload based on command type.
fn decode_payload(command: u8, payload: &[u8]) -> Option<String> {
    match command {
        CMD_PING => Some("🏓 PING".to_string()),
        CMD_MESSAGE | CMD_BROADCAST => {
            std::str::from_utf8(payload).ok().map(|s| {
                let text = s.trim_end_matches('\0').trim();
                // Gateway balance response: "BAL:{koinus}"
                if let Some(koinus_str) = text.strip_prefix("BAL:") {
                    if let Ok(koinus) = koinus_str.parse::<u64>() {
                        return format!("💰 Balance: {:.8} DOGE", koinus as f64 / 1e8);
                    }
                }
                // Gateway TX acknowledgement: "TX_ACK:{txid}"
                if let Some(txid) = text.strip_prefix("TX_ACK:") {
                    return format!("✅ TX confirmed: txid={}", txid);
                }
                format!("💬 {}", text)
            })
        }
        CMD_GET_NODE_ADDR => Some("📍 GET NODE ADDRESS".to_string()),
        CMD_SET_NODE_ADDRS => {
            if payload.len() >= 3 {
                Some(format!("📍 SET ADDR: {}.{}.{}", payload[0], payload[1], payload[2]))
            } else {
                None
            }
        }
        CMD_DOGE_TX => {
            if crate::wallet::is_signed_tx_payload(payload) {
                Some(format!("🐕 {}", crate::wallet::describe_signed_tx(payload)))
            } else {
                crate::wallet::decode_transaction_payload(payload)
                    .map(|s| format!("🐕 {}", s))
            }
        }
        CMD_REQUEST_BALANCE => {
            if payload.is_empty() {
                Some("💰 REQUEST BALANCE".to_string())
            } else {
                let addr = std::str::from_utf8(payload)
                    .unwrap_or("?")
                    .trim_matches('\0')
                    .trim();
                Some(format!("💰 REQUEST BALANCE: {}", addr))
            }
        }
        CMD_GET_FIRMWARE_VERSION => {
            std::str::from_utf8(payload)
                .ok()
                .map(|s| format!("🔧 FW: {}", s.trim_matches('\0').trim()))
        }
        CMD_GET_SETTINGS => {
            if payload.len() >= 4 {
                let gw = if payload[3] != 0 { "GW ON" } else { "GW OFF" };
                let wifi = if payload.len() >= 5 {
                    if payload[4] != 0 { " WiFi ON" } else { " WiFi OFF" }
                } else { "" };
                Some(format!("⚙️ SETTINGS: addr={}.{}.{} {}{}", payload[0], payload[1], payload[2], gw, wifi))
            } else {
                Some("⚙️ GET_SETTINGS".to_string())
            }
        }
        CMD_SET_GATEWAY => {
            if !payload.is_empty() {
                Some(format!("🌐 SET_GATEWAY: {}", if payload[0] != 0 { "ON" } else { "OFF" }))
            } else {
                Some("🌐 SET_GATEWAY".to_string())
            }
        }
        CMD_WIFI_TOGGLE => {
            if !payload.is_empty() {
                Some(format!("📡 WIFI: {}", if payload[0] != 0 { "ON" } else { "OFF" }))
            } else {
                Some("📡 WIFI_TOGGLE".to_string())
            }
        }
        CMD_ADDR_CONFLICT => {
            if payload.len() >= 3 {
                Some(format!("⚠️ ADDR CONFLICT: {}.{}.{}", payload[0], payload[1], payload[2]))
            } else {
                Some("⚠️ ADDR CONFLICT".to_string())
            }
        }
        CMD_GET_BATTERY => {
            if payload.len() >= 2 {
                let mv = u16::from_be_bytes([payload[0], payload[1]]);
                Some(format!("🔋 BATTERY: {:.2}V ({}mV)", mv as f32 / 1000.0, mv))
            } else {
                Some("🔋 GET_BATTERY".to_string())
            }
        }
        CMD_GET_MAC => {
            if payload.len() >= 6 {
                Some(format!("🔑 MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                    payload[0], payload[1], payload[2], payload[3], payload[4], payload[5]))
            } else {
                Some("🔑 GET_MAC".to_string())
            }
        }
        CMD_BLE_TOGGLE => {
            if !payload.is_empty() {
                Some(format!("📶 BLE: {}", if payload[0] != 0 { "ON" } else { "OFF" }))
            } else {
                Some("📶 BLE_TOGGLE".to_string())
            }
        }
        CMD_RECEIVED_ACK  => Some("📨 ACK received".to_string()),
        CMD_RECEIVED_PING => Some("🏓 Ping received".to_string()),
        _ => None,
    }
}

/// Build a standard 8-byte packet header.
fn build_header(
    cmd: u8,
    flags: u8,
    src: &NodeAddress,
    dst: &NodeAddress,
) -> Vec<u8> {
    vec![
        cmd,
        flags,
        src.region,
        src.community,
        src.node,
        dst.region,
        dst.community,
        dst.node,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_src() -> NodeAddress { NodeAddress::new(10, 0, 1) }
    fn test_dst() -> NodeAddress { NodeAddress::new(10, 0, 2) }

    #[test]
    fn test_build_ping() {
        let packet = build_ping(&test_src(), &test_dst());
        assert_eq!(packet.len(), SINGLE_HDR_LEN);
        assert_eq!(packet[0], CMD_PING);
        assert_eq!(packet[2], 10); // src region
        assert_eq!(packet[5], 10); // dst region
    }

    #[test]
    fn test_build_message() {
        let packet = build_message(&test_src(), &test_dst(), "much wow");
        assert_eq!(packet[0], CMD_MESSAGE);
        let payload = &packet[SINGLE_HDR_LEN..];
        assert_eq!(payload, b"much wow");
    }

    #[test]
    fn test_multipart_splitting() {
        let big_payload = vec![0xAA; 400];
        let parts = build_multipart_packets(&test_src(), &test_dst(), CMD_MESSAGE, &big_payload);
        assert!(parts.len() > 1, "Large payload should be split into multiple parts");
        for part in &parts {
            assert!(part.len() <= MAX_SINGLE_PAYLOAD_LEN + SINGLE_HDR_LEN + 4);
        }
    }

    #[test]
    fn test_parse_incoming() {
        let packet = build_ping(&test_src(), &test_dst());
        let parsed = parse_incoming(&packet, -70).expect("Should parse valid packet");
        assert_eq!(parsed.command, CMD_PING);
        assert_eq!(parsed.source.region, 10);
        assert_eq!(parsed.rssi, -70);
        assert_eq!(parsed.hops, 0, "a freshly built packet has 0 hops");
    }

    #[test]
    fn test_hop_flags_pack_unpack() {
        assert_eq!(hops_from_flags(0x00), 0);
        assert_eq!(hops_from_flags(0x01), 0); // multipart flag, 0 hops
        assert_eq!(hops_from_flags(0x30), 3);
        assert_eq!(hops_from_flags(0x31), 3); // 3 hops + multipart flag
        // Round-trip: base flag preserved in the low nibble, hops in the high nibble.
        let f = flags_with_hops(FLAG_MULTIPART, 5);
        assert_eq!(f & 0x0F, FLAG_MULTIPART);
        assert_eq!(hops_from_flags(f), 5);
        // Hop count saturates at 15 (4-bit field).
        assert_eq!(hops_from_flags(flags_with_hops(FLAG_STANDARD, 99)), 15);
    }

    #[test]
    fn test_relay_packet_increments_hops() {
        let pkt = build_broadcast(&test_src(), "mesh msg");
        let relayed = relay_packet(&pkt).expect("first relay should succeed");
        let parsed = parse_incoming(&relayed, 0).expect("relayed packet parses");
        assert_eq!(parsed.hops, 1);
        // Source/dest/command survive the relay unchanged.
        assert_eq!(parsed.command, CMD_MESSAGE);
        assert_eq!(parsed.source, test_src());

        // Relaying again bumps to 2.
        let relayed2 = relay_packet(&relayed).expect("second relay");
        assert_eq!(parse_incoming(&relayed2, 0).unwrap().hops, 2);
    }

    #[test]
    fn test_relay_packet_drops_at_hop_limit() {
        let mut pkt = build_broadcast(&test_src(), "loopy");
        // Force the hop count to the limit.
        pkt[1] = flags_with_hops(pkt[1], MAX_MESH_HOPS);
        assert!(
            relay_packet(&pkt).is_none(),
            "a packet at the hop limit must be dropped, not relayed"
        );
    }

    #[test]
    fn test_relay_packet_rejects_short_buffer() {
        assert!(relay_packet(&[0x03, 0x00, 0x01]).is_none());
    }

    /// Regression test for the back-to-back framing bug fixed in v0.3.16.
    ///
    /// When two packets arrive in a single serial read, the framing loop must
    /// pre-slice the accumulator to the exact packet length before calling
    /// parse_incoming.  Without this, the first packet's payload_hex includes
    /// the raw bytes of every subsequent packet.
    #[test]
    fn test_parse_incoming_does_not_bleed_into_next_packet() {
        let src = test_src();
        let dst = test_dst();

        // Two back-to-back packets in one buffer, as they might arrive from the
        // serial port: a CMD_MESSAGE followed immediately by a CMD_PING.
        let mut combined = build_message(&src, &dst, "hello");
        combined.extend_from_slice(&build_ping(&src, &dst));

        let msg_len = SINGLE_HDR_LEN + b"hello".len();

        // Parse only the first packet's bytes.
        let parsed = parse_incoming(&combined[..msg_len], 0)
            .expect("Should parse valid CMD_MESSAGE");

        assert_eq!(parsed.command, CMD_MESSAGE);
        // payload_hex must be exactly "hello" — not "hello" + the PING header bytes.
        assert_eq!(
            parsed.payload_hex,
            hex::encode(b"hello"),
            "payload_hex must not include bytes from the subsequent PING packet"
        );
    }

    /// A fixed-length reply split across two serial reads must not be emitted short.
    ///
    /// Regression test: the framing loops used to clamp the packet length with
    /// `.min(buffer.len())`, so a 13-byte GET_SETTINGS whose first 9 bytes had
    /// arrived was parsed as a 9-byte packet with a 1-byte payload. The
    /// remaining 4 bytes were then misread as the start of a new packet.
    #[test]
    fn test_frame_packet_len_waits_for_partial_fixed_length_packet() {
        let full = {
            let mut p = build_get_settings(&test_src());
            p.extend_from_slice(&[10, 0, 1, 1, 1]); // 5-byte settings payload
            p
        };
        assert_eq!(full.len(), 13);

        // Every prefix short of the full packet must report "not yet".
        for n in SINGLE_HDR_LEN..full.len() {
            assert_eq!(
                frame_packet_len(CMD_GET_SETTINGS, &full[..n]),
                None,
                "a {}-byte prefix of a 13-byte packet must wait for more data",
                n
            );
        }
        assert_eq!(frame_packet_len(CMD_GET_SETTINGS, &full), Some(13));

        // With a second packet appended, the length is still exactly the first packet.
        let mut two = full.clone();
        two.extend_from_slice(&build_ping(&test_src(), &test_dst()));
        assert_eq!(frame_packet_len(CMD_GET_SETTINGS, &two), Some(13));
    }

    /// A firmware-version string is NUL-terminated, so framing must wait for the
    /// terminator instead of emitting whatever bytes happen to have arrived.
    #[test]
    fn test_frame_packet_len_waits_for_version_terminator() {
        let mut pkt = vec![CMD_GET_FIRMWARE_VERSION, 0x00, 10, 0, 1, 0xFF, 0xFF, 0xFF];
        pkt.extend_from_slice(b"RadioDoge NV3FW08");
        // No NUL yet and the payload is well under the cap — keep waiting.
        assert_eq!(frame_packet_len(CMD_GET_FIRMWARE_VERSION, &pkt), None);

        pkt.push(0);
        assert_eq!(frame_packet_len(CMD_GET_FIRMWARE_VERSION, &pkt), Some(pkt.len()));

        // A board that never sends a terminator must not wedge the stream forever.
        let mut runaway = vec![CMD_GET_FIRMWARE_VERSION, 0x00, 10, 0, 1, 0xFF, 0xFF, 0xFF];
        runaway.extend(vec![b'x'; MAX_SINGLE_PAYLOAD_LEN + 40]);
        assert_eq!(
            frame_packet_len(CMD_GET_FIRMWARE_VERSION, &runaway),
            Some(SINGLE_HDR_LEN + MAX_SINGLE_PAYLOAD_LEN)
        );
    }

    /// frame_packet_len must never return a length past the end of the buffer,
    /// so `&buf[..n]` is always sliceable.
    #[test]
    fn test_frame_packet_len_never_exceeds_buffer() {
        for cmd in 0u8..=255 {
            if !is_known_command(cmd) {
                continue;
            }
            for len in 0..40usize {
                let buf = vec![cmd; len];
                if let Some(n) = frame_packet_len(cmd, &buf) {
                    assert!(
                        n <= buf.len(),
                        "cmd 0x{:02X} with {} bytes returned length {}",
                        cmd, len, n
                    );
                }
            }
        }
    }

    /// Every command the framing loops accept as a packet start must be known,
    /// and the two transports must agree on the set.
    #[test]
    fn test_is_known_command_covers_all_protocol_commands() {
        for cmd in [
            CMD_GET_NODE_ADDR, CMD_SET_NODE_ADDRS, CMD_PING, CMD_MESSAGE,
            CMD_BROADCAST, CMD_MULTIPART, CMD_DOGE_TX, CMD_REQUEST_BALANCE,
            CMD_GET_FIRMWARE_VERSION, CMD_SET_LORA_PARAMS, CMD_GET_SETTINGS,
            CMD_SET_GATEWAY, CMD_WIFI_TOGGLE, CMD_ADDR_CONFLICT, CMD_GET_BATTERY,
            CMD_GET_MAC, CMD_BLE_TOGGLE, CMD_RECEIVED_ACK, CMD_RECEIVED_PING,
        ] {
            assert!(is_known_command(cmd), "CMD 0x{:02X} must be recognised", cmd);
        }
        // Any command with a fixed reply length is by definition a real command.
        for cmd in 0u8..=255 {
            if exact_packet_len(cmd).is_some() {
                assert!(is_known_command(cmd), "CMD 0x{:02X} has a length but is unknown", cmd);
            }
        }
        assert!(!is_known_command(0x99), "arbitrary bytes are not commands");
    }

    /// Multipart must refuse payloads it cannot encode instead of truncating them.
    #[test]
    fn test_multipart_rejects_oversized_payload() {
        let src = test_src();
        let dst = test_dst();

        let max_ok = vec![0xAA; MAX_MULTIPART_PAYLOAD_LEN];
        let parts = try_build_multipart_packets(&src, &dst, CMD_DOGE_TX, &max_ok)
            .expect("a payload at the limit must encode");
        assert_eq!(parts.len(), MAX_MULTIPART_PARTS as usize);
        // Reassembling the chunks must reproduce the original payload exactly.
        let rebuilt: Vec<u8> = parts
            .iter()
            .flat_map(|p| p[MULTIPART_HDR_LEN..].iter().copied())
            .collect();
        assert_eq!(rebuilt, max_ok, "no bytes may be lost across the split");
        // Every part agrees on the total and carries the same session id.
        let session = &parts[0][10..12];
        for (i, p) in parts.iter().enumerate() {
            assert_eq!(p[8] as usize, parts.len(), "part {} has the wrong total", i);
            assert_eq!(p[9] as usize, i, "part {} has the wrong index", i);
            assert_eq!(&p[10..12], session, "part {} has a different session id", i);
        }

        let too_big = vec![0xAA; MAX_MULTIPART_PAYLOAD_LEN + 1];
        assert!(
            try_build_multipart_packets(&src, &dst, CMD_DOGE_TX, &too_big).is_err(),
            "an unencodable payload must error, not be silently truncated"
        );
    }

    /// The board reads header byte 1 as a payload length, so only single packets
    /// can be sent to it over serial.
    #[test]
    fn test_check_host_payload_fits() {
        assert!(check_host_payload_fits(0).is_ok());
        assert!(check_host_payload_fits(MAX_SINGLE_PAYLOAD_LEN).is_ok());
        let err = check_host_payload_fits(MAX_SINGLE_PAYLOAD_LEN + 1)
            .expect_err("oversized payloads must be rejected");
        assert!(err.contains("193"), "the error should name the actual size: {}", err);
    }

    /// Truncating an over-long message must not split a multi-byte character.
    #[test]
    fn test_build_message_truncates_on_char_boundary() {
        // 'é' is 2 bytes, so a naive cut at 192 would land mid-character.
        let text = "é".repeat(200);
        let pkt = build_message(&test_src(), &test_dst(), &text);
        let payload = &pkt[SINGLE_HDR_LEN..];
        assert!(payload.len() <= MAX_SINGLE_PAYLOAD_LEN);
        std::str::from_utf8(payload).expect("truncated payload must stay valid UTF-8");

        // ASCII text is unaffected and still fills the packet.
        let ascii = "a".repeat(200);
        let pkt = build_message(&test_src(), &test_dst(), &ascii);
        assert_eq!(pkt.len() - SINGLE_HDR_LEN, MAX_SINGLE_PAYLOAD_LEN);
    }

    // ─── Multipart reassembly ────────────────────────────────────────────────

    fn multipart_for(payload: &[u8]) -> Vec<Vec<u8>> {
        try_build_multipart_packets(&test_src(), &test_dst(), CMD_DOGE_TX, payload)
            .expect("payload should encode")
    }

    #[test]
    fn test_reassembles_split_payload_in_order() {
        let payload: Vec<u8> = (0..500u32).map(|i| (i % 251) as u8).collect();
        let packets = multipart_for(&payload);
        assert!(packets.len() > 1, "payload should actually split");

        let mut rx = MultipartReassembler::new();
        let mut out = None;
        for (i, pkt) in packets.iter().enumerate() {
            let part = parse_multipart(pkt).expect("each frame parses");
            let done = rx.push(&part, 0);
            if i + 1 < packets.len() {
                assert!(done.is_none(), "must not complete before the last part");
            } else {
                out = done;
            }
        }
        let done = out.expect("final part completes the payload");
        assert_eq!(done.payload, payload, "reassembled payload must be byte-exact");
        assert_eq!(done.command, CMD_DOGE_TX);
        assert_eq!(done.source, test_src());
        assert_eq!(done.destination, test_dst());
        assert_eq!(rx.pending_sessions(), 0, "completed session must be released");
    }

    #[test]
    fn test_reassembles_out_of_order_and_ignores_duplicates() {
        let payload = vec![0x5Au8; 600];
        let packets = multipart_for(&payload);
        assert!(packets.len() >= 3);

        let mut rx = MultipartReassembler::new();
        // Deliver last-to-first, and send every part twice.
        let mut out = None;
        for pkt in packets.iter().rev() {
            let part = parse_multipart(pkt).unwrap();
            if let Some(done) = rx.push(&part, 0) {
                out = Some(done);
            }
            // A duplicate must not double-count toward completion.
            if let Some(done) = rx.push(&part, 0) {
                out = Some(done);
            }
        }
        assert_eq!(out.expect("completes despite reordering").payload, payload);
    }

    /// A duplicate must never let a session look complete while parts are missing.
    #[test]
    fn test_duplicates_alone_never_complete_a_session() {
        let payload = vec![0x11u8; 600];
        let packets = multipart_for(&payload);
        let first = parse_multipart(&packets[0]).unwrap();

        let mut rx = MultipartReassembler::new();
        for _ in 0..50 {
            assert!(
                rx.push(&first, 0).is_none(),
                "resending one part must never complete a multi-part payload"
            );
        }
    }

    /// Two senders transmitting at once must not have their chunks interleaved.
    #[test]
    fn test_concurrent_sessions_stay_separate() {
        let a_payload = vec![0xAAu8; 400];
        let b_payload = vec![0xBBu8; 400];
        let src_b = NodeAddress::new(20, 5, 9);

        let a = try_build_multipart_packets(&test_src(), &test_dst(), CMD_DOGE_TX, &a_payload).unwrap();
        let b = try_build_multipart_packets(&src_b, &test_dst(), CMD_MESSAGE, &b_payload).unwrap();

        let mut rx = MultipartReassembler::new();
        let mut a_done = None;
        let mut b_done = None;
        // Interleave the two streams.
        for i in 0..a.len().max(b.len()) {
            if let Some(p) = a.get(i) {
                if let Some(d) = rx.push(&parse_multipart(p).unwrap(), 0) { a_done = Some(d); }
            }
            if let Some(p) = b.get(i) {
                if let Some(d) = rx.push(&parse_multipart(p).unwrap(), 0) { b_done = Some(d); }
            }
        }
        let a_done = a_done.expect("stream A completes");
        let b_done = b_done.expect("stream B completes");
        assert_eq!(a_done.payload, a_payload);
        assert_eq!(a_done.command, CMD_DOGE_TX);
        assert_eq!(b_done.payload, b_payload);
        assert_eq!(b_done.command, CMD_MESSAGE);
        assert_eq!(b_done.source, src_b);
    }

    #[test]
    fn test_stale_session_expires_and_does_not_corrupt_a_retry() {
        let payload = vec![0xC3u8; 600];
        let packets = multipart_for(&payload);

        let mut rx = MultipartReassembler::new();
        rx.push(&parse_multipart(&packets[0]).unwrap(), 0);
        assert_eq!(rx.pending_sessions(), 1);

        // Long after the timeout, the abandoned session is dropped.
        let later = MULTIPART_SESSION_TIMEOUT_SECS + 1;
        rx.push(&parse_multipart(&packets[1]).unwrap(), later);
        assert_eq!(rx.pending_sessions(), 1, "the expired session was replaced, not added to");

        // A clean retransmission of the whole sequence still assembles correctly.
        let mut out = None;
        for pkt in &packets {
            if let Some(d) = rx.push(&parse_multipart(pkt).unwrap(), later) {
                out = Some(d);
            }
        }
        assert_eq!(out.expect("retry completes").payload, payload);
    }

    /// A peer that opens sequences and never finishes them must not grow memory.
    #[test]
    fn test_session_table_is_bounded() {
        let payload = vec![0x77u8; 600];
        let mut rx = MultipartReassembler::new();

        for n in 0..(MAX_CONCURRENT_MULTIPART_SESSIONS * 4) {
            // A distinct source each time forces a distinct session key.
            let src = NodeAddress::new(30, (n / 256) as u8, (n % 256) as u8);
            let packets =
                try_build_multipart_packets(&src, &test_dst(), CMD_DOGE_TX, &payload).unwrap();
            rx.push(&parse_multipart(&packets[0]).unwrap(), 0);
            assert!(
                rx.pending_sessions() <= MAX_CONCURRENT_MULTIPART_SESSIONS,
                "session table must stay bounded, saw {}",
                rx.pending_sessions()
            );
        }
    }

    #[test]
    fn test_parse_multipart_rejects_malformed_frames() {
        let payload = vec![0x01u8; 400];
        let good = multipart_for(&payload);

        // A single (non-multipart) packet is not a multipart frame.
        assert!(parse_multipart(&build_ping(&test_src(), &test_dst())).is_none());
        // Too short to hold a 12-byte multipart header.
        assert!(parse_multipart(&good[0][..MULTIPART_HDR_LEN - 1]).is_none());

        // total_parts = 0 is structurally impossible.
        let mut zero = good[0].clone();
        zero[8] = 0;
        assert!(parse_multipart(&zero).is_none());

        // index >= total_parts would index past the session buffer.
        let mut oob = good[0].clone();
        oob[9] = oob[8];
        assert!(parse_multipart(&oob).is_none(), "out-of-range index must be rejected");

        // total_parts beyond the protocol maximum.
        let mut huge = good[0].clone();
        huge[8] = MAX_MULTIPART_PARTS + 1;
        huge[9] = 0;
        assert!(parse_multipart(&huge).is_none());
    }

    /// A relayed multipart frame carries hops in the flags' upper nibble; that
    /// must not stop it being recognised as multipart.
    #[test]
    fn test_multipart_survives_relay_hops() {
        let payload = vec![0x42u8; 400];
        let packets = multipart_for(&payload);

        let mut rx = MultipartReassembler::new();
        let mut out = None;
        for pkt in &packets {
            let relayed = relay_packet(pkt).expect("multipart frames relay");
            assert!(is_multipart_flags(relayed[1]), "relay must preserve the multipart bit");
            if let Some(d) = rx.push(&parse_multipart(&relayed).unwrap(), 0) {
                out = Some(d);
            }
        }
        let done = out.expect("relayed sequence still assembles");
        assert_eq!(done.payload, payload);
        assert_eq!(done.hops, 1, "hop count should survive reassembly");
    }

    /// ingest_packet is what both framing loops call: single packets pass
    /// straight through, multipart surfaces only once complete.
    #[test]
    fn test_ingest_packet_handles_both_shapes() {
        let mut rx = MultipartReassembler::new();

        // A single packet is returned immediately.
        let ping = build_ping(&test_src(), &test_dst());
        let got = ingest_packet(&ping, -70, &mut rx, 0).expect("single packet passes through");
        assert_eq!(got.command, CMD_PING);
        assert_eq!(got.rssi, -70);

        // A multipart sequence yields nothing until the last frame.
        let payload = vec![0x9Eu8; 400];
        let packets = multipart_for(&payload);
        for pkt in &packets[..packets.len() - 1] {
            assert!(ingest_packet(pkt, -70, &mut rx, 0).is_none());
        }
        let done = ingest_packet(packets.last().unwrap(), -70, &mut rx, 0)
            .expect("final frame completes the payload");
        assert_eq!(hex::decode(&done.payload_hex).unwrap(), payload);
        assert_eq!(done.command, CMD_DOGE_TX);
    }

    /// A signed transaction too large for one packet must survive the
    /// split → reassemble round trip intact, and be recognised as signed.
    #[test]
    fn test_signed_tx_round_trips_through_multipart() {
        // A plausible signed-tx shape: version prefix plus body, over the limit.
        let mut tx = vec![0x01, 0x00, 0x00, 0x00];
        tx.extend((0..300u32).map(|i| (i % 256) as u8));
        assert!(tx.len() > MAX_SINGLE_PAYLOAD_LEN);
        assert!(crate::wallet::is_signed_tx_payload(&tx));

        let packets = multipart_for(&tx);
        let mut rx = MultipartReassembler::new();
        let mut out = None;
        for pkt in &packets {
            if let Some(d) = ingest_packet(pkt, 0, &mut rx, 0) {
                out = Some(d);
            }
        }
        let done = out.expect("transaction reassembles");
        let recovered = hex::decode(&done.payload_hex).unwrap();
        assert_eq!(recovered, tx, "a signed transaction must survive byte-exact");
        assert!(
            crate::wallet::is_signed_tx_payload(&recovered),
            "the gateway must still recognise it as a signed transaction"
        );
    }

    /// Framing must size multipart frames by their 12-byte header, not the
    /// 8-byte single-packet one.
    #[test]
    fn test_frame_packet_len_handles_multipart() {
        let payload = vec![0xEEu8; 400];
        let packets = multipart_for(&payload);
        let first = &packets[0];

        assert_eq!(
            frame_packet_len(first[0], first),
            Some(first.len()),
            "a complete multipart frame is consumed whole"
        );
        // Still arriving: not even the header is in yet.
        assert_eq!(frame_packet_len(first[0], &first[..MULTIPART_HDR_LEN - 1]), None);
        // Back-to-back frames: the first must not swallow the second.
        let mut two = first.clone();
        two.extend_from_slice(&packets[1]);
        assert_eq!(frame_packet_len(two[0], &two), Some(first.len()));
    }

    /// Verify exact_packet_len returns the correct size for every fixed-length command
    /// and None for all variable-length commands.
    #[test]
    fn test_exact_packet_len_coverage() {
        // Fixed-length commands and their expected total byte counts
        let fixed = [
            (CMD_GET_NODE_ADDR,   8usize),
            (CMD_PING,            8),
            (CMD_SET_LORA_PARAMS, 8),
            (CMD_ADDR_CONFLICT,   8),
            (CMD_SET_NODE_ADDRS,  8),
            (CMD_SET_GATEWAY,     9),
            (CMD_WIFI_TOGGLE,     9),
            (CMD_BLE_TOGGLE,      9),
            (CMD_GET_SETTINGS,    13),
            (CMD_GET_BATTERY,     10),
            (CMD_GET_MAC,         14),
        ];
        for (cmd, expected) in fixed {
            assert_eq!(
                exact_packet_len(cmd),
                Some(expected),
                "CMD 0x{:02X} should have fixed length {}", cmd, expected
            );
        }

        // Variable-length commands must return None
        let variable = [
            CMD_MESSAGE, CMD_BROADCAST, CMD_MULTIPART,
            CMD_DOGE_TX, CMD_REQUEST_BALANCE, CMD_GET_FIRMWARE_VERSION,
        ];
        for cmd in variable {
            assert_eq!(
                exact_packet_len(cmd),
                None,
                "CMD 0x{:02X} should be variable-length (None)", cmd
            );
        }
    }

    /// Verify that the null-terminator scan logic used for CMD_GET_FIRMWARE_VERSION
    /// correctly stops at the '\0' and does not consume bytes from a subsequent packet.
    #[test]
    fn test_firmware_version_null_terminator_scan() {
        let src = test_src();
        let ver_string = b"v1.2.3\0";

        // Manually build a CMD_GET_FIRMWARE_VERSION response followed by CMD_GET_SETTINGS
        let mut combined = vec![CMD_GET_FIRMWARE_VERSION, 0x00];
        combined.extend_from_slice(&[src.region, src.community, src.node]);
        combined.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // broadcast dst
        combined.extend_from_slice(ver_string);
        // Append a CMD_GET_SETTINGS response immediately after
        combined.extend_from_slice(&build_get_settings(&src));

        // Simulate the null-terminator scan from the framing loop
        let after_hdr = &combined[SINGLE_HDR_LEN..];
        let payload_len = after_hdr
            .iter()
            .position(|&b| b == 0)
            .map(|i| i + 1)
            .unwrap_or_else(|| after_hdr.len().min(MAX_SINGLE_PAYLOAD_LEN));
        let packet_len = (SINGLE_HDR_LEN + payload_len).min(combined.len());

        let parsed = parse_incoming(&combined[..packet_len], 0)
            .expect("Should parse firmware version packet");

        assert_eq!(parsed.command, CMD_GET_FIRMWARE_VERSION);
        // Payload must be only "v1.2.3\0", not "v1.2.3\0" + GET_SETTINGS bytes
        let raw_payload = hex::decode(&parsed.payload_hex).expect("valid hex");
        assert_eq!(
            raw_payload,
            ver_string,
            "Firmware version payload must end at the null terminator"
        );
    }
}

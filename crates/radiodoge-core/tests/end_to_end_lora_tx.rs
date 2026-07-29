//! End-to-end test of the path a Dogecoin transaction actually takes:
//!
//! ```text
//!   offline host  --serial-->  sender board  --LoRa-->  gateway board  --serial-->  daemon
//! ```
//!
//! The two boards are modelled in `board.rs`-free Rust here rather than run on
//! hardware, but the model is not a convenience: it implements the *same*
//! framing rules as `heltec-firmware-v3/heltec-firmware.ino`, byte for byte, so
//! a change on either side that breaks the agreement fails this test. What it
//! proves is the property the firmware fix exists for — that a signed
//! transaction survives being cut into frames, put on a serial byte stream with
//! no packet boundaries, transmitted, and put on another byte stream at the
//! other end.
//!
//! The streams are deliberately concatenated with no gaps anywhere. That is the
//! worst case for a byte-stream framer and precisely the case that used to fail:
//! frames were merged, and the merged blob was neither a valid packet nor
//! recoverable.

use radiodoge_core::radio::{
    self, frame_packet_len, ingest_packet, resync_offset, MultipartReassembler, CMD_DOGE_TX,
    MULTIPART_EXTRA_LEN, MULTIPART_CHUNK_LEN, MULTIPART_HDR_LEN, SINGLE_HDR_LEN,
};
use radiodoge_core::types::NodeAddress;

/// Firmware build that frames host→board multipart correctly.
const FW_NEW: &str = "RadioDoge NV3FW11";

// ─── The sender board ────────────────────────────────────────────────────────

/// One frame as the board read it off the serial line.
struct BoardFrame {
    cmd: u8,
    flags: u8,
    hdr_rest: [u8; 6],
    extra: Vec<u8>,
}

/// Model of `HostSerialRead` + `HandleDesktopCommand` in the firmware.
///
/// Reads frames out of `stream` using the firmware's rules and returns the
/// packets it would put on the air. `Err` marks a frame the firmware would NACK.
fn board_read_and_transmit(stream: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut pos = 0usize;
    let mut on_air = Vec::new();

    while pos < stream.len() {
        // ReadSerialHeader: [cmd, flags]
        if stream.len() - pos < 2 {
            return Err(format!("truncated header at offset {}", pos));
        }
        let cmd = stream[pos];
        let flags = stream[pos + 1];
        pos += 2;

        if !is_desktop_command_byte(cmd) {
            return Err(format!("cmd 0x{:02X} is not a desktop command", cmd));
        }

        // The 6 remaining header bytes.
        if stream.len() - pos < 6 {
            return Err("truncated desktop header".to_string());
        }
        let mut hdr_rest = [0u8; 6];
        hdr_rest.copy_from_slice(&stream[pos..pos + 6]);
        pos += 6;

        let extra: Vec<u8> = if (flags & 0x0F) == radio::FLAG_MULTIPART {
            // Five bytes ending in the chunk length, then exactly that many.
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
            // Variable length with no in-band size: the firmware reads until the
            // host goes quiet, which for a host that sends one frame and waits
            // for the acknowledgement is the rest of what it wrote.
            let e = stream[pos..].to_vec();
            pos = stream.len();
            e
        };

        on_air.push(BoardFrame { cmd, flags, hdr_rest, extra }.to_air());
    }

    Ok(on_air)
}

impl BoardFrame {
    /// What `HandleDesktopCommand` puts on the air for 0x10 / 0x11: the frame
    /// verbatim, flags preserved.
    fn to_air(&self) -> Vec<u8> {
        let mut ota = vec![self.cmd, self.flags];
        ota.extend_from_slice(&self.hdr_rest);
        ota.extend_from_slice(&self.extra);
        ota
    }
}

/// Mirrors `isDesktopCommandByte` in the firmware.
fn is_desktop_command_byte(b: u8) -> bool {
    matches!(b, 0x10 | 0x11 | 0x20 | 0x21 | 0x22 | 0x23 | 0x24 | 0x26 | 0x27 | 0x28)
}

/// Mirrors `desktopCommandLength` in the firmware: total packet size, or `None`
/// when the command has no fixed size.
fn desktop_command_length(b: u8) -> Option<usize> {
    match b {
        0x20 | 0x22 | 0x26 | 0x27 => Some(8),
        0x23 | 0x24 | 0x28 => Some(9),
        0x21 => Some(16),
        _ => None,
    }
}

// ─── The receiving host ──────────────────────────────────────────────────────

/// The framing loop shared by `serial.rs` and the Android bridge, run over a
/// byte stream exactly as the gateway's daemon sees it.
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
            None => break, // still arriving
        };
        if let Some(pkt) = ingest_packet(&acc[..len], -70, &mut rx, 0) {
            out.push(pkt);
        }
        acc.drain(..len);
    }
    out
}

/// A transaction-shaped payload of `len` bytes that `is_signed_tx_payload`
/// accepts, with a recognisable pattern so any byte reordering is visible.
fn tx_like(len: usize) -> Vec<u8> {
    let mut v = vec![0x01, 0x00, 0x00, 0x00];
    v.extend((0..len - 4).map(|i| ((i * 7 + 13) % 251) as u8));
    v
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// The whole path, for transaction sizes spanning one packet up to the ceiling.
///
/// Both serial streams are handed over with no gaps, which is the case the
/// framing has to survive.
#[test]
fn signed_transaction_survives_host_to_board_to_air_to_gateway() {
    let sender = NodeAddress::new(10, 0, 1);
    let broadcast = NodeAddress::broadcast();

    // 192 is the single-packet limit; 193 is the smallest multipart payload;
    // 226 is the smallest realistic signed transaction with a change output;
    // 374 lands exactly on a part boundary; the rest span up to the ceiling.
    let sizes = [
        100,
        192,
        193,
        226,
        374,
        375,
        1000,
        radio::MAX_MULTIPART_PAYLOAD_LEN,
    ];

    for size in sizes {
        let tx = tx_like(size);
        assert!(
            radiodoge_core::wallet::is_signed_tx_payload(&tx),
            "{}-byte payload should look like a signed transaction",
            size
        );

        let frames =
            radio::build_tx_frames(&sender, &broadcast, CMD_DOGE_TX, &tx, Some(FW_NEW))
                .unwrap_or_else(|e| panic!("{}-byte transaction should encode: {}", size, e));

        // Host → board: every frame written back to back, no gaps.
        let host_stream: Vec<u8> = frames.concat();
        let on_air = board_read_and_transmit(&host_stream)
            .unwrap_or_else(|e| panic!("board should frame a {}-byte transaction: {}", size, e));
        assert_eq!(
            on_air.len(),
            frames.len(),
            "{}-byte transaction: every frame must reach the air as its own packet",
            size
        );

        // Board → air → board is packet-preserving; the gateway board writes each
        // received packet to its host, again with no gaps between them.
        let gateway_stream: Vec<u8> = on_air.concat();
        let packets = host_read_stream(&gateway_stream);

        assert_eq!(
            packets.len(),
            1,
            "{}-byte transaction must arrive as exactly one reassembled packet",
            size
        );
        assert_eq!(packets[0].command, CMD_DOGE_TX);
        assert_eq!(packets[0].source, sender);
        let recovered = hex::decode(&packets[0].payload_hex).expect("valid hex");
        assert_eq!(
            recovered, tx,
            "{}-byte transaction must arrive byte-for-byte identical",
            size
        );
        assert!(
            radiodoge_core::wallet::is_signed_tx_payload(&recovered),
            "{}-byte transaction: the gateway must still recognise it as signed",
            size
        );
    }
}

/// The gateway's reply travels the same way in reverse, and the board's legacy
/// 3-byte acknowledgement sits in the middle of that stream.
#[test]
fn tx_ack_reaches_the_sender_through_the_same_framing() {
    let gateway = NodeAddress::new(10, 0, 9);
    let sender = NodeAddress::new(10, 0, 1);
    let txid = "9f2c1e0a4b5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f7";

    let ack = radio::build_message(&gateway, &sender, &format!("TX_ACK:{}", txid));
    assert!(ack.len() <= SINGLE_HDR_LEN + radio::MAX_SINGLE_PAYLOAD_LEN);

    // What the sender's board actually writes: its own legacy result code for
    // the relay it performed, then the acknowledgement heard over the air.
    let mut stream = vec![radio::LEGACY_RESULT_CODE, 0x01, 0x06];
    stream.extend_from_slice(&ack);

    let packets = host_read_stream(&stream);
    assert_eq!(packets.len(), 1, "the result code is consumed, the ACK survives");
    assert_eq!(
        packets[0].decoded.as_deref(),
        Some(format!("✅ TX confirmed: txid={}", txid).as_str()),
        "the sender must recover the full txid"
    );
}

/// Two senders reaching the same gateway at once must not have their frames
/// mixed into one another's transaction.
#[test]
fn concurrent_transactions_from_two_senders_stay_separate() {
    let a_src = NodeAddress::new(10, 0, 1);
    let b_src = NodeAddress::new(10, 0, 2);
    let broadcast = NodeAddress::broadcast();

    let a_tx = tx_like(500);
    let b_tx = tx_like(700);

    let a = radio::build_tx_frames(&a_src, &broadcast, CMD_DOGE_TX, &a_tx, Some(FW_NEW)).unwrap();
    let b = radio::build_tx_frames(&b_src, &broadcast, CMD_DOGE_TX, &b_tx, Some(FW_NEW)).unwrap();

    // Interleave the two sequences on the gateway's serial stream, which is what
    // two boards transmitting in turn produces.
    let mut stream = Vec::new();
    for i in 0..a.len().max(b.len()) {
        if let Some(f) = a.get(i) {
            stream.extend_from_slice(f);
        }
        if let Some(f) = b.get(i) {
            stream.extend_from_slice(f);
        }
    }

    let packets = host_read_stream(&stream);
    assert_eq!(packets.len(), 2, "both transactions must complete");

    let recovered_a = packets
        .iter()
        .find(|p| p.source == a_src)
        .map(|p| hex::decode(&p.payload_hex).unwrap())
        .expect("sender A's transaction");
    let recovered_b = packets
        .iter()
        .find(|p| p.source == b_src)
        .map(|p| hex::decode(&p.payload_hex).unwrap())
        .expect("sender B's transaction");

    assert_eq!(recovered_a, a_tx);
    assert_eq!(recovered_b, b_tx);
}

/// A frame lost in the air must leave the gateway with nothing rather than a
/// transaction with a hole in it — a partially reassembled signed transaction
/// would be rejected by the network, but silently producing one would be worse.
#[test]
fn a_dropped_frame_never_produces_a_partial_transaction() {
    let sender = NodeAddress::new(10, 0, 1);
    let broadcast = NodeAddress::broadcast();
    let tx = tx_like(600);
    let frames =
        radio::build_tx_frames(&sender, &broadcast, CMD_DOGE_TX, &tx, Some(FW_NEW)).unwrap();
    assert!(frames.len() >= 4);

    for dropped in 0..frames.len() {
        let stream: Vec<u8> = frames
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != dropped)
            .flat_map(|(_, f)| f.clone())
            .collect();
        assert!(
            host_read_stream(&stream).is_empty(),
            "dropping part {} must yield no packet at all",
            dropped
        );
    }

    // And a retransmission of the missing part completes it, which is what the
    // host's retry does.
    let mut stream: Vec<u8> = frames.iter().skip(1).flat_map(|f| f.clone()).collect();
    stream.extend_from_slice(&frames[0]);
    let packets = host_read_stream(&stream);
    assert_eq!(packets.len(), 1);
    assert_eq!(hex::decode(&packets[0].payload_hex).unwrap(), tx);
}

/// A board running firmware older than FW11 must never be handed a multipart
/// sequence: it has no chunk length to frame by and would mis-read it.
#[test]
fn old_firmware_is_refused_rather_than_mis_framed() {
    let sender = NodeAddress::new(10, 0, 1);
    let broadcast = NodeAddress::broadcast();
    let tx = tx_like(400);

    for fw in [None, Some("RadioDoge NV3FW10"), Some("RadioDoge NV3FW09")] {
        let err = radio::build_tx_frames(&sender, &broadcast, CMD_DOGE_TX, &tx, fw)
            .expect_err("a board that cannot reassemble this must be refused");
        assert!(
            err.contains("v0.4.2"),
            "the refusal should say how to fix it: {}",
            err
        );
    }

    // The same board still accepts a transaction that fits in one packet.
    let small = tx_like(150);
    let frames =
        radio::build_tx_frames(&sender, &broadcast, CMD_DOGE_TX, &small, Some("RadioDoge NV3FW10"))
            .expect("a single-packet transaction is still fine on old firmware");
    assert_eq!(frames.len(), 1);
    let packets = host_read_stream(&board_read_and_transmit(&frames.concat()).unwrap().concat());
    assert_eq!(hex::decode(&packets[0].payload_hex).unwrap(), small);
}

/// Every multipart frame the host builds must fit in one LoRa transmission.
///
/// `Radio.Send` takes a `uint8_t` length, and the firmware's packet buffer is
/// 256 bytes; a frame larger than either would be truncated on the air with no
/// error, which is the quietest possible way to lose a transaction.
#[test]
fn every_frame_fits_the_radio_and_the_board_buffer() {
    const BOARD_BUFFER_SIZE: usize = 256; // BUFFER_SIZE in the firmware
    let sender = NodeAddress::new(10, 0, 1);
    let broadcast = NodeAddress::broadcast();

    for size in [193usize, 500, 2000, radio::MAX_MULTIPART_PAYLOAD_LEN] {
        let frames =
            radio::build_tx_frames(&sender, &broadcast, CMD_DOGE_TX, &tx_like(size), Some(FW_NEW))
                .unwrap();
        for (i, f) in frames.iter().enumerate() {
            assert!(
                f.len() <= 255,
                "frame {} of a {}-byte transaction is {} bytes — past the radio's length field",
                i, size, f.len()
            );
            assert!(
                f.len() <= BOARD_BUFFER_SIZE,
                "frame {} of a {}-byte transaction is {} bytes — past the board's buffer",
                i, size, f.len()
            );
            assert_eq!(
                f.len(),
                MULTIPART_HDR_LEN + f[MULTIPART_HDR_LEN - 1] as usize,
                "frame {} must be exactly as long as it declares",
                i
            );
        }
    }
}

/// A node in radio range must not be able to raise the host's payload ceiling.
///
/// The board forwards any over-the-air packet addressed to the broadcast address
/// to its serial host verbatim, so an attacker can put a `CMD_GET_FIRMWARE_VERSION`
/// packet on the air claiming any version string. If the host believed it, an
/// older board would be handed a multipart sequence it mis-frames, and the
/// transaction would be lost.
///
/// The host defends by only believing a version reply inside the window it opens
/// when it asks (`SerialManager::request_firmware_version`). This test pins the
/// consequence that makes that matter: the *decision* the version drives.
#[test]
fn a_spoofed_firmware_version_would_lift_the_payload_ceiling() {
    let (src, dst) = (NodeAddress::new(10, 0, 1), NodeAddress::broadcast());
    let tx = tx_like(400);

    // What an attacker would want the host to believe.
    let spoofed = "RadioDoge NV3FW11";
    assert!(radio::firmware_supports_multipart(Some(spoofed)));
    assert!(radio::build_tx_frames(&src, &dst, CMD_DOGE_TX, &tx, Some(spoofed)).is_ok());

    // What the board actually is. The gate must hold for anything below FW11,
    // and for a board that never answered at all.
    for real in [None, Some("RadioDoge NV3FW10"), Some("RadioDoge NV2FW01")] {
        assert!(
            radio::build_tx_frames(&src, &dst, CMD_DOGE_TX, &tx, real).is_err(),
            "a board reporting {:?} must never be handed a multipart sequence",
            real
        );
    }

    // The spoof packet is a well-formed, forwardable broadcast — this is not a
    // malformed frame the framer would reject, which is exactly why the host has
    // to correlate replies with requests rather than filter on shape.
    let mut spoof = vec![radio::CMD_GET_FIRMWARE_VERSION, 0x00, 9, 9, 9, 0xFF, 0xFF, 0xFF];
    spoof.extend_from_slice(spoofed.as_bytes());
    spoof.push(0);
    let seen = host_read_stream(&spoof);
    assert_eq!(seen.len(), 1, "it frames cleanly — the defence cannot be in the framer");
    assert_eq!(seen[0].command, radio::CMD_GET_FIRMWARE_VERSION);
}

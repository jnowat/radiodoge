//! Android bridge: USB-serial byte pump and BLE commands driven from the JS side.

use super::*;

// ─── v0.3.10: Android USB Serial Bridge ──────────────────────────────────────
//
// On Android, the `serialport` crate cannot open USB-OTG devices — that
// requires Android's USB Host API via Java. `tauri-plugin-serialplugin` wraps
// `usb-serial-for-android` and exposes a JavaScript API the frontend can call.
//
// The mobile bridge pattern:
//   1. JS lists USB devices  → SerialPort.available_ports()
//   2. JS opens the port     → SerialPort.open()  [triggers Android permission dialog]
//   3. JS writes bytes       → SerialPort.write()
//   4. JS receives bytes     → listener callback → calls mobile_push_bytes()
//   5. Rust frames packets   → same logic as desktop read-loop
//   6. Rust emits events     → "radio-packet", "board-sync", "connection-status", …
//
// All packet-building uses the existing radio:: functions so there is zero
// protocol duplication between platforms.

/// Notify the Rust backend that an Android USB serial connection is opening.
/// Sets `current_port` and emits "connection-status: connecting" so the
/// frontend (which listens to the same event as on desktop) updates its state.
#[tauri::command]
pub(crate) async fn mobile_set_connected(
    device_path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = Some(device_path.clone());
    *state.connection_type.lock().await = "usb-android".to_string();
    // Clear any stale firmware version from a previous session
    *state.mobile_fw_version.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Signal readings from a previous session no longer describe this link.
    *state.mobile_last_rssi.lock().await = 0;
    *state.mobile_last_snr.lock().await = None;
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    // FW14: a pending RX_SIGNAL belongs to the old session's next frame.
    state.mobile_signal_latch.lock().await.clear();
    let _ = app.emit(
        "connection-status",
        ConnectionStatusEvent::connecting(&device_path),
    );
    log::info!("mobile_set_connected: {}", device_path);
    Ok(())
}

/// Notify the Rust backend that the Android USB connection has been closed.
/// Clears state and emits "connection-status: disconnected".
#[tauri::command]
pub(crate) async fn mobile_set_disconnected(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = None;
    *state.mobile_fw_version.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    // FW14: a pending RX_SIGNAL belongs to the old session's next frame.
    state.mobile_signal_latch.lock().await.clear();
    *state.mobile_packets_rx.lock().await = 0;
    *state.mobile_packets_tx.lock().await = 0;
    *state.mobile_last_rssi.lock().await = 0;
    *state.mobile_last_snr.lock().await = None;
    let _ = app.emit("connection-status", ConnectionStatusEvent::disconnected());
    log::info!("mobile_set_disconnected");
    Ok(())
}

/// Discard all buffered bytes in the Android accumulator (called on disconnect).
#[tauri::command]
pub(crate) async fn mobile_clear_accumulator(state: State<'_, AppState>) -> Result<(), String> {
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    // FW14: a pending RX_SIGNAL belongs to the old session's next frame.
    state.mobile_signal_latch.lock().await.clear();
    Ok(())
}

/// Feed raw bytes received from the Android USB serial plugin into the packet
/// accumulator. Extracts complete RadioDoge packets using the same framing
/// logic as the desktop serial read-loop:
///
/// - Unknown command bytes at the front are discarded (sync recovery)
/// - Fixed-length commands advance by `exact_packet_len()` bytes
/// - Variable-length commands (messages, FW version) consume up to MAX_SINGLE_PAYLOAD_LEN
///
/// For each complete packet this function:
/// - Emits "radio-packet" to the frontend
/// - On CMD_GET_FIRMWARE_VERSION: caches the version string
/// - On CMD_GET_SETTINGS: emits "board-sync" + "connection-status: connected"
/// - On CMD_DOGE_TX: emits "mobile-doge-tx-received" notification event
/// - On CMD_ADDR_CONFLICT: emits "addr-conflict"
///
/// Returns the number of complete packets extracted (useful for debug logging).
#[tauri::command]
pub(crate) async fn mobile_push_bytes(
    bytes: Vec<u8>,
    rssi: i16,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<u32, String> {
    // Same set of known first-byte values as the desktop read-loop.
    // Keep this in lock-step with radio::is_known_command — a byte missing here
    // is discarded as noise, which then shifts every following packet by one.

    // Guard against unbounded accumulator growth from a misbehaving or spamming board.
    // In normal operation the packet-drain loop below keeps the accumulator small
    // (a few hundred bytes at most).  If the board enters a debug-print loop or sends
    // malformed data that never forms complete packets, the accumulator would otherwise
    // grow until the process is OOM-killed (especially risky on Android).
    pub(crate) const MOBILE_ACCUMULATOR_CAP: usize = 8 * 1024;
    let mut acc = state.mobile_accumulator.lock().await;
    if acc.len() + bytes.len() > MOBILE_ACCUMULATOR_CAP {
        eprintln!(
            "[mobile_push_bytes] accumulator overflow ({} bytes) — discarding stale data",
            acc.len()
        );
        acc.clear();
    }
    acc.extend_from_slice(&bytes);

    let mut packets_extracted: u32 = 0;

    loop {
        // Discard leading noise (sync recovery), mirroring the desktop loop.
        // One drain rather than a byte at a time — see radio::resync_offset.
        let skip = radio::resync_offset(&acc);
        if skip > 0 {
            acc.drain(..skip);
        }
        if acc.len() < radio::SINGLE_HDR_LEN {
            break; // Need more bytes
        }

        // Peek the leading byte so we can determine the packet length BEFORE
        // parsing.  This ensures the parser receives exactly the bytes belonging
        // to one packet, giving a clean payload_hex and preventing the next
        // packet from being swallowed as spurious payload when two arrive in a
        // single USB read callback.  (This is the frame's byte; the command used
        // for dispatch below comes from the parsed packet, which for a multipart
        // sequence is the reassembled payload's command.)
        let frame_cmd = acc[0];

        // Same framing rule as the desktop serial read loop (radiodoge-core).
        // `None` means the packet is still arriving — keep the bytes buffered
        // and wait for the next USB/BLE chunk rather than emitting a short packet.
        let packet_len = match radio::frame_packet_len(frame_cmd, &acc) {
            Some(n) => n,
            None => break,
        };

        // Parse exactly the bytes for this packet so payload_hex is clean, and
        // let multipart sequences reassemble before anything downstream sees them.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let ingested = {
            let mut reasm = state.mobile_reassembler.lock().await;
            let mut latch = state.mobile_signal_latch.lock().await;
            radio::ingest_with_signal(&acc[..packet_len], rssi, &mut latch, &mut reasm, now_ms)
        };
        // Consume the frame either way — a multipart fragment has been absorbed
        // even though it does not yield a packet yet.
        acc.drain(..packet_len);

        let packet = match ingested {
            Some(p) => p,
            None => continue, // fragment stored; wait for the rest of the sequence
        };
        // A reassembled payload carries the sequence's command, which may differ
        // from the raw frame's first byte only in malformed input — use the
        // packet's own command for dispatch below.
        let cmd = packet.command;

        packets_extracted += 1;

        // FW14: the board reported this packet's own RSSI/SNR - keep it as the
        // latest reading for the Dashboard (USB serial has no other source).
        if let Some(snr) = packet.snr {
            *state.mobile_last_rssi.lock().await = packet.rssi;
            *state.mobile_last_snr.lock().await = Some(snr);
        }

        // v0.3.20 — Mobile neighbor list (incl. shared POS: positions), same rules as desktop.
        state.serial.record_neighbor(&packet).await;

        // Emit to UI (same event as desktop)
        let _ = app.emit("radio-packet", &packet);
        emit_debug_traffic(
            &app,
            "RX",
            &packet.payload_hex,
            packet.decoded.as_deref().unwrap_or(""),
        );

        // Handle specific packet types
        match cmd {
            // Only inside the window opened by mobile_build_connect_queries — a
            // relayed over-the-air packet must not be able to set this.
            radio::CMD_GET_FIRMWARE_VERSION
                if now_millis() < state.mobile_fw_query_until_ms.load(Ordering::Relaxed) =>
            {
                if let Ok(raw) = hex::decode(&packet.payload_hex) {
                    if let Ok(ver) = String::from_utf8(raw) {
                        let ver = ver.trim_matches('\0').trim().to_string();
                        if !ver.is_empty() {
                            log::info!("Mobile: firmware version = {}", ver);
                            *state.mobile_fw_version.lock().await = Some(ver.clone());
                            // Emit so the frontend can display the badge without waiting
                            let _ = app.emit("mobile-firmware-version", &ver);
                        }
                    }
                }
            }

            radio::CMD_GET_SETTINGS => {
                // Payload: [region, community, node, gateway_mode, wifi_enabled(v0.3.7)]
                if let Ok(payload) = hex::decode(&packet.payload_hex) {
                    if payload.len() >= 4 {
                        let addr = NodeAddress::new(payload[0], payload[1], payload[2]);
                        let gw = payload[3] != 0;
                        let wifi = payload.get(4).map(|&b| b != 0).unwrap_or(true);

                        // Only trust the addr if it is not the broadcast address
                        if addr != NodeAddress::broadcast() {
                            state.serial.update_node_address(addr.clone()).await;
                        }

                        let bs = BoardSettings {
                            node_address: addr.clone(),
                            gateway_mode: gw,
                            wifi_enabled: wifi,
                        };

                        // Cache board settings in SerialManager so `get_board_settings()`
                        // returns the right value on Android (without the desktop read loop).
                        state.serial.update_board_settings(bs.clone()).await;

                        let _ = app.emit("board-sync", &bs);

                        // Now emit "connected" — we have everything we need.
                        // Include the firmware version if it arrived before settings.
                        let fw = state.mobile_fw_version.lock().await.clone();
                        let port = state
                            .current_port
                            .lock()
                            .await
                            .clone()
                            .unwrap_or_else(|| "usb-android".to_string());
                        let _ = app.emit(
                            "connection-status",
                            ConnectionStatusEvent::connected(&port, addr, fw),
                        );
                        log::info!("Mobile: board sync complete, emitting connected");
                    }
                }
            }

            // Skip the board's own bare acknowledgement of a send — see
            // is_incoming_doge_tx.
            radio::CMD_DOGE_TX if is_incoming_doge_tx(&packet) => {
                // No system-tray notification on Android; emit an in-app event instead
                let body = packet
                    .decoded
                    .clone()
                    .unwrap_or_else(|| "Incoming Dogecoin transaction over LoRa".to_string());
                let _ = app.emit(
                    "mobile-doge-tx-received",
                    serde_json::json!({ "body": body }),
                );
            }

            radio::CMD_ADDR_CONFLICT => {
                let addr_str = packet
                    .decoded
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                let _ = app.emit(
                    "addr-conflict",
                    serde_json::json!({
                        "detected": true,
                        "address": addr_str,
                    }),
                );
            }

            radio::CMD_GET_MAC => {
                // Cache the board MAC so query_mac() works on Android (same as desktop
                // serial.rs read loop).  Emit "mobile-board-mac" so SettingsTab can
                // update the UI without polling.
                if let Ok(payload) = hex::decode(&packet.payload_hex) {
                    if payload.len() >= 6 {
                        let mac = format!(
                            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                            payload[0], payload[1], payload[2], payload[3], payload[4], payload[5]
                        );
                        state.serial.update_board_mac(mac.clone()).await;
                        let _ = app.emit("mobile-board-mac", &mac);
                    }
                }
            }

            _ => {}
        }
    }

    // v0.3.15 — Emit radio-stats-update so the Dashboard packet counters and
    // NavBar signal bars stay current on mobile (USB/BLE).  On USB the RSSI
    // is always 0 (serial carries no signal-strength metadata), but the
    // packets_received counter will now increment correctly.
    if packets_extracted > 0 {
        let mut rx_count = state.mobile_packets_rx.lock().await;
        *rx_count += packets_extracted;
        let count_snap = *rx_count;
        drop(rx_count);

        // Accept a non-zero RSSI from the caller (future BLE path may supply it).
        if rssi != 0 {
            *state.mobile_last_rssi.lock().await = rssi;
        }
        let rssi_snap = *state.mobile_last_rssi.lock().await;

        let tx_snap = *state.mobile_packets_tx.lock().await;
        let lora = state.lora_settings.lock().await.clone();
        let stats = RadioStats {
            frequency_mhz: lora.frequency_mhz,
            power_dbm: lora.power_dbm,
            spreading_factor: lora.spreading_factor,
            bandwidth_khz: lora.bandwidth_khz,
            coding_rate: lora.coding_rate.clone(),
            rssi: rssi_snap,
            // FW14+ boards report SNR per forwarded frame; older firmware leaves it None.
            snr: state.mobile_last_snr.lock().await.map(f32::from),
            packets_sent: tx_snap,
            packets_received: count_snap,
        };
        let _ = app.emit("radio-stats-update", &stats);
    }

    Ok(packets_extracted)
}

/// Build a raw PING packet for the Android USB bridge to write directly to serial.
/// Uses the current node address from AppState (synced from board on connect).
///
/// Mirrors the desktop `serial::ping()` which sends CMD_PING from `local` to `local`
/// (self-addressed).  The firmware only echoes back a CMD_PING response when the
/// destination address matches the board's own address — broadcast PINGs are silently
/// ignored by some firmware builds.
#[tauri::command]
pub(crate) async fn mobile_build_ping(state: State<'_, AppState>) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_ping(&src, &src)) // self-addressed, same as desktop serial::ping()
}

/// Build a SET_GATEWAY packet for the Android USB bridge.
/// The JS layer writes the bytes to USB/BLE; the board responds via CMD_GET_SETTINGS
/// board-sync which updates gateway_mode in the connection store.
#[tauri::command]
pub(crate) async fn mobile_build_set_gateway(
    enable: bool,
    state: State<'_, AppState>,
) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_set_gateway(&src, enable))
}

/// Build a WIFI_TOGGLE packet for the Android USB bridge.
#[tauri::command]
pub(crate) async fn mobile_build_wifi_toggle(
    enable: bool,
    state: State<'_, AppState>,
) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_wifi_toggle(&src, enable))
}

/// Build a BLE_TOGGLE packet for the Android USB bridge.
/// Enables or disables BLE advertising on the board (persisted to NVS).
/// Requires firmware v0.3.16+.
#[tauri::command]
pub(crate) async fn mobile_build_ble_toggle(
    enable: bool,
    state: State<'_, AppState>,
) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_ble_toggle(&src, enable))
}

/// Build a GET_MAC packet for the Android USB bridge.
/// After writing, the board sends CMD_GET_MAC (0x27) response;
/// mobile_push_bytes processes it and emits "mobile-board-mac".
#[tauri::command]
pub(crate) async fn mobile_build_get_mac(state: State<'_, AppState>) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_get_mac(&src))
}

/// v0.3.16 — Enable/disable BLE advertising on the board (desktop path).
/// Mirrors set_wifi_enabled but uses CMD_BLE_TOGGLE (0x28).
/// Requires board firmware v0.3.16+.
#[tauri::command]
pub(crate) async fn set_ble_enabled(
    enable: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<bool, String> {
    if !state.serial.is_connected() {
        return Err("Not connected to board.".to_string());
    }
    let src = state.serial.get_node_address().await;
    let pkt = radio::build_ble_toggle(&src, enable);
    let pkt_hex = hex::encode(&pkt);
    emit_debug_traffic(
        &app,
        "TX",
        &pkt_hex,
        &format!(
            "CMD_BLE_TOGGLE (0x28) → {}",
            if enable { "ON" } else { "OFF" }
        ),
    );
    // The board answers 0x28 with a 9-byte echo carrying the state it applied.
    // This used to return `enable` without looking, so the UI reported a toggle
    // the board may never have received.
    let acked = state
        .serial
        .send_and_await_reply(pkt, radio::CMD_BLE_TOGGLE, 2000)
        .await
        .map_err(radiodoge_core::errors::friendly)?;
    if !acked {
        return Err(
            "The board did not acknowledge the Bluetooth change, so it may not have been              applied. Nothing was changed in the app."
                .to_string(),
        );
    }
    Ok(enable)
}

/// Build the two-packet connect sequence for the Android USB bridge:
///   [0] GET_FIRMWARE_VERSION (0x20)
///   [1] GET_SETTINGS         (0x22)
///
/// The JS layer sends these in order after opening the serial port.
/// When the board replies, `mobile_push_bytes` processes the responses and
/// emits "mobile-firmware-version" and "connection-status: connected".
#[tauri::command]
pub(crate) async fn mobile_build_connect_queries(
    state: State<'_, AppState>,
) -> Result<Vec<Vec<u8>>, String> {
    let src = state.serial.get_node_address().await;
    // Opening the window here rather than when the bytes are written: the JS
    // layer writes these immediately, and a few seconds covers BLE's slower
    // round trip. Outside it, a "version reply" is somebody else's LoRa packet.
    state.mobile_fw_query_until_ms.store(
        now_millis() + MOBILE_FIRMWARE_REPLY_WINDOW_MS,
        Ordering::Relaxed,
    );
    Ok(vec![
        radio::build_get_firmware_version(&src),
        radio::build_get_settings(&src),
    ])
}

/// Build Dogecoin transaction packet(s) for the Android USB bridge.
///
/// Returns a `Vec<Vec<u8>>` — each inner Vec is one complete packet to write
/// in order. Payloads ≤ 192 bytes produce a single packet; larger payloads
/// are split into a multipart sequence (identical to the desktop send path).
///
/// The JS bridge must write these **one at a time with a gap between them** —
/// see `MULTIPART_FRAME_GAP_MS` in `connection-bridge.ts`. The board's serial
/// receive buffer holds only a few hundred bytes and it does not read while the
/// radio is transmitting, so a burst is lost silently.
#[tauri::command]
pub(crate) async fn mobile_build_tx_packets(
    tx: TransactionRequest,
    state: State<'_, AppState>,
) -> Result<Vec<Vec<u8>>, String> {
    let src = state.serial.get_node_address().await;
    let dst = NodeAddress::broadcast();

    let payload = if let Some(ref wif) = tx.from_private_key_wif {
        offline_utxos::sign_payload(
            &state.utxo_snapshot,
            wif,
            &tx.to_address,
            tx.amount_doge,
            resolve_fee(tx.fee_doge)?,
        )
        .await?
    } else {
        wallet::encode_transaction_payload(&tx.to_address, tx.amount_doge, tx.memo.as_deref())
            .map_err(radiodoge_core::errors::friendly)?
    };

    // Whether a multipart sequence can be handed to the board depends on its
    // firmware. On Android the version is cached by mobile_push_bytes when the
    // board answers CMD_GET_FIRMWARE_VERSION during the connect handshake.
    let fw = state.mobile_fw_version.lock().await.clone();
    radio::build_tx_frames(&src, &dst, radio::CMD_DOGE_TX, &payload, fw.as_deref())
}

/// Build a SET_LORA_PARAMS packet (CMD 0x21) for the Android USB bridge.
/// Mirrors the desktop `update_lora_settings` command but returns raw bytes
/// instead of writing them to the (non-existent on Android) Rust serial handle.
#[tauri::command]
pub(crate) async fn mobile_build_lora_settings_packet(settings: LoraSettings) -> Vec<u8> {
    let bw_idx: u8 = match settings.bandwidth_khz as u32 {
        250 => 1,
        500 => 2,
        _ => 0, // default 125 kHz
    };
    let cr: u8 = match settings.coding_rate.as_str() {
        "4/6" => 6,
        "4/7" => 7,
        "4/8" => 8,
        _ => 5, // default 4/5
    };
    let freq_khz = (settings.frequency_mhz * 1000.0).round() as u32;
    radio::build_set_lora_params(
        &settings.node_address,
        settings.spreading_factor,
        bw_idx,
        cr,
        freq_khz,
        settings.power_dbm.clamp(2, 22) as u8,
    )
}

// ─── v0.3.10 Android BLE commands ────────────────────────────────────────────
//
// Architecture:
//   The tauri-plugin-blec plugin provides the actual BLE transport via its own
//   IPC commands (scan, connect, sendData, onReceiveData).  The four commands
//   below handle the *RadioDoge-side state machine* that wraps each BLE operation:
//
//   • mobile_ble_scan      — update connection_type, emit ble-scan-started event
//   • mobile_ble_connect   — set ble_device_address, emit "connection-status: connecting"
//   • mobile_ble_disconnect — clear BLE state, emit "connection-status: disconnected"
//   • mobile_ble_write_characteristic — log outgoing bytes in the debug traffic stream
//
//   Incoming BLE data:
//     blec plugin → JS onReceiveData callback → invoke('mobile_push_bytes', …)
//     → same accumulator + parser path as USB-OTG → same "radio-packet" / "board-sync"
//     events consumed by the existing frontend code.
//   No protocol duplication between BLE and USB paths.

/// Notify Rust that a BLE device scan is starting.
/// Updates connection_type and emits "ble-scan-started" so the frontend can
/// animate a scan spinner independently of the mobileRefreshing state.
///
/// The actual scan results come from the blec plugin's JS `scan(timeoutMs)`
/// function; Rust receives them only if cached via future state extensions.
#[tauri::command]
pub(crate) async fn mobile_ble_scan(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.connection_type.lock().await = "ble-android".to_string();
    let _ = app.emit("ble-scan-started", serde_json::json!({}));
    log::info!("mobile_ble_scan: BLE scan initiated");
    Ok(())
}

/// Notify Rust that a BLE connection to `address` is being opened.
///
/// - Caches the device MAC address in `ble_device_address`
/// - Sets `connection_type` = "ble-android"
/// - Clears the mobile accumulator and cached firmware version
/// - Emits "connection-status: connecting" (identical event shape to USB/desktop)
///
/// The JS layer then calls the blec plugin's `connect(address)` IPC,
/// subscribes to characteristic notifications (→ mobile_push_bytes on each chunk),
/// and sends the GET_FIRMWARE_VERSION + GET_SETTINGS connect queries.
#[tauri::command]
pub(crate) async fn mobile_ble_connect(
    address: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = Some(address.clone());
    *state.connection_type.lock().await = "ble-android".to_string();
    *state.ble_device_address.lock().await = Some(address.clone());
    *state.mobile_fw_version.lock().await = None;
    *state.mobile_last_rssi.lock().await = 0;
    *state.mobile_last_snr.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    // FW14: a pending RX_SIGNAL belongs to the old session's next frame.
    state.mobile_signal_latch.lock().await.clear();
    let _ = app.emit(
        "connection-status",
        ConnectionStatusEvent::connecting(&address),
    );
    log::info!("mobile_ble_connect: {}", address);
    Ok(())
}

/// Notify Rust that the BLE connection has been closed.
///
/// Mirrors mobile_set_disconnected but for the BLE transport:
/// clears ble_device_address, current_port, mobile_fw_version, accumulator,
/// and emits "connection-status: disconnected".
#[tauri::command]
pub(crate) async fn mobile_ble_disconnect(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = None;
    *state.ble_device_address.lock().await = None;
    *state.mobile_fw_version.lock().await = None;
    *state.mobile_last_rssi.lock().await = 0;
    *state.mobile_last_snr.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    // FW14: a pending RX_SIGNAL belongs to the old session's next frame.
    state.mobile_signal_latch.lock().await.clear();
    let _ = app.emit("connection-status", ConnectionStatusEvent::disconnected());
    log::info!("mobile_ble_disconnect");
    Ok(())
}

/// Log bytes written to the USB serial port in the debug traffic stream.
///
/// The Android USB mobile path performs physical writes in JS via the
/// serialplugin's `writeBinary()` API.  This command exists to:
///   1. Emit a "TX-USB" debug-serial-traffic event visible in the Debug Console
///   2. Mirror the equivalent `mobile_ble_write_characteristic` for the BLE path
#[tauri::command]
pub(crate) async fn mobile_emit_debug_tx(
    bytes: Vec<u8>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let hex = hex::encode(&bytes);
    let note = bytes
        .first()
        .map(|cmd| match *cmd {
            0x02 => "CMD_PING",
            0x10 => "CMD_DOGE_TX",
            0x20 => "CMD_GET_FIRMWARE_VERSION",
            0x22 => "CMD_GET_SETTINGS",
            0x23 => "CMD_SET_GATEWAY",
            0x24 => "CMD_WIFI_TOGGLE",
            0x26 => "CMD_GET_BATTERY",
            0x27 => "CMD_GET_MAC",
            0x28 => "CMD_BLE_TOGGLE",
            _ => "CMD_?",
        })
        .unwrap_or("(empty)");
    emit_debug_traffic(&app, "TX-USB", &hex, note);
    *state.mobile_packets_tx.lock().await += 1;
    Ok(())
}

/// Log bytes being written to the BLE TX characteristic in the debug traffic stream.
///
/// The actual GATT write is performed by the JS layer via the blec plugin's
/// `sendData(serviceUuid, charUuid, data)` API.  This command exists to:
///   1. Emit a "TX-BLE" debug-serial-traffic event visible in the Debug Console
///   2. Provide a single place to add future Rust-side flow-control / rate-limiting
#[tauri::command]
pub(crate) async fn mobile_ble_write_characteristic(
    data: Vec<u8>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let hex = hex::encode(&data);
    emit_debug_traffic(&app, "TX-BLE", &hex, "BLE write characteristic");
    *state.mobile_packets_tx.lock().await += 1;
    Ok(())
}

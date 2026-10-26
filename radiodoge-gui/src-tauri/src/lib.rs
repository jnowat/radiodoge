//! RadioDoge GUI — Tauri 2 backend library.
//!
//! v0.3.10 additions:
//!   - Android USB serial via tauri-plugin-serialplugin (CP2102/CH340 over USB-OTG)
//!   - Android BLE via tauri-plugin-blec (btleplug on desktop, native BLE on Android)
//!   - Mobile packet bridge: JS feeds raw bytes → Rust frames packets → emits events
//!   - New commands: mobile_push_bytes, mobile_build_ping, mobile_build_connect_queries,
//!     mobile_build_tx_packets, mobile_build_lora_settings_packet,
//!     mobile_set_connected, mobile_set_disconnected, mobile_clear_accumulator,
//!     mobile_ble_scan, mobile_ble_connect, mobile_ble_disconnect,
//!     mobile_ble_write_characteristic
//!   - Platform detection via tauri-plugin-os
//!
//! v0.3.8: Battery voltage, board MAC, persistent wallet, address book.
//! v0.3.7: WiFi toggle, mesh neighbors, addr-conflict detection, ping fix.
//! v0.3.6: board-sync, WIF import, history, gateway mode, USB/BLE toggle.
//!
//! All protocol, wallet, and serial logic lives in `radiodoge-core`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;

#[cfg(desktop)]
mod tray;

use radiodoge_core::{radio, tx_reply, wallet};
use radiodoge_core::serial::SerialManager;
use radiodoge_core::types::{
    BoardSettings, ConnectionStatusEvent, IncomingPacket, LoraSettings, NeighborEntry,
    NodeAddress, PortInfo, RadioStats, SendOutcome, TransactionRequest, TxHistoryEntry, WalletInfo,
};

mod mobile;

/// Maximum transaction history entries kept in memory + persisted to disk.
const MAX_HISTORY: usize = 50;

/// Shared application state — injected into every Tauri command via `State<AppState>`.
pub struct AppState {
    pub serial: Arc<SerialManager>,
    pub current_port: Arc<Mutex<Option<String>>>,
    pub lora_settings: Arc<Mutex<LoraSettings>>,
    /// Set to false when the user explicitly disconnects — stops the reconnect watchdog.
    pub reconnect_enabled: Arc<AtomicBool>,
    /// Incremented by every `connect_port`. Background tasks capture the value
    /// they were spawned with and exit as soon as it changes, so tasks belonging
    /// to a previous connection cannot outlive it.
    ///
    /// `reconnect_enabled` alone is not enough: it is one shared flag, so
    /// connecting to a second port within the watchdog's 2 s tick would set it
    /// back to true before the old watchdog noticed, leaving two watchdogs
    /// running — and the stale one still holds the *previous* port name, so it
    /// would happily reconnect the device the user just switched away from.
    pub connection_generation: Arc<AtomicU64>,
    /// v0.3.6 — Transaction history (last MAX_HISTORY entries), persisted to JSON.
    pub tx_history: Arc<Mutex<Vec<TxHistoryEntry>>>,
    /// v0.3.6 — Background gateway daemon process handle.
    pub gateway_process: Arc<Mutex<Option<tokio::process::Child>>>,

    /// The serial port this app released so the gateway daemon could own it.
    ///
    /// `Some` only when the app was itself connected to that port when the
    /// daemon was started, so `stop_gateway` knows whether reconnecting is
    /// taking back something of ours or stealing a port the user never gave us.
    pub gateway_owned_port: Arc<Mutex<Option<String>>>,

    /// Unix-epoch milliseconds until which a firmware-version reply is believed
    /// on the Android path.
    ///
    /// Same reasoning as `SerialManager::firmware_query_until_ms`: the board
    /// forwards over-the-air broadcasts to its host verbatim, so a node in radio
    /// range can send a `0x20` packet claiming any version — and the version is
    /// what decides whether a payload too large for one packet may be sent.
    pub mobile_fw_query_until_ms: Arc<AtomicU64>,
    /// v0.3.6 — Connection type: "usb", "ble", or "usb-android"
    pub connection_type: Arc<Mutex<String>>,
    /// v0.3.10 — Raw byte accumulator for the Android USB bridge.
    /// The JS layer (using tauri-plugin-serialplugin) feeds received bytes here;
    /// Rust extracts complete packets using the same framing logic as the desktop
    /// serial read-loop and emits the standard "radio-packet" / "board-sync" events.
    pub mobile_accumulator: Arc<Mutex<Vec<u8>>>,
    /// Reassembles multipart sequences arriving on the Android USB/BLE path,
    /// mirroring the one owned by the desktop serial read loop.
    pub mobile_reassembler: Arc<Mutex<radio::MultipartReassembler>>,
    /// v0.3.10 — Firmware version string cached during the Android USB connect
    /// sequence. Stored when GET_FIRMWARE_VERSION (0x20) response is received,
    /// included in the "connection-status: connected" event emitted after GET_SETTINGS.
    pub mobile_fw_version: Arc<Mutex<Option<String>>>,
    /// v0.3.10 — BLE device address (MAC) when connected via Bluetooth, None otherwise.
    /// Set by mobile_ble_connect, cleared by mobile_ble_disconnect.
    pub ble_device_address: Arc<Mutex<Option<String>>>,
    /// v0.3.15 — Running count of packets received via the Android mobile path.
    /// Used to synthesize `radio-stats-update` events so the Dashboard packet
    /// counters and NavBar signal bars stay current on mobile USB/BLE.
    pub mobile_packets_rx: Arc<Mutex<u32>>,
    /// Running count of packets sent via the Android mobile path.
    /// Incremented by mobile_emit_debug_tx (USB) and mobile_ble_write_characteristic (BLE).
    pub mobile_packets_tx: Arc<Mutex<u32>>,
    /// v0.3.15 — Last known RSSI seen on the mobile path.
    /// Always 0 on USB (RSSI is not available over serial); may be non-zero
    /// in a future BLE path that reports received signal strength.
    pub mobile_last_rssi: Arc<Mutex<i16>>,
}

impl AppState {
    fn new() -> Self {
        AppState {
            serial: Arc::new(SerialManager::new()),
            current_port: Arc::new(Mutex::new(None)),
            lora_settings: Arc::new(Mutex::new(LoraSettings::default())),
            reconnect_enabled: Arc::new(AtomicBool::new(false)),
            connection_generation: Arc::new(AtomicU64::new(0)),
            tx_history: Arc::new(Mutex::new(Vec::new())),
            gateway_process: Arc::new(Mutex::new(None)),
            gateway_owned_port: Arc::new(Mutex::new(None)),
            mobile_fw_query_until_ms: Arc::new(AtomicU64::new(0)),
            connection_type: Arc::new(Mutex::new("usb".to_string())),
            mobile_accumulator: Arc::new(Mutex::new(Vec::new())),
            mobile_reassembler: Arc::new(Mutex::new(radio::MultipartReassembler::new())),
            mobile_fw_version: Arc::new(Mutex::new(None)),
            ble_device_address: Arc::new(Mutex::new(None)),
            mobile_packets_rx: Arc::new(Mutex::new(0)),
            mobile_packets_tx: Arc::new(Mutex::new(0)),
            mobile_last_rssi: Arc::new(Mutex::new(0)),
        }
    }
}

// ─── Debug helper ─────────────────────────────────────────────────────────────

fn emit_debug_traffic(app: &AppHandle, direction: &str, raw_hex: &str, parsed: &str) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let _ = app.emit("debug-serial-traffic", serde_json::json!({
        "timestamp": ts,
        "direction": direction,
        "rawHex": raw_hex,
        "parsed": parsed,
    }));
}

/// Milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// How long a firmware-version reply is believed on the Android path after the
/// connect handshake is built. Longer than the desktop window because BLE round
/// trips are slower.
const MOBILE_FIRMWARE_REPLY_WINDOW_MS: u64 = 6_000;

// ─── Persistence helpers ─────────────────────────────────────────────────────

/// Write `contents` to `path` so that a crash can never leave a partial file.
///
/// `fs::write` truncates first and writes second, so an interruption between
/// the two leaves an empty or half-written file. For `tx_history.json` that
/// costs a log; for `wallet.json` it costs the user's only copy of an encrypted
/// private key. Writing to a sibling temp file and renaming it into place makes
/// the switch atomic on every platform this app targets — the old file survives
/// intact until the new one is complete.
///
/// The temp file is removed on failure so a full disk cannot accumulate debris.
async fn write_file_atomically(path: &std::path::Path, contents: &str) -> Result<(), String> {
    // A per-call temp name, not a fixed one. Tauri commands run concurrently, and
    // with a shared `wallet.tmp` two overlapping saves interleave: the slower
    // writer can rename a file the faster one had only half written.
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp{}",
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(e) = tokio::fs::write(&tmp, contents).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("could not write {}: {}", tmp.display(), e));
    }
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("could not replace {}: {}", path.display(), e));
    }
    Ok(())
}

// ─── History helpers ──────────────────────────────────────────────────────────

/// Load transaction history from disk (best-effort; returns empty vec on any error).
async fn load_history_from_disk(app: &AppHandle) -> Vec<TxHistoryEntry> {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("tx_history.json"),
        Err(_) => return vec![],
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("[history] tx_history.json parse failed ({}), starting fresh", e);
                vec![]
            }
        },
        Err(_) => vec![],
    }
}

/// Persist transaction history to disk (best-effort; silently ignores errors).
async fn save_history_to_disk(app: &AppHandle, history: &[TxHistoryEntry]) {
    let dir = match app.path().app_data_dir() {
        Ok(p) => p,
        Err(_) => return,
    };
    let _ = tokio::fs::create_dir_all(&dir).await;
    let path = dir.join("tx_history.json");
    if let Ok(json) = serde_json::to_string_pretty(history) {
        if let Err(e) = write_file_atomically(&path, &json).await {
            eprintln!("[history] {}", e);
        }
    }
}

// ─── Tauri Commands ───────────────────────────────────────────────────────────

#[tauri::command]
async fn list_ports() -> Result<Vec<String>, String> {
    Ok(SerialManager::list_ports())
}

#[tauri::command]
async fn list_ports_detailed() -> Result<Vec<PortInfo>, String> {
    Ok(SerialManager::list_ports_with_info())
}

/// `true` when a `CMD_DOGE_TX` packet is an actual incoming transaction rather
/// than the board's own acknowledgement of one we just sent.
///
/// The board answers every `0x10` the host writes with an 8-byte reply carrying
/// the same command byte and no payload. Treating that as an arrival popped a
/// "DOGE Transaction Received!" notification on the *sender* after every send —
/// and once per frame, so a multipart transaction produced a burst of them.
/// A real transaction always decodes to something; the bare acknowledgement
/// cannot.
fn is_incoming_doge_tx(packet: &IncomingPacket) -> bool {
    packet.command == radio::CMD_DOGE_TX
        && !packet.payload_hex.is_empty()
        && packet.decoded.is_some()
}

/// Open a serial connection to the Heltec device.
/// v0.3.6: After connecting, queries CMD_GET_SETTINGS (0x22) to sync board state.
/// Board is source of truth — node address and gateway_mode update from board reply.
#[tauri::command]
async fn connect_port(
    port: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    log::info!("connect_port: {}", port);

    let _ = app.emit("connection-status", ConnectionStatusEvent::connecting(&port));

    state.reconnect_enabled.store(true, Ordering::Relaxed);

    // Claim a new generation. Any stats poller or reconnect watchdog still
    // running from a previous connection sees the change and exits.
    let generation = state.connection_generation.fetch_add(1, Ordering::SeqCst) + 1;

    let app_for_packets = app.clone();
    let on_packet = Arc::new(move |packet: IncomingPacket| {
        let _ = app_for_packets.emit("radio-packet", &packet);

        let parsed = packet.decoded.clone().unwrap_or_default();
        emit_debug_traffic(&app_for_packets, "RX", &packet.payload_hex, &parsed);

        if is_incoming_doge_tx(&packet) {
            let body = packet
                .decoded
                .clone()
                .unwrap_or_else(|| "Incoming Dogecoin transaction over LoRa".to_string());
            let _ = app_for_packets
                .notification()
                .builder()
                .title("🐕 DOGE Transaction Received!")
                .body(&body)
                .show();
        }

        // v0.3.7 — Emit addr-conflict event to frontend when board detects duplicate addr
        if packet.command == radio::CMD_ADDR_CONFLICT {
            let addr_str = packet.decoded.clone().unwrap_or_else(|| "unknown".to_string());
            let _ = app_for_packets.emit("addr-conflict", serde_json::json!({
                "detected": true,
                "address": addr_str,
            }));
        }
    });

    state
        .serial
        .connect(&port, on_packet)
        .await
        .map_err(|e| {
            let raw = e.to_string();
            if raw.contains("Access is denied") || raw.contains("Permission denied") {
                format!(
                    "Access denied on {}.\n\
                     Hint: Another program (Arduino IDE, PuTTY, device manager) may \
                     already have this port open. Close it and try again.",
                    port
                )
            } else if raw.contains("could not open")
                || raw.contains("No such file")
                || raw.contains("The system cannot find")
            {
                format!(
                    "Could not open {}.\n\
                     Hint: Install the CP210x or CH340 USB driver for your Heltec board.\n\
                     CP210x: https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers\n\
                     CH340: https://www.wch-ic.com/products/CH340.html",
                    port
                )
            } else if raw.contains("The device is not connected")
                || raw.contains("device has been removed")
            {
                format!(
                    "Device disconnected unexpectedly on {}.\n\
                     Hint: Check the USB cable — some USB-C cables are power-only.",
                    port
                )
            } else {
                radiodoge_core::errors::friendly(raw)
            }
        })?;

    *state.current_port.lock().await = Some(port.clone());

    // ── Query firmware version (up to 3 attempts) ────────────────────────────
    let mut firmware_version: Option<String> = None;
    for attempt in 1..=3 {
        emit_debug_traffic(
            &app,
            "TX",
            &hex::encode(radio::build_get_firmware_version(&NodeAddress::default_local())),
            &format!("CMD_GET_FIRMWARE_VERSION (attempt {})", attempt),
        );
        // request_firmware_version, not send_raw: it opens the window in which a
        // reply is believed. See SerialManager::firmware_query_until_ms.
        state.serial.request_firmware_version().await.ok();
        tokio::time::sleep(Duration::from_millis(400)).await;
        firmware_version = state.serial.get_firmware_version().await;
        if firmware_version.is_some() {
            log::info!("Firmware version received on attempt {}: {:?}", attempt, firmware_version);
            break;
        }
    }

    // ── v0.3.6: Query board settings (CMD_GET_SETTINGS 0x22) ─────────────────
    // Board is source of truth. App syncs node address + gateway_mode from board reply.
    let settings_query = radio::build_get_settings(&NodeAddress::default_local());
    let sq_hex = hex::encode(&settings_query);
    emit_debug_traffic(&app, "TX", &sq_hex, "CMD_GET_SETTINGS (0x22) — board-sync on connect");
    state.serial.send_raw(settings_query).await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let board_settings = state.serial.get_board_settings().await;

    // If board reported settings, emit board-sync event and update node address.
    // Board wins on mismatch — no local cache overrides board state.
    if let Some(ref bs) = board_settings {
        log::info!("Board sync: addr={} gateway={}", bs.node_address.to_display_string(), bs.gateway_mode);
        let _ = app.emit("board-sync", bs);
        state.serial.update_node_address(bs.node_address.clone()).await;
    }

    let node_addr = state.serial.get_node_address().await;
    let _ = app.emit(
        "connection-status",
        ConnectionStatusEvent::connected(&port, node_addr, firmware_version),
    );

    #[cfg(desktop)]
    tray::update_tray_status(&app, true, Some(&port));

    // Background stats polling every 2 s
    let serial_poll = Arc::clone(&state.serial);
    let app_for_poll = app.clone();
    let gen_poll = Arc::clone(&state.connection_generation);
    tokio::spawn(async move {
        loop {
            // Exit if this connection has been superseded, otherwise a poller
            // from an old session keeps emitting alongside the current one.
            if gen_poll.load(Ordering::SeqCst) != generation { break; }
            if !serial_poll.is_connected() { break; }
            let stats = serial_poll.get_stats().await;
            let _ = app_for_poll.emit("radio-stats-update", &stats);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    // Auto-reconnect watchdog (exponential backoff 5s → 60s)
    let serial_wr = Arc::clone(&state.serial);
    let current_port_wr = Arc::clone(&state.current_port);
    let reconnect_enabled_wr = Arc::clone(&state.reconnect_enabled);
    let app_wr = app.clone();
    let port_wr = port.clone();
    let gen_wr = Arc::clone(&state.connection_generation);
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            // This watchdog belongs to one connection. If the user has since
            // connected to a different port, `port_wr` is stale — exit rather
            // than reconnecting the device they switched away from.
            if gen_wr.load(Ordering::SeqCst) != generation { break; }
            if !reconnect_enabled_wr.load(Ordering::Relaxed) { break; }
            if serial_wr.is_connected() {
                backoff = Duration::from_secs(5);
                continue;
            }

            log::info!("Auto-reconnect: connection lost on {}, retrying…", port_wr);
            let _ = app_wr.emit("connection-status", ConnectionStatusEvent::reconnecting(&port_wr));

            let app_inner = app_wr.clone();
            let on_pkt = Arc::new(move |packet: IncomingPacket| {
                let _ = app_inner.emit("radio-packet", &packet);
                let parsed = packet.decoded.clone().unwrap_or_default();
                emit_debug_traffic(&app_inner, "RX", &packet.payload_hex, &parsed);
                if is_incoming_doge_tx(&packet) {
                    let body = packet.decoded.clone().unwrap_or_else(|| "Incoming Dogecoin transaction".to_string());
                    let _ = app_inner.notification().builder().title("🐕 DOGE Transaction Received!").body(&body).show();
                }
            });

            // Re-check after building the callback — disconnect_port or a
            // connect to a different port may have happened in the narrow
            // window between the checks above and here.
            if !reconnect_enabled_wr.load(Ordering::Relaxed) { break; }
            if gen_wr.load(Ordering::SeqCst) != generation { break; }

            match serial_wr.connect(&port_wr, on_pkt).await {
                Ok(_) => {
                    *current_port_wr.lock().await = Some(port_wr.clone());
                    // v0.3.6 — re-sync board settings after reconnect
                    let sq = radio::build_get_settings(&NodeAddress::default_local());
                    serial_wr.send_raw(sq).await.ok();
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if let Some(bs) = serial_wr.get_board_settings().await {
                        let _ = app_wr.emit("board-sync", &bs);
                        serial_wr.update_node_address(bs.node_address).await;
                    }
                    let node_addr = serial_wr.get_node_address().await;
                    let _ = app_wr.emit("connection-status", ConnectionStatusEvent::connected(&port_wr, node_addr, None));
                    #[cfg(desktop)]
                    tray::update_tray_status(&app_wr, true, Some(&port_wr));
                    backoff = Duration::from_secs(5);
                    log::info!("Auto-reconnect: reconnected to {}", port_wr);
                }
                Err(e) => {
                    log::warn!("Auto-reconnect failed: {} — retrying in {:?}", e, backoff);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    });

    // Load persisted history on connect (once per session)
    {
        let mut hist = state.tx_history.lock().await;
        if hist.is_empty() {
            *hist = load_history_from_disk(&app).await;
        }
    }

    Ok(())
}

#[tauri::command]
async fn disconnect_port(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    log::info!("disconnect_port");
    state.reconnect_enabled.store(false, Ordering::Relaxed);
    state.serial.disconnect().await.map_err(radiodoge_core::errors::friendly)?;
    *state.current_port.lock().await = None;
    let _ = app.emit("connection-status", ConnectionStatusEvent::disconnected());
    #[cfg(desktop)]
    tray::update_tray_status(&app, false, None);
    Ok(())
}

#[tauri::command]
async fn is_connected(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.serial.is_connected())
}

#[tauri::command]
async fn get_radio_stats(state: State<'_, AppState>) -> Result<RadioStats, String> {
    Ok(state.serial.get_stats().await)
}

#[tauri::command]
async fn get_node_address(state: State<'_, AppState>) -> Result<NodeAddress, String> {
    Ok(state.serial.get_node_address().await)
}

#[tauri::command]
async fn get_firmware_version(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.serial.get_firmware_version().await)
}

#[tauri::command]
async fn query_firmware_version(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    if !state.serial.is_connected() { return Ok(None); }
    emit_debug_traffic(
        &app,
        "TX",
        &hex::encode(radio::build_get_firmware_version(&NodeAddress::default_local())),
        "CMD_GET_FIRMWARE_VERSION (manual re-query)",
    );
    state.serial.request_firmware_version().await.map_err(radiodoge_core::errors::friendly)?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    Ok(state.serial.get_firmware_version().await)
}

// ─── v0.3.25 — Meshtastic bridge mode over raw LoRa (firmware FW12) ─────────

fn bridge_profile(freq_khz: u32) -> Result<radio::RawProfile, String> {
    let p = radio::RawProfile { freq_khz, ..radio::RawProfile::meshtastic_longfast_us() };
    p.validate()?;
    Ok(p)
}

async fn require_raw_firmware(state: &State<'_, AppState>) -> Result<NodeAddress, String> {
    if !state.serial.is_connected() {
        return Err("Not connected to a board.".into());
    }
    let fw = state.serial.ensure_firmware_version().await;
    if !radio::firmware_supports_raw(fw.as_deref()) {
        return Err(format!(
            "Bridge mode needs firmware FW{} (v0.4.4) or newer; this board reports {}.",
            radio::MIN_RAW_LORA_FIRMWARE, fw.as_deref().unwrap_or("no version")
        ));
    }
    Ok(state.serial.get_node_address().await)
}

/// Enter (or leave) volatile raw receive on the Meshtastic LongFast channel.
/// Returns whether raw RX is active. Never changes the board's saved settings.
#[tauri::command]
async fn meshtastic_bridge_listen(
    state: State<'_, AppState>, enable: bool, freq_khz: u32, timeout_s: u16,
) -> Result<bool, String> {
    let host = require_raw_firmware(&state).await?;
    let profile = bridge_profile(freq_khz)?;
    let pkt = radio::build_raw_rx_mode(&host, enable.then_some(&profile), timeout_s)?;
    match state.serial.send_and_get_reply(pkt, radio::CMD_RAW_RX_MODE, 3000).await.map_err(radiodoge_core::errors::friendly)? {
        Some(r) => Ok(r.payload_hex == "01"),
        None => Err("The board did not answer the raw-mode command.".into()),
    }
}

/// Send a Meshtastic text message on LongFast via a single raw transmit.
#[tauri::command]
async fn meshtastic_bridge_send_text(
    state: State<'_, AppState>, text: String, freq_khz: u32,
) -> Result<bool, String> {
    if text.trim().is_empty() {
        return Err("Message is empty.".into());
    }
    let host = require_raw_firmware(&state).await?;
    let profile = bridge_profile(freq_khz)?;
    let ch = radiodoge_core::meshtastic::Channel::default_longfast();
    let pkt = radiodoge_core::meshtastic::bridge_text_command(
        &host, 0x5244_0000 | u32::from(host.node), radiodoge_core::meshtastic::new_packet_id(), &text, &ch, &profile,
    )?;
    match state.serial.send_and_get_reply(pkt, radio::CMD_RAW_TX, 8000).await.map_err(radiodoge_core::errors::friendly)? {
        Some(r) => Ok(r.payload_hex == "01"),
        None => Err("No reply to the raw transmit.".into()),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeDecoded {
    kind: &'static str,
    from: Option<String>,
    text: Option<String>,
    rssi: i16,
    snr: i8,
}

/// Decode the payload of a CMD_RAW_RX (0x2D) packet on LongFast.
#[tauri::command]
fn meshtastic_bridge_decode(payload_hex: String) -> Option<BridgeDecoded> {
    use radiodoge_core::meshtastic::{bridge_decode, BridgeEvent, Channel};
    let f = radio::parse_raw_rx(&hex::decode(payload_hex).ok()?)?;
    let (kind, from, text) = match bridge_decode(&f.data, &Channel::default_longfast()) {
        BridgeEvent::RadioDoge { from, packet } => ("radiodoge", Some(from), Some(hex::encode(packet))),
        BridgeEvent::Text { from, text } => ("text", Some(from), Some(text)),
        BridgeEvent::Other { from, portnum } => ("other", Some(from), Some(format!("portnum {}", portnum))),
        BridgeEvent::Undecodable => ("undecodable", None, None),
    };
    Some(BridgeDecoded { kind, from: from.map(|n| format!("!{:08x}", n)), text, rssi: f.rssi, snr: f.snr })
}

#[tauri::command]
async fn ping_device(state: State<'_, AppState>, app: AppHandle) -> Result<bool, String> {
    if !state.serial.is_connected() {
        emit_debug_traffic(&app, "TX", "", "CMD_PING — not connected, skipped");
        return Ok(false);
    }
    let local = state.serial.get_node_address().await;
    let pkt_hex = hex::encode(radio::build_ping(&local, &local));
    emit_debug_traffic(&app, "TX", &pkt_hex, "CMD_PING → device health check");
    let ok = state.serial.ping().await;
    emit_debug_traffic(
        &app, "RX", "",
        if ok { "✅ PONG — device responded within 1500 ms 🐕" }
        else  { "❌ No PONG within 1500 ms — firmware may be busy" },
    );
    Ok(ok)
}

#[tauri::command]
async fn generate_wallet() -> Result<WalletInfo, String> {
    wallet::generate_keypair().map_err(radiodoge_core::errors::friendly)
}

/// v0.3.6 — Import a Dogecoin wallet from a WIF-encoded private key.
/// Only compressed mainnet keys (starting with "Q") are supported.
/// ⚠️ Hot wallet warning — only use with test amounts.
#[tauri::command]
async fn import_wif(wif: String) -> Result<WalletInfo, String> {
    wallet::import_wif(&wif).map_err(radiodoge_core::errors::friendly)
}

/// v0.3.16 — Generate a new wallet with a 12-word BIP39 recovery phrase.
/// Returns the mnemonic alongside the derived WalletInfo.
/// The mnemonic is NOT stored — the caller must back it up before it disappears.
#[tauri::command]
async fn generate_mnemonic_wallet() -> Result<serde_json::Value, String> {
    let mnemonic = wallet::generate_mnemonic().map_err(radiodoge_core::errors::friendly)?;
    let phrase = mnemonic.to_string();
    let info = wallet::wallet_from_mnemonic(&phrase).map_err(radiodoge_core::errors::friendly)?;
    Ok(serde_json::json!({
        "mnemonic": phrase,
        "address": info.address,
        "publicKeyHex": info.public_key_hex,
        "privateKeyWif": info.private_key_wif,
    }))
}

/// v0.3.16 — Import a wallet from a BIP39 mnemonic phrase.
/// Derives the key at m/44'/3'/0'/0/0 (Dogecoin BIP44 path).
#[tauri::command]
async fn import_mnemonic_wallet(phrase: String) -> Result<WalletInfo, String> {
    wallet::wallet_from_mnemonic(&phrase).map_err(radiodoge_core::errors::friendly)
}

/// v0.3.16 — Query the confirmed Dogecoin balance for an address via the configured chain backends (Core RPC, then BlockCypher).
/// Returns the balance as a floating-point DOGE amount.
#[tauri::command]
async fn get_balance(address: String) -> Result<f64, String> {
    if !wallet::is_valid_address(&address) {
        return Err("Invalid Dogecoin address".to_string());
    }
    let koinus = wallet::fetch_balance(&address)
        .await
        .map_err(radiodoge_core::errors::friendly)?;
    Ok(koinus as f64 / 1e8)
}

/// Validate a fee typed in the UI. `None` = let core pick the network rate.
fn resolve_fee(fee_doge: Option<f64>) -> Result<Option<f64>, String> {
    match fee_doge {
        None => Ok(None),
        Some(f) if f.is_finite() && (0.0..=wallet::MAX_AUTO_FEE_DOGE).contains(&f) => Ok(Some(f)),
        Some(f) => Err(format!("Invalid fee {} DOGE (must be 0 to {} DOGE)", f, wallet::MAX_AUTO_FEE_DOGE)),
    }
}

/// Estimate the network fee for sending `amount_doge` from `from_address`
/// using the configured backends' current fee rate (display only; signing
/// with no explicit fee recomputes it).
#[tauri::command]
async fn estimate_fee(from_address: String, amount_doge: f64) -> Result<wallet::FeeEstimate, String> {
    if !wallet::is_valid_address(&from_address) {
        return Err("Invalid Dogecoin address".to_string());
    }
    wallet::estimate_fee(&from_address, amount_doge).await.map_err(radiodoge_core::errors::friendly)
}

/// v0.4.3 — Override the chain backend list for this session, e.g.
/// `"core=http://127.0.0.1:22555,blockcypher"`. An empty string reverts to
/// `RADIODOGE_BACKENDS` / the default (`core,blockcypher`). RPC credentials come
/// from `RADIODOGE_RPC_USER`/`RADIODOGE_RPC_PASSWORD` or Core's `.cookie`.
#[tauri::command]
async fn set_chain_backends(spec: String) -> Result<(), String> {
    if spec.trim().is_empty() {
        radiodoge_core::backend::set_backends(None);
        return Ok(());
    }
    let list = radiodoge_core::backend::parse_backends(&spec, &|k| std::env::var(k).ok())
        .map_err(radiodoge_core::errors::friendly)?;
    radiodoge_core::backend::set_backends(Some(list));
    Ok(())
}

/// Lightweight SPV inclusion check for a txid (confirmations + block) via the configured chain backends.
#[tauri::command]
async fn verify_tx_inclusion(txid: String) -> Result<radiodoge_core::spv::TxInclusion, String> {
    radiodoge_core::spv::fetch_tx_inclusion(&txid)
        .await
        .map_err(radiodoge_core::errors::friendly)
}

/// v0.4.0 — Decode a QR code from raw image bytes (PNG/JPEG from a file picker or
/// camera capture) and parse it as a Dogecoin address / BIP21 payment URI.
///
/// Returns the parsed address (+ optional amount/label) so the Send tab can fill
/// the recipient field. Errors if no QR code is found or it isn't a Dogecoin
/// address. Decoding runs on a blocking thread so the UI stays responsive.
#[tauri::command]
async fn scan_qr_from_image(
    image_bytes: Vec<u8>,
) -> Result<radiodoge_core::qr::ParsedPayment, String> {
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&image_bytes)
            .map_err(|e| format!("Could not read image: {}", e))?
            .to_luma8();
        let mut prepared = rqrr::PreparedImage::prepare(img);
        let grids = prepared.detect_grids();
        for grid in grids {
            if let Ok((_meta, text)) = grid.decode() {
                if let Some(parsed) = radiodoge_core::qr::parse_payment(&text) {
                    return Ok(parsed);
                }
            }
        }
        Err("No Dogecoin address QR code found in the image".to_string())
    })
    .await
    .map_err(|e| format!("QR decode task failed: {}", e))?
}

/// `true` when the JS bridge, rather than the Rust `SerialManager`, owns the port.
///
/// This is an Android-only arrangement: there the serial port is opened by
/// `tauri-plugin-serialplugin` or the BLE plugin in JavaScript, so
/// `serial.is_connected()` is always false even while the board is attached.
///
/// It used to be *inferred* — "a port is set but the Rust side is not connected"
/// — which is exactly the state a desktop machine enters when the USB cable is
/// pulled: `is_connected()` goes false while `current_port` stays set until the
/// user presses Disconnect. A desktop app in that state took the Android path,
/// emitted the transaction frames as an event nothing listens to on desktop, and
/// reported success for a signed transaction that was dropped on the floor.
///
/// Compiled per platform now, so a dropped cable is a dropped cable.
async fn js_bridge_owns_port(state: &State<'_, AppState>) -> bool {
    cfg!(target_os = "android") && state.current_port.lock().await.is_some()
}

#[tauri::command]
async fn send_transaction(
    tx: TransactionRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<SendOutcome, String> {
    // On Android the JS bridge owns the USB/BLE port, so serial.is_connected() is
    // always false. Everywhere else, not connected means not connected.
    let is_mobile = js_bridge_owns_port(&state).await;
    if !is_mobile && !state.serial.is_connected() {
        return Err(
            "Not connected to a Heltec device. Nothing was sent — check the cable and reconnect."
                .to_string(),
        );
    }

    if !wallet::is_valid_address(&tx.to_address) {
        return Err(format!(
            "Invalid Dogecoin address: {}. Addresses start with 'D'.",
            tx.to_address
        ));
    }

    if tx.amount_doge <= 0.0 {
        return Err("Amount must be greater than 0 DOGE".to_string());
    }

    // Build payload: real signed transaction when WIF is available, legacy stub otherwise.
    let payload = if let Some(ref wif) = tx.from_private_key_wif {
        wallet::build_signed_transaction_with_fee(
            wif,
            &tx.to_address,
            tx.amount_doge,
            resolve_fee(tx.fee_doge)?,
        )
        .await
        .map_err(|e| format!("Transaction signing failed: {}", e))?
        .0
    } else {
        wallet::encode_transaction_payload(
            &tx.to_address,
            tx.amount_doge,
            tx.memo.as_deref(),
        )
        .map_err(radiodoge_core::errors::friendly)?
    };

    // Duplicate-broadcast guard: the same signed bytes radioed twice in a short
    // window is almost always a double click or an impatient retry. The network
    // would drop the copy, but it costs airtime and confuses the ACK display.
    let txid = tx.from_private_key_wif.as_ref().map(|_| wallet::compute_txid(&payload));
    if let Some(id) = txid.as_deref() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut recent = RECENT_SENDS.lock().await;
        if let Some(age) = recent.recently_sent(id, now) {
            if !tx.allow_resend {
                return Err(format!(
                    "DUPLICATE: this exact transaction ({}) was radioed {} s ago. Nothing was sent. Check {} before resending.",
                    id, age, tx_reply::explorer_url(id)
                ));
            }
        }
        recent.record(id, now);
    }

    // How the payload is framed depends on what the board's firmware can
    // receive: one packet on older builds, a multipart sequence on v0.4.2+.
    // `build_tx_frames` refuses anything the board would mis-frame rather than
    // transmitting a transaction no receiver can reconstruct.
    let src = state.serial.get_node_address().await;
    let dst = NodeAddress::broadcast();
    let signed_label = tx.from_private_key_wif.is_some();
    let frame_count: usize;

    if is_mobile {
        // Mobile path: the JS bridge owns the port; emit packet bytes via a
        // Tauri event so the connection-bridge session listener can write them.
        let fw = state.mobile_fw_version.lock().await.clone();
        let packets =
            radio::build_tx_frames(&src, &dst, radio::CMD_DOGE_TX, &payload, fw.as_deref())?;
        for (i, pkt) in packets.iter().enumerate() {
            let label = if signed_label {
                format!("CMD_DOGE_TX signed (mobile {}/{})", i + 1, packets.len())
            } else {
                format!("CMD_DOGE_TX (mobile {}/{})", i + 1, packets.len())
            };
            emit_debug_traffic(&app, "TX", &hex::encode(pkt), &label);
        }
        frame_count = packets.len();
        let _ = app.emit("mobile-tx-packets", &packets);
    } else {
        let fw = state.serial.ensure_firmware_version().await;
        let frames =
            radio::build_tx_frames(&src, &dst, radio::CMD_DOGE_TX, &payload, fw.as_deref())?;
        for (i, pkt) in frames.iter().enumerate() {
            let label = match (signed_label, frames.len()) {
                (true, 1) => "CMD_DOGE_TX signed (single)".to_string(),
                (false, 1) => "CMD_DOGE_TX (single packet)".to_string(),
                (signed, n) => format!(
                    "CMD_DOGE_TX{} (multipart {}/{})",
                    if signed { " signed" } else { "" },
                    i + 1,
                    n
                ),
            };
            emit_debug_traffic(&app, "TX", &hex::encode(pkt), &label);
        }
        // Frames go out one at a time, each acknowledged by the board before the
        // next is written — see SerialManager::send_frames. A multipart send is
        // several seconds of real airtime, so report progress rather than
        // leaving the UI to guess whether anything is happening.
        let total_frames = frames.len();
        frame_count = total_frames;
        let app_for_progress = app.clone();
        let sent = state
            .serial
            .send_frames_with_progress(frames, move |sent, total| {
                let _ = app_for_progress.emit(
                    "transaction-progress",
                    serde_json::json!({ "sent": sent, "total": total }),
                );
            })
            .await
            .map_err(radiodoge_core::errors::friendly);
        if let Err(e) = sent {
            // Not every frame reached the board, so no complete transaction
            // was aired — allow an immediate retry.
            if let Some(id) = txid.as_deref() {
                RECENT_SENDS.lock().await.forget(id);
            }
            return Err(e);
        }
        if total_frames > 1 {
            log::info!("Transaction sent as {} LoRa frames", total_frames);
        }
    }

    let msg = if tx.from_private_key_wif.is_some() {
        format!(
            "Signed tx broadcast: {:.8} DOGE → {} via LoRa! 🐕🌙",
            tx.amount_doge, tx.to_address
        )
    } else {
        format!(
            "Broadcast {:.8} DOGE → {} via LoRa! 🐕🌙",
            tx.amount_doge, tx.to_address
        )
    };

    let _ = app.emit("transaction-sent", &msg);

    let tx_log_payload = serde_json::json!({
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "source": { "region": src.region, "community": src.community, "node": src.node },
        "destination": { "region": 255, "community": 255, "node": 255 },
        "command": radio::CMD_DOGE_TX,
        "payloadHex": hex::encode(&payload),
        "decoded": format!("🐕 DOGE TX: {:.8} DOGE → {}{}", tx.amount_doge, tx.to_address,
            tx.memo.as_deref().map(|m| format!(" [{}]", m)).unwrap_or_default()),
        "rssi": 0,
        "direction": "TX"
    });
    let _ = app.emit("radio-packet-tx", &tx_log_payload);

    // v0.3.6 — Record to history
    {
        let entry = TxHistoryEntry {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            to_address: tx.to_address.clone(),
            amount_doge: tx.amount_doge,
            memo: tx.memo.clone(),
            status: "sent".to_string(),
        };
        let mut hist = state.tx_history.lock().await;
        hist.insert(0, entry);
        hist.truncate(MAX_HISTORY);
        save_history_to_disk(&app, &hist).await;
    }

    log::info!("Transaction sent: {}", msg);
    Ok(SendOutcome {
        message: msg,
        explorer_url: txid.as_deref().map(tx_reply::explorer_url),
        txid,
        frames: frame_count,
    })
}

/// Txids radioed this session, for the duplicate-broadcast guard.
static RECENT_SENDS: std::sync::LazyLock<tokio::sync::Mutex<radiodoge_core::tx_reply::RecentSends>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(Default::default()));

// ── v0.3.22 — Multi-channel scanning ───────────────────────────────────────

static SCAN_RUNNING: AtomicBool = AtomicBool::new(false);
static SCAN_CANCEL: AtomicBool = AtomicBool::new(false);

/// Retune the board's radio only (no address change). `Ok(true)` when the
/// board answered with a 0x21 acknowledgement.
async fn retune_radio(state: &AppState, app: &AppHandle, s: &LoraSettings) -> Result<bool, String> {
    let (sf, bw, cr, khz, pwr) = radiodoge_core::scan::lora_param_fields(s);
    let pkt = radio::build_set_lora_params(&s.node_address, sf, bw, cr, khz, pwr);
    emit_debug_traffic(app, "TX", &hex::encode(&pkt), &format!(
        "CMD_SET_LORA_PARAMS (scan) → SF{} BW{}kHz {}MHz", sf, s.bandwidth_khz, s.frequency_mhz
    ));
    state
        .serial
        .send_and_await_reply(pkt, radio::CMD_SET_LORA_PARAMS, 1500)
        .await
        .map_err(radiodoge_core::errors::friendly)
}

/// Built-in scan plan for a region (`us915` or `eu868`).
#[tauri::command]
fn scan_plan(region: String) -> Vec<radiodoge_core::scan::ScanChannel> {
    radiodoge_core::scan::plan(&region)
}

/// Hop across `channels`, listening `dwell_secs` on each, then return the board
/// to the app's saved LoRa settings — always, even on cancel or error.
/// Emits `scan-progress` before each channel.
#[tauri::command]
async fn scan_channels(
    channels: Vec<radiodoge_core::scan::ScanChannel>,
    dwell_secs: u64,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    use radiodoge_core::scan::{is_air_packet, ChannelResult, MAX_CHANNELS, MAX_DWELL_SECS};
    if !state.serial.is_connected() {
        return Err("Connect to a board first".into());
    }
    let fw = state.serial.get_firmware_version().await;
    if !fw.as_deref().and_then(radio::firmware_build_number).is_some_and(|b| b >= 10) {
        return Err("Channel scanning needs firmware v0.4.1 or newer (older boards ignore radio retuning).".into());
    }
    if channels.is_empty() || channels.len() > MAX_CHANNELS {
        return Err(format!("Pick 1–{MAX_CHANNELS} channels"));
    }
    if let Some(bad) = channels.iter().find(|c| !c.is_valid()) {
        return Err(format!("{} is outside the board's ranges (SF 7–12, 125/250/500 kHz, 150–960 MHz)", bad.label));
    }
    if SCAN_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("A scan is already running".into());
    }
    SCAN_CANCEL.store(false, Ordering::SeqCst);
    let dwell = dwell_secs.clamp(3, MAX_DWELL_SECS);
    let home = state.lora_settings.lock().await.clone();
    let total = channels.len();
    let mut results = Vec::with_capacity(total);

    for (i, ch) in channels.iter().enumerate() {
        if SCAN_CANCEL.load(Ordering::SeqCst) || !state.serial.is_connected() {
            break;
        }
        let _ = app.emit("scan-progress", serde_json::json!({
            "index": i, "total": total, "label": ch.label, "dwellSecs": dwell,
        }));
        match retune_radio(&state, &app, &ch.apply_to(&home)).await {
            Ok(true) => {}
            Ok(false) => {
                results.push(serde_json::json!({ "channel": ch, "error": "board did not confirm the retune" }));
                continue;
            }
            Err(e) => {
                results.push(serde_json::json!({ "channel": ch, "error": e }));
                continue;
            }
        }
        let mut rx = state.serial.subscribe_packets();
        let mut res = ChannelResult::default();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(dwell);
        while tokio::time::Instant::now() < deadline && !SCAN_CANCEL.load(Ordering::SeqCst) {
            match tokio::time::timeout(std::time::Duration::from_millis(250), rx.recv()).await {
                Ok(Ok(p)) if is_air_packet(&p) => res.record(&p),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
                _ => {}
            }
        }
        results.push(serde_json::json!({ "channel": ch, "result": res }));
    }

    let cancelled = SCAN_CANCEL.load(Ordering::SeqCst);
    let restored = if state.serial.is_connected() {
        retune_radio(&state, &app, &home).await.unwrap_or(false)
    } else {
        false
    };
    SCAN_RUNNING.store(false, Ordering::SeqCst);
    let _ = app.emit("scan-progress", serde_json::json!({ "index": total, "total": total, "done": true }));
    Ok(serde_json::json!({
        "results": results, "cancelled": cancelled, "restored": restored,
        "home": { "frequencyMhz": home.frequency_mhz, "spreadingFactor": home.spreading_factor, "bandwidthKhz": home.bandwidth_khz },
    }))
}

/// Stop a running scan after the current listen slice; the board is still retuned home.
#[tauri::command]
fn cancel_scan() {
    SCAN_CANCEL.store(true, Ordering::SeqCst);
}

#[tauri::command]
async fn update_lora_settings(
    settings: LoraSettings,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<bool, String> {
    *state.lora_settings.lock().await = settings.clone();

    if !state.serial.is_connected() {
        return Ok(false);
    }

    let current_addr = state.serial.get_node_address().await;
    let pkt = radio::build_set_node_addr(&current_addr, &settings.node_address);
    let pkt_hex = hex::encode(&pkt);
    emit_debug_traffic(
        &app, "TX", &pkt_hex,
        &format!("CMD_SET_NODE_ADDRS → {}.{}.{}",
            settings.node_address.region, settings.node_address.community, settings.node_address.node),
    );
    state.serial.send_raw(pkt).await.map_err(radiodoge_core::errors::friendly)?;

    let freq_khz = (settings.frequency_mhz * 1000.0).round() as u32;
    let bw_idx: u8 = match settings.bandwidth_khz as u32 {
        250 => 1,
        500 => 2,
        _   => 0,
    };
    let cr: u8 = settings.coding_rate
        .trim_start_matches("4/")
        .parse::<u8>()
        .unwrap_or(5);
    let tx_power = settings.power_dbm.clamp(2, 22) as u8;
    let lora_pkt = radio::build_set_lora_params(
        &settings.node_address, settings.spreading_factor, bw_idx, cr, freq_khz, tx_power,
    );
    let lora_pkt_hex = hex::encode(&lora_pkt);
    emit_debug_traffic(&app, "TX", &lora_pkt_hex, &format!(
        "CMD_SET_LORA_PARAMS → SF{} BW{}kHz CR4/{} {}MHz {}dBm",
        settings.spreading_factor, settings.bandwidth_khz, cr, settings.frequency_mhz, tx_power,
    ));
    // The board validates these before applying them and answers a legacy NACK,
    // not a 0x21 packet, when they are out of range — it leaves the radio
    // untouched rather than retuning to somewhere it can never be reached again.
    // Without waiting for the 0x21 the UI reported "Verified" for settings the
    // board had thrown away, which is precisely the failure the NACK exists to
    // make visible. Firmware before v0.4.1 (FW10) treated 0x21 as a no-op ACK,
    // so only newer boards are held to this.
    let fw = state.serial.get_firmware_version().await;
    let board_applies_lora_params = fw
        .as_deref()
        .and_then(radio::firmware_build_number)
        .is_some_and(|b| b >= 10);

    let params_acked = state
        .serial
        .send_and_await_reply(lora_pkt, radio::CMD_SET_LORA_PARAMS, 1500)
        .await
        .map_err(radiodoge_core::errors::friendly)?;

    if board_applies_lora_params && !params_acked {
        return Err(format!(
            "The board rejected these radio settings and left its radio unchanged: \
             SF{}, {} kHz, CR4/{}, {} MHz, {} dBm. Valid ranges are SF 7–12, \
             bandwidth 125/250/500 kHz, coding rate 4/5–4/8, 150–960 MHz, and 2–22 dBm.",
            settings.spreading_factor, settings.bandwidth_khz, cr, settings.frequency_mhz, tx_power
        ));
    }

    let verified = state
        .serial
        .verify_node_address_set(&settings.node_address, 1500)
        .await;

    if verified {
        state.serial.update_node_address(settings.node_address.clone()).await;
        let port = state.current_port.lock().await.clone().unwrap_or_default();
        let fw = state.serial.get_firmware_version().await;
        let _ = app.emit(
            "connection-status",
            ConnectionStatusEvent::connected(&port, settings.node_address.clone(), fw),
        );
        emit_debug_traffic(&app, "RX", "", &format!(
            "✅ Node address verified: {}.{}.{}",
            settings.node_address.region, settings.node_address.community, settings.node_address.node
        ));
    }

    log::info!(
        "LoRa settings updated (verified={}): {}MHz, SF{}, {}kHz, addr={}.{}.{}",
        verified,
        settings.frequency_mhz,
        settings.spreading_factor,
        settings.bandwidth_khz,
        settings.node_address.region,
        settings.node_address.community,
        settings.node_address.node,
    );

    Ok(verified)
}

#[tauri::command]
async fn get_lora_settings(state: State<'_, AppState>) -> Result<LoraSettings, String> {
    Ok(state.lora_settings.lock().await.clone())
}

/// v0.3.6 — Query board live state (CMD_GET_SETTINGS 0x22).
/// Board is source of truth — returns node address + gateway_mode as stored on board.
#[tauri::command]
async fn get_board_settings(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Option<BoardSettings>, String> {
    // Android mobile path: the serial port is owned by the JS bridge, not by
    // the Rust SerialManager.  is_connected() is always false on mobile.
    // Return the board settings cached by mobile_push_bytes when CMD_GET_SETTINGS
    // last arrived — this is the same data the frontend already has from board-sync.
    let is_mobile_connected = js_bridge_owns_port(&state).await;

    if is_mobile_connected {
        let cached = state.serial.get_board_settings().await;
        emit_debug_traffic(
            &app, "INFO", "",
            "get_board_settings: Android path — returning cached settings from last board-sync",
        );
        return Ok(cached);
    }

    if !state.serial.is_connected() {
        return Ok(None);
    }

    // Desktop path: actively query the board over the Rust-owned serial port.
    let src = state.serial.get_node_address().await;
    let pkt = radio::build_get_settings(&src);
    let pkt_hex = hex::encode(&pkt);
    emit_debug_traffic(&app, "TX", &pkt_hex, "CMD_GET_SETTINGS (0x22) — manual query");
    state.serial.send_raw(pkt).await.map_err(radiodoge_core::errors::friendly)?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    Ok(state.serial.get_board_settings().await)
}

/// v0.3.6 — Set gateway mode on board (CMD_SET_GATEWAY 0x23).
/// Persists to NVS flash — survives power cycles. Board is source of truth.
#[tauri::command]
async fn set_gateway_mode(
    enable: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<bool, String> {
    if !state.serial.is_connected() {
        return Err("Not connected to board — cannot set gateway mode.".to_string());
    }
    let src = state.serial.get_node_address().await;
    let pkt = radio::build_set_gateway(&src, enable);
    let pkt_hex = hex::encode(&pkt);
    emit_debug_traffic(&app, "TX", &pkt_hex, &format!("CMD_SET_GATEWAY (0x23) → {}", if enable { "ON" } else { "OFF" }));
    state.serial.send_raw(pkt).await.map_err(radiodoge_core::errors::friendly)?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Re-query to confirm board saved the setting
    let confirm_pkt = radio::build_get_settings(&src);
    state.serial.send_raw(confirm_pkt).await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let bs = state.serial.get_board_settings().await;
    let confirmed = bs.as_ref().map(|s| s.gateway_mode).unwrap_or(false);

    if let Some(ref bs) = bs {
        let _ = app.emit("board-sync", bs);
    }

    log::info!("Gateway mode set: enable={} confirmed={}", enable, confirmed);
    Ok(confirmed)
}

/// v0.3.6 — Transaction history (last MAX_HISTORY entries).
#[tauri::command]
async fn get_history(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Vec<TxHistoryEntry>, String> {
    let hist = state.tx_history.lock().await.clone();
    if hist.is_empty() {
        // Try loading from disk on first call (e.g. app just started, no TX yet)
        let loaded = load_history_from_disk(&app).await;
        if !loaded.is_empty() {
            drop(hist);
            *state.tx_history.lock().await = loaded.clone();
            return Ok(loaded);
        }
    }
    Ok(hist)
}

/// v0.3.6 — Spawn `radiodoge-cli daemon -p <port>` as a background gateway process.
/// Emits "gateway-status" event on start.
///
/// **The daemon takes ownership of the serial port.** A serial port has exactly
/// one owner: on Windows the daemon's open would simply fail, and on Linux both
/// processes read the same device and each receives a random subset of the
/// bytes — on the path that carries transactions. Since the button is normally
/// pressed for the port the app itself is holding, the app releases it here and
/// takes it back in [`stop_gateway`], rather than leaving two readers fighting
/// over one board.
#[tauri::command]
async fn start_gateway(
    port: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // Stop any existing daemon first
    {
        let mut gp = state.gateway_process.lock().await;
        if let Some(ref mut child) = *gp {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *gp = None;
        *state.gateway_owned_port.lock().await = None;
    }

    // Hand the port over if we are the one holding it. The auto-reconnect
    // watchdog has to be stopped first, or it will reopen the port underneath
    // the daemon a couple of seconds later.
    let holding_this_port = state
        .current_port
        .lock()
        .await
        .as_deref()
        .is_some_and(|p| p == port);
    if holding_this_port {
        log::info!("Releasing {} so the gateway daemon can own it", port);
        state.reconnect_enabled.store(false, Ordering::Relaxed);
        state.connection_generation.fetch_add(1, Ordering::SeqCst);
        state.serial.disconnect().await.map_err(radiodoge_core::errors::friendly)?;
        *state.current_port.lock().await = None;
        let _ = app.emit("connection-status", ConnectionStatusEvent::disconnected());
        #[cfg(desktop)]
        tray::update_tray_status(&app, false, None);
    }

    // v0.3.7 — Find radiodoge-cli: check same dir as this executable first (bundled),
    // then fall back to PATH.  This eliminates "program not found" for packaged apps.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let cli_name = if cfg!(windows) { "radiodoge-cli.exe" } else { "radiodoge-cli" };
    let cli_path = exe_dir
        .map(|d| d.join(cli_name))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| cli_name.to_string()); // fall back to PATH

    // The daemon writes its counters here for the gateway dashboard. Remove the
    // previous run's snapshot so the dashboard never shows stale numbers.
    let stats_path = gateway_stats_path(&app);
    if let Some(ref p) = stats_path {
        if let Some(dir) = p.parent() {
            let _ = tokio::fs::create_dir_all(dir).await;
        }
        let _ = tokio::fs::remove_file(p).await;
    }
    let mut daemon_args: Vec<String> = vec!["daemon".into(), "-p".into(), port.clone()];
    if let Some(ref p) = stats_path {
        daemon_args.push("--stats-file".into());
        daemon_args.push(p.to_string_lossy().into_owned());
    }
    // Store-and-forward: transactions still waiting for the network survive a
    // gateway restart. Unlike the stats snapshot, this file is kept between runs.
    if let Some(outbox) = stats_path.as_ref().and_then(|p| p.parent()).map(|d| d.join("gateway-outbox.json")) {
        daemon_args.push("--outbox-file".into());
        daemon_args.push(outbox.to_string_lossy().into_owned());
    }

    let child = tokio::process::Command::new(&cli_path)
        .args(&daemon_args)
        // Without this the daemon outlives the app: closing the window would
        // leave an orphan holding the serial port, and the next launch could not
        // open the board at all.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!(
            "Failed to spawn '{}': {}. \
             Bundle radiodoge-cli next to the app executable, or add it to PATH.",
            cli_path, e
        ))?;

    // Spawning succeeding only means the binary launched. The daemon can still
    // exit immediately — no board on that port, no permission, port still held —
    // and reporting "Gateway Online" for a process that is already dead is worse
    // than reporting nothing. Give it a moment and check it is still alive.
    let mut child = child;
    tokio::time::sleep(Duration::from_millis(700)).await;
    match child.try_wait() {
        Ok(Some(status)) => {
            let mut msg = format!(
                "The gateway daemon exited immediately ({}). The most common cause is that \
                 {} could not be opened — check the board is attached and that nothing else \
                 is using the port.",
                status, port
            );
            // We took the port away for it; give it back rather than leaving the
            // app disconnected after a failure it did not cause.
            if holding_this_port {
                match connect_port(port.clone(), state, app).await {
                    Ok(()) => msg.push_str(" Reconnected the app to the board."),
                    Err(e) => msg.push_str(&format!(" Reconnecting the app also failed: {}", e)),
                }
            }
            return Err(msg);
        }
        Ok(None) => {}  // still running, as expected
        Err(e) => log::warn!("Could not check the gateway daemon's state: {}", e),
    }

    *state.gateway_process.lock().await = Some(child);
    *state.gateway_owned_port.lock().await = Some(port.clone());
    let _ = app.emit(
        "gateway-status",
        serde_json::json!({ "online": true, "port": port, "portReleased": holding_this_port }),
    );
    log::info!("Gateway daemon started on port {}", port);
    Ok(())
}

/// Where the gateway daemon writes its dashboard snapshot.
fn gateway_stats_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("gateway-stats.json"))
}

/// Gateway dashboard: the running daemon's latest counters.
///
/// `running` reflects the daemon process this app started; `stats` is `None`
/// until the daemon has written its first snapshot (a few seconds after start).
/// A daemon that died on its own reports `running: false` while the app still
/// shows Stop, which reclaims the port.
#[tauri::command]
async fn get_gateway_stats(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let running = {
        let mut gp = state.gateway_process.lock().await;
        match gp.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            // Exited (or unknowable). Leave the handle and the owned port alone so
            // Stop still reclaims the serial port; the dashboard says it stopped.
            _ => false,
        }
    };
    let stats = match gateway_stats_path(&app) {
        Some(p) => match tokio::fs::read(&p).await {
            Ok(bytes) => serde_json::from_slice::<radiodoge_core::gateway_stats::GatewayStats>(&bytes).ok(),
            Err(_) => None,
        },
        None => None,
    };
    Ok(serde_json::json!({ "running": running, "stats": stats }))
}

/// v0.3.21 — Multi-gateway dashboard.
///
/// Reads this app's own daemon snapshot plus every extra stats file the user
/// added (other gateways writing `--stats-file` into a shared folder), and
/// returns each source's status alongside the merged fleet summary.
#[tauri::command]
async fn get_fleet_stats(paths: Vec<String>, app: AppHandle) -> Result<serde_json::Value, String> {
    use radiodoge_core::fleet::{aggregate, is_online, FleetMember};
    use radiodoge_core::gateway_stats::GatewayStats;
    const MAX_BYTES: u64 = 4 * 1024 * 1024;

    let mut sources: Vec<(String, Option<std::path::PathBuf>)> = Vec::new();
    if let Some(p) = gateway_stats_path(&app) {
        sources.push(("This PC".into(), Some(p)));
    }
    for raw in paths.iter().take(32) {
        let t = raw.trim();
        if !t.is_empty() {
            sources.push((String::new(), Some(std::path::PathBuf::from(t))));
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut members = Vec::new();
    let mut rows = Vec::new();
    let mut used = std::collections::HashSet::new();
    for (i, (fixed, path)) in sources.into_iter().enumerate() {
        let Some(path) = path else { continue };
        let read = async {
            let meta = tokio::fs::metadata(&path).await.map_err(radiodoge_core::errors::friendly)?;
            if meta.len() > MAX_BYTES {
                return Err("file is larger than 4 MB".to_string());
            }
            let bytes = tokio::fs::read(&path).await.map_err(radiodoge_core::errors::friendly)?;
            serde_json::from_slice::<GatewayStats>(&bytes).map_err(|e| format!("not a stats file: {e}"))
        }
        .await;
        let is_local = i == 0 && fixed == "This PC";
        let base = if !fixed.is_empty() {
            fixed
        } else if let Ok(s) = &read {
            s.gateway_address.clone().unwrap_or_else(|| {
                path.file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "gateway".into())
            })
        } else {
            path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "gateway".into())
        };
        let mut label = base.clone();
        let mut n = 2;
        while !used.insert(label.clone()) {
            label = format!("{base} #{n}");
            n += 1;
        }
        match read {
            Ok(stats) => {
                rows.push(serde_json::json!({
                    "label": label, "path": path.to_string_lossy(), "local": is_local,
                    "ok": true, "online": is_online(&stats, now), "updatedAt": stats.updated_at,
                    "port": stats.port, "address": stats.gateway_address,
                    "packetsHeard": stats.packets_heard, "txBroadcast": stats.tx_broadcast,
                    "txFailed": stats.tx_failed,
                    "outbox": stats.outbox_broadcasts + stats.outbox_replies,
                }));
                members.push(FleetMember { label, stats });
            }
            // A missing local snapshot just means this PC is not running a gateway.
            Err(_) if is_local => {}
            Err(e) => rows.push(serde_json::json!({
                "label": label, "path": path.to_string_lossy(), "local": false, "ok": false, "error": e,
            })),
        }
    }
    let summary = aggregate(&members, now);
    Ok(serde_json::json!({ "now": now, "gateways": rows, "summary": summary }))
}

/// v0.3.6 — Stop the background gateway daemon.
///
/// Reconnects to the port the daemon was given, if the app was the one that
/// released it — otherwise stopping the gateway would leave the user staring at
/// a disconnected app with no indication that they need to press Connect again.
#[tauri::command]
async fn stop_gateway(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    if let Some(mut child) = state.gateway_process.lock().await.take() {
        let _ = child.kill().await;
        // The OS releases the port when the process actually exits, which is not
        // instantaneous; reopening too early fails with "access denied".
        let _ = child.wait().await;
    }
    let released = state.gateway_owned_port.lock().await.take();
    let _ = app.emit("gateway-status", serde_json::json!({ "online": false }));
    log::info!("Gateway daemon stopped");

    if let Some(port) = released {
        if !state.serial.is_connected() {
            log::info!("Reclaiming {} now the gateway daemon has exited", port);
            if let Err(e) = connect_port(port.clone(), state, app).await {
                // Not fatal: the user can press Connect. Say so rather than
                // reporting a failure to stop the gateway, which did stop.
                log::warn!("Could not reopen {} after stopping the gateway: {}", port, e);
            }
        }
    }
    Ok(())
}

/// v0.3.6 — Get or set connection type ("usb" | "ble").
#[tauri::command]
async fn get_connection_type(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.connection_type.lock().await.clone())
}

#[tauri::command]
async fn set_connection_type(
    conn_type: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if conn_type != "usb" && conn_type != "ble" {
        return Err("Connection type must be 'usb' or 'ble'".to_string());
    }
    *state.connection_type.lock().await = conn_type;
    Ok(())
}

/// v0.3.7 — Enable or disable the WiFi radio on the board (CMD_WIFI_TOGGLE 0x24).
/// Setting persisted to NVS — survives power cycles.
#[tauri::command]
async fn set_wifi_enabled(
    enable: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<bool, String> {
    if !state.serial.is_connected() {
        return Err("Not connected to board.".to_string());
    }
    let src = state.serial.get_node_address().await;
    let pkt = radio::build_wifi_toggle(&src, enable);
    let pkt_hex = hex::encode(&pkt);
    emit_debug_traffic(&app, "TX", &pkt_hex, &format!("CMD_WIFI_TOGGLE (0x24) → {}", if enable { "ON" } else { "OFF" }));
    // Wait for the board's own 0x24 reply. Without it this returned the value
    // that had been *requested* — `unwrap_or(enable)` — so a board that never
    // answered still flipped the toggle in the UI, and the user was looking at a
    // setting that had not been applied.
    let acked = state
        .serial
        .send_and_await_reply(pkt, radio::CMD_WIFI_TOGGLE, 2000)
        .await
        .map_err(radiodoge_core::errors::friendly)?;
    if !acked {
        return Err(
            "The board did not acknowledge the WiFi change, so it may not have been applied.              Nothing was changed in the app."
                .to_string(),
        );
    }

    // Re-query for the authoritative state.
    let confirm = radio::build_get_settings(&src);
    state.serial.send_raw(confirm).await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let bs = state.serial.get_board_settings().await;
    // The board acknowledged, so falling back to the requested value is now a
    // statement about something that actually happened.
    let confirmed = bs.as_ref().map(|s| s.wifi_enabled).unwrap_or(enable);
    if let Some(ref bs) = bs {
        let _ = app.emit("board-sync", bs);
    }
    Ok(confirmed)
}

/// v0.3.7 — Return recently heard mesh neighbors.
#[tauri::command]
async fn get_neighbors(state: State<'_, AppState>) -> Result<Vec<NeighborEntry>, String> {
    Ok(state.serial.get_neighbors().await)
}

/// v0.3.19 — Broadcast this node's GPS position to the mesh as `POS:lat,lon[,acc]`.
///
/// Only ever sent when the user asks. LoRa traffic is unencrypted, so the
/// position is readable by anyone in range.
#[tauri::command]
async fn share_position(
    lat: f64,
    lon: f64,
    accuracy_m: Option<f64>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let pos = parse_position(lat, lon, accuracy_m)?;
    if !state.serial.is_connected() {
        return Err("Connect to a board first".into());
    }
    let local = state.serial.get_node_address().await;
    let text = pos.to_message();
    let pkt = radio::build_broadcast(&local, &text);
    emit_debug_traffic(&app, "TX", &hex::encode(&pkt), &format!("CMD_MESSAGE → broadcast {}", pos.describe()));
    state.serial.send_raw(pkt).await.map_err(radiodoge_core::errors::friendly)?;
    Ok(text)
}

fn parse_position(lat: f64, lon: f64, accuracy_m: Option<f64>) -> Result<radiodoge_core::geo::Position, String> {
    let acc = accuracy_m
        .filter(|a| a.is_finite() && *a >= 0.0)
        .map(|a| a.round().min(u32::MAX as f64) as u32);
    radiodoge_core::geo::Position::new(lat, lon, acc)
        .ok_or_else(|| "Latitude must be −90…90 and longitude −180…180".to_string())
}

/// v0.3.20 — Build a POS: broadcast for the Android USB/BLE bridge; JS writes the bytes.
#[tauri::command]
async fn mobile_build_position(
    lat: f64,
    lon: f64,
    accuracy_m: Option<f64>,
    state: State<'_, AppState>,
) -> Result<(String, Vec<u8>), String> {
    let pos = parse_position(lat, lon, accuracy_m)?;
    let local = state.serial.get_node_address().await;
    let text = pos.to_message();
    Ok((text.clone(), radio::build_broadcast(&local, &text)))
}

/// v0.3.7 — Whether the board has reported a duplicate node address.
#[tauri::command]
async fn get_addr_conflict(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.serial.has_addr_conflict())
}

/// v0.3.7 — Dismiss the address-conflict warning (user acknowledged).
#[tauri::command]
async fn clear_addr_conflict(state: State<'_, AppState>) -> Result<(), String> {
    state.serial.clear_addr_conflict();
    Ok(())
}

// ─── v0.3.8: Battery + MAC ────────────────────────────────────────────────────

/// v0.3.8 — Query board battery voltage (CMD_GET_BATTERY 0x26).
/// Sends the command and waits up to 600 ms for the board to reply.
/// Returns voltage in millivolts, or None if not connected / no reply.
#[tauri::command]
async fn query_battery(state: State<'_, AppState>, app: AppHandle) -> Result<Option<u16>, String> {
    if !state.serial.is_connected() {
        return Ok(None);
    }
    let src = state.serial.get_node_address().await;
    let pkt = radio::build_get_battery(&src);
    let pkt_hex = hex::encode(&pkt);
    emit_debug_traffic(&app, "TX", &pkt_hex, "CMD_GET_BATTERY (0x26)");
    state.serial.send_raw(pkt).await.map_err(radiodoge_core::errors::friendly)?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    Ok(state.serial.get_battery_mv().await)
}

/// v0.3.8 — Query board MAC address (CMD_GET_MAC 0x27).
/// Returns the WiFi station MAC as a colon-separated hex string, or None if unavailable.
#[tauri::command]
async fn query_mac(state: State<'_, AppState>, app: AppHandle) -> Result<Option<String>, String> {
    // Android mobile path: MAC is cached by mobile_push_bytes on CMD_GET_MAC arrival.
    // Return the cached value immediately; the JS caller writes GET_MAC via bridge first.
    let is_mobile_connected = js_bridge_owns_port(&state).await;
    if is_mobile_connected {
        return Ok(state.serial.get_board_mac().await);
    }

    if !state.serial.is_connected() {
        return Ok(None);
    }

    // Desktop path: send CMD_GET_MAC via Rust-owned serial port and wait for response.
    let src = state.serial.get_node_address().await;
    let pkt = radio::build_get_mac(&src);
    let pkt_hex = hex::encode(&pkt);
    emit_debug_traffic(&app, "TX", &pkt_hex, "CMD_GET_MAC (0x27)");
    state.serial.send_raw(pkt).await.map_err(radiodoge_core::errors::friendly)?;
    tokio::time::sleep(Duration::from_millis(600)).await;  // raised from 400ms (too tight)
    Ok(state.serial.get_board_mac().await)
}

// ─── v0.3.8: Persistent Wallet ───────────────────────────────────────────────

/// v0.3.16 — Encrypt and persist the wallet to app-local storage.
/// The WIF private key is encrypted with ChaCha20-Poly1305 (argon2id KDF, 64 MiB).
/// Marker the frontend matches on to offer "replace it anyway".
///
/// A distinguishable prefix rather than prose, so the UI can react to this one
/// case without pattern-matching an English sentence.
pub const ERR_DIFFERENT_WALLET_SAVED: &str = "different_wallet_saved:";

/// The address of the wallet currently saved on disk, if there is one.
///
/// Read from the plaintext `address` field of the encrypted file, so it needs no
/// passphrase — the point is to know *which* wallet is stored without being able
/// to open it.
async fn saved_wallet_address(app: &AppHandle) -> Option<String> {
    let path = app.path().app_data_dir().ok()?.join("wallet.json");
    let json = tokio::fs::read_to_string(&path).await.ok()?;
    if let Ok(enc) = serde_json::from_str::<wallet::EncryptedWalletFile>(&json) {
        return Some(enc.address);
    }
    // Legacy plaintext format (pre-v0.3.16).
    serde_json::from_str::<WalletInfo>(&json).ok().map(|w| w.address)
}

#[tauri::command]
async fn save_wallet(
    wallet_info: WalletInfo,
    passphrase: String,
    allow_replace: Option<bool>,
    app: AppHandle,
) -> Result<(), String> {
    if passphrase.len() < 8 {
        return Err("Passphrase must be at least 8 characters.".to_string());
    }

    // Never silently replace a *different* wallet. There is one wallet file, and
    // overwriting it destroys the only copy of the previous key — which is
    // trivially reachable: dismiss the unlock prompt on launch, generate a new
    // wallet, save it. The UI cannot be the only thing standing between a user
    // and that, so the refusal lives here and has to be overridden explicitly.
    if !allow_replace.unwrap_or(false) {
        if let Some(existing) = saved_wallet_address(&app).await {
            if existing != wallet_info.address {
                return Err(format!("{}{}", ERR_DIFFERENT_WALLET_SAVED, existing));
            }
        }
    }
    let encrypted = wallet::encrypt_wallet(&wallet_info, &passphrase)
        .map_err(radiodoge_core::errors::friendly)?;

    // Prove the ciphertext decrypts back to this exact wallet before anything is
    // written. This file is often the user's only copy of the key, and a wallet
    // that saved "successfully" but cannot be reopened is indistinguishable from
    // losing the coins. The cost is one extra argon2 pass on save.
    match wallet::decrypt_wallet(&encrypted, &passphrase) {
        Ok(check) if check.private_key_wif == wallet_info.private_key_wif
            && check.address == wallet_info.address => {}
        Ok(_) => {
            return Err("Refusing to save: the encrypted wallet did not decrypt back to the \
                        same key. Nothing was written; your existing wallet file is untouched."
                .to_string())
        }
        Err(e) => {
            return Err(format!(
                "Refusing to save: the encrypted wallet could not be decrypted back ({}). \
                 Nothing was written; your existing wallet file is untouched.",
                e
            ))
        }
    }

    let dir = app.path().app_data_dir().map_err(radiodoge_core::errors::friendly)?;
    tokio::fs::create_dir_all(&dir).await.map_err(radiodoge_core::errors::friendly)?;
    let path = dir.join("wallet.json");
    let json = serde_json::to_string_pretty(&encrypted).map_err(radiodoge_core::errors::friendly)?;
    write_file_atomically(&path, &json).await?;
    log::info!("Encrypted wallet saved to disk.");
    Ok(())
}

/// Returns true if a wallet.json exists on disk (so the frontend can show a passphrase prompt).
#[tauri::command]
async fn wallet_needs_passphrase(app: AppHandle) -> bool {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("wallet.json"),
        Err(_) => return false,
    };
    tokio::fs::metadata(&path).await.is_ok()
}

/// v0.3.16 — Load and decrypt a wallet from disk.
/// - Encrypted format (v0.3.16+): requires a non-empty passphrase.
/// - Legacy plaintext format (pre-v0.3.16): loaded directly; the caller should prompt
///   the user to re-save with a passphrase.
/// Returns `Err("passphrase_required")` if the file is encrypted but no passphrase was supplied.
#[tauri::command]
async fn load_saved_wallet(passphrase: Option<String>, app: AppHandle) -> Result<Option<WalletInfo>, String> {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("wallet.json"),
        Err(_) => return Ok(None),
    };
    let json = match tokio::fs::read_to_string(&path).await {
        Ok(j) => j,
        Err(_) => return Ok(None),
    };
    // Try encrypted format first (v0.3.16+).
    if let Ok(enc) = serde_json::from_str::<wallet::EncryptedWalletFile>(&json) {
        let pp = match &passphrase {
            Some(p) if !p.is_empty() => p.as_str(),
            _ => return Err("passphrase_required".to_string()),
        };
        let info = wallet::decrypt_wallet(&enc, pp).map_err(radiodoge_core::errors::friendly)?;
        return Ok(Some(info));
    }
    // Legacy plaintext format (pre-v0.3.16) — load directly.
    if let Ok(w) = serde_json::from_str::<WalletInfo>(&json) {
        log::warn!("Loaded legacy plaintext wallet — user should re-save with encryption.");
        return Ok(Some(w));
    }
    Err("wallet file is corrupted or in an unrecognised format".to_string())
}

/// v0.3.8 — Delete the saved wallet from disk (user explicitly cleared it).
#[tauri::command]
async fn delete_saved_wallet(app: AppHandle) -> Result<(), String> {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("wallet.json"),
        Err(_) => return Ok(()),
    };
    let _ = tokio::fs::remove_file(&path).await;
    Ok(())
}

// ─── v0.3.8: Address Book ────────────────────────────────────────────────────

/// v0.3.8 — Load the address book from app-local storage.
/// Returns an empty array if no address book has been saved.
#[tauri::command]
async fn load_address_book(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let path = match app.path().app_data_dir() {
        Ok(p) => p.join("address_book.json"),
        Err(_) => return Ok(vec![]),
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(json) => match serde_json::from_str::<Vec<serde_json::Value>>(&json) {
            Ok(entries) => Ok(entries),
            Err(e) => {
                eprintln!("[addrbook] address_book.json parse failed ({}), starting fresh", e);
                Ok(vec![])
            }
        },
        Err(_) => Ok(vec![]),
    }
}

/// v0.3.8 — Save the address book to app-local storage.
#[tauri::command]
async fn save_address_book(entries: Vec<serde_json::Value>, app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(radiodoge_core::errors::friendly)?;
    tokio::fs::create_dir_all(&dir).await.map_err(radiodoge_core::errors::friendly)?;
    let path = dir.join("address_book.json");
    let json = serde_json::to_string_pretty(&entries).map_err(radiodoge_core::errors::friendly)?;
    write_file_atomically(&path, &json).await?;
    Ok(())
}

// ─── App Builder ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        // Core plugins
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        // v0.3.10 — Android USB serial bridge (usb-serial-for-android under the hood)
        // Also available on desktop (no-op for the primary serial path, but provides
        // the JS API surface that connection-bridge.ts calls on Android).
        .plugin(tauri_plugin_serialplugin::init())
        // v0.3.10 — Platform detection (returns "android", "windows", "linux", …)
        .plugin(tauri_plugin_os::init())
        // v0.3.10 — BLE client: btleplug on desktop, native Android BLE on mobile.
        // Exposes scan / connect / sendData / onReceiveData to the JS layer.
        .plugin(tauri_plugin_blec::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            meshtastic_bridge_listen,
            meshtastic_bridge_send_text,
            meshtastic_bridge_decode,
            // ── Desktop serial (existing) ─────────────────────────────────
            list_ports,
            list_ports_detailed,
            get_gateway_stats,
            get_fleet_stats,
            connect_port,
            disconnect_port,
            is_connected,
            get_radio_stats,
            get_node_address,
            get_firmware_version,
            query_firmware_version,
            ping_device,
            generate_wallet,
            import_wif,
            generate_mnemonic_wallet,
            import_mnemonic_wallet,
            get_balance,
            estimate_fee,
            verify_tx_inclusion,
            set_chain_backends,
            scan_qr_from_image,
            send_transaction,
            update_lora_settings,
            get_lora_settings,
            scan_plan,
            scan_channels,
            cancel_scan,
            get_board_settings,
            set_gateway_mode,
            get_history,
            start_gateway,
            stop_gateway,
            get_connection_type,
            set_connection_type,
            set_wifi_enabled,
            get_neighbors,
            share_position,
            get_addr_conflict,
            clear_addr_conflict,
            query_battery,
            query_mac,
            save_wallet,
            wallet_needs_passphrase,
            load_saved_wallet,
            delete_saved_wallet,
            load_address_book,
            save_address_book,
            // ── v0.3.10 Android USB bridge ────────────────────────────────
            // JS layer handles physical serial I/O; Rust handles framing + protocol
            mobile::mobile_set_connected,
            mobile::mobile_set_disconnected,
            mobile::mobile_clear_accumulator,
            mobile::mobile_push_bytes,
            mobile::mobile_build_ping,
            mobile::mobile_build_connect_queries,
            mobile::mobile_build_tx_packets,
            mobile::mobile_build_lora_settings_packet,
            // ── v0.3.16 Settings-tab mobile commands ──────────────────────
            mobile::mobile_build_set_gateway,
            mobile::mobile_build_wifi_toggle,
            mobile::mobile_build_ble_toggle,
            mobile::mobile_build_get_mac,
            mobile_build_position,
            mobile::set_ble_enabled,
            // ── v0.3.16 Android USB TX debug ──────────────────────────────
            mobile::mobile_emit_debug_tx,
            // ── v0.3.10 Android BLE bridge ────────────────────────────────
            // blec plugin handles GATT transport; Rust handles state + debug logging
            mobile::mobile_ble_scan,
            mobile::mobile_ble_connect,
            mobile::mobile_ble_disconnect,
            mobile::mobile_ble_write_characteristic,
        ])
        .setup(|app| {
            #[cfg(desktop)]
            tray::setup_tray(app)?;
            log::info!("RadioDoge GUI v0.3.16 started — much mesh, very wow 🐕");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Error running RadioDoge GUI")
        .run(|app, event| {
            // v0.4.2 — Kill the gateway daemon on the way out.
            //
            // The child is spawned with `kill_on_drop`, but that only fires when
            // the `Child` is actually dropped — and it lives in managed state
            // that the process never drops on exit. So closing the window left a
            // `radiodoge-cli daemon` running and holding the serial port, and the
            // next launch could not open the board at all.
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                let gateway = Arc::clone(&state.gateway_process);
                tauri::async_runtime::block_on(async move {
                    if let Some(mut child) = gateway.lock().await.take() {
                        log::info!("Stopping the gateway daemon before exit");
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                });
            }
        })
}

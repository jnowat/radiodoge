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

use radiodoge_core::{radio, wallet};
use radiodoge_core::serial::SerialManager;
use radiodoge_core::types::{
    BoardSettings, ConnectionStatusEvent, IncomingPacket, LoraSettings, NeighborEntry,
    NodeAddress, PortInfo, RadioStats, TransactionRequest, TxHistoryEntry, WalletInfo,
};

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
                raw
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
    state.serial.disconnect().await.map_err(|e| e.to_string())?;
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
    state.serial.request_firmware_version().await.map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    Ok(state.serial.get_firmware_version().await)
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
    wallet::generate_keypair().map_err(|e| e.to_string())
}

/// v0.3.6 — Import a Dogecoin wallet from a WIF-encoded private key.
/// Only compressed mainnet keys (starting with "Q") are supported.
/// ⚠️ Hot wallet warning — only use with test amounts.
#[tauri::command]
async fn import_wif(wif: String) -> Result<WalletInfo, String> {
    wallet::import_wif(&wif).map_err(|e| e.to_string())
}

/// v0.3.16 — Generate a new wallet with a 12-word BIP39 recovery phrase.
/// Returns the mnemonic alongside the derived WalletInfo.
/// The mnemonic is NOT stored — the caller must back it up before it disappears.
#[tauri::command]
async fn generate_mnemonic_wallet() -> Result<serde_json::Value, String> {
    let mnemonic = wallet::generate_mnemonic().map_err(|e| e.to_string())?;
    let phrase = mnemonic.to_string();
    let info = wallet::wallet_from_mnemonic(&phrase).map_err(|e| e.to_string())?;
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
    wallet::wallet_from_mnemonic(&phrase).map_err(|e| e.to_string())
}

/// v0.3.16 — Query the confirmed Dogecoin balance for an address via Trezor Blockbook.
/// Returns the balance as a floating-point DOGE amount.
#[tauri::command]
async fn get_balance(address: String) -> Result<f64, String> {
    if !wallet::is_valid_address(&address) {
        return Err("Invalid Dogecoin address".to_string());
    }
    let koinus = wallet::fetch_balance_blockbook(&address)
        .await
        .map_err(|e| e.to_string())?;
    Ok(koinus as f64 / 1e8)
}

/// Lightweight SPV inclusion check for a txid (confirmations + block) via Blockbook.
#[tauri::command]
async fn verify_tx_inclusion(txid: String) -> Result<radiodoge_core::spv::TxInclusion, String> {
    radiodoge_core::spv::fetch_tx_inclusion(&txid)
        .await
        .map_err(|e| e.to_string())
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
) -> Result<String, String> {
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
        wallet::build_signed_transaction(
            wif,
            &tx.to_address,
            tx.amount_doge,
            wallet::DEFAULT_TX_FEE_DOGE,
        )
        .await
        .map_err(|e| format!("Transaction signing failed: {}", e))?
    } else {
        wallet::encode_transaction_payload(
            &tx.to_address,
            tx.amount_doge,
            tx.memo.as_deref(),
        )
        .map_err(|e| e.to_string())?
    };

    // How the payload is framed depends on what the board's firmware can
    // receive: one packet on older builds, a multipart sequence on v0.4.2+.
    // `build_tx_frames` refuses anything the board would mis-frame rather than
    // transmitting a transaction no receiver can reconstruct.
    let src = state.serial.get_node_address().await;
    let dst = NodeAddress::broadcast();
    let signed_label = tx.from_private_key_wif.is_some();

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
        let app_for_progress = app.clone();
        state
            .serial
            .send_frames_with_progress(frames, move |sent, total| {
                if total > 1 {
                    let _ = app_for_progress.emit(
                        "transaction-progress",
                        serde_json::json!({ "sent": sent, "total": total }),
                    );
                }
            })
            .await
            .map_err(|e| e.to_string())?;
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
    Ok(msg)
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
    state.serial.send_raw(pkt).await.map_err(|e| e.to_string())?;

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
        .map_err(|e| e.to_string())?;

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
    state.serial.send_raw(pkt).await.map_err(|e| e.to_string())?;
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
    state.serial.send_raw(pkt).await.map_err(|e| e.to_string())?;
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
        state.serial.disconnect().await.map_err(|e| e.to_string())?;
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

    let child = tokio::process::Command::new(&cli_path)
        .args(["daemon", "-p", &port])
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
        .map_err(|e| e.to_string())?;
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
    state.serial.send_raw(pkt).await.map_err(|e| e.to_string())?;
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
    state.serial.send_raw(pkt).await.map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?;

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

    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let path = dir.join("wallet.json");
    let json = serde_json::to_string_pretty(&encrypted).map_err(|e| e.to_string())?;
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
        let info = wallet::decrypt_wallet(&enc, pp).map_err(|e| e.to_string())?;
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
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let path = dir.join("address_book.json");
    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    write_file_atomically(&path, &json).await?;
    Ok(())
}

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
async fn mobile_set_connected(
    device_path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = Some(device_path.clone());
    *state.connection_type.lock().await = "usb-android".to_string();
    // Clear any stale firmware version from a previous session
    *state.mobile_fw_version.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    let _ = app.emit("connection-status", ConnectionStatusEvent::connecting(&device_path));
    log::info!("mobile_set_connected: {}", device_path);
    Ok(())
}

/// Notify the Rust backend that the Android USB connection has been closed.
/// Clears state and emits "connection-status: disconnected".
#[tauri::command]
async fn mobile_set_disconnected(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = None;
    *state.mobile_fw_version.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    *state.mobile_packets_rx.lock().await = 0;
    *state.mobile_packets_tx.lock().await = 0;
    *state.mobile_last_rssi.lock().await = 0;
    let _ = app.emit("connection-status", ConnectionStatusEvent::disconnected());
    log::info!("mobile_set_disconnected");
    Ok(())
}

/// Discard all buffered bytes in the Android accumulator (called on disconnect).
#[tauri::command]
async fn mobile_clear_accumulator(state: State<'_, AppState>) -> Result<(), String> {
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
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
async fn mobile_push_bytes(
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
    const MOBILE_ACCUMULATOR_CAP: usize = 8 * 1024;
    let mut acc = state.mobile_accumulator.lock().await;
    if acc.len() + bytes.len() > MOBILE_ACCUMULATOR_CAP {
        eprintln!("[mobile_push_bytes] accumulator overflow ({} bytes) — discarding stale data", acc.len());
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
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let ingested = {
            let mut reasm = state.mobile_reassembler.lock().await;
            radio::ingest_packet(&acc[..packet_len], rssi, &mut reasm, now_secs)
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
                        let gw   = payload[3] != 0;
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
                        let port = state.current_port.lock().await.clone()
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
                let body = packet.decoded.clone()
                    .unwrap_or_else(|| "Incoming Dogecoin transaction over LoRa".to_string());
                let _ = app.emit(
                    "mobile-doge-tx-received",
                    serde_json::json!({ "body": body }),
                );
            }

            radio::CMD_ADDR_CONFLICT => {
                let addr_str = packet.decoded.clone().unwrap_or_else(|| "unknown".to_string());
                let _ = app.emit("addr-conflict", serde_json::json!({
                    "detected": true,
                    "address": addr_str,
                }));
            }

            radio::CMD_GET_MAC => {
                // Cache the board MAC so query_mac() works on Android (same as desktop
                // serial.rs read loop).  Emit "mobile-board-mac" so SettingsTab can
                // update the UI without polling.
                if let Ok(payload) = hex::decode(&packet.payload_hex) {
                    if payload.len() >= 6 {
                        let mac = format!(
                            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                            payload[0], payload[1], payload[2],
                            payload[3], payload[4], payload[5]
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
            snr: None, // SNR not available over USB serial or BLE
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
async fn mobile_build_ping(state: State<'_, AppState>) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_ping(&src, &src)) // self-addressed, same as desktop serial::ping()
}

/// Build a SET_GATEWAY packet for the Android USB bridge.
/// The JS layer writes the bytes to USB/BLE; the board responds via CMD_GET_SETTINGS
/// board-sync which updates gateway_mode in the connection store.
#[tauri::command]
async fn mobile_build_set_gateway(enable: bool, state: State<'_, AppState>) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_set_gateway(&src, enable))
}

/// Build a WIFI_TOGGLE packet for the Android USB bridge.
#[tauri::command]
async fn mobile_build_wifi_toggle(enable: bool, state: State<'_, AppState>) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_wifi_toggle(&src, enable))
}

/// Build a BLE_TOGGLE packet for the Android USB bridge.
/// Enables or disables BLE advertising on the board (persisted to NVS).
/// Requires firmware v0.3.16+.
#[tauri::command]
async fn mobile_build_ble_toggle(enable: bool, state: State<'_, AppState>) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_ble_toggle(&src, enable))
}

/// Build a GET_MAC packet for the Android USB bridge.
/// After writing, the board sends CMD_GET_MAC (0x27) response;
/// mobile_push_bytes processes it and emits "mobile-board-mac".
#[tauri::command]
async fn mobile_build_get_mac(state: State<'_, AppState>) -> Result<Vec<u8>, String> {
    let src = state.serial.get_node_address().await;
    Ok(radio::build_get_mac(&src))
}

/// v0.3.16 — Enable/disable BLE advertising on the board (desktop path).
/// Mirrors set_wifi_enabled but uses CMD_BLE_TOGGLE (0x28).
/// Requires board firmware v0.3.16+.
#[tauri::command]
async fn set_ble_enabled(
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
    emit_debug_traffic(&app, "TX", &pkt_hex, &format!("CMD_BLE_TOGGLE (0x28) → {}", if enable { "ON" } else { "OFF" }));
    // The board answers 0x28 with a 9-byte echo carrying the state it applied.
    // This used to return `enable` without looking, so the UI reported a toggle
    // the board may never have received.
    let acked = state
        .serial
        .send_and_await_reply(pkt, radio::CMD_BLE_TOGGLE, 2000)
        .await
        .map_err(|e| e.to_string())?;
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
async fn mobile_build_connect_queries(state: State<'_, AppState>) -> Result<Vec<Vec<u8>>, String> {
    let src = state.serial.get_node_address().await;
    // Opening the window here rather than when the bytes are written: the JS
    // layer writes these immediately, and a few seconds covers BLE's slower
    // round trip. Outside it, a "version reply" is somebody else's LoRa packet.
    state
        .mobile_fw_query_until_ms
        .store(now_millis() + MOBILE_FIRMWARE_REPLY_WINDOW_MS, Ordering::Relaxed);
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
async fn mobile_build_tx_packets(
    tx: TransactionRequest,
    state: State<'_, AppState>,
) -> Result<Vec<Vec<u8>>, String> {
    let src = state.serial.get_node_address().await;
    let dst = NodeAddress::broadcast();

    let payload = if let Some(ref wif) = tx.from_private_key_wif {
        wallet::build_signed_transaction(
            wif,
            &tx.to_address,
            tx.amount_doge,
            wallet::DEFAULT_TX_FEE_DOGE,
        )
        .await
        .map_err(|e| format!("Transaction signing failed: {}", e))?
    } else {
        wallet::encode_transaction_payload(
            &tx.to_address,
            tx.amount_doge,
            tx.memo.as_deref(),
        )
        .map_err(|e| e.to_string())?
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
async fn mobile_build_lora_settings_packet(settings: LoraSettings) -> Vec<u8> {
    let bw_idx: u8 = match settings.bandwidth_khz as u32 {
        250 => 1,
        500 => 2,
        _   => 0, // default 125 kHz
    };
    let cr: u8 = match settings.coding_rate.as_str() {
        "4/6" => 6,
        "4/7" => 7,
        "4/8" => 8,
        _     => 5, // default 4/5
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
async fn mobile_ble_scan(
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
async fn mobile_ble_connect(
    address: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = Some(address.clone());
    *state.connection_type.lock().await = "ble-android".to_string();
    *state.ble_device_address.lock().await = Some(address.clone());
    *state.mobile_fw_version.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
    let _ = app.emit("connection-status", ConnectionStatusEvent::connecting(&address));
    log::info!("mobile_ble_connect: {}", address);
    Ok(())
}

/// Notify Rust that the BLE connection has been closed.
///
/// Mirrors mobile_set_disconnected but for the BLE transport:
/// clears ble_device_address, current_port, mobile_fw_version, accumulator,
/// and emits "connection-status: disconnected".
#[tauri::command]
async fn mobile_ble_disconnect(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    *state.current_port.lock().await = None;
    *state.ble_device_address.lock().await = None;
    *state.mobile_fw_version.lock().await = None;
    state.mobile_accumulator.lock().await.clear();
    // Drop any half-assembled multipart sequences too — fragments from a
    // previous session must never be stitched onto the next connection's.
    *state.mobile_reassembler.lock().await = radio::MultipartReassembler::new();
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
async fn mobile_emit_debug_tx(bytes: Vec<u8>, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let hex = hex::encode(&bytes);
    let note = bytes.first().map(|cmd| match *cmd {
        0x02 => "CMD_PING",
        0x10 => "CMD_DOGE_TX",
        0x20 => "CMD_GET_FIRMWARE_VERSION",
        0x22 => "CMD_GET_SETTINGS",
        0x23 => "CMD_SET_GATEWAY",
        0x24 => "CMD_WIFI_TOGGLE",
        0x26 => "CMD_GET_BATTERY",
        0x27 => "CMD_GET_MAC",
        0x28 => "CMD_BLE_TOGGLE",
        _    => "CMD_?",
    }).unwrap_or("(empty)");
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
async fn mobile_ble_write_characteristic(
    data: Vec<u8>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let hex = hex::encode(&data);
    emit_debug_traffic(&app, "TX-BLE", &hex, "BLE write characteristic");
    *state.mobile_packets_tx.lock().await += 1;
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
            // ── Desktop serial (existing) ─────────────────────────────────
            list_ports,
            list_ports_detailed,
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
            verify_tx_inclusion,
            scan_qr_from_image,
            send_transaction,
            update_lora_settings,
            get_lora_settings,
            get_board_settings,
            set_gateway_mode,
            get_history,
            start_gateway,
            stop_gateway,
            get_connection_type,
            set_connection_type,
            set_wifi_enabled,
            get_neighbors,
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
            mobile_set_connected,
            mobile_set_disconnected,
            mobile_clear_accumulator,
            mobile_push_bytes,
            mobile_build_ping,
            mobile_build_connect_queries,
            mobile_build_tx_packets,
            mobile_build_lora_settings_packet,
            // ── v0.3.16 Settings-tab mobile commands ──────────────────────
            mobile_build_set_gateway,
            mobile_build_wifi_toggle,
            mobile_build_ble_toggle,
            mobile_build_get_mac,
            set_ble_enabled,
            // ── v0.3.16 Android USB TX debug ──────────────────────────────
            mobile_emit_debug_tx,
            // ── v0.3.10 Android BLE bridge ────────────────────────────────
            // blec plugin handles GATT transport; Rust handles state + debug logging
            mobile_ble_scan,
            mobile_ble_connect,
            mobile_ble_disconnect,
            mobile_ble_write_characteristic,
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

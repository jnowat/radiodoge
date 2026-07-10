<div align="center">

# 🗺️ RadioDoge Roadmap

**Much ambition. Very phases. Such plan. Wow.** 🐕

*Current release: **v0.3.16** (app) · **v0.3.8** (Heltec V3 firmware)*

[← Back to the README](README.md) · [User Manual](docs/USER_MANUAL.md) · [Changelog](CHANGELOG.md)

</div>

---

This roadmap tracks what RadioDoge can do today and where it's headed. Legend:
✅ shipped · 🔨 in progress · 🔜 planned.

---

## ✅ v0.2.x — Windows Foundation

The bedrock: a real, installable desktop app.

- ✅ Tauri 2 + Svelte 5 GUI with a full Dogecoin yellow/orange theme
- ✅ Pure-Rust Dogecoin wallet (`secp256k1` + `sha2` + `ripemd` + `bs58`) — genuine mainnet `D…` addresses, no FFI
- ✅ Real serial comms with the Heltec ESP32 at 115,200 baud (`serialport` crate)
- ✅ Binary packet protocol: `PING`, `GET_ADDR`, `SET_ADDR`, `DOGE_TX`, `MULTIPART`
- ✅ Async Tokio I/O — blocking reads run on `spawn_blocking`, never stalling the runtime
- ✅ Multipart packets for payloads > 192 bytes
- ✅ Live RSSI/SNR polling + 5-bar signal widget, refreshed every 2 s
- ✅ Live packet monitor with decoded payloads
- ✅ Windows x64 MSI built by GitHub Actions on every push
- ✅ Cargo workspace: `radiodoge-core`, `radiodoge-cli`, and the GUI backend
- ✅ `radiodoge-cli` — headless CLI (ports, wallet, send, receive, ping, connect, balance, broadcast, daemon)

---

## ✅ v0.3.x — Heltec Polish, Real-World UX & Android

Rock-solid against real hardware, and now on your phone.

### Connection & hardware UX
- ✅ Doge-boombox artwork throughout the app and installers
- ✅ Smart USB port detection — CP210x, CH340, CH9102, FTDI, and Espressif native flagged `🟢 Heltec`
- ✅ Ports sorted likely-Heltec first, last-used port remembered, auto-selected on refresh
- ✅ **Ping Device** button with millisecond round-trip readout
- ✅ Actionable connection errors (access-denied, driver-missing, cable-failure) with driver links
- ✅ **Firmware version badge** — queried on connect
- ✅ **Auto-reconnect** — detects USB re-plug and retries with exponential backoff (5 → 10 → 20 → 40 → 60 s)
- ✅ **Save-to-Device round-trip verification** — writes settings, reads them back, shows `Verified ✓`
- ✅ **Board-is-source-of-truth** — node address, gateway mode, and WiFi state are read back from the board's
  NVS on every connect, reconnect, and toggle

### Wallet & transactions
- ✅ **Full P2PKH signing** — UTXO fetch → largest-first coin selection → secp256k1 `SIGHASH_ALL` → gateway
  broadcast via Trezor Blockbook, with a `TX_ACK` reply over LoRa
- ✅ **Wallet encryption at rest** — argon2id (64 MiB) + ChaCha20-Poly1305; passphrase modals on save & unlock;
  legacy plaintext wallets auto-detected with a re-encrypt nudge
- ✅ **Incoming TX verification** — secp256k1 ECDSA verified in-process; ✅/⚠️ label in every packet view
- ✅ **Balance query** — direct Blockbook query, plus a `REQUEST_BALANCE` gateway path that relays a balance
  back over LoRa
- ✅ **BIP32/BIP44 HD wallet** — 12-word BIP39 mnemonic, derivation at `m/44'/3'/0'/0/0`, recovery-phrase backup
  modal, and mnemonic/WIF import
- ✅ **Address book**, **transaction history** (last 50), and **QR display** for your address

### Interface polish
- ✅ Copy-to-clipboard on every address, key, and packet field with inline ✅ feedback
- ✅ TX/RX direction icons + filter toggles; dynamic signal descriptions ("Much Strong Radio!")
- ✅ Tooltips everywhere; ARIA labels, focus rings, and live regions for accessibility
- ✅ Light/dark theme toggle; system-tray icon with connection status (desktop)
- ✅ Desktop OS notification on incoming DOGE transactions
- ✅ Hidden **Konami-code Easter egg** 🎮 (↑ ↑ ↓ ↓ ← → ← → b a)
- ✅ **Mesh neighbor map** with address-conflict detection; **battery** and **board MAC** readouts

### Android & gateway
- ✅ **Android app** — Tauri Mobile port; debug APK built by CI on every push; mobile-responsive, icon-only navbar
- ✅ **Android USB-C serial** — USB-OTG CP2102/CH340 via `tauri-plugin-serialplugin`; identical protocol to desktop
- ✅ **Android Bluetooth LE (preview)** — GATT scan → connect → notify via `tauri-plugin-blec`, feeding the same
  Rust packet framer as USB; Nordic UART Service UUIDs aligned with the firmware
- ✅ **Gateway mode** toggle + **gateway daemon** — the app can spawn `radiodoge-cli daemon` and shows its status
- ✅ **Debug Console** (Ctrl+Shift+D) — raw hex traffic with parsed command labels and export

---

## 🔨 v0.4.x — Mesh Reliability & Firmware Parity *(in progress)*

- ✅ **Broadcast deduplication** — FNV-1a hash + source address, 20-entry table, 2-minute TTL, to suppress
  mesh-storm re-delivery
- ✅ **Host ACK/Ping notifications** — the board now tells the host over serial when it hears an over-the-air ACK
  (`0x29`) or Ping (`0x2A`)
- ✅ **Supply-chain CI** — weekly `cargo-audit` fails the build on any known CVE in a pinned dependency
- 🔨 **Gateway radio→host forwarding** — let a gateway Heltec hand incoming LoRa packets to its serial host so the
  full LoRa → firmware → `daemon` → network path is end-to-end
- 🔨 **Firmware command reachability** — wire the implemented WiFi-toggle / battery / MAC handlers into the desktop
  command dispatch (see limitations below)
- 🔜 **SPV verification** — lightweight header-chain validation so the app can verify inclusion without a full node
- 🔜 **Multi-hop relay status** — show hop count and intermediate node addresses in the packet log
- 🔜 **QR code scanning** — camera/image input for the recipient field
- 🔜 **Fee estimation** — gateway reports the current mempool fee rate; the app sets an appropriate sat/byte fee

---

## 📡 v0.5.x — Meshtastic Interoperability

Bridge RadioDoge with the existing Meshtastic community.

- 🔜 **Meshtastic packet encoding** — wrap RadioDoge payloads in Meshtastic protobuf so standard nodes can relay them
- 🔜 **Node discovery** — scan for nodes on the same channel and render a network map
- 🔜 **Bridge mode** — `radiodoge-cli daemon` transparently bridges RadioDoge ↔ Meshtastic
- 🔜 **Multi-channel scanning** — hop across frequencies/SFs to reach different Meshtastic presets
- 🔜 **GPS location embedding** — sender coordinates in packets for disaster-response positioning
- 🔜 **Store-and-forward** — cache undelivered packets and retry when the destination node is next heard

---

## 📱 Future Horizons

- 🔜 **iOS** — Bluetooth LE to the Heltec via the BLE-serial bridge
- 🔜 **Linux AppImage + macOS .dmg** in CI — Tauri already supports these targets
- 🔜 **WebAssembly packet inspector** — a browser tool to decode RadioDoge packets from hex
- 🔜 **Gateway dashboard** — monitor multiple gateways, relay stats, and mesh health at a glance
- 🔜 **Lightning over LoRa** — forward BOLT-11 invoices and HTLC state through the mesh

---

## 🧭 Known Limitations & In Progress

Honesty keeps the mesh healthy. These are real gaps in the current build, each already on a roadmap line above:

- **BLE is notify-oriented in firmware.** The app can scan, connect, and receive board notifications over
  Bluetooth LE, and the app's BLE write path is wired end-to-end — but the firmware currently buffers inbound
  BLE writes without consuming them, so **commands sent over BLE are not yet executed on the board**. Use
  **USB-C** as the reliable transport today; BLE is a preview. *(Tracked under v0.4.x firmware parity.)*
- **Three desktop commands aren't reachable in firmware yet.** `WIFI_TOGGLE` (`0x24`), `GET_BATTERY` (`0x26`),
  and `GET_MAC` (`0x27`) have handlers in the firmware but aren't wired into its desktop-command dispatch, so
  the board NACKs them. The app's WiFi toggle, battery gauge, and MAC readout depend on a firmware update.
- **`SET_LORA_PARAMS` (`0x21`) is a no-op ACK.** LoRa frequency/SF/bandwidth are compile-time constants in the
  firmware; the app can send new parameters but the radio isn't reconfigured at runtime yet.
- **No hop limit on mesh rebroadcast.** Rebroadcast is bounded only by the 2-minute dedup table, not by a hop
  count. Multi-hop relay status (v0.4.x) will add visibility here.
- **`gateway_mode` is a reporting flag, not a forwarding switch.** Actual forwarding is decided by the board's
  configured gateway type / IP / internet state, independent of the `gateway_mode` toggle.
- **The firmware web UI is unauthenticated.** Every `/api/*` route is open to anyone on the board's WiFi AP, and
  a couple of status endpoints return stored credentials in plaintext. Treat a firmware gateway as trusted-network
  only until authentication lands.
- **`radiodoge-cli --version` reports a stale `0.2.4`** (a hard-coded clap string); the crate is at `0.3.16`.

Found something else? [Open an issue](https://github.com/jnowat/RadioDoge/issues) — much appreciated. 🐕

---

<div align="center">

*Such roadmap. Very direction. Much wow.* 🐕🌙

</div>

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

- ✅ **LoRa transmissions actually complete (v0.4.2)** — `Radio.Send` only *starts* a transmission, but the
  desktop command handlers marked the radio idle immediately afterwards, so the main loop switched it back to
  receive a couple of milliseconds into a packet that needed hundreds. Every send now goes through
  `SendLoRaAndWait`, which blocks until `TxDone`; the fixed inter-part delays in the firmware's own multipart
  senders (100 ms and 500 ms, both shorter than the airtime they were pacing) are gone with it
- ✅ **Exact host→board framing (v0.4.2)** — the board read a desktop command by sleeping 500 ms and draining
  its serial buffer, so two commands sent close together merged and the second was lost, and a multipart
  sequence arrived as one unparseable blob. Frames are now read to an exact length wherever the protocol
  defines one, and only genuinely unbounded payloads fall back to a gap in the byte stream
- ✅ **Gateway-side stream alignment (v0.4.2)** — the board's legacy 3-byte ACK/NACK result code and its
  duplicate-address warning were framed by the host as 8-byte packets, so each one corrupted the packet behind
  it. A gateway emits a result code every time it relays a `TX_ACK`, so this was on the money path
- ✅ **Broadcast deduplication** — FNV-1a hash + source address, 20-entry table, 2-minute TTL, to suppress
  mesh-storm re-delivery
- ✅ **Host ACK/Ping notifications** — the board now tells the host over serial when it hears an over-the-air ACK
  (`0x29`) or Ping (`0x2A`)
- ✅ **Supply-chain CI** — weekly `cargo-audit` fails the build on any known CVE in a pinned dependency
- ✅ **Gateway radio→host forwarding** — a gateway Heltec now hands incoming LoRa packets to its serial host, so the
  full LoRa → firmware → `daemon` → network path is end-to-end. Transactions/balance requests are relayed over the
  air in the desktop packet format, forwarded to the host in gateway mode, and the daemon's `TX_ACK`/`BAL` reply is
  relayed back over LoRa to the originating node.
- ✅ **Firmware command reachability** — the implemented WiFi-toggle (`0x24`), battery (`0x26`), and MAC (`0x27`)
  handlers are now wired into the firmware's desktop-command dispatch, and `BLE_TOGGLE` (`0x28`) is implemented
- ✅ **SPV verification** — a `radiodoge-core::spv` module with block-header parsing, `SHA256d` hashing, `nBits`
  target math, header-chain linkage validation, and merkle-proof verification (all offline unit-tested), plus a
  Blockbook-backed lightweight inclusion check surfaced as `radiodoge-cli verify-tx`, a `verify_tx_inclusion`
  Tauri command, and a "Verify a transaction on-chain" panel in the History tab
- ✅ **Multi-hop relay status** — packets now carry a mesh hop count (header flags upper nibble; the firmware's
  multipart `reserved` byte on the air), surfaced as a "⇄ N hops" badge in the packet log, in the CLI receive /
  daemon output, and in exported logs. Firmware mesh rebroadcast increments the count and enforces
  `MAX_REBROADCAST_HOPS`, so relays are bounded by hop count, not just the dedup table
- ✅ **QR code scanning** — a "📷 Scan QR" button on the Send tab decodes a Dogecoin QR from an image file (or
  camera capture on mobile) via a pure-Rust `rqrr` backend command, and a `radiodoge-core::qr` parser fills the
  recipient — plus amount and memo — from a bare address or a BIP21 `dogecoin:…?amount=…&label=…` URI
- ✅ **Host → board multipart framing** — the gap that kept most real transactions off the air. Firmware v0.4.2
  reads each frame to an exact length instead of draining its serial buffer, multipart frames carry a chunk
  length so they are self-delimiting on a byte stream, and transmissions wait for `TxDone` instead of being
  aborted by the main loop. The host splits a payload over 192 bytes into frames and sends them one at a time,
  waiting for the board to acknowledge each, gated on the board reporting `FW11` or newer
- 🔜 **Fee estimation** — gateway reports the current mempool fee rate; the app sets an appropriate sat/byte fee.
  Today the fee is a flat 1 DOGE regardless of transaction size

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

- **🟡 App-format packets are not relayed hop to hop.**
  A board that hears a `DOGE_TX` addressed elsewhere hands it to its own serial host but does not retransmit
  it, so the sender and the gateway must be within direct radio range of each other. The hop count in the
  header flags is carried and reported end to end, and the firmware's own broadcast format does rebroadcast
  with a hop limit and a dedup table — but the desktop packet format has neither a relay path nor the dedup
  a relay path would need, and adding one without both is how mesh storms start.
- **🔴 An incoming transaction display is not proof of payment.**
  Nothing on the LoRa link is authenticated. When a `DOGE_TX` packet arrives, the app checks that its
  signatures are consistent with the public keys inside it — but the UTXO those signatures are checked against
  is reconstructed from those same public keys, because that check has no network access. It therefore says
  nothing about whether the inputs exist, are unspent, or belong to the sender. **Anyone within radio range can
  build a transaction paying you any amount, sign it with a key they generated a second earlier, and send it
  for the cost of one packet.** The app says so in the packet description now rather than showing a checkmark,
  but the only real answer is the chain: use `radiodoge-cli verify-tx <txid>` or the History tab's inclusion
  check before treating anything as received.
- **🟡 The web/REST interface has no authentication, by design.**
  Any device that can reach the board's web server can call every `/api/*` route — and in dual-WiFi mode that
  includes hosts on the upstream LAN, not only devices joined to the board's own access point. Treat a firmware
  gateway as a device on a trusted network. `/proxy` is now gated on the internet bridge being explicitly
  enabled and capped at 32 KB per response, but it remains a forward proxy for whoever can reach it.
- **🟡 A LoRa frame lost in the air is not retransmitted.**
  A multipart transaction is several independent LoRa transmissions and boards do not acknowledge each other's
  data packets. If one part is lost, the gateway's reassembler discards the session after 30 s
  (`MULTIPART_SESSION_TIMEOUT_SECS`) and the transaction is never broadcast — the sender simply never sees a
  `TX_ACK` and has to send again. The host *does* retransmit a frame the **board** fails to acknowledge over
  serial, and a duplicate is harmless (the reassembler is keyed by `(source, session id)` and ignores a part it
  already holds; the gateway daemon remembers txids it has broadcast and re-acknowledges rather than
  re-broadcasting), so per-part over-the-air acknowledgement is a natural next step rather than a rewrite.
  The larger a transaction is, the more frames it needs and the likelier this becomes — prefer a wallet with
  few UTXOs, or broadcast over the internet when the link is marginal.
- **🟡 The v0.4.2 firmware has not been validated on hardware by the authors.**
  The host↔board contract it implements is tested end to end in software — `crates/radiodoge-core/tests/`
  models both boards' framing byte for byte and drives a transaction through the whole path — but the sketch
  itself has not been flashed to a board and exercised over the air here. The changes it contains are
  substantial: exact-length host framing, blocking LoRa transmission, and the multipart chunk-length byte. If
  you have two boards, this is the single most valuable thing to confirm.
- ~~**BLE is notify-oriented in firmware.**~~ *Fixed in v0.4.1 firmware (needs hardware validation):* inbound
  BLE writes accumulated in `bleRxBuffer` and were never read, so the app could connect and receive
  notifications but every command it sent over Bluetooth was silently ignored. `HandleDesktopCommand` no longer
  reads `Serial` itself — its bytes are passed in — so the USB and BLE paths share one implementation of every
  command instead of growing a second copy. `ProcessBleCommands()` drains the buffer each loop and dispatches
  through it. Because BLE has no equivalent of the serial path's inter-packet delay and a packet can be split
  across GATT writes, a packet is executed once its fixed length has arrived, or — for the variable-length
  `DOGE_TX`/`REQUEST_BALANCE` — after a 60 ms quiet gap. Replies already went to both transports, so a command
  sent over either link is answered on both. **USB-C remains the better-tested transport until this has hardware
  validation.**
- ~~**Three desktop commands aren't reachable in firmware yet.**~~ *Fixed in v0.4.0 firmware:* `WIFI_TOGGLE`
  (`0x24`), `GET_BATTERY` (`0x26`), and `GET_MAC` (`0x27`) are now routed to their handlers, and `BLE_TOGGLE`
  (`0x28`) toggles BLE advertising and persists to NVS. Flash the updated Heltec V3 firmware to use these.
- ~~**`SET_LORA_PARAMS` (`0x21`) is a no-op ACK.**~~ *Fixed in v0.4.1 firmware (needs hardware validation):*
  the radio parameters are now runtime variables applied via `SetChannel`/`SetTxConfig`/`SetRxConfig` and
  persisted to NVS, so a retuned board comes back retuned. Values are validated first (SF 7–12, BW 0–2,
  CR 1–4, 150–960 MHz, 2–22 dBm) and out-of-range requests are NACKed with the radio left untouched, so a bad
  setting can't strand the board off-channel.
  While wiring this up, the **frequency field turned out to be too narrow to carry 915 MHz**: it was two bytes
  for a 20-bit value, so 915000 kHz was transmitted as 63032 kHz. It is now a big-endian `u32` in bytes `[3..7]`
  with TX power moved to `[7]` — a safe wire change, since no released firmware ever read those bytes.
- ~~**No hop limit on mesh rebroadcast.**~~ *Fixed in v0.4.0:* multipart rebroadcasts carry a hop count in the
  `reserved` byte and are dropped once they reach `MAX_REBROADCAST_HOPS` (3), on top of the 2-minute dedup table.
- ~~**`gateway_mode` is a reporting flag, not a forwarding switch.**~~ *Fixed in v0.4.1 firmware (needs
  hardware validation):* the transaction and broadcast forwarders checked only whether a gateway was
  *configured* (`gateway_type` + `gateway_ip`) and never consulted the toggle, so switching gateway mode off in
  the app did not stop the board pushing other nodes' transactions to the internet. All four relay paths now go
  through `gatewayForwardingEnabled()`, making the toggle authoritative. Saving a gateway from the web UI
  enables the mode, so a board configured entirely through the web interface keeps forwarding as before. The
  web UI's *own* "send transaction" endpoint is deliberately not gated — that is the operator acting directly,
  not the board relaying someone else's traffic.
- **The firmware web UI is unauthenticated.** All 40 `/api/*` routes are open to anyone on the board's WiFi AP.
  Treat a firmware gateway as trusted-network only until authentication lands.
  - ✅ *The credential disclosure is fixed (v0.4.1, needs hardware validation).* `GET /api/password/status`
    returned the AP password and `GET /api/gateway/load` returned the stored gateway RPC password, both in
    plaintext to any unauthenticated caller. Neither is returned now: the password endpoint reports only
    `is_default` and `length`, and the gateway endpoint reports `has_password`. The RPC password is write-only —
    the web UI leaves the field blank with a placeholder showing whether one is saved, and submitting it blank
    keeps the stored value rather than erasing it.
  - 🔜 *Route authentication itself is still open.* The remaining work is a guard on all 40 handlers; it is not
    done here because it cannot be compile- or hardware-tested in the review environment, and a mistake locks
    the operator out of their own board.
- ~~**`radiodoge-cli --version` reports a stale `0.2.4`.**~~ *Fixed:* the CLI now derives its version from the
  crate version via clap's `version` attribute, so it always matches the package.
- ~~**`cargo build --workspace` failed on a fresh clone.**~~ *Fixed:* `tauri.conf.json` declared an
  `externalBin` sidecar whose binary is gitignored and only produced by CI, so every clean build died with
  `resource path binaries/radiodoge-cli-<triple> doesn't exist`. The sidecar moved to an opt-in
  `tauri.sidecar.conf.json` overlay that release CI merges in with `--config`.
- ~~**Partial serial reads emitted truncated packets.**~~ *Fixed:* both framing loops clamped a packet's length
  to the bytes available, so a 13-byte `GET_SETTINGS` split across two reads was parsed as a 9-byte packet and
  its tail misread as a new one. Framing now waits for the full packet (`radio::frame_packet_len`), shared by
  the desktop and Android paths.
- ~~**Android discarded the board's ACK/Ping notifications.**~~ *Fixed:* the mobile bridge's known-command table
  had drifted from the desktop one and was missing `0x29`/`0x2A`, so those packets were dropped as noise and
  resynced byte-by-byte. Both paths now share `radio::is_known_command`.
- ~~**`ping()` could report a false timeout.**~~ *Fixed:* it subscribed to the packet channel *after* sending,
  losing the reply whenever the board answered before the task was rescheduled.
- ~~**The declared MSRV was unbuildable.**~~ *Fixed:* the manifests claimed Rust 1.77.2 while `image`, `time`,
  and `darling` require 1.88. All three crates now declare **1.88**, which is the real minimum.

Found something else? [Open an issue](https://github.com/jnowat/RadioDoge/issues) — much appreciated. 🐕

---

<div align="center">

*Such roadmap. Very direction. Much wow.* 🐕🌙

</div>

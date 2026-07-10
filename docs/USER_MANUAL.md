<div align="center">

# 📖 RadioDoge User Manual

**Everything you can do with the app — one tab at a time.**

*App v0.3.16 · Heltec V3 firmware v0.3.8*

[← README](../README.md) · [Roadmap](../ROADMAP.md) · [Protocol reference](PROTOCOL.md)

</div>

---

## Contents

1. [Before you start](#1-before-you-start)
2. [Installing](#2-installing)
3. [The interface at a glance](#3-the-interface-at-a-glance)
4. [Connecting a board](#4-connecting-a-board)
5. [Wallet](#5-wallet)
6. [Sending Dogecoin](#6-sending-dogecoin)
7. [Monitoring: Dashboard, Receive & Mesh](#7-monitoring-dashboard-receive--mesh)
8. [History & Address Book](#8-history--address-book)
9. [Settings](#9-settings)
10. [Power-user features](#10-power-user-features)
11. [Where your data lives](#11-where-your-data-lives)
12. [The command line (CLI)](#12-the-command-line-cli)
13. [Troubleshooting](#13-troubleshooting)
14. [Glossary](#14-glossary)

---

## 1. Before you start

You'll need:

- **The RadioDoge app** — a Windows MSI or an Android APK (see [Installing](#2-installing)).
- **A Heltec ESP32 LoRa V3 board** flashed with the RadioDoge firmware
  ([flashing guide](../README.md#-flashing-the-heltec-firmware)).
- **A USB-C cable** (desktop or Android USB-C OTG), or a board advertising over Bluetooth LE (Android preview).

For a real on-chain send you'll also need a **gateway** in radio range — either a host running
`radiodoge-cli daemon`, or a Heltec configured as a firmware WiFi gateway — and a wallet with some DOGE in it.

> **This is beta software.** Keep your recovery phrase offline and don't send more than you're happy to
> experiment with.

---

## 2. Installing

### Windows

1. Open [Windows CI](https://github.com/jnowat/RadioDoge/actions/workflows/build-windows.yml), pick the latest
   **🪟 Windows x64 MSI** run, download the artifact zip, unzip, and run the `.msi`.
2. If SmartScreen appears, click **More info → Run anyway** (dev builds are unsigned).
3. Launch **RadioDoge** — the window opens on the **Connect** tab.

### Android

1. Open [Android CI](https://github.com/jnowat/RadioDoge/actions/workflows/build-android.yml), pick the latest
   **Android (APK)** run, download the artifact zip, and copy the `.apk` to your phone.
2. **Settings → Apps → Install unknown apps** → allow your file manager.
3. Tap the `.apk` → **Install** → open RadioDoge.

---

## 3. The interface at a glance

### The navigation bar

Nine tabs run across the top, each with an icon:

| Tab | Icon | Unlocked when |
|-----|------|---------------|
| **Connect** | 🔌 | Always |
| **Dashboard** | 📡 | Connected |
| **Wallet** | 👛 | Connected |
| **Send** | 📤 | Connected |
| **Receive** | 📥 | Connected |
| **History** | 📜 | Always |
| **Mesh** | 🕸️ | Always |
| **Addresses** | 📒 | Always |
| **Settings** | ⚙️ | Always |

Tabs that need a board (Dashboard, Wallet, Send, Receive) are greyed out until you connect — hovering shows
*"Connect to Heltec first to unlock this tab."* On connect, the app jumps to **Dashboard** automatically; on
disconnect it returns to **Connect**; on an address conflict it jumps to **Mesh**.

The **right side** of the navbar shows your signal bars and a port badge (📶 for BLE, 🟢 for serial), plus
pills for **🌐 GATEWAY** (board gateway mode on) and **▶ Daemon** (the `radiodoge-cli` daemon is running).

On phone-width screens (≤640 px) the tab labels and logo text collapse to **icons only**, with 48 px touch targets.

### The footer

The bottom status bar always shows the app version (`RadioDoge v0.3.16`), your connection state (🟢 with port,
node address, and ↑sent/↓received counters, or 🔴 Not connected), a **🐛 Debug Console** button, and the slogan
*"such decentralize. wow 🐕."*

---

## 4. Connecting a board

### Desktop (USB serial)

1. Plug the Heltec into your PC via USB-C.
2. On the **Connect** tab, click **⟳** to refresh the port list. Ports are badged
   **🟢 Heltec** (matched by USB vendor/product ID), **🔵 USB**, or **⚪ COM** (native — probably not your board),
   and the likely Heltec is auto-selected. Your last-used port is remembered between launches.
3. Click **🔌 Connect to Heltec**. The app queries the firmware version (up to 3 attempts) and reads the board's
   settings, then shows **✅ Connected!** with the firmware badge, signal bars, port, node address, RSSI, and SNR.
4. Use **📡 Ping Device** any time to measure the round-trip — *"Pong! Device responded in ~N ms 🐕."*

**No ports found?** The Connect tab shows a help panel with clickable driver links —
[Silicon Labs CP210x](https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers) and
[WCH CH340](https://www.wch-ic.com/products/CH340.html). Selecting a non-USB port shows a warning.

**Errors** are colour-coded: a soft ⚠️ (yellow) for recoverable issues, a hard ❌ (red) for failures — for
example *"Another program may already have this port open"* (close the other app) or a cable-failure hint
(*"some USB-C cables are power-only"* — try a data cable). Both banners have a ✕ to dismiss.

**Auto-reconnect** is on by default: if the board drops, the app shows *"🔄 Reconnecting…"* and retries with
backoff (5 → 10 → 20 → 40 → 60 s), re-syncing board settings on success. It stops once you disconnect manually.

### Android (USB-C & Bluetooth)

On Android the Connect tab shows **📱 Connect to Heltec Device** with two sub-tabs:

- **🔌 USB-C** *(CP2102 / CH340)* — plug the Heltec in via a USB-C OTG cable, tap **⟳ Scan**, pick the device,
  tap **🔌 Connect via USB-C**, and approve the **USB Permission** dialog. This is the fully-supported mobile
  transport.
- **📶 Bluetooth** *(BLE — preview)* — tap **⟳ Scan** (a 5-second BLE discovery pass), select the board
  (it advertises as `RadioDoge-X.X.X` on the Nordic UART Service), and tap **📶 Connect via Bluetooth**.

The status badge animates through *Searching… → Connecting… → ✅ Connected* (or *❌ Connection failed*). Once
connected you'll see the device path, node address, transport, firmware badge, signal bars, a **📡 Ping Device**
button, a **📶 BLE Advertising** quick-toggle, and **🔌 Disconnect**.

> **BLE preview note:** scan, connect, and incoming notifications work end-to-end, but the current firmware
> doesn't yet execute commands received over BLE. For reliable two-way control, use **USB-C**. See the
> [Roadmap](../ROADMAP.md#-known-limitations--in-progress).

Under the hood, Android's JS layer owns the USB/BLE transport but feeds every byte to the same Rust packet
framer as desktop, and builds every outgoing packet with the same Rust code — one protocol implementation,
two platforms.

---

## 5. Wallet

Open the **Wallet** tab (after connecting). Three header buttons create or import a wallet:
**🎲 Generate Wallet**, **📥 Import WIF**, and **🌱 Mnemonic**.

### Generate a new wallet

Click **🎲 Generate Wallet**. The app creates a **12-word BIP39 recovery phrase**, derives your key at
`m/44'/3'/0'/0/0` (Dogecoin, BIP44), and opens the **Recovery Phrase** backup modal:

- The 12 words are shown in a numbered grid with **📋 Copy phrase to clipboard**.
- ⚠️ *This phrase will not be shown again.* Write it down offline, then click **✅ I've saved my recovery phrase**.

Your address starts with `D` (e.g. `DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L`). The recovery phrase itself is **never
stored** by the app — only you have it.

### Import an existing wallet

- **📥 Import WIF** — paste a compressed mainnet WIF key (starts with `Q`) into the red-bordered panel and click
  **🔑 Import Key**. (A "hot wallet" warning reminds you the key lives in the app.)
- **🌱 Mnemonic** — paste a 12- or 24-word BIP39 phrase and click **🌱 Restore Wallet**; it re-derives
  `m/44'/3'/0'/0/0`.

### Save your wallet (encrypted)

Click **🔐 Save Encrypted** and enter a passphrase (min 8 characters) twice. The app encrypts your WIF key with
**ChaCha20-Poly1305**, keyed by **argon2id (64 MiB)**, and writes `wallet.json` to the app data directory. The
panel then shows **🔐 Wallet saved (encrypted)** plus **🔑 Change passphrase** and **🗑️ Remove saved wallet**.

**Unlocking:** the first time you open the Wallet tab with a saved wallet, the app tries to load it. If it's
encrypted, an **Unlock Saved Wallet** modal appears — enter your passphrase and click **🔓 Unlock** (or press
Enter), or **Skip**. If you have an old *plaintext* wallet from an earlier version, it loads directly and the
app nudges you: *"Your saved wallet uses an old unencrypted format — click Save Encrypted to upgrade."*

### The wallet cards

- **Address** — monospace, with **📋 Copy** and a **📷 Show Address QR** toggle (independent of the private-key
  reveal). The QR is a scannable PNG for sharing your receive address.
- **Balance** — click **🔄 Check Balance** to fetch your confirmed balance from Trezor Blockbook (shown to 8
  decimals, labelled *"🌐 Confirmed balance from Trezor Blockbook"*). If a gateway relays a balance to you over
  LoRa, the card updates and re-labels the source *"📡 Relayed by gateway over LoRa."*
- **Public key** — compressed hex, with copy.
- **Private key** — hidden as dots by default; **👁️ Reveal** / **🙈 Hide** toggles it, with copy and a security
  warning when revealed. *Never share this.*

---

## 6. Sending Dogecoin

Open the **Send** tab (needs a connection *and* a loaded wallet).

1. **Recipient Address** — paste a Dogecoin address (or use **📋 Paste**). Live validation requires exactly
   34 characters starting with `D`; you'll see ✅ or a hint.
2. **Amount (DOGE)** — any amount > 0 (step `0.00000001`).
3. **Memo** *(optional)* — up to 100 characters. *(The memo rides in the LoRa packet; it is ignored when the CLI
   signs a real transaction with `--wif`.)*
4. The **From** row shows your loaded wallet address, and a fee summary appears:
   **Network fee 1.00000000 DOGE (fixed)** — so the total debit is `amount + 1 DOGE`.
5. Click **🚀 Sign & Broadcast via Radio**. The button is disabled until you're connected, have a wallet, and
   both fields are valid; its tooltip tells you what's missing.

What happens: the app builds a **real signed P2PKH transaction** (UTXO fetch → largest-first coin selection →
secp256k1 `SIGHASH_ALL`), splits it into multipart LoRa frames if it exceeds 192 bytes, and broadcasts to the
mesh. On success, **🎉 confetti** fires and a green *"Transaction Sent! Much broadcast. Wow!"* panel shows the
result — it stays on screen for 5 seconds (so you can screenshot the txid) before the form clears. A failure
shows *"😢 Sad! Send failed."* with the error.

> A transaction only reaches the Dogecoin network once a **gateway** relays it. If no gateway is in range, the
> packet is broadcast but not forwarded — add a second Heltec running as a gateway, or a host running
> `radiodoge-cli daemon`.

---

## 7. Monitoring: Dashboard, Receive & Mesh

### Dashboard 📡

The command center while connected:

- **Seven stat cards** — Frequency, TX Power, Spreading Factor, Bandwidth, Packets Sent, Packets Received
  ("Packets Wow'd"), and Battery. Each has a hover tooltip.
- **Battery** polls the board every 15 s and maps millivolts through a realistic Li-ion curve (100 % ≈ 4.20 V,
  0 % ≈ 3.00 V); it switches to 🪫 with a low-battery warning below 3.4 V. *(Requires firmware battery support —
  see [known limitations](../ROADMAP.md#-known-limitations--in-progress).)*
- **Signal panel** — a live RSSI meter (−120…−50 dBm) with a playful, genuinely useful description:
  *🌟 Much Strong Radio!* (≥ −50) down to *❌ Very Weak — Such Distance* (below −110), plus the SNR readout.
- **Live packet log** — a scrolling feed of TX (↑ green) and RX (↓ blue) packets with timestamp, command badge,
  source address, RSSI, decoded text or hex, and a per-row 📋 copy button.

### Receive 📥

A focused packet inspector:

- **Filter** packets by **All / ↑ TX / ↓ RX**, and toggle **Hex** to see raw bytes instead of decoded text.
- **💾 Export** downloads the currently filtered packets as timestamped JSON
  (`radiodoge-packets-<ISO>.json`); **🗑️ Clear** resets the buffer and counters.
- Command badges are colour-coded (PING blue, MESSAGE yellow, BROADCAST orange, DOGE TX green) with tooltips,
  and RX rows colour their RSSI by strength.

The buffer holds the newest 100 packets; the lifetime TX/RX counters keep counting past evicted rows.

### Mesh 🕸️

A live map of nearby nodes:

- Auto-refreshes every 5 s (plus manual **🔄 Refresh**), listing each neighbor's **address**, **signal**
  (🟢 Strong / 🟡 Good / 🟠 Weak / 🔴 Very weak), **RSSI**, and **Last Heard** (relative time).
- A **This node** card shows your own address and neighbor count.
- If the board detects another node using **your** address, a **Duplicate Node Address Detected!** banner
  appears (and the app auto-switches to this tab) — change your address in **Settings** to resolve it.

---

## 8. History & Address Book

### History 📜

Every send is recorded (last 50) in `tx_history.json` and shown here — status icon (✅ sent / ❌ failed), amount
to 8 decimals, timestamp, recipient (with copy), and any memo. It reloads automatically after each send; use
**↺ Refresh** to reload manually.

### Address Book 📒

Save and label the addresses you send to. Click **+ Add Address**, fill in a **Label** (required), a Dogecoin
**address** (starts with `D`), and optional **Notes**, then **💾 Save**. Entries appear in a table with copy and
🗑️ remove buttons and persist immediately to `address_book.json`.

---

## 9. Settings

The **Settings** tab is available even when disconnected (changes are stored locally and pushed to the board
when you connect).

### LoRa radio parameters

Set **Frequency** (433–915 MHz), **TX Power** (5–20 dBm), **Spreading Factor** (SF7–SF12), **Bandwidth**
(125/250/500 kHz), **Coding Rate** (4/5–4/8), and your **Node Address** (three fields, `Region.Community.Node`).
**↺ Defaults** restores 915 MHz / 5 dBm / SF7 / 125 kHz / 4/5 / `10.0.1`.

Click **📡 Save to Device** to push them. The app writes the settings, reads the address back to confirm, and
reports **✅ Verified ✓**, a soft warning if the board didn't confirm in time, or *"stored locally"* if you're
not connected.

> Runtime radio reconfiguration (frequency/SF/bandwidth) is not yet applied by the firmware — the parameters are
> saved and sent, but the board's radio uses its compile-time settings for now. Node address **does** persist.

### Other settings cards

- **Connection Type** — choose **🔌 USB Serial** or **📶 Bluetooth LE** (BLE needs firmware v0.3.6+).
- **WiFi** — enable/disable the board's WiFi radio (persists in NVS). *(Firmware support pending — see roadmap.)*
- **Board MAC** — **🔍 Read MAC** shows the board's WiFi-station MAC. *(Firmware support pending.)*
- **Theme** — toggle **🌙 Dark** / **☀️ Light**; your choice persists across launches (dark is default).
- **Gateway Mode** — **🌐 Start Gateway** / **🟢 Stop Gateway** toggles the board's gateway flag (persisted in
  NVS). Separately, **▶ Spawn Daemon** launches a `radiodoge-cli daemon` child process on your host and drives
  the navbar **▶ Daemon** pill. *(These are two independent things — see the [glossary](#14-glossary).)*
- **BLE Advertising** — enable/disable the board's BLE advertising (firmware v0.3.16+).
- **Board State Sync** — **🔄 Sync from Board** re-reads the board's live address, gateway, and WiFi state.

---

## 10. Power-user features

### The Debug Console

Press **Ctrl+Shift+D** (or click the footer **🐛**) to open a bottom terminal showing **raw serial traffic** in
real time: millisecond timestamps, ↑TX/↓RX badges, space-grouped hex bytes, and parsed command labels. It has
**Auto-scroll**, **💾 Export** to a `.txt` file (Android uses the native share sheet), **🗑️ Clear**, and
per-line copy. Capped at 500 lines. This is the fastest way to see exactly what's on the wire.

### Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| **Ctrl+Shift+D** | Toggle the Debug Console |
| **Enter** (unlock modal) | Submit the wallet passphrase |

### The Easter egg 🎮

Type the Konami code anywhere in the app — **↑ ↑ ↓ ↓ ← → ← → b a** — for a burst of confetti and a random Doge
toast. *Such secret. Very Konami. Much wow.*

### Confetti

A physics-driven confetti burst (200 particles) fires on **every successful send** — and on the Easter egg.
Purely for the vibes. 🎉

---

## 11. Where your data lives

All app files live in Tauri's app-data directory for `com.jnowat.radiodoge`:

| OS | Path |
|----|------|
| Windows | `%APPDATA%\com.jnowat.radiodoge\` |
| Linux | `~/.local/share/com.jnowat.radiodoge/` |
| macOS | `~/Library/Application Support/com.jnowat.radiodoge/` |

| File | Contents |
|------|----------|
| `wallet.json` | Your wallet — encrypted (argon2id + ChaCha20-Poly1305) or legacy plaintext |
| `address_book.json` | Saved addresses and labels |
| `tx_history.json` | Last 50 sent transactions |

Your **theme** and **last-used port** are stored in the app's local storage (`rd-theme`, `radiodoge-last-port`).
Your **recovery phrase is never written to disk** — back it up yourself.

---

## 12. The command line (CLI)

`radiodoge-cli` does everything the GUI does — and runs the gateway — from a terminal. Build it with
`cargo build -p radiodoge-cli --release`; the binary lands at `target/release/radiodoge-cli`.

### Commands

| Command | Purpose |
|---------|---------|
| `ports` | List serial ports |
| `wallet generate` | New keypair (address, pubkey, WIF) — not saved |
| `wallet mnemonic` | New wallet with a 12-word BIP39 phrase |
| `wallet import-mnemonic "<phrase>"` | Restore from a 12/24-word phrase |
| `wallet import-wif <WIF>` | Import from a WIF key (`Q…`) |
| `wallet validate <ADDRESS>` | Validate a Dogecoin address |
| `ping -p <PORT>` | Ping the board (500 ms window) |
| `send -p <PORT> -t <ADDR> -a <DOGE> [-m <MEMO>] [-w <WIF>]` | Send over LoRa; `-w` signs a real tx first |
| `receive -p <PORT> [-T <SECS>]` | Listen for packets (`-T 0` = forever; default 30 s; note capital **-T**) |
| `connect <PORT>` | Interactive REPL (positional port) |
| `balance -a <ADDRESS>` | Confirmed balance via Blockbook (no board needed) |
| `broadcast -w <WIF> -t <ADDR> -a <DOGE>` | Sign + push straight to the network over the internet |
| `daemon -p <PORT>` | Run as a gateway daemon |

Global `-v`/`--verbose` enables debug logging (or set `RUST_LOG=debug`). A fixed **1 DOGE** fee is added to
`send --wif` and `broadcast`.

### The `connect` REPL

`radiodoge-cli connect COM3` opens an interactive session with live packet printing and these commands:

```
ping                         Ping the board
wallet                       Generate a keypair
wallet-mnemonic              Generate a wallet with a recovery phrase
balance <addr>               Query a balance
send <addr> <amount> [memo]  Send over LoRa
stats                        Show radio stats
help                         List commands
quit | exit | q              Leave
```

### Running a gateway

`radiodoge-cli daemon -p /dev/ttyUSB0` turns a host + Heltec into a two-way gateway:

- On an incoming **signed transaction**, it broadcasts to the Dogecoin network via Blockbook (3 attempts,
  exponential backoff) and radios back `TX_ACK:<txid>`.
- On an incoming **balance request**, it queries Blockbook and radios back the balance.
- It logs every packet and runs until you press Ctrl-C.

> Tip: run the daemon on a Raspberry Pi with a Heltec attached for an always-on gateway.

---

## 13. Troubleshooting

| Symptom | Fix |
|---------|-----|
| **No ports found** | Check the cable, install the [CP210x](https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers) or CH340 driver, click ⟳ |
| **"Not connected" after clicking Connect** | Wrong port, or firmware not flashed — check the OLED on the board |
| **Connection failed: port in use** | Close any other app (Arduino IDE serial monitor, etc.) holding the port |
| **Connect fails with a power-only cable** | Use a USB-C **data** cable, not a charge-only one |
| **Board shows address `0.0.0`** | It's never been configured — set an address in **Settings → Save to Device** |
| **RSSI very low (< −100 dBm)** | Move nodes closer, raise a node, or reduce obstructions |
| **Send succeeds but never confirms** | No gateway in range — add a Heltec gateway or run `radiodoge-cli daemon` |
| **Battery / MAC / WiFi toggle shows no data** | Those depend on a firmware update — see the [roadmap](../ROADMAP.md#-known-limitations--in-progress) |
| **BLE connects but commands do nothing** | Firmware BLE is notify-only today — use **USB-C** for two-way control |
| **SmartScreen blocks the MSI** | **More info → Run anyway** — dev builds are unsigned |
| **Android: no USB devices after Scan** | Check the OTG cable, ensure USB host mode, and grant the USB permission prompt |
| **Android: "USB serial open failed"** | Unplug/replug, and close any app holding the device |

---

## 14. Glossary

- **Node address** — a device's `Region.Community.Node` identity on the mesh (e.g. `10.0.2`). `255.255.255` is
  the broadcast address.
- **Gateway** — a node with a path to the internet that forwards transactions to the Dogecoin network. RadioDoge
  has two kinds: the **`radiodoge-cli daemon`** (a host process) and a **firmware WiFi gateway** (a Heltec
  configured with gateway credentials).
- **Gateway mode** — an NVS flag on the board, shown as the 🌐 GATEWAY pill. It's a *reporting* flag; actual
  forwarding depends on the board's configured gateway type/IP/internet.
- **Daemon** — the `radiodoge-cli daemon` child process the app can spawn; shown as the ▶ Daemon pill.
- **WIF** — Wallet Import Format, the encoded private key. Dogecoin compressed mainnet WIFs start with `Q`.
- **BIP39 / BIP44** — the standards for the 12-word recovery phrase and HD key derivation (`m/44'/3'/0'/0/0`).
- **RSSI / SNR** — received signal strength (dBm) and signal-to-noise ratio (dB); higher is better.
- **Multipart** — how payloads over 192 bytes are split into several LoRa frames and reassembled.
- **Blockbook** — the Trezor block explorer API RadioDoge uses to fetch UTXOs/balances and broadcast transactions.

---

<div align="center">

*Much manual. Very thorough. Such wow.* 🐕🌙

[← README](../README.md) · [Roadmap](../ROADMAP.md) · [Protocol reference](PROTOCOL.md)

</div>

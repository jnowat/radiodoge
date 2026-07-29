<div align="center">

<img src="images/Radio_Doge.png" width="360" alt="RadioDoge — a Shiba Inu tuning a vintage radio while Dogecoins beam out over the airwaves">

# 🐕 RadioDoge

### Wireless, peer-to-peer Dogecoin over LoRa radio — no internet required.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Built%20with-Rust%20🦀-orange)](https://rust-lang.org)
[![Tauri 2](https://img.shields.io/badge/Tauri-v2-blue)](https://tauri.app)
[![Dogecoin](https://img.shields.io/badge/Powered%20by-Dogecoin-f7d02c?logo=dogecoin)](https://dogecoin.com)
[![Platform](https://img.shields.io/badge/App-Windows%20%7C%20Android-informational)]()
[![CLI](https://img.shields.io/badge/CLI-Windows%20%7C%20Linux%20%7C%20macOS-informational)]()
[![Windows CI](https://github.com/jnowat/RadioDoge/actions/workflows/build-windows.yml/badge.svg)](https://github.com/jnowat/RadioDoge/actions/workflows/build-windows.yml)
[![Android CI](https://github.com/jnowat/RadioDoge/actions/workflows/build-android.yml/badge.svg)](https://github.com/jnowat/RadioDoge/actions/workflows/build-android.yml)
[![Cargo Audit](https://github.com/jnowat/RadioDoge/actions/workflows/cargo-audit.yml/badge.svg)](https://github.com/jnowat/RadioDoge/actions/workflows/cargo-audit.yml)

**Much wireless. Very transaction. Wow.** 🌙

[**Get the app**](#-get-the-app) · [**How it works**](#-how-radiodoge-works) · [**User manual**](docs/USER_MANUAL.md) · [**Roadmap**](ROADMAP.md) · [**Protocol**](docs/PROTOCOL.md)

</div>

---

RadioDoge sends and receives **real Dogecoin transactions over LoRa radio waves** — completely offline, no
internet on your device required. It pairs a friendly desktop/mobile app with low-cost
[Heltec ESP32 LoRa V3](https://heltec.org/project/wifi-lora-32-v3/) boards to form a wireless mesh: your
phone or PC builds and signs a transaction locally, beams the raw bytes to a nearby Heltec node, and the mesh
relays it — hop by hop — until it reaches a gateway node that pushes it to the Dogecoin network.

> **What's real today (v0.3.16):** the wallet generates 12-word BIP39 recovery phrases and derives keys at
> `m/44'/3'/0'/0/0`. It builds, signs (secp256k1, `SIGHASH_ALL`), and broadcasts genuine P2PKH transactions.
> Private keys are encrypted at rest with ChaCha20-Poly1305 + argon2id. Incoming signed transactions are
> verified in-process. Balances are fetched directly from Trezor Blockbook, or relayed over LoRa by a gateway.
> Desktop (Windows) and Android over **USB-C** are the fully-supported transports; Android **Bluetooth LE** is
> a working preview. *Much real. Very encrypted. Wow.*

---

## 🚀 Get the App

> Every push builds a fresh **Windows MSI** and **Android APK** — grab them straight from the Actions artifacts.

### 🖥️ Windows (MSI)

| Option | How |
|--------|-----|
| **Instant (every push)** | [Windows CI](https://github.com/jnowat/RadioDoge/actions/workflows/build-windows.yml) → latest **🪟 Windows x64 MSI** run → **Artifacts** → download `RadioDoge-…-msi.zip` → unzip → double-click the `.msi` |
| **Tagged release** | [Releases](https://github.com/jnowat/RadioDoge/releases) → download `RadioDoge_x.x.x_x64_en-US.msi` → install |

Push artifacts are kept for **30 days**. On any GitHub Release, the MSI auto-attaches permanently.

> **SmartScreen warning?** Dev builds are unsigned — click **More info → Run anyway**. See
> [code signing](#optional-code-signing) to sign your own releases.

### 📱 Android (APK — sideload)

Every push also builds a debug APK, signed with the Gradle debug key so it installs on any Android 7.0+ device.

1. [Android CI](https://github.com/jnowat/RadioDoge/actions/workflows/build-android.yml) → latest **Android (APK)** run → **Artifacts** → download `radiodoge-android-debug-apk-…`
2. Unzip and copy the `.apk` to your phone
3. Enable **Settings → Apps → Install unknown apps** for your file manager
4. Tap the `.apk` → **Install** → 🐕

> **Connecting a board on Android:** use a **USB-C OTG** cable (fully supported) or **Bluetooth LE** (preview).
> The app requests USB permission on first use; BLE scanning lists boards advertising as `RadioDoge-X.X.X`.

---

## ✨ Highlights

- **No internet required** — transactions travel over LoRa radio (line-of-sight range in the kilometres)
- **Direct-to-gateway** — any board in range forwards what it hears to its host; a gateway pushes it on-chain
- **Real, local crypto** — pure-Rust wallet generates genuine `D…` mainnet addresses; keys never leave your device unencrypted
- **BIP39 + BIP44** — 12-word recovery phrase, HD derivation at `m/44'/3'/0'/0/0`, mnemonic import
- **Encrypted at rest** — WIF keys sealed with ChaCha20-Poly1305, keyed by argon2id (64 MiB)
- **Signed on-device** — secp256k1 `SIGHASH_ALL` P2PKH signing; incoming transactions verified in-process
- **Delightful GUI** — 9 tabs, live signal meters, confetti on every send, a hidden Easter egg 🎉
- **Cross-platform** — Windows desktop + Android APK, built by CI on every push
- **Headless CLI** — `radiodoge-cli` does everything without a GUI, including a gateway daemon
- **Open hardware** — runs on stock Heltec ESP32 LoRa V3 boards (~$20)

---

## 🔧 How RadioDoge Works

```
   Your PC / Phone
        │  builds + signs a real P2PKH transaction locally
        │  USB-C serial  (or Bluetooth LE)
        ▼
  [ Heltec ESP32 V3 ]  ← SX1262 LoRa radio, 915 MHz
        │
        │   ~~~~ LoRa radio waves ~~~~
        ▼
  [ Relay nodes ]  →  [ Gateway node ]
                            │  runs `radiodoge-cli daemon` (or firmware WiFi gateway)
                            │  Internet
                            ▼
                   [ Dogecoin network ]
```

1. The app **builds and signs a real Dogecoin P2PKH transaction** locally — UTXO fetch → coin selection
   (largest-first) → secp256k1 `SIGHASH_ALL` signing.
2. The raw signed transaction is sent to the Heltec over USB-C serial (or BLE) and broadcast over LoRa.
3. Any board in range hands the packet to its own serial host. *(Multi-hop relay of app-format packets is not
   implemented in the firmware — the sender and the gateway have to hear each other directly. Hop counting and
   rebroadcast exist for the firmware's own broadcast format only; see
   [Known Limitations](ROADMAP.md#-known-limitations--in-progress).)*
4. A **gateway** — either a host running `radiodoge-cli daemon`, or a Heltec with the firmware WiFi gateway
   configured — receives the packet and **POSTs the raw transaction to Trezor Blockbook** for broadcast.
5. The gateway radios a `TX_ACK:<txid>` message back to the sender.
6. 🎉 Your transaction lands on-chain.

> **⚠️ End-to-end status — read this before trying to move real money over the air.**
> **Flash firmware v0.4.2 on both boards.** A transaction of any realistic size now goes over LoRa: the host
> splits it into multipart frames, sends them one at a time waiting for the board to acknowledge each, and the
> gateway reassembles and broadcasts. Older firmware could not receive multipart at all, so against a board
> reporting anything below `FW11` the app still refuses payloads over 192 bytes and tells you to flash — a
> signed transaction is 192 bytes only at its very smallest (1 input, 1 output, **no change**), so on old
> firmware most real transactions still have to go over the internet (`radiodoge-cli broadcast`, or the Wallet
> tab). Details: [PROTOCOL.md → Host → board multipart](docs/PROTOCOL.md#host--board-multipart).
>
> The remaining gap is **over-the-air loss**: individual LoRa frames are not acknowledged between boards, so if
> one part of a multipart transaction is lost in the air the gateway times out after 30 s and you simply never
> receive a `TX_ACK`. Re-send. Everything in this repository is tested in software end to end; the firmware
> changes have not been validated on hardware by the authors — see
> [Known Limitations](ROADMAP.md#-known-limitations--in-progress).

---

## ⚡ Quick Start

### To *run* the app

- Windows 10/11 (x64) **or** Android 7.0+ (API 24)
- A [Heltec ESP32 LoRa V3](https://heltec.org/project/wifi-lora-32-v3/) board + a USB-C **data** cable
  (charge-only cables are a common cause of "no ports found")

### To *build* from source

- [Rust](https://rustup.rs) **1.88 or newer** (`rustup update stable`) — several dependencies require it
- [Node.js 20+](https://nodejs.org) — only needed for the GUI, not the CLI
- **Platform libraries**, listed below. These are the usual reason a first build fails.

<details>
<summary><b>Linux</b> — required system packages</summary>

The `serialport` crate needs `libudev`, and Tauri needs GTK/WebKit:

```bash
# Debian / Ubuntu
sudo apt update && sudo apt install -y \
  build-essential pkg-config libudev-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev

# Fedora
sudo dnf install -y @development-tools pkgconf-pkg-config systemd-devel \
  gtk3-devel webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel

# Arch
sudo pacman -S --needed base-devel pkgconf systemd-libs gtk3 webkit2gtk-4.1 libappindicator-gtk3 librsvg
```

Building **only the CLI** (`-p radiodoge-cli`) needs just `pkg-config` and `libudev-dev`.

You'll also need permission to open serial devices — add yourself to the `dialout` group
(`sudo usermod -aG dialout $USER`, then log out and back in), or the app will report "permission denied".

</details>

<details>
<summary><b>macOS</b> — required tooling</summary>

```bash
xcode-select --install     # Command Line Tools (provides WebKit + the linker)
```

No extra libraries are needed; `serialport` uses IOKit from the system SDK.

</details>

<details>
<summary><b>Windows</b> — required tooling</summary>

- [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the
  **Desktop development with C++** workload (supplies the MSVC linker)
- WebView2 — preinstalled on Windows 11 and current Windows 10; otherwise
  [download the Evergreen runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
- A [CP210x](https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers) or
  [CH340](https://www.wch-ic.com/products/CH340.html) USB-UART driver for your board

</details>

### Option A — Install a prebuilt binary

Grab the [Windows MSI or Android APK](#-get-the-app) from CI artifacts (above).

### Option B — Build everything from source

```bash
git clone https://github.com/jnowat/RadioDoge.git
cd RadioDoge

cargo build --workspace     # all three Rust crates
cargo test  --workspace     # 47 unit tests, no hardware needed
```

### Option C — Run the GUI with hot-reloading

```bash
cd RadioDoge/radiodoge-gui
npm install                 # frontend dependencies
npm run tauri dev           # builds the Rust backend and opens the app
```

`npm run tauri` uses the `@tauri-apps/cli` dev-dependency, so there's nothing to install globally.
To produce an installer instead, use `npm run tauri build`.

> **Bundling the CLI with the app.** The GUI's gateway button spawns `radiodoge-cli`, looking for it next to
> the app executable and then on `PATH`. Release builds ship it as a Tauri sidecar; that's an opt-in config
> overlay so a plain source build never fails on a missing binary:
>
> ```bash
> cargo build -p radiodoge-cli --release
> mkdir -p radiodoge-gui/src-tauri/binaries
> cp target/release/radiodoge-cli \
>    "radiodoge-gui/src-tauri/binaries/radiodoge-cli-$(rustc -vV | sed -n 's/^host: //p')"
> cd radiodoge-gui && npm run tauri build -- --config src-tauri/tauri.sidecar.conf.json
> ```

### Option D — Use the headless CLI

`radiodoge-cli` is a pure-Rust binary that replaces the old `RadioDogeSharp` (C#) and `serdog` (C) tools —
no native dependencies.

```bash
cargo build -p radiodoge-cli --release
BIN=./target/release/radiodoge-cli
```

| Command | What it does |
|---------|--------------|
| `radiodoge-cli ports` | List available serial ports |
| `radiodoge-cli wallet generate` | Generate a keypair (address, pubkey, WIF) — not stored |
| `radiodoge-cli wallet mnemonic` | Generate a wallet **with** a 12-word BIP39 phrase |
| `radiodoge-cli wallet import-mnemonic "<12 or 24 words>"` | Restore a wallet from a recovery phrase |
| `radiodoge-cli wallet import-wif <WIF>` | Import a wallet from a WIF key (starts with `Q`) |
| `radiodoge-cli wallet validate <ADDRESS>` | Check whether an address is a valid Dogecoin address |
| `radiodoge-cli ping -p <PORT>` | Ping the Heltec and report round-trip time |
| `radiodoge-cli send -p <PORT> -t <ADDR> -a <DOGE> [-m <MEMO>] [-w <WIF>]` | Send over LoRa. With `-w`, signs a real P2PKH tx first; without it, sends a gateway-signed stub. Payloads over 192 bytes are split into multipart frames on firmware v0.4.2+; on older firmware the send is refused — [see the size gate](docs/PROTOCOL.md#host--board-multipart) |
| `radiodoge-cli receive -p <PORT> [-T <SECS>]` | Listen for incoming packets (`-T 0` = forever; default 30 s) |
| `radiodoge-cli connect <PORT>` | Interactive REPL (`port` is positional, no `-p`) |
| `radiodoge-cli balance -a <ADDRESS>` | Query a confirmed balance via Blockbook (internet, no board) |
| `radiodoge-cli broadcast -w <WIF> -t <ADDR> -a <DOGE>` | Sign **and** broadcast straight to the network over the internet |
| `radiodoge-cli daemon -p <PORT>` | Run as a headless gateway (replaces `serdog`) |

Add `-v`/`--verbose` to any command for debug logging (or set `RUST_LOG=debug`). A fixed network fee of
**1 DOGE** is added on top of the amount for `send --wif` and `broadcast`, so the total debit is `amount + 1`.

```bash
# Examples
$BIN wallet mnemonic
$BIN send -p COM3 -t DH5yaieq… -a 4.20 -m "much transfer" -w Qxxxxxxxx…   # real signed tx over LoRa
$BIN broadcast -w Qxxxxxxxx… -t DH5yaieq… -a 4.20                          # sign + push over the internet
$BIN daemon -p /dev/ttyUSB0                                                # gateway
```

> Full walkthroughs — including the `connect` REPL commands and the gateway flow — live in the
> [**User Manual**](docs/USER_MANUAL.md#12-the-command-line-cli).

---

## 🔌 Flashing the Heltec Firmware

1. Install [Arduino IDE 2.x](https://www.arduino.cc/en/software).
2. **File → Preferences → Additional boards manager URLs**, add Heltec's package (it bundles the required
   `LoRaWan_APP` framework):
   ```
   https://resource.heltec.cn/download/package_heltec_esp32_index.json
   ```
3. **Tools → Board → Boards Manager** → install **Heltec ESP32 Series Dev-Boards**.
4. **Sketch → Include Library → Manage Libraries** → install `Adafruit GFX Library` and `Adafruit SSD1306`.
5. Open `heltec-firmware-v3/heltec-firmware.ino`.
6. **Tools → Board → Heltec WiFi LoRa 32(V3)**, pick the port, and **Upload** (921600 baud works reliably).
   If the upload won't start, hold **BOOT**, tap **RESET**, release **BOOT**, then upload.
7. On boot the OLED shows a splash, then cycles through node address, signal, and packet-count pages.

> A board that has never been configured boots with address **0.0.0**. Set a real address from the app's
> **Settings** tab, or POST `/api/lora/clear` on the firmware's web UI to reset it to `10.1.1`. Full firmware
> docs: [`heltec-firmware-v3/README.md`](heltec-firmware-v3/README.md).

---

## 📡 Transports

| Transport | Platform | Status | Notes |
|-----------|----------|--------|-------|
| USB serial | Windows / Linux / macOS | ✅ Supported | Built-in CP2102 at 115,200 baud via the `serialport` crate |
| USB-C OTG | Android | ✅ Supported | `tauri-plugin-serialplugin` → `usb-serial-for-android`; identical protocol to desktop |
| Bluetooth LE | Android | 🧪 Preview | Nordic UART Service; scan/connect/notify work end-to-end. Firmware v0.4.1 also executes commands received over BLE, but that path is not yet hardware-validated — see [known limitations](ROADMAP.md#-known-limitations--in-progress) |

**Bluetooth LE UUIDs** (Nordic UART Service — matched between firmware and app):

```
Service : 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
Write   : 6E400002-B5A3-F393-E0A9-E50E24DCCA9E   (host → board)
Notify  : 6E400003-B5A3-F393-E0A9-E50E24DCCA9E   (board → host)
```

On Android the JS layer owns the physical USB/BLE transport, but **every packet is built and parsed by the same
Rust core as desktop** (`mobile_build_*` / `mobile_push_bytes`) — zero protocol duplication. Details in the
[User Manual](docs/USER_MANUAL.md#android-usb-c--bluetooth).

---

## 🧩 Serial & Packet Protocol

The app talks to the Heltec at **115,200 baud** using a compact binary frame. Single packets carry an 8-byte
header. Larger payloads use a multipart form with a 13-byte header, whose last byte is the chunk length —
that is what makes a frame self-delimiting on a serial byte stream. Host→board multipart needs
[firmware v0.4.2 or newer](docs/PROTOCOL.md#host--board-multipart); older boards are held to one 192-byte
packet.

```
Byte 0   Command   (see table)
Byte 1   Flags     (low nibble: 0x0 single, 0x1 multipart
                    high nibble: mesh hop count 0–15)
Byte 2   Source region
Byte 3   Source community
Byte 4   Source node
Byte 5   Destination region
Byte 6   Destination community
Byte 7   Destination node
Byte 8+  Payload   (up to 192 bytes per frame)
```

Node addresses use the `Region.Community.Node` format (e.g. `10.0.2`); `255.255.255` is the broadcast address.

| Cmd | Name | Cmd | Name |
|-----|------|-----|------|
| `0x00` | GET_NODE_ADDR | `0x23` | SET_GATEWAY |
| `0x01` | SET_NODE_ADDRS | `0x24` | WIFI_TOGGLE |
| `0x02` | PING | `0x25` | ADDR_CONFLICT *(board→host)* |
| `0x03` | MESSAGE | `0x26` | GET_BATTERY |
| `0x04` | BROADCAST | `0x27` | GET_MAC |
| `0x05` | MULTIPART | `0x28` | BLE_TOGGLE |
| `0x10` | DOGE_TX | `0x29` | RECEIVED_ACK *(board→host)* |
| `0x11` | REQUEST_BALANCE | `0x2A` | RECEIVED_PING *(board→host)* |
| `0x20` | GET_FIRMWARE_VERSION | | |
| `0x21` | SET_LORA_PARAMS | | |
| `0x22` | GET_SETTINGS | | |

> The full frame layout, multipart reassembly, per-command reply sizes, and the firmware's dual (legacy +
> desktop) dispatch are documented in [**docs/PROTOCOL.md**](docs/PROTOCOL.md).

---

<a id="architecture"></a>

## 🏛️ Architecture

RadioDoge is a Cargo workspace of three Rust crates plus the Arduino firmware:

```
radiodoge/
├── Cargo.toml                Workspace root — shared dependency versions
├── SKILLS.md                 Orientation guide for contributors and coding agents
├── crates/
│   ├── radiodoge-core/       Pure-Rust core, no Tauri — the single source of protocol truth
│   │   └── src/              types.rs · radio.rs · serial.rs · wallet.rs · spv.rs · qr.rs
│   └── radiodoge-cli/        Headless CLI + gateway daemon (replaces RadioDogeSharp + serdog)
│       └── src/main.rs       ports · wallet · send · receive · ping · connect · balance · broadcast · daemon
├── radiodoge-gui/            Tauri 2 desktop + Android app (the primary client)
│   ├── src/                  Svelte 5 + TypeScript frontend
│   └── src-tauri/            Rust backend — depends on radiodoge-core
│       ├── tauri.conf.json           Base config (builds anywhere, no sidecar)
│       ├── tauri.sidecar.conf.json   Overlay that bundles radiodoge-cli — used by release CI
│       └── tauri.android.conf.json   Android overrides (auto-merged by Tauri)
├── heltec-firmware-v3/       Production Arduino firmware (WiFi, BLE, web UI, mesh) — flash this one
├── heltec-firmware/          v2 prototype firmware; does NOT speak the app's protocol (reference only)
├── .github/workflows/        CI: Windows MSI · Android APK · cargo-audit
├── images/                   Artwork
└── docs/                     User manual, protocol reference, design PDF
```

The GUI is the **primary client**: a Svelte 5 (runes) frontend on a Tauri 2 Rust backend that reuses
`radiodoge-core` for all crypto and protocol logic — so the app, the CLI, and the mobile bridge share one
implementation.

### Tech stack

| Layer | Technology |
|-------|-----------|
| UI | Svelte 5 + TypeScript |
| Styling | TailwindCSS v4 (CSS-first) with a Doge yellow/orange theme, light + dark |
| Shell | Tauri 2 (desktop + Android) |
| Async runtime | Tokio |
| Serial | `serialport` (desktop) · `tauri-plugin-serialplugin` (Android USB) · `tauri-plugin-blec` (Android BLE) |
| Crypto | `secp256k1` · `sha2` · `ripemd` · `bs58` · `bip39` · `argon2` · `chacha20poly1305` |
| Network | `reqwest` (rustls) → Trezor Blockbook |
| CI/CD | GitHub Actions → Windows MSI + Android APK on every push, weekly cargo-audit |

---

## 🏗️ CI/CD

Three GitHub Actions workflows keep every push shippable and auditable:

| Workflow | Trigger | Output |
|----------|---------|--------|
| **🐕 Build Windows MSI** (`build-windows.yml`) | push to `master`/`main`/`0.0.1-Beta-1`/`claude/**`, tags `v*`, releases, manual | MSI artifact (30-day retention); auto-attached to releases |
| **Build Android APK** (`build-android.yml`) | same triggers | Debug APK artifact (30 days) + build log (14 days) |
| **Cargo Audit** (`cargo-audit.yml`) | push touching manifests, weekly (Mon 06:00 UTC), manual | Fails on any known CVE in a pinned dependency |

### Optional: code signing

```bash
cargo tauri signer generate -w ~/.tauri/radiodoge.key
# Add as repo secrets (Settings → Secrets → Actions):
#   TAURI_SIGNING_PRIVATE_KEY           (base64 of the .key file)
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD  (your passphrase)
```

Unsigned builds are fine for development; Windows may show a SmartScreen prompt on first run.

---

## 🔐 Security Notes

- **Wallet keys** are encrypted at rest with ChaCha20-Poly1305, keyed by argon2id (64 MiB, 2 iterations) from
  your passphrase. Legacy plaintext wallets are auto-detected and the app nudges you to re-encrypt.
- **Your recovery phrase is shown once** on generate and never stored — write it down offline.
- **The firmware web UI has no authentication.** Anyone on the board's WiFi AP can reach every `/api/*` route,
  and a couple of status endpoints echo stored credentials in plaintext. Treat a firmware gateway as a device
  on a trusted network only. This is tracked in the [roadmap](ROADMAP.md#-known-limitations--in-progress).
- RadioDoge is beta software for the Dogecoin community. Don't send more than you're willing to experiment with.

---

## 🗺️ Roadmap

The full, phase-by-phase roadmap — what's shipped, what's next (SPV, QR scanning, fee estimation, Meshtastic
interop, iOS), and an honest **Known Limitations** section — lives in [**ROADMAP.md**](ROADMAP.md).

**Shipped:** Tauri + Svelte GUI · pure-Rust wallet · real P2PKH signing & broadcast · wallet encryption ·
BIP39/BIP44 HD wallet · balance queries · Android APK · Android USB-C · Android BLE (preview) · headless CLI +
gateway daemon · broadcast dedup + host ACK/Ping notifications · SPV inclusion checks · multi-hop relay status ·
QR scanning · host→board multipart so a full-size signed transaction fits over LoRa.

**Next up:** hardware validation of the v0.4.2 firmware · over-the-air retransmission of lost frames · fee
estimation · Meshtastic bridging.

---

## 🤝 Contributing

Contributions are welcome! See [**CONTRIBUTING.md**](CONTRIBUTING.md) for the workflow, build commands, and
coding conventions. In short: fork → branch → commit with clear messages → open a PR. Run
`cargo test --workspace` before pushing.

---

## 🏛️ Legacy

The `RadioDogeSharp` (C#) app and `serdog` (C) daemon have been fully replaced by `crates/radiodoge-cli`, and
the vendored `libdogecoin` C library was removed — all cryptography is now pure Rust (`secp256k1`, `sha2`,
`ripemd`, `bs58`). Those directories no longer exist in the tree; see the [CHANGELOG](CHANGELOG.md) for the
migration history.

---

## 📄 License

[MIT](LICENSE) © 2023 The Dogecoin Foundation and RadioDoge contributors.

---

<div align="center">

*Much open source. Very community. Such LoRa. Wow.* 🐕🌙

</div>

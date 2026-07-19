<div align="center">

# 🧩 RadioDoge Protocol Reference

**The binary packet format, command set, and multipart framing.**

[← README](../README.md) · [User Manual](USER_MANUAL.md) · [Roadmap](../ROADMAP.md)

</div>

---

RadioDoge speaks a compact binary protocol between the host (app or CLI) and the Heltec board over a
**115,200-baud** serial link (or Bluetooth LE), and between boards over the air on **LoRa**. This document is
the authoritative reference for the framing the app uses. It reflects the implementation in
[`crates/radiodoge-core/src/radio.rs`](../crates/radiodoge-core/src/radio.rs), which is the single source of
truth for how the app and CLI build and parse packets.

---

## 1. Node addressing

Every node has a 3-byte address written `Region.Community.Node` (e.g. `10.0.2`):

| Field | Range | Notes |
|-------|-------|-------|
| Region | 0–255 | |
| Community | 0–255 | |
| Node | 0–255 | |

- **Broadcast address:** `255.255.255` (`0xFF 0xFF 0xFF`) — every node.
- **Default local address:** `10.0.1` (app default). A firmware board with empty NVS boots as `0.0.0` until
  configured.

---

## 2. Single packet framing (host ↔ board)

The app builds an **8-byte header** followed by an optional payload:

```
Offset  Field
  0     Command byte        (see §4)
  1     Flags               (low nibble: 0x0 = single, 0x1 = multipart;
                             high nibble: mesh hop count 0–15, v0.4.0)
  2     Source region
  3     Source community
  4     Source node
  5     Destination region
  6     Destination community
  7     Destination node
  8+    Payload             (0 … MAX_SINGLE_PAYLOAD_LEN bytes)
```

- `MAX_SINGLE_PAYLOAD_LEN = 192`. Payloads longer than this are split into multipart frames (§3).
- `SINGLE_HDR_LEN = 8`.
- **Mesh hop count (v0.4.0):** the flags byte's upper nibble carries a 0–15 hop count. Freshly built packets have
  0 hops (upper nibble clear), so this is fully backward compatible. A relay increments it and drops the packet
  once it reaches `MAX_MESH_HOPS` (8). The app surfaces the hop count in the packet log. In the **firmware's**
  multipart mesh frames (§3), the hop count instead rides in the previously-unused `reserved` byte and is bounded
  by `MAX_REBROADCAST_HOPS` (3).

---

## 3. Multipart framing

When a payload exceeds 192 bytes, the app emits a sequence of frames, each with a **12-byte header**
(the standard 8-byte header with `Flags = 0x01`, plus four multipart bytes):

```
Offset  Field
  0..7  Standard header, Flags = 0x01 (FLAG_MULTIPART)
  8     Total parts
  9     Part index (0-based)
 10     Session ID, high byte   (random u16, identical across all parts)
 11     Session ID, low byte
 12+    Payload chunk
```

- `MULTIPART_HDR_LEN = 12`; chunk size = `192 − (12 − 8) = 188` bytes per part.
- `MAX_MULTIPART_PARTS = 20` → up to ~3.76 KB reassembled by the app path.
- The CLI/app space multipart frames ~50 ms apart; the Android bridge uses ~120 ms.

> The **firmware's** over-the-air multipart layout differs (it carries its own 12-byte header with `partNumber`,
> `totalParts`, and a `dataType` field, chunked at 200 bytes, up to 20 parts = 4000 bytes, spaced 500 ms for
> transactions/broadcasts and 100 ms for messages). The two schemes meet at the gateway; app-originated packets
> use the layout above.

---

## 4. Command set

Command bytes as defined by the app core. Commands marked *(board→host)* are asynchronous notifications the
board originates.

| Byte | Name | Direction | Purpose |
|------|------|-----------|---------|
| `0x00` | `GET_NODE_ADDR` | host→board | Query the board's node address |
| `0x01` | `SET_NODE_ADDRS` | host→board | Set/persist the board's node address |
| `0x02` | `PING` | host↔board | Liveness check (ACK reply) |
| `0x03` | `MESSAGE` | any | Text/data message |
| `0x04` | `BROADCAST` | any | Message to all nodes |
| `0x05` | `MULTIPART` | any | Multipart carrier |
| `0x10` | `DOGE_TX` | any | Dogecoin transaction payload |
| `0x11` | `REQUEST_BALANCE` | host→gateway | Ask a gateway to look up a balance |
| `0x20` | `GET_FIRMWARE_VERSION` | host→board | Query the firmware version string |
| `0x21` | `SET_LORA_PARAMS` | host→board | Set SF/BW/CR/frequency/TX-power *(ACK-only in firmware today)* |
| `0x22` | `GET_SETTINGS` | host→board | Read live board state (address + gateway + WiFi) |
| `0x23` | `SET_GATEWAY` | host→board | Set/persist gateway mode |
| `0x24` | `WIFI_TOGGLE` | host→board | Enable/disable the WiFi radio *(firmware wiring pending)* |
| `0x25` | `ADDR_CONFLICT` | board→host | Duplicate node address detected |
| `0x26` | `GET_BATTERY` | host→board | Query battery voltage *(firmware wiring pending)* |
| `0x27` | `GET_MAC` | host→board | Query the board's WiFi-station MAC *(firmware wiring pending)* |
| `0x28` | `BLE_TOGGLE` | host→board | Enable/disable BLE advertising |
| `0x29` | `RECEIVED_ACK` | board→host | The board heard an over-the-air ACK |
| `0x2A` | `RECEIVED_PING` | board→host | The board heard an over-the-air Ping |

See the [Roadmap → Known Limitations](../ROADMAP.md#-known-limitations--in-progress) for the *"pending"* items.

---

## 5. Fixed-length replies

The serial read loop advances its accumulator by an exact byte count for commands with fixed-size replies,
which prevents packet-boundary drift when several replies arrive in one read:

| Command | Reply length | Payload after header |
|---------|--------------|----------------------|
| `GET_NODE_ADDR` | 8 | none (header only) |
| `PING` | 8 | none (ACK) |
| `SET_LORA_PARAMS` | 8 | none (ACK) |
| `ADDR_CONFLICT` | 8 | none |
| `SET_NODE_ADDRS` | 8 | none (ACK) |
| `RECEIVED_ACK` | 8 | none |
| `RECEIVED_PING` | 8 | none |
| `SET_GATEWAY` | 9 | 1 byte — gateway mode |
| `WIFI_TOGGLE` | 9 | 1 byte — WiFi enabled |
| `BLE_TOGGLE` | 9 | 1 byte — BLE enabled |
| `GET_BATTERY` | 10 | 2 bytes — voltage (mV, big-endian) |
| `GET_SETTINGS` | 13 | 5 bytes — `[region, community, node, gateway, wifi]` |
| `GET_MAC` | 14 | 6 bytes — MAC address |

All other commands (`MESSAGE`, `BROADCAST`, `MULTIPART`, `DOGE_TX`, `REQUEST_BALANCE`,
`GET_FIRMWARE_VERSION`) are variable-length; the firmware-version reply is null-terminated.

---

## 6. Notable payloads

- **`SET_LORA_PARAMS` (`0x21`)** — 8-byte payload:
  `[SF(7–12), BW index(0=125,1=250,2=500 kHz), CR denom(5–8), freq_hi, freq_lo (kHz), TX power(dBm), 0x00, 0x00]`.
- **`GET_FIRMWARE_VERSION` (`0x20`)** — the board replies with a version string such as `RadioDoge NV3FW08`.
  The app strips the `RadioDoge ` prefix for display.
- **`REQUEST_BALANCE` (`0x11`)** — payload is the ASCII Dogecoin address. A gateway replies with a `MESSAGE`
  containing `BAL:<koinus>`, which the app renders as `💰 Balance: N DOGE`.
- **Gateway acknowledgements** arrive as `MESSAGE` text:
  - `TX_ACK:<txid>` → `✅ TX confirmed: txid=…`
  - `BAL:<koinus>` → `💰 Balance: … DOGE`

---

## 7. Firmware dispatch (host serial)

The Heltec V3 firmware serves **two protocols on the same serial port**, which is worth understanding if you're
writing your own host tooling:

1. A **legacy** 2-byte host header `[command, payloadSize]` with its own command enum (e.g. `0x03` = ping
   request, `0x04` = message request in the legacy enum), plus commands like `0x68` (host-formed packet) and
   `0x6D` (multipart part).
2. The **desktop** 8-byte packet header used by this app. The firmware recognises these because the app sends
   `Flags = 0x00` in byte 1 — which the legacy parser reads as `payloadSize = 0` — then routes the desktop
   command bytes (`0x10`, `0x11`, `0x20`–`0x23`) to a dedicated handler.

Because of this overlap, the numeric meaning of a byte can differ between the *app command set* (this document)
and the *firmware's legacy enum*. When writing host software, follow the app command set above — it's what the
GUI, the CLI, and the firmware's desktop handler agree on. The firmware's own REST/serial internals are
documented in [`heltec-firmware-v3/README.md`](../heltec-firmware-v3/README.md).

---

## 8. LoRa radio parameters

The firmware's compile-time LoRa configuration (identical on V2 and V3):

| Parameter | Value |
|-----------|-------|
| Frequency | 915 MHz |
| TX power | 5 dBm |
| Bandwidth | 125 kHz |
| Spreading factor | SF7 |
| Coding rate | 4/5 |
| Preamble | 8 symbols |

> These are fixed at compile time in the current firmware; the app can *send* new parameters via
> `SET_LORA_PARAMS`, but runtime reconfiguration is on the [roadmap](../ROADMAP.md#-known-limitations--in-progress).

---

<div align="center">

*Such protocol. Very framing. Much wow.* 🐕🌙

</div>

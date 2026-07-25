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

- `MAX_SINGLE_PAYLOAD_LEN = 192`. Longer payloads split into multipart frames (§3) — but see
  [Host → board is single-packet only](#host--board-single-packet-only): a host may not send multipart to a
  board on today's firmware.
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
- `MAX_MULTIPART_PARTS = 20` → a hard ceiling of 3,760 bytes. A payload larger than that cannot be encoded;
  `try_build_multipart_packets` returns an error rather than truncating it.

> The **firmware's** over-the-air multipart layout differs (it carries its own 12-byte header with `partNumber`,
> `totalParts`, and a `dataType` field, chunked at 200 bytes, up to 20 parts = 4000 bytes, spaced 500 ms for
> transactions/broadcasts and 100 ms for messages). The two schemes meet at the gateway; app-originated packets
> use the layout above.

<a id="host--board-single-packet-only"></a>

### ⚠️ Host → board is single-packet only

**A host cannot currently send multipart frames to a board.** The firmware reads the host header as
`[command, payload_size]` (§7) — byte 1 is a *length* to it, while the host writes its *flags* byte there. The
whole desktop protocol works only because flags are normally `0x00`, which the firmware reads as
`payload_size = 0`. A multipart frame sets `FLAG_MULTIPART` (`0x01`), so the board reads one payload byte,
swallows the first source-address byte, and misframes everything after it.

Consequences, and what the code does about them:

- **Host→board payloads must fit in one 192-byte packet.** `radio::check_host_payload_fits` enforces this, and
  every send path (GUI, CLI, Android bridge) calls it. Oversized sends fail with an explanatory error instead
  of transmitting frames the board will garble.
- **A signed P2PKH transaction is 192 bytes at its smallest** (1 input, 1 output, no change) — exactly the
  limit. Add a change output (`+34`) or a second input (`+148`) and it no longer fits, so **most real
  transactions cannot be relayed over LoRa today**. Broadcast them over the internet instead
  (`radiodoge-cli broadcast`, or the Wallet tab).

This is the **host → board** direction only. Every other direction reassembles correctly:
`radio::MultipartReassembler` stitches sequences back together keyed by `(source, session id)`, and both
framing loops route packets through `radio::ingest_packet`, so a multipart payload arriving **over the air**
at a gateway is delivered to the daemon as one complete packet.

What remains is a firmware change: a host framing that doesn't overload byte 1 as a length. Tracked in the
[Roadmap → Known Limitations](../ROADMAP.md#-known-limitations--in-progress).

### Reassembly semantics

`MultipartReassembler` is what any host implementation should match:

| Behaviour | Rule |
|---|---|
| Session key | `(source address, session id)` — concurrent senders never interleave |
| Ordering | Parts may arrive in any order |
| Duplicates | Ignored; a retransmit never counts twice toward completion |
| Session timeout | 30 s (`MULTIPART_SESSION_TIMEOUT_SECS`), matching the firmware's `MULTIPART_TIMEOUT_MS` |
| Concurrent sessions | 8 max (`MAX_CONCURRENT_MULTIPART_SESSIONS`); the oldest is evicted beyond that |
| Malformed frames | `total_parts == 0`, `total_parts > 20`, or `index >= total_parts` are rejected at parse time |
| Session-id reuse | A reused id with a different part count or command restarts the session |
| Hop count | The reassembled packet reports the highest hop count seen across its parts |

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
| `0x21` | `SET_LORA_PARAMS` | host→board | Set SF/BW/CR/frequency/TX-power (applied and persisted since v0.4.1) |
| `0x22` | `GET_SETTINGS` | host→board | Read live board state (address + gateway + WiFi) |
| `0x23` | `SET_GATEWAY` | host→board | Set/persist gateway mode |
| `0x24` | `WIFI_TOGGLE` | host→board | Enable/disable the WiFi radio |
| `0x25` | `ADDR_CONFLICT` | board→host | Duplicate node address detected |
| `0x26` | `GET_BATTERY` | host→board | Query battery voltage |
| `0x27` | `GET_MAC` | host→board | Query the board's WiFi-station MAC |
| `0x28` | `BLE_TOGGLE` | host→board | Enable/disable BLE advertising |
| `0x29` | `RECEIVED_ACK` | board→host | The board heard an over-the-air ACK |
| `0x2A` | `RECEIVED_PING` | board→host | The board heard an over-the-air Ping |

See the [Roadmap → Known Limitations](../ROADMAP.md#-known-limitations--in-progress) for commands whose
firmware behaviour is partial, such as `SET_LORA_PARAMS`.

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
  `[SF(7–12), BW index(0=125,1=250,2=500 kHz), CR denom(5–8), freq_khz(u32 big-endian), TX power(dBm)]`.

  > **Changed in v0.4.1.** The frequency was a 2-byte field, which cannot hold 915000 kHz (a 20-bit value) —
  > it went out as 63032 kHz with the top 4 bits dropped. The field is now a 4-byte big-endian `u32` occupying
  > bytes `[3..7]`, using the two previously-reserved bytes, and TX power moved from `[5]` to `[7]`. This was a
  > safe wire change: `0x21` was a no-op ACK in every firmware released before v0.4.1, so no deployed board
  > parsed the old layout. Build it with `radio::build_set_lora_params` and read it with
  > `radio::parse_set_lora_params`.
  >
  > The firmware validates before applying (SF 7–12, BW 0–2, CR 1–4, 150–960 MHz, 2–22 dBm) and NACKs
  > out-of-range values rather than retuning to somewhere it can't be reached. Accepted values are applied to
  > the radio and persisted to NVS.
- **`GET_FIRMWARE_VERSION` (`0x20`)** — the board replies with a version string such as `RadioDoge NV3FW09`.
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

> These are the power-on defaults. Since v0.4.1 the app can retune the radio at runtime via
> `SET_LORA_PARAMS` (`0x21`); accepted values are applied immediately and persisted to NVS, so a retuned board
> comes back retuned. Out-of-range values are NACKed and the radio is left untouched.

---

<div align="center">

*Such protocol. Very framing. Much wow.* 🐕🌙

</div>

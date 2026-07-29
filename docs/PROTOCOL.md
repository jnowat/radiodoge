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

- `MAX_SINGLE_PAYLOAD_LEN = 192`. Longer payloads split into multipart frames (§3), which host→board requires
  [firmware v0.4.2 or newer](#host--board-multipart); against older boards the host caps payloads at one packet.
- `SINGLE_HDR_LEN = 8`.
- **Mesh hop count (v0.4.0):** the flags byte's upper nibble carries a 0–15 hop count. Freshly built packets have
  0 hops (upper nibble clear), so this is fully backward compatible. A relay increments it and drops the packet
  once it reaches `MAX_MESH_HOPS` (8). The app surfaces the hop count in the packet log. In the **firmware's**
  multipart mesh frames (§3), the hop count instead rides in the previously-unused `reserved` byte and is bounded
  by `MAX_REBROADCAST_HOPS` (3).

---

## 3. Multipart framing

When a payload exceeds 192 bytes, the app emits a sequence of frames, each with a **13-byte header**
(the standard 8-byte header with `Flags = 0x01`, plus five multipart bytes):

```
Offset  Field
  0..7  Standard header, Flags = 0x01 (FLAG_MULTIPART)
  8     Total parts
  9     Part index (0-based)
 10     Session ID, high byte   (random u16, identical across all parts)
 11     Session ID, low byte
 12     Chunk length            (payload bytes in this part, 0 … 187)
 13+    Payload chunk
```

- `MULTIPART_HDR_LEN = 13`; chunk size = `192 − (13 − 8) = 187` bytes per part.
- `MAX_MULTIPART_PARTS = 20` → a hard ceiling of 3,740 bytes. A payload larger than that cannot be encoded;
  `try_build_multipart_packets` returns an error rather than truncating it.

> **The chunk-length byte (v0.4.2).** Serial is a byte stream with no packet boundaries, in both directions.
> Without a declared length a framer had to assume every chunk was full, so a short final part consumed 187
> bytes and swallowed whatever packet was queued behind it — at a gateway, a `TX_ACK` eaten by the transaction
> it acknowledges. With the length in the header every multipart frame is self-delimiting and framing is exact
> on the host, in the firmware, and over BLE. A frame whose declared length exceeds 187, or exceeds the bytes
> actually present, is rejected rather than shortened: reassembling a transaction from bytes the sender never
> wrote is worse than dropping the frame.

> The **firmware's** over-the-air multipart layout differs (it carries its own 12-byte header with `partNumber`,
> `totalParts`, and a `dataType` field, chunked at 200 bytes, up to 20 parts = 4000 bytes, spaced 500 ms for
> transactions/broadcasts and 100 ms for messages). The two schemes meet at the gateway; app-originated packets
> use the layout above.

<a id="host--board-single-packet-only"></a>
<a id="host--board-multipart"></a>

### Host → board multipart, and the firmware version gate

**Since firmware v0.4.2 (`FIRMWARE_VERSION 11`) a host may hand the board a multipart sequence**, so a signed
transaction of any realistic size can go over LoRa. Against older firmware the host still refuses anything
over 192 bytes.

Three things had to be true at once, and none of them were:

1. **The board framed host commands by draining the serial buffer.** It slept 500 ms and then read every
   buffered byte as one frame's payload. Frames sent back to back — which is what a multipart sequence *is* —
   arrived glued together, and any command queued behind another was consumed and lost. The board now reads
   the exact number of bytes each frame declares: five multipart header bytes ending in a chunk length, then
   that many payload bytes.
2. **Multipart frames had no in-band length.** Even framed one at a time, a receiver reading a byte stream
   could not tell where a short final part ended. §3's chunk-length byte fixes this in every direction.
3. **The board aborted its own transmissions.** `Radio.Send` only *starts* a transmission; the desktop command
   handlers marked the radio idle immediately afterwards, so the next iteration of the main loop switched it
   to receive a couple of milliseconds into a packet needing hundreds — the transaction never left the board.
   Transmissions now go through `SendLoRaAndWait`, which blocks until `TxDone`.

How a send works today:

- The host asks the board for its firmware version and calls `radio::build_tx_frames`, which chooses single or
  multipart framing and **refuses** a payload the board cannot reassemble. A board reporting a build older
  than `MIN_MULTIPART_FIRMWARE` (11), or reporting nothing at all, is held to one 192-byte packet.
- Frames are written **one at a time**, each waiting for the board's `0x10` reply before the next
  (`SerialManager::send_frames`, up to two retransmissions per frame). Because the board answers only after
  the transmission completes, that reply is both the delivery check and the flow control — the host paces
  itself to real airtime instead of a guessed delay. The board's serial buffer holds far less than a whole
  sequence, so an unpaced burst is dropped with no error anywhere.
- A frame that is never acknowledged fails the send with an explanatory error. Nothing further is written, so
  a failure leaves no partial transaction on the air.
- The Android bridge has no acknowledgement plumbed through to JS and uses a fixed
  `MULTIPART_FRAME_GAP_MS` (900 ms) between frames instead.

Every other direction already reassembled correctly and still does: `radio::MultipartReassembler` stitches
sequences back together keyed by `(source, session id)`, and both framing loops route packets through
`radio::ingest_packet`, so a multipart payload arriving over the air at a gateway is delivered to the daemon
as one complete packet.

> **Retransmission is per frame, not end to end.** If a frame is lost *in the air* rather than on the serial
> link, the gateway's reassembler simply times out after 30 s and the transaction is not broadcast — the
> sender sees no `TX_ACK`. There is no over-the-air acknowledgement of individual parts. Tracked in the
> [Roadmap](../ROADMAP.md#-known-limitations--in-progress).

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
`GET_FIRMWARE_VERSION`) are variable-length; the firmware-version reply is null-terminated. Multipart frames
are sized by their declared chunk length (§3) regardless of which command they carry.

**Legacy replies that are shorter than a desktop header.** The board also serves its older serial protocol on
the same port (§7), and two of its replies reach a host that never asked for them. They are framed at their
real lengths so the stream stays aligned; neither yields a packet.

| Bytes | Meaning |
|-------|---------|
| `[0xFE, 1, 0x06\|0x15]` | Legacy ACK / NACK result code — **3 bytes**. A gateway board emits one every time it relays a host `MESSAGE`, which is exactly what a `TX_ACK` is, so this appears in a daemon's stream routinely. Framed as an 8-byte packet it swallowed five bytes of whatever came next. |
| `[0x3F, 3, 'h', board, firmware]` | Legacy hardware info — **5 bytes**. Only sent in reply to the legacy `0x3F` command, which the app never issues. |

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
2. The **desktop** 8-byte packet header used by this app. The firmware reads the first two bytes, then — for
   any byte in the desktop command set (`0x10`, `0x11`, `0x20`–`0x24`, `0x26`–`0x28`) — dispatches on the
   command *before* byte 1 can be mistaken for a length, because to the desktop protocol byte 1 is the flags
   byte. It then reads the rest of the frame to an exact length wherever the protocol defines one: a fixed-size
   command by its known total, a multipart frame by its declared chunk length, and only the genuinely
   unbounded cases (single-packet `0x10` / `0x11`, a relayed `0x03`) by waiting for a 30 ms gap in the byte
   stream.

> Until v0.4.2 the second path slept 500 ms and swallowed every buffered byte, which is why two commands sent
> close together lost the second and a multipart sequence could not be sent at all. Host tooling no longer
> needs to space its commands out to be heard.

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

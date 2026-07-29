<div align="center">

# 🛠️ Working on RadioDoge

**Orientation for contributors and coding agents.**

[← README](README.md) · [Protocol](docs/PROTOCOL.md) · [Roadmap](ROADMAP.md) · [Contributing](CONTRIBUTING.md)

</div>

---

This file is the fast path to being useful in this repository: what lives where, which commands actually work,
and the handful of non-obvious traps that have bitten people before. Read the section that matches your task
rather than the whole thing.

---

## 1. Orient in 60 seconds

RadioDoge moves Dogecoin over LoRa radio. Three Rust crates, one Svelte frontend, one Arduino firmware.

| You're changing… | Go to |
|---|---|
| Packet format, command bytes, framing | `crates/radiodoge-core/src/radio.rs` |
| Serial port lifecycle, the read loop | `crates/radiodoge-core/src/serial.rs` |
| Keys, signing, encryption, broadcast | `crates/radiodoge-core/src/wallet.rs` |
| Block headers, merkle proofs | `crates/radiodoge-core/src/spv.rs` |
| QR / BIP21 payment URIs | `crates/radiodoge-core/src/qr.rs` |
| Anything crossing the IPC boundary | `radiodoge-gui/src-tauri/src/lib.rs` |
| UI | `radiodoge-gui/src/lib/components/*.svelte` |
| Board behaviour | `heltec-firmware-v3/heltec-firmware.ino` |

**The one rule that matters:** `radiodoge-core` is the single source of protocol truth. The desktop app, the
CLI, and the Android bridge all call into it. If you find yourself writing protocol logic anywhere else, you
are creating a second implementation that will drift — and it has, twice (see §5).

---

## 2. Build and test

```bash
cargo build --workspace          # all three crates
cargo test  --workspace          # 62 tests + doc tests, no hardware required
cargo clippy --workspace --all-targets   # expected to be warning-clean
```

Frontend:

```bash
cd radiodoge-gui
npm install
npm run check                    # svelte-check — expected to be 0 errors, 0 warnings
npm run build                    # static build into radiodoge-gui/build/
npm run tauri dev                # full app with hot reload
```

**System prerequisites are real and non-optional.** On Linux, `cargo build` fails without `libudev-dev` (for
`serialport`) and GTK/WebKit (for Tauri). The exact package lists per platform are in the
[README Quick Start](README.md#-quick-start). If a build fails with `pkg-config exited with status code 1`,
that's what happened — install the packages, don't work around it.

**Rust 1.88+ is required.** Transitive dependencies (`image`, `time`, `darling`) demand it.

**No hardware? You can still do most work.** Every test in the suite runs offline. The parts that need a board
are the serial read loop and the firmware itself; everything else — packet building, framing, wallet, SPV, QR —
is unit-testable and already covered.

---

## 3. Conventions that reviewers will look for

- **Comments explain *why*, not *what*.** The existing code is unusually well-commented and several comments
  encode hard-won knowledge (timing values, protocol quirks). Don't strip them. When you fix a bug, leave a
  note saying what the broken behaviour was — that's why the regression tests here read the way they do.
- **Version markers.** Code carries `// v0.3.7 — …` markers tying a change to a release. Follow the pattern
  for new work.
- **Doge voice in user-facing strings** ("Much wireless. Very transaction. Wow."), plain technical English in
  code comments and docs.
- **Errors are user-facing.** `connect_port` translates raw OS serial errors into actionable hints with driver
  links. Match that standard: an error a user can act on beats an accurate one they can't.
- **Never log or serialize a private key.** WIFs are encrypted at rest (argon2id + ChaCha20-Poly1305); keep it
  that way.

---

## 4. Where the bodies are buried

These are non-obvious and have each caused a real bug.

### The firmware reads header byte 1 as a length

The host writes `[command, flags, src×3, dst×3, payload…]`. The firmware's legacy parser reads
`[command, payload_size, …]`. The whole desktop protocol worked **only because flags are normally `0x00`**,
which the board happens to read as "zero payload bytes follow". Anything else — multipart (`0x01`), or a
relayed packet with a non-zero hop nibble — was mis-framed.

Since v0.4.1 desktop commands are dispatched *before* `ReadSerialPayload`, so byte 1 is no longer read as a
length for them; since v0.4.2 the rest of the frame is read to an exact length rather than by draining the
serial buffer. What to respect now:

- **Every host→board send goes through `radio::build_tx_frames`.** It is the single decision point for single
  vs multipart framing and it refuses payloads the connected board cannot reassemble, gated on the reported
  firmware build (`MIN_MULTIPART_FIRMWARE`). Don't build packets for a send path by hand; the reason there is
  one function is that the GUI, the CLI and the Android bridge drifted apart three times before.
- **Frames are written one at a time, waiting for the board's acknowledgement** (`SerialManager::send_frames`).
  This is not politeness — the board's serial buffer holds less than a sequence and it does not read while the
  radio is transmitting, so an unpaced burst is dropped with no error anywhere.
- **A multipart frame declares its own chunk length** (header byte 12). That byte is what makes the frame
  findable on a byte stream; without it a short final part consumed whatever was queued behind it.
- Full analysis: [PROTOCOL.md](docs/PROTOCOL.md#host--board-multipart).

### `Radio.Send` does not wait, and `isLoRaIdle` is not "TX finished"

`Radio.Send()` only *starts* a transmission; `TxDone` arrives later through `Radio.IrqProcess()`. `isLoRaIdle`
means something different — "nothing is happening, put the radio back into receive" — and the main loop acts on
it. Setting it true right after `Radio.Send()` therefore had the next loop iteration abort the transmission a
couple of milliseconds in. That silently killed every `CMD_DOGE_TX` and every gateway `TX_ACK` relay.

Use `SendLoRaAndWait()` for anything you transmit. Never pace packets with a fixed `delay()`: the firmware's
own multipart senders used 100 ms and 500 ms, both shorter than the airtime of the packet being paced, so each
part overwrote the one before it.

### Firmware changes cannot be built in a container

There is no Arduino toolchain here and the Heltec board-package hosts are blocked by network policy, so
`heltec-firmware-v3/heltec-firmware.ino` cannot be compiled or flashed from a review environment. Anything you
change there is unverified until someone builds it.

What that means in practice:

- Prefer APIs the sketch **already uses** over ones you believe exist — `Radio.Sleep()` over `Radio.Standby()`,
  for instance. A wrong symbol is a hard compile failure for the user.
- Keep firmware changes in **separate commits** from Rust changes, so a bad one can be reverted without losing
  the tested work.
- Say plainly in the commit message that a change is compile-unverified. Never describe untested firmware as
  "fixed" without that qualifier.
- Weigh blast radius. A localized handler change is a reasonable thing to write blind; adding auth to 40 route
  handlers, where a mistake locks the operator out of their own board, is not.

### Framing must wait, never truncate

Serial data arrives in arbitrary chunks. A 13-byte reply routinely lands across two reads. `frame_packet_len`
returns `None` when a packet is incomplete, and callers **must** `break` and wait for more bytes. Clamping the
length to what's available (the old bug) emits a short packet *and* leaves its tail to be misparsed as the next
one — corruption that compounds.

### Two framing loops, one rule

Packet framing exists in two places:

- `crates/radiodoge-core/src/serial.rs` — desktop serial read loop
- `radiodoge-gui/src-tauri/src/lib.rs` → `mobile_push_bytes` — Android USB/BLE bridge

They now share `radio::frame_packet_len`, `radio::is_known_command`, and `radio::ingest_packet`. **Keep it that
way.** Both bugs found in the last review were the two copies drifting apart. If you add a command byte, add it
to `is_known_command` — a byte missing there is discarded as noise, which shifts every packet behind it.

### Multipart reassembly happens in the framing layer

`radio::ingest_packet` is the entry point both loops use. It returns `None` for a multipart fragment and the
complete packet once the sequence finishes, so **everything downstream — GUI, packet log, gateway daemon —
only ever sees whole payloads.** Don't add reassembly at a higher layer; that's how you get two
implementations again.

The frame is drained from the accumulator whether or not a packet comes out; a fragment has still been
consumed. Getting that backwards makes the loop spin on the same bytes forever.

Multipart is identified by the **flags byte**, not the command byte — a multipart `CMD_DOGE_TX` frame still
starts with `0x10`. Use `radio::is_multipart_flags`, which masks off the hop nibble so relayed frames are
still recognised.

### Resynchronisation is heuristic, and that's inherent

The protocol has no frame delimiter and no checksum, so after noise the framer
can only *guess* where the next packet starts. Worse, the command set overlaps
printable ASCII — `0x20` is both `CMD_GET_FIRMWARE_VERSION` and the space
character — so any text sharing the link produces false packet starts.

Firmware v0.4.2 stopped echoing its runtime log to the serial port, which is
where nearly all of that text came from: `addLog` ended in `Serial.println`, and
several addLog calls fire per received packet, *while it is being handled*. Do
not put it back. The log is served by the web UI and `GET /api/logs`, and
`HOST_SERIAL_DEBUG` exists for debugging with no host attached. Every board
still prints a boot banner, and older firmware still logs to the wire, so the
heuristic remains load-bearing.

`looks_like_packet_start` also requires the next byte to be a legal flags value
(low nibble `0x0` or `0x1`), which cuts false starts on realistic log lines by
~84%. It cannot eliminate them. What it must never do is reject a real packet —
that's the property the tests pin down. A false start costs one discarded
garbage packet; a false negative loses real traffic.

Use `radio::resync_offset` and drain once. Never scan with `remove(0)` in a
loop: that shifts the whole buffer per byte, and a 200-byte log line in a 1 KB
accumulator moves 200 KB.

### Chunks must reach `mobile_push_bytes` in order

Rust appends every chunk to one accumulator, so the byte stream must stay in
order. Tauri does not guarantee ordering between concurrent `invoke` calls, and
both mobile byte sources — the USB read loop and the BLE notification callback —
can produce a chunk before the previous one finishes crossing IPC.

Always route pushes through `_queuePushBytes` in `connection-bridge.ts`, which
chains them. Firing `invoke('mobile_push_bytes', …)` directly reintroduces the
reordering bug, and it corrupts every packet that straddles the seam.

### Background tasks must check the connection generation

`connect_port` spawns a stats poller and a reconnect watchdog, and each captures
the port name it was spawned with. `reconnect_enabled` alone can't retire them:
it's one shared flag, so connecting to a different port within the watchdog's
2 s tick sets it true again before the old one notices — leaving a stale
watchdog that reconnects the port the user just left.

Any long-lived task spawned per connection must capture `connection_generation`
and exit when it changes.

### `exact_packet_len` is a contract with the firmware

If you change a reply's size in the firmware, change `exact_packet_len` in the same commit. A mismatch
desynchronises the stream rather than failing loudly.

### Broadcast-channel subscribers must subscribe first

`packet_tx` is a `tokio::sync::broadcast`. It only delivers packets sent *after* you subscribe. Always
subscribe **before** sending the request you're awaiting a reply to — `ping()` had this backwards and reported
false timeouts.

### Two firmware directories, only one is real

Flash `heltec-firmware-v3/`. The `heltec-firmware/` directory is a 589-line prototype that implements **none**
of the desktop command bytes (`0x10`–`0x28`) — a board flashed with it cannot talk to the app.

### The Tauri sidecar is opt-in

`tauri.conf.json` deliberately has **no** `externalBin`. Adding one back breaks `cargo build --workspace` on
every clean clone, because the binary is gitignored and only CI produces it. The sidecar lives in
`tauri.sidecar.conf.json`, merged by release CI with `--config`.

---

## 5. Adding a new command byte — the checklist

Touching the protocol means touching every layer. In order:

1. `radio.rs` — add the `CMD_*` constant.
2. `radio.rs` — add it to `is_known_command`.
3. `radio.rs` — add it to `exact_packet_len` **if** the reply is a fixed size.
4. `radio.rs` — add a `build_*` function and a `decode_payload` arm.
5. `radio.rs` — add a test.
6. `serial.rs` — handle the reply in the read loop if it updates cached state.
7. `lib.rs` — add a `#[tauri::command]`, **and register it in `invoke_handler!`** (easy to forget; the failure
   is a runtime "command not found", not a compile error).
8. `lib.rs` — handle it in `mobile_push_bytes` if Android needs it.
9. Firmware — implement the handler **and** route it in the desktop-command dispatch. Handlers that exist but
   aren't routed silently NACK; that was a real bug for three commands.
10. `docs/PROTOCOL.md` — update the command table and, if fixed-length, the reply-size table.

---

## 6. Things that look like bugs but aren't

- **`ADDRESS_SET` maps to `0x02` = `CMD_PING`.** The firmware serves a legacy enum and the desktop command set
  on the same port, disambiguated by whether byte 1 is zero. Intentional. See PROTOCOL.md §7.
- **`sleep(400ms)` / `sleep(600ms)` after sending a query.** The board needs real time to answer over UART;
  these were tuned against hardware. Don't shrink them without a board to test on.
- **The flat 1 DOGE fee.** Deliberate — it covers any plausible transaction size on Dogecoin. Dynamic fee
  estimation is on the roadmap.
- **SPV checks `SHA256d` against the `nBits` target.** Dogecoin's actual PoW is Scrypt/AuxPoW, so this is a
  consistency check, not trustless PoW validation. The module documents this; chain linkage and merkle proofs
  *are* fully correct.
- **`qr.rs` only structurally validates addresses.** Full base58check happens at the send boundary via
  `wallet::is_valid_address`. Intentional layering.

---

## 7. Before you open a PR

```bash
cargo test --workspace && cargo clippy --workspace --all-targets
cd radiodoge-gui && npm run check
```

All three must be clean. Then:

- Update `docs/PROTOCOL.md` if you touched the wire format.
- Update `ROADMAP.md` if you fixed or discovered a limitation — the **Known Limitations** section is meant to
  be honest and current, and it's how the next person avoids re-deriving what you learned.
- Add a `CHANGELOG.md` entry.

If you find a real problem you can't fix in scope, **document it in ROADMAP.md rather than leaving it silent.**
A known limitation is a feature; a silent failure that loses someone's DOGE is not.

---

<div align="center">

*Much context. Very onboarding. Such wow.* 🐕🌙

</div>

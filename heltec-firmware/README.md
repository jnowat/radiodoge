# 🐕 RadioDoge Firmware — v2 Prototype

> **This is the original prototype firmware.** For production use, flash
> [`heltec-firmware-v3/`](../heltec-firmware-v3/) instead — it adds WiFi, a web/REST interface, Bluetooth LE,
> NVS persistence, multipart reassembly, mesh rebroadcast, and the full binary protocol the RadioDoge app
> speaks. This v2 sketch is kept for reference and for the Heltec **WiFi LoRa 32 (V2)** board.

The first RadioDoge firmware: a compact (~575-line) serial + LoRa sketch for the Heltec WiFi LoRa 32 modules.
It forms addressed LoRa packets and exchanges them with other modules under host control.

Board: [Heltec WiFi LoRa 32 (V2 / V3)](https://heltec.org/project/wifi-lora-32-v3/)

---

## Setting up the development environment

Install the Arduino IDE and the required Heltec libraries:

- Arduino IDE — <https://www.arduino.cc/en/software>
- Heltec board setup — <https://heltec.org/wifi_kit_install/>

On Windows you may also need the CP210x USB-to-UART driver:

- <https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers>

Open `heltec-firmware.ino` in the Arduino IDE, select your Heltec board, and upload.

---

## What it does

LoRa packets are addressed to individual modules using the RadioDoge scheme:

```
Region.Community.Node        e.g. 10.1.3
```

Over these packets, modules can exchange:

- **Pings**
- **ACKs**
- **User-defined messages** (e.g. text)

Command and control happens over the serial link to a host, using a 2-byte `[command, payloadSize]` header. The
host issues commands — set address, send ping, send message — and the module reports back.

> **Note:** this prototype prints verbose serial output for debugging, and received multipart parts are passed
> to the host **without** reassembly. `ReceivedACK` / `ReceivedPing` host notifications are stubs here — they
> exist for real in [v3](../heltec-firmware-v3/) as serial commands `0x29` / `0x2A`.

---

## Where to go next

- **Production firmware:** [`heltec-firmware-v3/README.md`](../heltec-firmware-v3/README.md)
- **Protocol reference:** [`docs/PROTOCOL.md`](../docs/PROTOCOL.md)
- **The app:** [project README](../README.md)

*Much prototype. Very origin. Wow.* 🐕

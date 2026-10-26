# Hardware session checklist (two Heltec V3 boards)

Goal: prove the full path **sender board → LoRa → gateway board → Dogecoin
network** with as little time at the bench as possible. Everything up to the
final live send is scripted in [`scripts/hw-session.ps1`](../scripts/hw-session.ps1)
and never broadcasts a transaction.

| Board | Role    | Node address | Port (example) |
|-------|---------|--------------|----------------|
| A     | sender  | `10.0.1`     | `COM3`         |
| B     | gateway | `10.0.2`     | `COM4`         |

## Before plugging in

1. Build the CLI: `cargo build --release -p radiodoge-cli`.
2. Build the firmware app image:
   `arduino-cli compile -b Heltec-esp32:esp32:heltec_wifi_lora_32_V3 --output-dir %USERPROFILE%\fwbuild heltec-firmware-v3`
   → `%USERPROFILE%\fwbuild\heltec-firmware-v3.ino.bin`.
3. `esptool` on PATH (or `python -m esptool`).
4. For the dry-run sign, set **one** of these in the shell (never on a command line):
   `$env:RADIODOGE_WIF` or `$env:RADIODOGE_MNEMONIC_FILE` (path to a file with the phrase).
   Use a test wallet holding only a few DOGE.

## Run

```powershell
pwsh -File scripts\hw-session.ps1 -PortA COM3 -PortB COM4 `
  -BalanceAddress <test wallet address> -DryRunTo <recipient> -Amount 1
```

Each step that touches a board prompts first; `-Yes` skips the prompts and
`-SkipFlash` skips step 1. A log, gateway stats, and the signed hex land in
`%USERPROFILE%\rd-hw-session\`.

| # | Step | Command | Pass when |
|---|------|---------|-----------|
| 0 | Ports present | `radiodoge-cli ports` | both COM ports listed |
| 1 | Flash app only | `esptool --chip esp32s3 --port P --baud 921600 write-flash 0x10000 heltec-firmware-v3.ino.bin` | esptool exit 0. **Never** the merged image — it erases NVS (license, node address) |
| 2 | Identity | `radiodoge-cli info -p P --expect 10.0.x` | address matches; firmware ≥ FW12 shows `raw LoRa: yes` |
| 3 | USB + LoRa ping | `ping -p P`, `lora-ping -p A -t 10.0.2` and back | PONG, then `✅ ACK after … ms` both ways |
| 4 | Gateway up | `daemon -p B --stats-file …` (background) | process still running after 6 s |
| 5 | Balance over LoRa | `lora-balance -p A -a D… -g 10.0.2` | `💰 Gateway 10.0.2 reports …` |
| 6 | Dry-run sign | `sign-tx -t D… -a 1 -o dryrun.hex` | `✅ Signed (NOT broadcast)` + txid |

Fixes for common failures:

- Wrong address → `radiodoge-cli set-address -p P 10.0.1` (persisted in NVS), rerun with `-SkipFlash`.
- No LoRa ACK → both boards on the same frequency/SF (Settings tab or `info`), antennas on, boards > 1 m apart.
- No balance reply → gateway needs internet; check `daemon-*.err` in the session folder.

## Live send (manual, only on Jacob's go)

The script prints the two commands at the end. This is the only step that
broadcasts:

```powershell
radiodoge-cli daemon -p COM4                       # gateway, leave running
radiodoge-cli send-tx-hex -p COM3 -f <dryrun.hex> -g 10.0.2
```

The sender shows `[n/4]` progress (radio, frames, reply), then one of:

| Reply | Meaning | Action |
|---|---|---|
| `TX_ACK` | Gateway broadcast it | Open the printed `https://dogechain.info/tx/<txid>` link |
| `TX_QUEUED` | Gateway offline; held for retry | Keep waiting; the gateway retries on its own |
| `TX_NACK:<txid>:<why>` | Network rejected it permanently | **Do not resend**; fix the cause |
| no reply | Unknown, not failure | Check the explorer link before any retry |

Re-sending the same txid within 30 minutes is refused (ledger:
`~/.radiodoge/sent-txids.json`, or `RADIODOGE_SENT_LEDGER`). Pass `--force` only
after the explorer shows it never landed. The GUI has the same guard with a
"Resend anyway" button. Confirm with `radiodoge-cli verify-tx -t <txid>`.

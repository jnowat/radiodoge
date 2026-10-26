<#
.SYNOPSIS
  RadioDoge two-board hardware session: flash, verify, ping, LoRa balance, dry-run sign.

.DESCRIPTION
  Runs the end-to-end checklist in docs/HARDWARE_SESSION.md against two Heltec
  WiFi LoRa 32 V3 boards. Every step that touches a board asks first (pass -Yes
  to skip the prompts). Nothing in this script broadcasts a transaction:
  step 6 signs a tiny send and writes the hex to disk, and the live LoRa send is
  a separate, manual command printed at the end.

  Flashing writes the APP partition only (0x10000), so NVS — node address,
  Heltec license, AP password — survives. Never flash the merged image here.

  Keys: step 6 reads RADIODOGE_WIF or RADIODOGE_MNEMONIC_FILE from the
  environment. The script never prints, logs, or passes a key on a command line.

.EXAMPLE
  pwsh -File scripts\hw-session.ps1 -PortA COM3 -PortB COM4 -BalanceAddress D... -DryRunTo D...
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)] [string] $PortA,          # sender board, expected 10.0.1
  [Parameter(Mandatory)] [string] $PortB,          # gateway board, expected 10.0.2
  [string] $AddrA = '10.0.1',
  [string] $AddrB = '10.0.2',
  [string] $Cli = '',                               # radiodoge-cli.exe (default: target\release, then PATH)
  [string] $FirmwareBin = "$env:USERPROFILE\fwbuild\heltec-firmware-v3.ino.bin",
  [string] $BalanceAddress = '',                    # address the gateway should look up (step 5)
  [string] $DryRunTo = '',                          # recipient for the dry-run sign (step 6)
  [double] $Amount = 1.0,                           # DOGE, dry-run only
  [string] $OutDir = "$env:USERPROFILE\rd-hw-session",
  [switch] $SkipFlash,
  [switch] $Yes
)
$ErrorActionPreference = 'Stop'
$results = [System.Collections.Generic.List[object]]::new()
New-Item -ItemType Directory -Force $OutDir | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$log = Join-Path $OutDir "session-$stamp.log"

function Say($m) { Write-Host $m; Add-Content $log $m }
function Record($step, $status, $detail = '') {
  $results.Add([pscustomobject]@{ Step = $step; Status = $status; Detail = $detail })
  $c = @{ PASS = 'Green'; FAIL = 'Red'; SKIP = 'Yellow' }[$status]
  Write-Host ("  [{0}] {1} {2}" -f $status, $step, $detail) -ForegroundColor $c
  Add-Content $log ("[{0}] {1} {2}" -f $status, $step, $detail)
}
function Confirm($what) {
  if ($Yes) { return $true }
  $a = Read-Host "$what  — go? [y/N]"
  return $a -match '^(y|yes)$'
}
function Run($step, [string[]] $cmdArgs) {
  Say "> radiodoge-cli $($cmdArgs -join ' ')"
  $out = & $script:CliPath @cmdArgs 2>&1 | ForEach-Object { "$_" }
  $out | ForEach-Object { Say "    $_" }
  if ($LASTEXITCODE -eq 0) { Record $step PASS } else { Record $step FAIL "exit $LASTEXITCODE" }
  return ,$out
}

# ── 0. Preflight ────────────────────────────────────────────────────────────
Say "RadioDoge hardware session $stamp  (A=$PortA/$AddrA sender, B=$PortB/$AddrB gateway)"
$repo = Split-Path $PSScriptRoot -Parent
$script:CliPath = if ($Cli) { $Cli } elseif (Test-Path "$repo\target\release\radiodoge-cli.exe") { "$repo\target\release\radiodoge-cli.exe" } else { (Get-Command radiodoge-cli -ErrorAction SilentlyContinue).Source }
if (-not $script:CliPath) { throw "radiodoge-cli not found — cargo build --release -p radiodoge-cli, or pass -Cli" }
Say "cli: $script:CliPath"
$ports = & $script:CliPath ports 2>&1 | Out-String
foreach ($p in $PortA, $PortB) {
  if ($ports -match [regex]::Escape($p)) { Record "port $p present" PASS } else { Record "port $p present" FAIL 'not listed by `radiodoge-cli ports`' }
}

# ── 1. Flash app partition (keeps NVS) ──────────────────────────────────────
if ($SkipFlash) { Record 'flash' SKIP '-SkipFlash' }
elseif (-not (Test-Path $FirmwareBin)) { Record 'flash' FAIL "no $FirmwareBin (arduino-cli compile ... --output-dir $(Split-Path $FirmwareBin))" }
elseif ($FirmwareBin -match 'merged') { Record 'flash' FAIL 'refusing the merged image: it erases NVS (license + node address)' }
else {
  $esptool = (Get-Command esptool -ErrorAction SilentlyContinue).Source
  $esp = if ($esptool) { @($esptool) } else { @('python', '-m', 'esptool') }
  foreach ($p in $PortA, $PortB) {
    if (Confirm "Flash $FirmwareBin to $p at 0x10000 (app only, NVS kept)") {
      Say "> $($esp -join ' ') --chip esp32s3 --port $p --baud 921600 write-flash 0x10000 $FirmwareBin"
      $exe, $rest = $esp
      & $exe @rest --chip esp32s3 --port $p --baud 921600 write-flash 0x10000 $FirmwareBin 2>&1 | ForEach-Object { Say "    $_" }
      if ($LASTEXITCODE -eq 0) { Record "flash $p" PASS } else { Record "flash $p" FAIL "esptool exit $LASTEXITCODE" }
      Start-Sleep -Seconds 4   # board reboots after flashing
    } else { Record "flash $p" SKIP 'declined' }
  }
}

# ── 2. Identity: node addresses + firmware ─────────────────────────────────
Run "info A expects $AddrA" @('info', '-p', $PortA, '--expect', $AddrA) | Out-Null
Run "info B expects $AddrB" @('info', '-p', $PortB, '--expect', $AddrB) | Out-Null
Say "  (wrong address? radiodoge-cli set-address -p <port> <addr> — persisted in NVS)"

# ── 3. USB ping + LoRa ping both ways ───────────────────────────────────────
Run 'usb ping A' @('ping', '-p', $PortA) | Out-Null
Run 'usb ping B' @('ping', '-p', $PortB) | Out-Null
if (Confirm 'Transmit LoRa PINGs A->B and B->A') {
  Run "lora ping $AddrA->$AddrB" @('lora-ping', '-p', $PortA, '-t', $AddrB) | Out-Null
  Run "lora ping $AddrB->$AddrA" @('lora-ping', '-p', $PortB, '-t', $AddrA) | Out-Null
} else { Record 'lora ping' SKIP 'declined' }

# ── 4/5. Gateway daemon on B, balance over LoRa from A ──────────────────────
$daemon = $null
if (-not $BalanceAddress) { Record 'lora balance' SKIP 'no -BalanceAddress' }
elseif (Confirm "Start the gateway daemon on $PortB (needs internet) and ask for a balance over LoRa from $PortA") {
  $stats = Join-Path $OutDir "gateway-stats-$stamp.json"
  $daemon = Start-Process -FilePath $script:CliPath -ArgumentList @('daemon', '-p', $PortB, '--stats-file', $stats) `
    -RedirectStandardOutput (Join-Path $OutDir "daemon-$stamp.out") -RedirectStandardError (Join-Path $OutDir "daemon-$stamp.err") -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 6
  if ($daemon.HasExited) { Record 'gateway daemon' FAIL "exited $($daemon.ExitCode) — see daemon-$stamp.err" }
  else {
    Record 'gateway daemon' PASS "pid $($daemon.Id)"
    Run 'lora balance' @('lora-balance', '-p', $PortA, '-a', $BalanceAddress, '-g', $AddrB, '-T', '45') | Out-Null
  }
} else { Record 'lora balance' SKIP 'declined' }

# ── 6. Dry-run sign (no board, nothing broadcast) ───────────────────────────
$txFile = Join-Path $OutDir "dryrun-$stamp.hex"
if (-not $DryRunTo) { Record 'dry-run sign' SKIP 'no -DryRunTo' }
elseif (-not ($env:RADIODOGE_WIF -or $env:RADIODOGE_MNEMONIC_FILE)) { Record 'dry-run sign' SKIP 'set RADIODOGE_WIF or RADIODOGE_MNEMONIC_FILE first' }
else {
  $out = Run 'dry-run sign' @('sign-tx', '-t', $DryRunTo, '-a', "$Amount", '-o', $txFile)
  $txid = ($out | Select-String 'txid:\s+([0-9a-f]{64})').Matches | ForEach-Object { $_.Groups[1].Value } | Select-Object -First 1
  if ($txid) { Say "  dry-run txid $txid (NOT broadcast)" }
}

if ($daemon -and -not $daemon.HasExited) { Stop-Process -Id $daemon.Id; Say "gateway daemon stopped" }

# ── Summary ─────────────────────────────────────────────────────────────────
Say ''
$results | Format-Table -AutoSize | Out-String | ForEach-Object { Say $_ }
Say "log: $log"
if (Test-Path $txFile) {
  Say 'Live send (only when Jacob says go — this DOES broadcast via the gateway):'
  Say "  1. restart the gateway:  radiodoge-cli daemon -p $PortB"
  Say "  2. radio the signed tx:  radiodoge-cli send-tx-hex -p $PortA -f $txFile -g $AddrB"
}
if ($results.Status -contains 'FAIL') { exit 1 }

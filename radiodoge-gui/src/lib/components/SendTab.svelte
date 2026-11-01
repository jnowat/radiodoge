<script lang="ts">
  import { errorMessage } from '$lib/errors';
  /**
   * Send tab — craft and broadcast a Dogecoin transaction over LoRa.
   */

  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { connection } from '$lib/stores/connection.svelte';
  import { wallet } from '$lib/stores/wallet.svelte';
  import type { FeeEstimate, IncomingPacket, ParsedPayment, SendOutcome } from '$lib/types';
  import { open } from '@tauri-apps/plugin-shell';
  import DogeSpinner from './DogeSpinner.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  interface Props {
    onTriggerConfetti: () => void;
  }
  let { onTriggerConfetti }: Props = $props();

  let toAddress = $state('');
  let amountDoge = $state('');
  let memo = $state('');
  let feeDoge = $state('');  // empty = network-estimated rate (min 0.01 DOGE per started kB)

  // ── Offline signing: UTXO snapshot from `radiodoge-cli utxo-snapshot` ──
  interface SnapshotInfo { address: string; fetchedAt: number; utxoCount: number; totalDoge: number }
  let snapshot = $state<SnapshotInfo | null>(null);
  let snapshotError = $state<string | null>(null);
  const snapshotMatches = $derived(!!snapshot && snapshot.address === wallet.address);

  async function importSnapshot(ev: Event) {
    const input = ev.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    snapshotError = null;
    try {
      snapshot = await invoke<SnapshotInfo>('import_utxo_snapshot', { json: await file.text() });
    } catch (e: unknown) {
      snapshotError = e instanceof Error ? e.message : String(e);
    }
  }

  async function clearSnapshot() {
    await invoke('clear_utxo_snapshot');
    snapshot = null;
    snapshotError = null;
  }

  onMount(() => { invoke<SnapshotInfo | null>('utxo_snapshot_info').then((s) => (snapshot = s)).catch(() => {}); });

  function snapshotAge(s: SnapshotInfo): string {
    const h = Math.max(0, (Date.now() / 1000 - s.fetchedAt) / 3600);
    return h < 1 ? `${Math.round(h * 60)} min old` : h < 48 ? `${h.toFixed(1)} h old` : `${Math.round(h / 24)} days old`;
  }
  let isSending = $state(false);
  let reviewing = $state(false);
  let success = $state<string | null>(null);
  let error = $state<string | null>(null);

  // Timer used to clear the form + success message 5s after a successful send.
  // Tracked so it can be cancelled if the user starts a new transaction before
  // the 5s window elapses (otherwise their freshly-typed inputs would be wiped).
  let _successClearTimer: ReturnType<typeof setTimeout> | null = null;

  // On Android the backend hands the frames to the JS bridge and returns before
  // they have been written, so a write failure arrives after we have already
  // shown "sent". Replace the success message when that happens — a transaction
  // that never reached the board is the one thing the user must not miss.
  /**
   * How far a multipart send has got, e.g. "3/8".
   *
   * A transaction larger than one packet becomes several independent LoRa
   * transmissions, each acknowledged by the board before the next is written —
   * seconds of real airtime, and considerably more at a high spreading factor.
   * Without this the button just sat there.
   */
  let sendProgress = $state<{ sent: number; total: number } | null>(null);

  /**
   * The last signed transaction radioed, and what the gateway said about it.
   * Silence is "unknown", never failure: older gateways only send TX_ACK, and
   * a broadcast can succeed even when the reply is lost on the air.
   */
  type TxStatus = 'waiting' | 'queued' | 'acked' | 'nacked' | 'timeout';
  let lastTx = $state<{ txid: string; url: string; status: TxStatus; reason?: string } | null>(null);
  let duplicateBlocked = $state(false);
  let copied = $state(false);
  let _replyTimer: ReturnType<typeof setTimeout> | null = null;
  const REPLY_TIMEOUT_MS = 90_000;

  function hexToText(hex: string): string {
    const bytes = new Uint8Array((hex.match(/../g) ?? []).map((h) => parseInt(h, 16)));
    return new TextDecoder().decode(bytes);
  }

  function onGatewayReply(pkt: IncomingPacket) {
    if (!lastTx || lastTx.status === 'acked' || lastTx.status === 'nacked') return;
    const text = hexToText(pkt.payloadHex ?? '');
    const m = /^TX_(ACK|QUEUED|NACK):([0-9a-fA-F]{64})(?::(.*))?/.exec(text);
    if (!m || m[2].toLowerCase() !== lastTx.txid) return;
    if (m[1] === 'ACK') lastTx.status = 'acked';
    else if (m[1] === 'QUEUED') lastTx.status = 'queued';
    else { lastTx.status = 'nacked'; lastTx.reason = m[3] || 'rejected'; }
    if (lastTx.status !== 'queued' && _replyTimer) { clearTimeout(_replyTimer); _replyTimer = null; }
  }

  async function copyTxid() {
    if (!lastTx) return;
    try { await navigator.clipboard.writeText(lastTx.txid); copied = true; setTimeout(() => (copied = false), 1500); } catch { /* no clipboard */ }
  }

  function onMobileSendFailed(ev: Event) {
    const detail = (ev as CustomEvent<string>).detail;
    if (_successClearTimer !== null) {
      clearTimeout(_successClearTimer);
      _successClearTimer = null;
    }
    success = null;
    sendProgress = null;
    error = `😢 The transaction did not reach the board: ${detail}. Nothing was broadcast — check the cable or Bluetooth link and try again.`;
    isSending = false;
  }

  onMount(() => {
    window.addEventListener('radiodoge:tx-send-failed', onMobileSendFailed);
    const progress = listen<{ sent: number; total: number }>('transaction-progress', (ev) => {
      sendProgress = ev.payload;
    });
    const replies = listen<IncomingPacket>('radio-packet', (ev) => onGatewayReply(ev.payload));
    return () => {
      window.removeEventListener('radiodoge:tx-send-failed', onMobileSendFailed);
      progress.then((fn) => fn());
      replies.then((fn) => fn());
      if (_replyTimer) clearTimeout(_replyTimer);
      if (_successClearTimer) clearTimeout(_successClearTimer);
    };
  });

  // Validation
  const isValidAddress = $derived(() => {
    return toAddress.length === 34 && toAddress.startsWith('D');
  });

  const isValidAmount = $derived(() => {
    const n = parseFloat(amountDoge);
    return !isNaN(n) && n > 0;
  });

  /** Empty fee means "recommended"; otherwise a non-negative number. */
  const isValidFee = $derived(() => {
    if (!feeDoge.trim()) return true;
    const n = parseFloat(feeDoge);
    return !isNaN(n) && n >= 0;
  });

  /** True only when ALL conditions are met — including a loaded wallet. */
  // ── Fee estimation ──────────────────────────────────────────────────────
  // Show what an empty fee box will pay: the backends' current rate, floored
  // at 0.01 DOGE per started kB (debounced; display only).
  let feeEstimate = $state<FeeEstimate | null>(null);
  let feeEstimating = $state(false);
  let feeError = $state<string | null>(null);

  $effect(() => {
    const address = wallet.address;
    const amount = parseFloat(amountDoge);
    feeEstimate = null;
    feeError = null;
    if (!wallet.isGenerated || !address || !(amount > 0)) return;
    let cancelled = false;
    const t = setTimeout(async () => {
      feeEstimating = true;
      try {
        const est = await invoke<FeeEstimate>('estimate_fee', { fromAddress: address, amountDoge: amount });
        if (!cancelled) feeEstimate = est;
      } catch (e) {
        if (!cancelled) feeError = errorMessage(e);
      } finally {
        if (!cancelled) feeEstimating = false;
      }
    }, 600);
    return () => { cancelled = true; clearTimeout(t); };
  });

  const canSend = $derived(() =>
    connection.isConnected &&
    wallet.isGenerated &&
    isValidAddress() &&
    isValidAmount() &&
    isValidFee() &&
    !isSending
  );

  /** Fees above this look like a typo (the recommended fee is ~0.01 DOGE). */
  const HIGH_FEE_DOGE = 1;

  function shortAddr(a: string) {
    return a.length > 16 ? `${a.slice(0, 8)}…${a.slice(-6)}` : a;
  }

  /** Step 1: open the review dialog. Nothing is signed until the user confirms. */
  function reviewTransaction() {
    if (!canSend()) return;
    error = null;
    reviewing = true;
  }

  async function confirmSend() {
    reviewing = false;
    await sendTransaction();
  }

  async function sendTransaction(allowResend = false) {
    // Cancel any pending form-clear timer so a new transaction doesn't wipe
    // inputs the user has already started filling in.
    if (_successClearTimer !== null) {
      clearTimeout(_successClearTimer);
      _successClearTimer = null;
    }

    // Hard gate: never invoke the backend without a "From" wallet.
    // The button is already disabled, but guard here too in case of keyboard/script triggers.
    if (!connection.isConnected) {
      error = '🔌 Not connected — plug in your Heltec device first.';
      return;
    }
    if (!wallet.isGenerated) {
      error = '👛 No sender wallet loaded — go to the Wallet tab to generate or import one first.';
      return;
    }
    if (!canSend()) return;

    isSending = true;
    success = null;
    error = null;
    sendProgress = null;
    duplicateBlocked = false;

    try {
      const result = await invoke<SendOutcome>('send_transaction', {
        tx: {
          toAddress: toAddress.trim(),
          amountDoge: parseFloat(amountDoge),
          memo: memo.trim() || undefined,
          fromPrivateKeyWif: wallet.privateKeyWif || undefined,
          feeDoge: feeDoge.trim() ? parseFloat(feeDoge) : undefined,
          allowResend,
        }
      });

      success = result.message;
      if (result.txid) {
        if (_replyTimer) clearTimeout(_replyTimer);
        lastTx = { txid: result.txid.toLowerCase(), url: result.explorerUrl ?? '', status: 'waiting' };
        _replyTimer = setTimeout(() => {
          if (lastTx && (lastTx.status === 'waiting' || lastTx.status === 'queued')) lastTx.status = 'timeout';
          _replyTimer = null;
        }, REPLY_TIMEOUT_MS);
      }
      onTriggerConfetti(); // 🎉 BOOM — confetti!

      // Clear form 5s after success — gives user time to screenshot/note the result.
      // The timer ID is stored so it can be cancelled if the user starts a new
      // transaction during the 5s window (preventing their new inputs from being wiped).
      _successClearTimer = setTimeout(() => {
        toAddress = '';
        amountDoge = '';
        memo = '';
        success = null;
        _successClearTimer = null;
      }, 5000);

    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : errorMessage(e);
      duplicateBlocked = msg.startsWith('DUPLICATE:');
      error = duplicateBlocked ? msg.slice('DUPLICATE:'.length).trim() : msg;
    } finally {
      isSending = false;
      sendProgress = null;
    }
  }

  async function pasteAddress() {
    try {
      toAddress = await navigator.clipboard.readText();
    } catch {
      // No clipboard permission
    }
  }

  // ── QR code scanning (image file → Rust rqrr decoder) ─────────────────────
  let qrInput = $state<HTMLInputElement | null>(null);
  let scanning = $state(false);
  let scanNote = $state<string | null>(null);

  function openQrPicker() {
    scanNote = null;
    qrInput?.click();
  }

  async function onQrFileSelected(event: Event) {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    // Reset so selecting the same file again re-triggers change.
    input.value = '';
    if (!file) return;

    scanning = true;
    scanNote = null;
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const parsed = await invoke<ParsedPayment>('scan_qr_from_image', {
        imageBytes: Array.from(bytes),
      });
      toAddress = parsed.address;
      if (parsed.amount != null) amountDoge = String(parsed.amount);
      if (parsed.label && !memo) memo = parsed.label;
      scanNote = `✅ Scanned ${parsed.address.slice(0, 8)}…${parsed.amount != null ? ` · ${parsed.amount} DOGE` : ''}`;
    } catch (e: unknown) {
      scanNote = `❌ ${errorMessage(e)}`;
    } finally {
      scanning = false;
    }
  }
</script>

<div class="tab-page narrow">
  <div style="margin-bottom: 24px;">
    <h2 style="margin: 0 0 4px 0; font-size: var(--fs-xl); font-weight: 700;">📤 Send Dogecoin</h2>
    <p style="margin: 0; color: var(--doge-muted); font-size: var(--fs-base);">
      Broadcast via LoRa radio — no internet required 🚀
    </p>
  </div>

  <!-- Not connected warning -->
  {#if !connection.isConnected}
    <div style="
      padding: 16px;
      background: rgba(255, 68, 68, 0.08);
      border: 1px solid rgba(255, 68, 68, 0.25);
      border-radius: 12px;
      color: var(--doge-red);
      font-size: var(--fs-base);
      margin-bottom: 20px;
      text-align: center;
    ">
      🔌 Connect to a Heltec device first to send transactions.
    </div>
  {/if}

  <!-- Send form -->
  <div class="card-doge" style="display: flex; flex-direction: column; gap: 20px;">
    <!-- Recipient address -->
    <div>
      <label for="send-to-address" style="display: block; font-size: var(--fs-sm); color: var(--doge-muted); text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 8px;">
        Recipient Address
      </label>
      <div style="display: flex; gap: 8px;">
        <input
          id="send-to-address"
          bind:value={toAddress}
          placeholder="DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L"
          class="input-doge"
          style="flex: 1; border-color: {toAddress && !isValidAddress() ? 'var(--doge-red)' : 'var(--doge-border)'};"
        />
        <button onclick={pasteAddress} class="btn-ghost" style="padding: 10px 14px; flex-shrink: 0;">
          📋 Paste
        </button>
        <button
          onclick={openQrPicker}
          disabled={scanning}
          class="btn-ghost"
          style="padding: 10px 14px; flex-shrink: 0;"
          title="Scan a Dogecoin QR code from an image file"
        >
          {scanning ? '⏳' : '📷'} Scan QR
        </button>
        <!-- Hidden picker: accepts an image; `capture` hints a camera on mobile. -->
        <input
          bind:this={qrInput}
          onchange={onQrFileSelected}
          type="file"
          accept="image/*"
          capture="environment"
          style="display: none;"
          aria-hidden="true"
        />
      </div>
      {#if scanNote}
        <p style="font-size: var(--fs-xs); margin: 6px 0 0 0; color: {scanNote.startsWith('✅') ? 'var(--doge-neon)' : 'var(--doge-red)'};">
          {scanNote}
        </p>
      {/if}
      {#if toAddress && !isValidAddress()}
        <p style="color: var(--doge-red); font-size: var(--fs-xs); margin: 6px 0 0 0;">
          ⚠️ Dogecoin addresses start with "D" and are 25-34 characters
        </p>
      {:else if isValidAddress()}
        <p style="color: var(--doge-neon); font-size: var(--fs-xs); margin: 6px 0 0 0;">
          ✅ Valid Dogecoin address
        </p>
      {/if}
    </div>

    <!-- Amount -->
    <div>
      <label for="send-amount" style="display: block; font-size: var(--fs-sm); color: var(--doge-muted); text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 8px;">
        Amount (DOGE)
      </label>
      <div style="position: relative;">
        <input
          id="send-amount"
          bind:value={amountDoge}
          type="number"
          min="0"
          step="0.00000001"
          placeholder="100.00"
          class="input-doge"
          style="padding-right: 70px;"
        />
        <span style="
          position: absolute;
          right: 14px;
          top: 50%;
          transform: translateY(-50%);
          color: var(--doge-yellow);
          font-weight: 700;
          font-size: var(--fs-base);
        ">
          DOGE 🐕
        </span>
      </div>
      {#if amountDoge && !isValidAmount()}
        <p style="color: var(--doge-red); font-size: var(--fs-xs); margin: 6px 0 0 0;">
          ⚠️ Amount must be greater than 0
        </p>
      {/if}
    </div>

    <!-- Memo (optional) -->
    <div>
      <label for="send-memo" style="display: block; font-size: var(--fs-sm); color: var(--doge-muted); text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 8px;">
        Memo <span style="color: var(--doge-subtle); text-transform: none; font-size: var(--fs-xs);">(optional)</span>
      </label>
      <input
        id="send-memo"
        bind:value={memo}
        placeholder="such payment, very thanks, wow"
        class="input-doge"
        maxlength="100"
      />
    </div>

    <!-- From wallet indicator — required; blocks send if missing -->
    {#if wallet.isGenerated}
      <div
        title="This address will sign the transaction"
        style="
          display: flex;
          align-items: center;
          gap: 10px;
          padding: 10px 14px;
          background: rgba(245, 197, 24, 0.05);
          border: 1px solid rgba(245, 197, 24, 0.2);
          border-radius: 8px;
          font-size: var(--fs-sm);
        "
      >
        <span style="color: var(--doge-muted); flex-shrink: 0;">From:</span>
        <span class="mono" style="color: var(--doge-yellow); flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
          {wallet.address}
        </span>
        <span style="color: var(--doge-neon); font-size: var(--fs-2xs); flex-shrink: 0;" title="Wallet loaded ✅">✅</span>
      </div>
    {:else}
      <div style="
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 10px 14px;
        background: rgba(255, 68, 68, 0.06);
        border: 1px solid rgba(255, 68, 68, 0.25);
        border-radius: 8px;
        font-size: var(--fs-sm);
        color: var(--doge-red);
      ">
        <span style="flex-shrink: 0;">❌</span>
        <span style="flex: 1;">
          No sender wallet — <strong>required to sign transactions</strong>.
          Go to the <em>Wallet</em> tab to generate or import one.
        </span>
      </div>
    {/if}

    <!-- Fee summary (shown when wallet is loaded and amount is valid) -->
    {#if wallet.isGenerated && isValidAmount()}
      <div style="
        display: flex; align-items: center; justify-content: space-between;
        padding: 8px 14px;
        background: rgba(245, 197, 24, 0.04);
        border: 1px solid rgba(245, 197, 24, 0.15);
        border-radius: 8px;
        font-size: var(--fs-sm);
        color: var(--doge-muted);
      ">
        <label for="send-fee">Network fee (DOGE)</label>
        <input
          id="send-fee"
          bind:value={feeDoge}
          type="number"
          min="0"
          step="0.01"
          placeholder="auto (network rate)"
          class="input-doge"
          style="max-width: 190px; font-family: var(--font-mono); text-align: right;"
        />
      </div>
      {#if !feeDoge.trim()}
        <p style="margin: 4px 0 0; font-size: var(--fs-xs); color: var(--doge-muted); text-align: right;">
          {#if feeEstimating}
            Estimating network fee…
          {:else if feeEstimate}
            Will pay ≈ <span style="font-family: var(--font-mono); color: var(--doge-text);">{feeEstimate.feeDoge.toFixed(8)} DOGE</span>
            ({(feeEstimate.rateKoinusPerKb / 1e8).toFixed(4)} DOGE/kB, ~{feeEstimate.sizeBytes} B)
          {:else if feeError}
            <span title={feeError}>Network estimate unavailable — 0.01 DOGE per kB will be used.</span>
          {/if}
        </p>
      {/if}
      {#if !isValidFee()}
        <p style="margin: 4px 0 0; font-size: var(--fs-xs); color: var(--doge-red);">⚠️ Fee must be 0 or more DOGE</p>
      {/if}
    {/if}

    <!-- Offline signing (UTXO snapshot) -->
    {#if wallet.isGenerated}
      <details class="offline-snap" open={!!snapshot}>
        <summary>📴 Offline signing {snapshot ? (snapshotMatches ? '— snapshot loaded' : '— snapshot is for another address') : ''}</summary>
        <p class="snap-help">
          No internet here? On a connected machine run
          <code>radiodoge-cli utxo-snapshot -a &lt;your address&gt; -o utxos.json</code>,
          copy the file to this device and import it. Signing then uses it instead of the network;
          enter the fee by hand. The file holds only public chain data and is kept in memory only.
        </p>
        <div class="snap-row">
          <label class="btn-doge snap-btn">
            Import snapshot…
            <input type="file" accept=".json,application/json" onchange={importSnapshot} hidden />
          </label>
          {#if snapshot}
            <button class="btn-doge snap-btn" onclick={clearSnapshot}>Clear</button>
          {/if}
        </div>
        {#if snapshot}
          <p class="snap-status" class:warn={!snapshotMatches}>
            {snapshot.utxoCount} UTXO(s), {snapshot.totalDoge.toFixed(8)} DOGE, {snapshotAge(snapshot)}.
            {#if !snapshotMatches}
              It does not match the loaded wallet, so signing will use the network.
            {:else}
              Coins spent since the snapshot was taken make the transaction invalid — use a fresh one, and only one send per snapshot.
            {/if}
          </p>
        {/if}
        {#if snapshotError}
          <p class="snap-status warn">⚠️ {snapshotError}</p>
        {/if}
      </details>
    {/if}

    <!-- Send button -->
    <button
      onclick={reviewTransaction}
      disabled={!canSend()}
      class="btn-doge"
      title={
        !connection.isConnected ? 'Connect to a Heltec device first' :
        !wallet.isGenerated    ? 'Generate or import a wallet in the Wallet tab first' :
        !isValidAddress()      ? 'Enter a valid Dogecoin recipient address' :
        !isValidAmount()       ? 'Enter an amount greater than 0' :
        'Sign & broadcast this transaction over LoRa 🚀'
      }
      style="
        width: 100%;
        padding: 16px;
        font-size: var(--fs-lg);
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 10px;
        {isSending ? 'animation: pulse-glow 1s ease-in-out infinite;' : ''}
      "
    >
      {#if isSending && sendProgress}
        <DogeSpinner size="sm" message="" />
        Beaming part {sendProgress.sent} of {sendProgress.total}... 📡
      {:else if isSending}
        <DogeSpinner size="sm" message="" />
        Beaming into the radio ether... 📡
      {:else}
        🚀 Sign & Broadcast via Radio
      {/if}
    </button>
  </div>

  <!-- Success message -->
  {#if success}
    <div
      class="slide-up"
      style="
        margin-top: 16px;
        padding: 16px 20px;
        background: rgba(0, 255, 136, 0.1);
        border: 1px solid rgba(0, 255, 136, 0.4);
        border-radius: 12px;
        text-align: center;
      "
    >
      <div style="font-size: 2.5rem; margin-bottom: 8px;">🎉</div>
      <div style="color: var(--doge-neon); font-weight: 700; font-size: var(--fs-md); margin-bottom: 4px;">
        Transaction Sent! Much broadcast. Wow!
      </div>
      <div style="color: var(--doge-muted); font-size: var(--fs-sm);">
        {success}
      </div>
    </div>
  {/if}

  <!-- Error message -->
  {#if error}
    <div
      class="slide-up"
      style="
        margin-top: 16px;
        padding: 16px 20px;
        background: rgba(255, 68, 68, 0.1);
        border: 1px solid rgba(255, 68, 68, 0.3);
        border-radius: 12px;
      "
    >
      <div style="font-size: var(--fs-xl); margin-bottom: 8px;">😢</div>
      <div style="color: var(--doge-red); font-weight: 700; font-size: var(--fs-base); margin-bottom: 4px;">
        Sad! Send failed.
      </div>
      <div style="color: var(--doge-muted); font-size: var(--fs-sm); font-family: var(--font-mono);">
        {error}
      </div>
      {#if duplicateBlocked}
        <button class="btn-ghost" style="margin-top: 10px;" onclick={() => sendTransaction(true)} disabled={isSending}>
          Resend anyway
        </button>
      {/if}
    </div>
  {/if}

  <!-- Last signed transaction: txid, explorer link, gateway reply -->
  {#if lastTx}
    <div class="tx-status" style="margin-top: 16px; padding: 14px 18px; border: 1px solid var(--doge-border, rgba(255,255,255,0.15)); border-radius: 12px; font-size: var(--fs-sm);">
      <div style="font-weight: 700; margin-bottom: 6px;">
        {#if lastTx.status === 'waiting'}📡 Radioed — waiting for the gateway…
        {:else if lastTx.status === 'queued'}⏳ Gateway queued it (offline) — it will retry the broadcast.
        {:else if lastTx.status === 'acked'}✅ Broadcast confirmed by the gateway.
        {:else if lastTx.status === 'nacked'}❌ Rejected by the network: {lastTx.reason}. Do not resend this transaction.
        {:else}❔ No reply heard. It may still have been broadcast — check the explorer before resending.{/if}
      </div>
      <div style="font-family: var(--font-mono); word-break: break-all; color: var(--doge-muted);">{lastTx.txid}</div>
      <div style="display: flex; gap: 8px; margin-top: 8px;">
        <button class="btn-ghost" onclick={copyTxid}>{copied ? 'Copied' : 'Copy txid'}</button>
        {#if lastTx.url}
          <button class="btn-ghost" onclick={() => open(lastTx!.url)}>View on explorer</button>
        {/if}
      </div>
    </div>
  {/if}

  <!-- Info footer -->
  <div style="
    margin-top: 24px;
    padding: 12px 16px;
    background: var(--doge-card);
    border-radius: 8px;
    font-size: var(--fs-sm);
    color: var(--doge-subtle);
    line-height: 1.6;
  ">
    📡 <strong>How it works:</strong> Your transaction is packaged into a RadioDoge LoRa packet
    and sent to the Heltec device via serial. The device broadcasts it over the LoRa mesh network.
    Any node within radio range can relay it to a gateway connected to the Dogecoin network.
    Very decentralize. Much wow.
  </div>
</div>

<ConfirmDialog
  open={reviewing}
  title="Review transaction"
  confirmLabel="Sign & send over LoRa"
  onConfirm={confirmSend}
  onCancel={() => (reviewing = false)}
>
  <dl class="review">
    <dt>From</dt>
    <dd class="mono" title={wallet.address}>{shortAddr(wallet.address ?? '')}</dd>
    <dt>To</dt>
    <dd class="mono">{toAddress.trim()}</dd>
    <dt>Amount</dt>
    <dd><strong>{parseFloat(amountDoge || '0')} DOGE</strong></dd>
    <dt>Network fee</dt>
    <dd>{feeDoge.trim() ? `${parseFloat(feeDoge)} DOGE` : 'Recommended (≈0.01 DOGE)'}</dd>
    {#if memo.trim()}
      <dt>Memo</dt>
      <dd>{memo.trim()}</dd>
    {/if}
  </dl>
  {#if feeDoge.trim() && parseFloat(feeDoge) >= HIGH_FEE_DOGE}
    <p class="warn" role="alert">⚠️ That fee is far above the ~0.01 DOGE a normal transaction needs. Double-check it.</p>
  {/if}
  <p class="note">Dogecoin transactions can't be reversed once a gateway broadcasts them.</p>
</ConfirmDialog>

<style>
  .review { display: grid; grid-template-columns: auto 1fr; gap: 6px 14px; margin: 0; font-size: var(--fs-base); }
  .review dt { color: var(--doge-muted); }
  .review dd { margin: 0; overflow-wrap: anywhere; }
  .mono { font-family: var(--font-mono); font-size: var(--fs-sm); }
  .warn { margin: 12px 0 0; color: var(--doge-orange); font-size: var(--fs-sm); }
  .note { margin: 12px 0 0; color: var(--doge-muted); font-size: var(--fs-xs); }
  .offline-snap { font-size: var(--fs-sm); color: var(--doge-muted); border: 1px solid rgba(245, 197, 24, 0.15); border-radius: 8px; padding: 8px 14px; }
  .offline-snap summary { cursor: pointer; color: var(--doge-text); }
  .snap-help { margin: 8px 0; font-size: var(--fs-xs); }
  .snap-row { display: flex; gap: 8px; }
  .snap-btn { padding: 6px 12px; font-size: var(--fs-sm); cursor: pointer; }
  .snap-status { margin: 8px 0 0; font-size: var(--fs-xs); }
  .snap-status.warn { color: var(--doge-red); }
</style>

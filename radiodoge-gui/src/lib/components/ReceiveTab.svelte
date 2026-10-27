<script lang="ts">
  import { errorMessage } from '$lib/errors';
  /**
   * Receive tab — live packet monitor for ALL LoRa traffic (TX + RX).
   *
   * v0.3.1 improvements:
   *   - Clear TX (↑ green) / RX (↓ blue) direction labels with icons
   *   - TX/RX filter toggle buttons (show all / TX only / RX only)
   *   - Distinct colors and tooltips per direction and command type
   *   - Copy-to-clipboard on every packet row
   *   - "Much radio" Doge flair throughout
   */

  import { connection } from '$lib/stores/connection.svelte';
  import { radio, clearPackets, type PacketEntry } from '$lib/stores/radio.svelte';
  import { formatTimestamp, commandName, type TxInclusion } from '$lib/types';
  import { invoke } from '@tauri-apps/api/core';

  // ── On-chain check for incoming transactions ─────────────────────────────
  // A DOGE_TX heard over LoRa is unauthenticated: anyone in range can forge
  // one. The only proof of payment is the chain, so offer a one-click lookup.
  let chainChecks = $state<Record<string, { busy: boolean; text: string; ok?: boolean }>>({});

  function incomingTxid(decoded?: string): string | null {
    if (!decoded || !decoded.includes('NOT confirmed on-chain')) return null;
    return /txid=([0-9a-f]{64})/.exec(decoded)?.[1] ?? null;
  }

  async function checkOnChain(txid: string) {
    chainChecks[txid] = { busy: true, text: 'Checking…' };
    try {
      const r = await invoke<TxInclusion>('verify_tx_inclusion', { txid });
      chainChecks[txid] = r.confirmed
        ? { busy: false, ok: true, text: `✅ In block ${r.blockHeight ?? '?'} · ${r.confirmations} conf` }
        : { busy: false, ok: false, text: '⏳ Not in a block yet (mempool or unknown) — not proof of payment' };
    } catch (e) {
      chainChecks[txid] = { busy: false, ok: false, text: `❌ ${errorMessage(e)}` };
    }
  }

  // ── Filter state ─────────────────────────────────────────────────────────
  type Filter = 'all' | 'tx' | 'rx';
  let filter = $state<Filter>('all');

  let showRaw = $state(false);

  // ── Computed filtered list ────────────────────────────────────────────────
  const filteredPackets = $derived(() => {
    if (filter === 'tx') return radio.packets.filter(p => p.direction === 'TX');
    if (filter === 'rx') return radio.packets.filter(p => p.direction === 'RX');
    return radio.packets;
  });

  // ── Command badge helpers ─────────────────────────────────────────────────
  function commandColor(cmd: number): string {
    switch (cmd) {
      case 0x02: return 'rgba(68, 136, 255, 0.2)';  // PING — blue
      case 0x03: return 'rgba(245, 197, 24, 0.15)'; // MESSAGE — yellow
      case 0x04: return 'rgba(255, 140, 0, 0.15)';  // BROADCAST — orange
      case 0x10: return 'rgba(0, 255, 136, 0.15)';  // DOGE TX — green
      default:   return 'rgba(136, 136, 136, 0.15)';
    }
  }

  function commandTextColor(cmd: number): string {
    switch (cmd) {
      case 0x02: return 'var(--doge-blue)';
      case 0x03: return 'var(--doge-yellow)';
      case 0x04: return 'var(--doge-orange)';
      case 0x10: return 'var(--doge-neon)';
      default:   return 'var(--doge-muted)';
    }
  }

  function commandTooltip(cmd: number): string {
    const map: Record<number, string> = {
      0x00: 'GET ADDR — query device mesh address',
      0x01: 'SET ADDR — update device mesh address',
      0x02: 'PING — health check (much echo, very pong)',
      0x03: 'MESSAGE — direct text to a node',
      0x04: 'BROADCAST — text to ALL nodes in range',
      0x05: 'MULTIPART — large payload split across packets',
      0x10: 'DOGE TX — Dogecoin transaction over LoRa 🐕',
      0x11: 'BALANCE — UTXO balance request',
      0x20: 'FW VERSION — firmware version query',
    };
    return map[cmd] ?? `Unknown command 0x${cmd.toString(16).toUpperCase()}`;
  }

  // ── Export packet log ─────────────────────────────────────────────────────
  // OPTIMIZED FOR DESKTOP v0.3.3 – SAFE
  function exportPackets() {
    const rows = filteredPackets().map(p => ({
      timestamp:   new Date(p.timestamp * 1000).toISOString(),
      direction:   p.direction,
      command:     `0x${p.command.toString(16).toUpperCase().padStart(2, '0')} (${commandName(p.command)})`,
      source:      `${p.source.region}.${p.source.community}.${p.source.node}`,
      destination: `${p.destination.region}.${p.destination.community}.${p.destination.node}`,
      decoded:     p.decoded ?? '',
      payload_hex: p.payloadHex,
      hops:        p.hops ?? 0,
    }));
    const blob = new Blob([JSON.stringify(rows, null, 2)], { type: 'application/json' });
    const url  = URL.createObjectURL(blob);
    const a    = document.createElement('a');
    a.href     = url;
    a.download = `radiodoge-packets-${new Date().toISOString().slice(0, 19).replace(/:/g, '-')}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }

  // ── Copy state ───────────────────────────────────────────────────────────
  let copiedIndex = $state<number | null>(null);
  async function copyPacket(text: string, idx: number) {
    try {
      await navigator.clipboard.writeText(text);
      copiedIndex = idx;
      setTimeout(() => { copiedIndex = null; }, 1500);
    } catch {
      // Browser may block in some sandboxed envs — fail silently
    }
  }
</script>

<div class="tab-page wide">

  <!-- ── Header ──────────────────────────────────────────────────────────── -->
  <div style="display: flex; align-items: center; justify-content: space-between; margin-bottom: 20px; flex-wrap: wrap; gap: 12px;">
    <div>
      <h2 style="margin: 0 0 4px 0; font-size: var(--fs-xl); font-weight: 700;">📡 Live Packet Log</h2>
      <p style="margin: 0; color: var(--doge-muted); font-size: var(--fs-base);">
        All LoRa traffic — TX (↑ sent) and RX (↓ received)
      </p>
    </div>

    <div style="display: flex; gap: 8px; align-items: center; flex-wrap: wrap;">

      <!-- TX/RX filter pills -->
      <div
        role="group"
        aria-label="Filter packets by direction"
        style="display: flex; background: var(--doge-dark); border: 1px solid var(--doge-border); border-radius: 10px; overflow: hidden;"
      >
        {#each ([['all','All'],['tx','↑ TX'],['rx','↓ RX']] as const) as [val, label]}
          <button
            onclick={() => filter = val}
            aria-pressed={filter === val}
            title="Show {val === 'all' ? 'all packets' : val === 'tx' ? 'sent (TX) packets only' : 'received (RX) packets only'}"
            style="
              padding: 5px 14px;
              border: none;
              background: {filter === val ? 'rgba(245,197,24,0.15)' : 'transparent'};
              color: {filter === val ? 'var(--doge-yellow)' : 'var(--doge-muted)'};
              font-size: var(--fs-sm);
              font-weight: {filter === val ? 700 : 400};
              cursor: pointer;
              transition: all 0.15s;
              border-right: 1px solid var(--doge-border);
            "
          >{label}</button>
        {/each}
      </div>

      <!-- Raw hex toggle -->
      <label
        title="Toggle between human-readable decoded text and raw hex bytes"
        style="display: flex; align-items: center; gap: 6px; cursor: pointer; font-size: var(--fs-sm); color: var(--doge-muted);"
      >
        <input
          type="checkbox"
          bind:checked={showRaw}
          style="accent-color: var(--doge-yellow);"
          aria-label="Show raw hex"
        />
        Hex
      </label>

      <!-- Export button -->
      {#if radio.packets.length > 0}
        <button
          onclick={exportPackets}
          class="btn-ghost"
          title="Export visible packet log as JSON (includes all filtered packets)"
          style="padding: 5px 14px; font-size: var(--fs-sm);"
          aria-label="Export packet log"
        >
          💾 Export
        </button>
      {/if}

      <!-- Clear button -->
      {#if radio.packets.length > 0}
        <button
          onclick={clearPackets}
          class="btn-ghost"
          title="Clear all packets from the log"
          style="padding: 5px 14px; font-size: var(--fs-sm);"
          aria-label="Clear packet log"
        >
          🗑️ Clear
        </button>
      {/if}

      <!-- Stats badges -->
      <span
        title="Total RX packets received since connect"
        style="
          background: rgba(68, 136, 255, 0.1);
          color: var(--doge-blue);
          border: 1px solid rgba(68, 136, 255, 0.25);
          border-radius: 12px;
          padding: 4px 10px;
          font-size: var(--fs-xs);
          font-family: var(--font-mono);
          white-space: nowrap;
        "
      >
        ↓ {radio.totalReceived} RX
      </span>
      <span
        title="Total TX packets sent since connect"
        style="
          background: rgba(0, 255, 136, 0.1);
          color: var(--doge-neon);
          border: 1px solid rgba(0, 255, 136, 0.25);
          border-radius: 12px;
          padding: 4px 10px;
          font-size: var(--fs-xs);
          font-family: var(--font-mono);
          white-space: nowrap;
        "
      >
        ↑ {radio.totalSent} TX
      </span>
    </div>
  </div>

  <!-- ── Empty / disconnected states ────────────────────────────────────── -->
  {#if !connection.isConnected}
    <div style="text-align: center; padding: 60px 20px; color: var(--doge-muted);">
      <div style="font-size: 3rem; margin-bottom: 16px; animation: bounce-doge 2s ease-in-out infinite; display: inline-block;">🐕</div>
      <h3 style="margin: 0 0 8px 0; color: var(--doge-text);">Ears down</h3>
      <p style="margin: 0; font-size: var(--fs-base);">
        Connect to a Heltec device to monitor LoRa traffic.<br/>
        <span style="font-style: italic;">Such listen. Very wait. Wow.</span>
      </p>
    </div>

  {:else if filteredPackets().length === 0}
    <div style="text-align: center; padding: 60px 20px; color: var(--doge-muted);">
      <div style="font-size: 3rem; margin-bottom: 16px; display: inline-block; animation: bounce-doge 1.5s ease-in-out infinite;">🐕</div>
      <h3 style="margin: 0 0 8px 0; color: var(--doge-text);">
        {filter === 'all' ? 'Listening for packets...' : filter === 'tx' ? 'No TX packets yet' : 'No RX packets yet'}
      </h3>
      <p style="margin: 0; font-size: var(--fs-base);">
        {#if filter === 'tx'}
          Send a transaction to see TX packets here. 🐕
        {:else if filter === 'rx'}
          🐕 Ears up! Waiting for incoming LoRa transmissions.<br/>
          <span style="font-style: italic;">Such patience. Very radio. Wow.</span>
        {:else}
          🐕 Ears up! Waiting for LoRa traffic...<br/>
          <span style="font-style: italic;">Such patience. Very radio. Wow.</span>
        {/if}
      </p>
    </div>

  {:else}
    <!-- ── Packet list ────────────────────────────────────────────────── -->
    <div style="display: flex; flex-direction: column; gap: 6px;" role="log" aria-label="Packet log" aria-live="polite">
      {#each filteredPackets() as packet, i (packet.seq)}
        {@const isTx = packet.direction === 'TX'}
        <div
          class="slide-up card-doge"
          style="
            padding: 10px 14px;
            display: flex;
            gap: 10px;
            align-items: flex-start;
            border-left: 3px solid {isTx ? 'var(--doge-neon)' : 'var(--doge-blue)'};
          "
          aria-label="{isTx ? 'Sent' : 'Received'} packet at {formatTimestamp(packet.timestamp)}"
        >
          <!-- Direction indicator with icon + tooltip -->
          <span
            title="{isTx ? '↑ TX — Sent by this node over LoRa. Such broadcast, very radio.' : '↓ RX — Received from remote node. Much incoming, very packet.'}"
            style="
              font-size: var(--fs-base);
              font-weight: 700;
              flex-shrink: 0;
              color: {isTx ? 'var(--doge-neon)' : 'var(--doge-blue)'};
              min-width: 28px;
              text-align: center;
              padding-top: 1px;
            "
            aria-label="{isTx ? 'Transmitted' : 'Received'}"
          >
            {isTx ? '↑' : '↓'}
          </span>

          <!-- Timestamp -->
          <span style="
            font-family: var(--font-mono);
            font-size: var(--fs-xs);
            color: var(--doge-subtle);
            flex-shrink: 0;
            padding-top: 2px;
            white-space: nowrap;
          ">
            {formatTimestamp(packet.timestamp)}
          </span>

          <!-- Command badge with tooltip -->
          <span
            title={commandTooltip(packet.command)}
            style="
              background: {commandColor(packet.command)};
              color: {commandTextColor(packet.command)};
              border-radius: 4px;
              padding: 2px 7px;
              font-size: var(--fs-2xs);
              font-family: var(--font-mono);
              font-weight: 600;
              flex-shrink: 0;
              white-space: nowrap;
              cursor: help;
            "
          >
            {commandName(packet.command)}
          </span>

          <!-- Source address -->
          <span
            title="Source node address (Region.Community.Node)"
            style="
              font-family: var(--font-mono);
              font-size: var(--fs-xs);
              color: var(--doge-muted);
              flex-shrink: 0;
              padding-top: 2px;
              cursor: help;
            "
          >
            {packet.source.region}.{packet.source.community}.{packet.source.node}
          </span>

          <!-- Decoded / raw content -->
          <span style="
            flex: 1;
            font-size: var(--fs-sm);
            color: {isTx ? '#b3ffe0' : 'var(--doge-text)'};
            word-break: break-all;
            line-height: 1.4;
          ">
            {#if showRaw || !packet.decoded}
              <span style="font-family: var(--font-mono); color: var(--doge-muted); font-size: var(--fs-xs);">
                {packet.payloadHex}
              </span>
            {:else}
              {packet.decoded}
              {@const txid = incomingTxid(packet.decoded)}
              {#if txid}
                <span style="display: inline-flex; gap: 6px; align-items: center; margin-left: 6px;">
                  <button
                    class="btn-ghost"
                    style="padding: 1px 6px; font-size: var(--fs-2xs);"
                    title="Look this txid up on the chain — the only real proof of payment"
                    disabled={chainChecks[txid]?.busy}
                    onclick={() => checkOnChain(txid)}
                  >🔎 Check on-chain</button>
                  {#if chainChecks[txid]}
                    <span style="font-size: var(--fs-2xs); color: {chainChecks[txid].ok ? 'var(--doge-neon)' : 'var(--doge-muted)'};">
                      {chainChecks[txid].text}
                    </span>
                  {/if}
                </span>
              {/if}
            {/if}
          </span>

          <!-- Mesh hop count (relayed packets only) -->
          {#if !isTx && (packet.hops ?? 0) > 0}
            <span
              title="Relayed across {packet.hops} mesh hop{packet.hops === 1 ? '' : 's'} before reaching this node"
              style="
                font-family: var(--font-mono);
                font-size: var(--fs-2xs);
                color: var(--doge-yellow);
                background: rgba(255, 193, 7, 0.12);
                border: 1px solid rgba(255, 193, 7, 0.3);
                border-radius: 4px;
                padding: 1px 5px;
                flex-shrink: 0;
                white-space: nowrap;
                cursor: help;
              "
            >
              ⇄ {packet.hops} hop{packet.hops === 1 ? '' : 's'}
            </span>
          {/if}

          <!-- RSSI (only meaningful for RX packets) -->
          {#if !isTx}
            <span
              title="Received Signal Strength Indicator — {packet.rssi >= -80 ? 'Strong signal! Much wow.' : packet.rssi >= -100 ? 'Moderate signal' : 'Weak signal — move nodes closer!'}"
              style="
                font-family: var(--font-mono);
                font-size: var(--fs-2xs);
                color: {packet.rssi >= -80 ? 'var(--doge-neon)' : packet.rssi >= -100 ? 'var(--doge-yellow)' : 'var(--doge-red)'};
                flex-shrink: 0;
                white-space: nowrap;
                cursor: help;
              "
            >
              {packet.rssi} dBm
            </span>
          {:else}
            <span style="font-size: var(--fs-2xs); color: var(--doge-subtle); flex-shrink: 0; white-space: nowrap;">
              sent ↑
            </span>
          {/if}

          <!-- Copy button -->
          <button
            onclick={() => copyPacket(
              showRaw || !packet.decoded ? packet.payloadHex : (packet.decoded ?? packet.payloadHex),
              i
            )}
            class="btn-ghost"
            title="Copy {showRaw || !packet.decoded ? 'raw hex' : 'decoded text'} to clipboard"
            aria-label="Copy packet data"
            style="padding: 2px 6px; font-size: var(--fs-2xs); flex-shrink: 0;"
          >
            {copiedIndex === i ? '✅' : '📋'}
          </button>
        </div>
      {/each}
    </div>

    <!-- Footer summary -->
    <p style="
      margin-top: 14px;
      text-align: center;
      color: var(--doge-subtle);
      font-size: var(--fs-xs);
    ">
      Showing {filteredPackets().length} of {radio.packets.length} packets
      {filter !== 'all' ? `(${filter.toUpperCase()} filter active)` : ''}
      · {radio.totalReceived} total received · {radio.totalSent} total sent
    </p>
  {/if}
</div>

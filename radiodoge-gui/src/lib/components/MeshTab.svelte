<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { connection, setNeighbors, setAddrConflict } from '$lib/stores/connection.svelte';
  import { nodeAddressToString } from '$lib/types';

  let isRefreshing = $state(false);
  let lastRefreshed = $state<number | null>(null);

  async function refreshNeighbors() {
    if (!connection.isConnected) return;
    isRefreshing = true;
    try {
      const nbrs = await invoke<{ address: { region: number; community: number; node: number }; rssi: number; lastSeen: number }[]>('get_neighbors');
      setNeighbors(nbrs.map(n => ({
        address: n.address,
        rssi: n.rssi,
        lastSeen: n.lastSeen,
      })));
      lastRefreshed = Date.now();
    } catch (e) {
      console.error('get_neighbors error', e);
    } finally {
      isRefreshing = false;
    }
  }

  // ── Node discovery ──────────────────────────────────────────────────────
  let isScanning = $state(false);
  let scanResult = $state<string | null>(null);
  let lastScanFound = $state<Set<string>>(new Set());

  async function scanNetwork() {
    if (!connection.isConnected || isScanning) return;
    isScanning = true;
    scanResult = null;
    try {
      const found = await invoke<{ address: { region: number; community: number; node: number } }[]>('discover_nodes', { listenMs: 6000 });
      lastScanFound = new Set(found.map(f => nodeAddressToString(f.address)));
      scanResult = found.length
        ? `${found.length} node${found.length !== 1 ? 's' : ''} answered`
        : 'No nodes answered (they need discovery-capable firmware and the same LoRa settings)';
      await refreshNeighbors();
    } catch (e) {
      scanResult = `Scan failed: ${e}`;
    } finally {
      isScanning = false;
    }
  }

  // Radial network map: this node in the centre, neighbors spread evenly by
  // angle, distance from the centre growing as RSSI weakens. RSSI 0 means the
  // board did not report one (ACK notifications) — drawn on the outer ring.
  const MAP = 260, C = MAP / 2;
  function mapRadius(rssi: number): number {
    if (rssi === 0) return C - 18;
    const t = Math.min(1, Math.max(0, (-rssi - 40) / 80)); // -40 → 0, -120 → 1
    return 40 + t * (C - 60);
  }
  const mapNodes = $derived(connection.neighbors.map((n, i, all) => {
    const a = (2 * Math.PI * i) / Math.max(1, all.length) - Math.PI / 2;
    const r = mapRadius(n.rssi);
    const label = nodeAddressToString(n.address);
    return { label, rssi: n.rssi, x: C + r * Math.cos(a), y: C + r * Math.sin(a), fresh: lastScanFound.has(label) };
  }));
  function rssiColor(rssi: number): string {
    if (rssi === 0) return 'var(--doge-muted)';
    if (rssi >= -60) return '#22c55e';
    if (rssi >= -80) return '#eab308';
    if (rssi >= -100) return '#f97316';
    return '#ef4444';
  }

  async function dismissConflict() {
    await invoke('clear_addr_conflict').catch(() => {});
    setAddrConflict(false);
  }

  function rssiQuality(rssi: number): string {
    if (rssi >= -60) return '🟢 Strong';
    if (rssi >= -80) return '🟡 Good';
    if (rssi >= -100) return '🟠 Weak';
    return '🔴 Very weak';
  }

  function relativeTime(unixSec: number): string {
    const diff = Math.floor((Date.now() / 1000) - unixSec);
    if (diff < 10) return 'just now';
    if (diff < 60) return `${diff}s ago`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    return `${Math.floor(diff / 3600)}h ago`;
  }

  // Auto-refresh neighbors every 5 s while connected
  $effect(() => {
    if (!connection.isConnected) return;
    const id = setInterval(refreshNeighbors, 5000);
    refreshNeighbors(); // immediate first fetch
    return () => clearInterval(id);
  });
</script>

<div style="padding: 0 0 24px 0;">

  <!-- ── Address Conflict Banner ──────────────────────────────────────────── -->
  {#if connection.addrConflict}
    <div style="
      background: rgba(255,60,60,0.12);
      border: 1px solid rgba(255,60,60,0.5);
      border-radius: 10px;
      padding: 14px 18px;
      margin-bottom: 20px;
      display: flex;
      align-items: center;
      gap: 14px;
      flex-wrap: wrap;
    ">
      <span style="font-size: 1.4rem;">⚠️</span>
      <div style="flex: 1;">
        <p style="margin: 0; font-weight: 700; color: #ff6060; font-size: 0.95rem;">
          Duplicate Node Address Detected!
        </p>
        <p style="margin: 4px 0 0 0; font-size: 0.78rem; color: var(--doge-muted);">
          Another device on the mesh is broadcasting with address
          <strong style="color: var(--doge-yellow);">
            {connection.nodeAddress ? nodeAddressToString(connection.nodeAddress) : '?'}
          </strong>.
          Change your node address in Settings to avoid collisions.
        </p>
      </div>
      <button
        onclick={dismissConflict}
        style="
          padding: 6px 14px;
          border: 1px solid rgba(255,60,60,0.4);
          background: rgba(255,60,60,0.1);
          color: #ff8080;
          border-radius: 6px;
          cursor: pointer;
          font-size: 0.8rem;
        "
      >
        Dismiss
      </button>
    </div>
  {/if}

  <!-- ── Header ───────────────────────────────────────────────────────────── -->
  <div style="display: flex; align-items: center; justify-content: space-between; margin-bottom: 18px; flex-wrap: wrap; gap: 10px;">
    <div>
      <h2 style="margin: 0; font-size: 1.1rem; font-weight: 700;">🕸️ Mesh Neighbors</h2>
      <p style="margin: 4px 0 0 0; font-size: 0.75rem; color: var(--doge-muted);">
        Nodes recently heard on the LoRa mesh · auto-refreshes every 5 s
        {#if lastRefreshed}
          · last updated {relativeTime(lastRefreshed / 1000)}
        {/if}
      </p>
    </div>
    <div style="display: flex; gap: 8px;">
    <button
      onclick={scanNetwork}
      disabled={!connection.isConnected || isScanning}
      title="Broadcast a discovery ping and wait 6 s for nodes to answer"
      style="
        padding: 7px 16px;
        border: 1px solid var(--doge-yellow);
        background: transparent;
        color: var(--doge-yellow);
        border-radius: 8px;
        cursor: pointer;
        font-size: 0.82rem;
        opacity: {!connection.isConnected || isScanning ? 0.45 : 1};
      "
    >
      {isScanning ? '📡 Scanning…' : '📡 Scan network'}
    </button>
    <button
      onclick={refreshNeighbors}
      disabled={!connection.isConnected || isRefreshing}
      style="
        padding: 7px 16px;
        border: 1px solid var(--doge-border);
        background: transparent;
        color: var(--doge-yellow);
        border-radius: 8px;
        cursor: pointer;
        font-size: 0.82rem;
        opacity: {!connection.isConnected || isRefreshing ? 0.45 : 1};
      "
    >
      {isRefreshing ? '⏳ Refreshing…' : '🔄 Refresh'}
    </button>
    </div>
  </div>
  {#if scanResult}
    <p style="margin: -8px 0 14px; font-size: 0.78rem; color: var(--doge-muted);">{scanResult}</p>
  {/if}

  <!-- ── Network Map ──────────────────────────────────────────────────────── -->
  {#if connection.isConnected && connection.neighbors.length > 0}
    <div class="card-doge" style="margin-bottom: 16px; display: flex; justify-content: center;">
      <svg viewBox="0 0 {MAP} {MAP}" width={MAP} height={MAP} role="img" aria-label="Mesh network map">
        {#each [0.33, 0.66, 1] as f}
          <circle cx={C} cy={C} r={f * (C - 18)} fill="none" stroke="var(--doge-border)" stroke-dasharray="3 4" />
        {/each}
        {#each mapNodes as n}
          <line x1={C} y1={C} x2={n.x} y2={n.y} stroke={rssiColor(n.rssi)} stroke-opacity="0.5" />
        {/each}
        <circle cx={C} cy={C} r="9" fill="var(--doge-yellow)" />
        <text x={C} y={C + 22} text-anchor="middle" font-size="9" fill="var(--doge-text)">
          {connection.nodeAddress ? nodeAddressToString(connection.nodeAddress) : 'me'}
        </text>
        {#each mapNodes as n}
          <circle cx={n.x} cy={n.y} r="7" fill={rssiColor(n.rssi)} stroke={n.fresh ? 'var(--doge-yellow)' : 'none'} stroke-width="2">
            <title>{n.label} · {n.rssi === 0 ? 'RSSI n/a' : `${n.rssi} dBm`}</title>
          </circle>
          <text x={n.x} y={n.y - 11} text-anchor="middle" font-size="9" fill="var(--doge-muted)">{n.label}</text>
        {/each}
      </svg>
    </div>
  {/if}

  <!-- ── Neighbors Table ──────────────────────────────────────────────────── -->
  {#if !connection.isConnected}
    <div class="card-doge" style="text-align: center; padding: 32px; color: var(--doge-muted);">
      🔌 Connect to a board to see mesh neighbors.
    </div>
  {:else if connection.neighbors.length === 0}
    <div class="card-doge" style="text-align: center; padding: 32px; color: var(--doge-muted);">
      <p style="font-size: 1.3rem; margin: 0 0 8px 0;">🦴</p>
      <p style="margin: 0; font-size: 0.9rem;">No neighbors heard yet.</p>
      <p style="margin: 6px 0 0 0; font-size: 0.75rem; opacity: 0.7;">
        Waiting for other RadioDoge nodes to transmit on the mesh…
      </p>
    </div>
  {:else}
    <div class="card-doge" style="padding: 0; overflow: hidden;">
      <table style="width: 100%; border-collapse: collapse;">
        <thead>
          <tr style="border-bottom: 1px solid var(--doge-border); background: rgba(255,255,255,0.02);">
            <th style="padding: 10px 14px; text-align: left; font-size: 0.75rem; color: var(--doge-muted); font-weight: 600;">Node Address</th>
            <th style="padding: 10px 14px; text-align: left; font-size: 0.75rem; color: var(--doge-muted); font-weight: 600;">Signal</th>
            <th style="padding: 10px 14px; text-align: left; font-size: 0.75rem; color: var(--doge-muted); font-weight: 600;">RSSI</th>
            <th style="padding: 10px 14px; text-align: left; font-size: 0.75rem; color: var(--doge-muted); font-weight: 600;">Last Heard</th>
          </tr>
        </thead>
        <tbody>
          {#each connection.neighbors as nbr}
            <tr style="border-bottom: 1px solid rgba(255,255,255,0.04);">
              <td style="padding: 10px 14px; font-family: var(--font-mono); font-size: 0.88rem; color: var(--doge-yellow); font-weight: 700;">
                {nodeAddressToString(nbr.address)}
              </td>
              <td style="padding: 10px 14px; font-size: 0.8rem;">{nbr.rssi === 0 ? '⚪ Unknown' : rssiQuality(nbr.rssi)}</td>
              <td style="padding: 10px 14px; font-family: var(--font-mono); font-size: 0.8rem; color: var(--doge-muted);">{nbr.rssi === 0 ? 'n/a' : `${nbr.rssi} dBm`}</td>
              <td style="padding: 10px 14px; font-size: 0.8rem; color: var(--doge-muted);">{relativeTime(nbr.lastSeen)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <!-- ── My Node ───────────────────────────────────────────────────────────── -->
  {#if connection.isConnected && connection.nodeAddress}
    <div class="card-doge" style="margin-top: 16px; display: flex; align-items: center; gap: 12px; flex-wrap: wrap;">
      <span style="font-size: 1.2rem;">📍</span>
      <div>
        <p style="margin: 0; font-size: 0.82rem; color: var(--doge-muted);">This node</p>
        <p style="margin: 2px 0 0 0; font-family: var(--font-mono); font-size: 1rem; color: var(--doge-yellow); font-weight: 700;">
          {nodeAddressToString(connection.nodeAddress)}
        </p>
      </div>
      <div style="margin-left: auto; font-size: 0.75rem; color: var(--doge-muted);">
        {connection.neighbors.length} neighbor{connection.neighbors.length !== 1 ? 's' : ''} heard
      </div>
    </div>
  {/if}
</div>

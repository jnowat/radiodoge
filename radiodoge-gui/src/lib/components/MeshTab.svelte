<script lang="ts">
  import ChannelScan from './ChannelScan.svelte';
  import { bridge as scanBridge } from '$lib/device/connection-bridge';
  // Scanning drives the desktop serial link; the Android bridge is not wired for it.
  let isAndroidScan = $state(false);
  scanBridge.isAndroid().then((v) => (isAndroidScan = v)).catch(() => {});
  import { invoke } from '@tauri-apps/api/core';
  import { connection, setNeighbors, setAddrConflict } from '$lib/stores/connection.svelte';
  import { open } from '@tauri-apps/plugin-shell';
  import { nodeAddressToString, mapUrl, type NeighborEntry, type GeoPosition } from '$lib/types';
  import ShareLocation from './ShareLocation.svelte';

  let isRefreshing = $state(false);
  let lastRefreshed = $state<number | null>(null);

  async function refreshNeighbors() {
    if (!connection.isConnected) return;
    isRefreshing = true;
    try {
      const nbrs = await invoke<NeighborEntry[]>('get_neighbors');
      setNeighbors(nbrs.map(n => ({
        address: n.address,
        rssi: n.rssi,
        lastSeen: n.lastSeen,
        position: n.position,
        positionAt: n.positionAt,
      })));
      lastRefreshed = Date.now();
    } catch (e) {
      console.error('get_neighbors error', e);
    } finally {
      isRefreshing = false;
    }
  }

  async function dismissConflict() {
    await invoke('clear_addr_conflict').catch(() => {});
    setAddrConflict(false);
  }

  function fmtPos(p: GeoPosition): string {
    return `${p.lat.toFixed(5)}, ${p.lon.toFixed(5)}${p.accuracyM != null ? ` ±${p.accuracyM} m` : ''}`;
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
      <span style="font-size: var(--fs-xl);">⚠️</span>
      <div style="flex: 1;">
        <p style="margin: 0; font-weight: 700; color: #ff6060; font-size: var(--fs-md);">
          Duplicate Node Address Detected!
        </p>
        <p style="margin: 4px 0 0 0; font-size: var(--fs-sm); color: var(--doge-muted);">
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
          font-size: var(--fs-sm);
        "
      >
        Dismiss
      </button>
    </div>
  {/if}

  <!-- ── Header ───────────────────────────────────────────────────────────── -->
  <div style="display: flex; align-items: center; justify-content: space-between; margin-bottom: 18px; flex-wrap: wrap; gap: 10px;">
    <div>
      <h2 style="margin: 0; font-size: var(--fs-lg); font-weight: 700;">🕸️ Mesh Neighbors</h2>
      <p style="margin: 4px 0 0 0; font-size: var(--fs-xs); color: var(--doge-muted);">
        Nodes recently heard on the LoRa mesh · auto-refreshes every 5 s
        {#if lastRefreshed}
          · last updated {relativeTime(lastRefreshed / 1000)}
        {/if}
      </p>
    </div>
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
        font-size: var(--fs-sm);
        opacity: {!connection.isConnected || isRefreshing ? 0.45 : 1};
      "
    >
      {isRefreshing ? '⏳ Refreshing…' : '🔄 Refresh'}
    </button>
  </div>

  <!-- ── Neighbors Table ──────────────────────────────────────────────────── -->
  {#if !connection.isConnected}
    <div class="card-doge" style="text-align: center; padding: 32px; color: var(--doge-muted);">
      🔌 Connect to a board to see mesh neighbors.
    </div>
  {:else if connection.neighbors.length === 0}
    <div class="card-doge" style="text-align: center; padding: 32px; color: var(--doge-muted);">
      <p style="font-size: var(--fs-xl); margin: 0 0 8px 0;">🦴</p>
      <p style="margin: 0; font-size: var(--fs-base);">No neighbors heard yet.</p>
      <p style="margin: 6px 0 0 0; font-size: var(--fs-xs); opacity: 0.7;">
        Waiting for other RadioDoge nodes to transmit on the mesh…
      </p>
    </div>
  {:else}
    <div class="card-doge" style="padding: 0; overflow: hidden;">
      <table style="width: 100%; border-collapse: collapse;">
        <thead>
          <tr style="border-bottom: 1px solid var(--doge-border); background: rgba(255,255,255,0.02);">
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">Node Address</th>
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">Signal</th>
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">RSSI</th>
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">Last Heard</th>
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">Position</th>
          </tr>
        </thead>
        <tbody>
          {#each connection.neighbors as nbr, i}
            <tr style="border-bottom: 1px solid rgba(255,255,255,0.04);">
              <td style="padding: 10px 14px; font-family: var(--font-mono); font-size: var(--fs-base); color: var(--doge-yellow); font-weight: 700;">
                {nodeAddressToString(nbr.address)}
              </td>
              <td style="padding: 10px 14px; font-size: var(--fs-sm);">{rssiQuality(nbr.rssi)}</td>
              <td style="padding: 10px 14px; font-family: var(--font-mono); font-size: var(--fs-sm); color: var(--doge-muted);">{nbr.rssi} dBm</td>
              <td style="padding: 10px 14px; font-size: var(--fs-sm); color: var(--doge-muted);">{relativeTime(nbr.lastSeen)}</td>
              <td style="padding: 10px 14px; font-size: var(--fs-sm);">
                {#if nbr.position}
                  <button class="maplink" title="Open in OpenStreetMap{nbr.positionAt ? ` · shared ${relativeTime(nbr.positionAt)}` : ''}"
                    onclick={() => open(mapUrl(nbr.position!)).catch(() => {})}>📍 {fmtPos(nbr.position)}</button>
                {:else}
                  <span style="color: var(--doge-muted);">—</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <!-- ── My Node ───────────────────────────────────────────────────────────── -->
  {#if connection.isConnected && connection.nodeAddress}
    <div class="card-doge" style="margin-top: 16px; display: flex; align-items: center; gap: 12px; flex-wrap: wrap;">
      <span style="font-size: var(--fs-lg);">📍</span>
      <div>
        <p style="margin: 0; font-size: var(--fs-sm); color: var(--doge-muted);">This node</p>
        <p style="margin: 2px 0 0 0; font-family: var(--font-mono); font-size: var(--fs-md); color: var(--doge-yellow); font-weight: 700;">
          {nodeAddressToString(connection.nodeAddress)}
        </p>
      </div>
      <div style="margin-left: auto; font-size: var(--fs-xs); color: var(--doge-muted);">
        {connection.neighbors.length} neighbor{connection.neighbors.length !== 1 ? 's' : ''} heard
      </div>
    </div>
    <ShareLocation />
    {#if !isAndroidScan}<ChannelScan />{/if}
  {/if}
</div>

<style>
  .maplink {
    background: none; border: none; padding: 0; cursor: pointer;
    color: var(--doge-blue); font-family: var(--font-mono); font-size: var(--fs-sm); text-align: left;
  }
  .maplink:hover { text-decoration: underline; }
</style>

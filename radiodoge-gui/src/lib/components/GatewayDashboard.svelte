<!--
  Gateway dashboard — live counters from the `radiodoge-cli daemon` this app started.

  The daemon writes a JSON snapshot every few seconds (`--stats-file`); this
  component polls `get_gateway_stats` and renders it. Nothing here talks to the
  board or the network.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';

  type EventKind =
    | 'txReceived' | 'txBroadcast' | 'txFailed' | 'ackSent' | 'ackFailed'
    | 'balanceRequest' | 'balanceReplied' | 'balanceFailed';
  interface NodeTraffic { packets: number; transactions: number; lastSeen: number; lastRssi: number; lastHops: number }
  interface GatewayEvent { at: number; kind: EventKind; node: string; detail: string }
  interface GatewayStats {
    startedAt: number; updatedAt: number; port: string;
    gatewayAddress: string | null; firmware: string | null;
    packetsHeard: number; packetsRelayedIn: number;
    txReceived: number; txBroadcast: number; txDuplicates: number; txFailed: number;
    acksSent: number; acksFailed: number;
    balanceRequests: number; balanceReplies: number; balanceFailures: number;
    nodes: Record<string, NodeTraffic>;
    events: GatewayEvent[];
  }

  let running = $state(true);
  let stats = $state<GatewayStats | null>(null);
  let error = $state<string | null>(null);
  let now = $state(Math.floor(Date.now() / 1000));

  async function refresh() {
    try {
      const r = await invoke<{ running: boolean; stats: GatewayStats | null }>('get_gateway_stats');
      running = r.running;
      stats = r.stats;
      error = null;
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : String(e);
    }
    now = Math.floor(Date.now() / 1000);
  }

  onMount(() => {
    refresh();
    const id = setInterval(refresh, 3000);
    return () => clearInterval(id);
  });

  const EVENT_META: Record<EventKind, { icon: string; label: string; tone: string }> = {
    txReceived: { icon: '📥', label: 'Transaction received', tone: 'var(--doge-blue)' },
    txBroadcast: { icon: '🚀', label: 'Broadcast to network', tone: 'var(--doge-neon)' },
    txFailed: { icon: '⛔', label: 'Broadcast failed', tone: 'var(--doge-red)' },
    ackSent: { icon: '📡', label: 'TX_ACK sent', tone: 'var(--doge-neon)' },
    ackFailed: { icon: '⚠️', label: 'TX_ACK failed', tone: 'var(--doge-red)' },
    balanceRequest: { icon: '🔎', label: 'Balance request', tone: 'var(--doge-blue)' },
    balanceReplied: { icon: '💰', label: 'Balance replied', tone: 'var(--doge-yellow)' },
    balanceFailed: { icon: '⚠️', label: 'Balance failed', tone: 'var(--doge-red)' },
  };

  function ago(t: number): string {
    const s = Math.max(0, now - t);
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m ago`;
  }
  function uptime(t: number): string {
    const s = Math.max(0, now - t);
    const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
    return h ? `${h}h ${m}m` : `${m}m ${s % 60}s`;
  }
  function short(d: string): string {
    return /^[0-9a-f]{64}/.test(d) ? `${d.slice(0, 10)}…${d.slice(54, 64)}${d.slice(64)}` : d;
  }

  const successRate = $derived.by(() => {
    if (!stats) return null;
    const done = stats.txBroadcast + stats.txFailed;
    return done ? Math.round((stats.txBroadcast * 100) / done) : null;
  });
  const nodes = $derived(
    stats ? Object.entries(stats.nodes).sort((a, b) => b[1].lastSeen - a[1].lastSeen) : [],
  );
  const stale = $derived(stats ? now - stats.updatedAt > 15 : false);
</script>

<div class="card-doge gw" aria-label="Gateway dashboard">
  <div class="head">
    <div>
      <div class="title">📊 Gateway Dashboard</div>
      <div class="sub">
        {#if stats}
          {stats.gatewayAddress ?? '—'} on {stats.port}{stats.firmware ? ` · ${stats.firmware}` : ''} · up {uptime(stats.startedAt)}
        {:else}
          Waiting for the daemon's first snapshot…
        {/if}
      </div>
    </div>
    <span class="pill" class:off={!running || stale}>
      {#if !running}⚫ Daemon exited{:else if stale}🟡 No update for {now - (stats?.updatedAt ?? now)}s{:else}🟢 Live{/if}
    </span>
  </div>

  {#if error}
    <div class="err" role="alert">❌ {error}</div>
  {/if}
  {#if !running}
    <div class="note" role="status">The daemon is no longer running. Press <b>Gateway Online — Stop</b> to give the port back to the app.</div>
  {/if}

  {#if stats}
    <div class="tiles">
      <div class="tile"><div class="v">{stats.packetsHeard}</div><div class="k">Packets heard</div>
        <div class="h">{stats.packetsRelayedIn} via relays</div></div>
      <div class="tile"><div class="v">{stats.txReceived}</div><div class="k">Transactions in</div>
        <div class="h">{stats.txDuplicates} repeats re-ACKed</div></div>
      <div class="tile"><div class="v" style="color: var(--doge-neon)">{stats.txBroadcast}</div><div class="k">Broadcast OK</div>
        <div class="h">{successRate === null ? 'no attempts yet' : `${successRate}% success`}</div></div>
      <div class="tile"><div class="v" class:bad={stats.txFailed > 0}>{stats.txFailed}</div><div class="k">Broadcast failed</div>
        <div class="h">after 3 attempts each</div></div>
      <div class="tile"><div class="v">{stats.acksSent}</div><div class="k">ACKs radioed</div>
        <div class="h">{stats.acksFailed} failed</div></div>
      <div class="tile"><div class="v">{stats.balanceReplies}/{stats.balanceRequests}</div><div class="k">Balance replies</div>
        <div class="h">{stats.balanceFailures} failed</div></div>
    </div>

    <div class="cols">
      <section>
        <h4>Nodes heard ({nodes.length})</h4>
        {#if nodes.length === 0}
          <p class="empty">No packets yet — such quiet. 🦴</p>
        {:else}
          <table>
            <thead><tr><th>Node</th><th>Pkts</th><th>TXs</th><th>RSSI</th><th>Hops</th><th>Last</th></tr></thead>
            <tbody>
              {#each nodes as [addr, n] (addr)}
                <tr>
                  <td class="mono">{addr}</td><td>{n.packets}</td><td>{n.transactions}</td>
                  <td>{n.lastRssi ? `${n.lastRssi} dBm` : 'n/a'}</td><td>{n.lastHops}</td><td>{ago(n.lastSeen)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>
      <section>
        <h4>Recent activity</h4>
        {#if stats.events.length === 0}
          <p class="empty">Nothing relayed yet.</p>
        {:else}
          <ul class="feed">
            {#each stats.events.slice(0, 15) as ev, i (ev.at + ':' + i)}
              <li>
                <span class="ic">{EVENT_META[ev.kind]?.icon ?? '•'}</span>
                <span class="lbl" style="color: {EVENT_META[ev.kind]?.tone ?? 'inherit'}">{EVENT_META[ev.kind]?.label ?? ev.kind}</span>
                <span class="mono node">{ev.node}</span>
                {#if ev.detail}<span class="mono det" title={ev.detail}>{short(ev.detail)}</span>{/if}
                <span class="when">{ago(ev.at)}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .gw { display: flex; flex-direction: column; gap: 12px; }
  .head { display: flex; justify-content: space-between; align-items: flex-start; gap: 12px; }
  .title { font-weight: 700; color: var(--doge-yellow); }
  .sub { font-size: 0.75rem; color: var(--doge-muted); margin-top: 2px; }
  .pill { font-size: 0.72rem; padding: 3px 10px; border-radius: 999px; white-space: nowrap;
    background: rgba(0, 255, 136, 0.1); color: var(--doge-neon); border: 1px solid rgba(0, 255, 136, 0.35); }
  .pill.off { background: rgba(100, 100, 100, 0.1); color: var(--doge-subtle); border-color: var(--doge-border); }
  .err { color: var(--doge-red); font-size: 0.78rem; }
  .note { color: var(--doge-muted); font-size: 0.78rem; }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(130px, 1fr)); gap: 8px; }
  .tile { border: 1px solid var(--doge-border); border-radius: 10px; padding: 10px 12px; background: rgba(255, 255, 255, 0.02); }
  .v { font-size: 1.35rem; font-weight: 800; color: var(--doge-text); font-variant-numeric: tabular-nums; }
  .v.bad { color: var(--doge-red); }
  .k { font-size: 0.72rem; color: var(--doge-muted); margin-top: 2px; }
  .h { font-size: 0.68rem; color: var(--doge-subtle); margin-top: 4px; }
  .cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 12px; }
  h4 { margin: 0 0 6px; font-size: 0.78rem; color: var(--doge-muted); font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; }
  table { width: 100%; border-collapse: collapse; font-size: 0.75rem; }
  th, td { text-align: left; padding: 5px 6px; border-bottom: 1px solid var(--doge-border); }
  th { color: var(--doge-subtle); font-weight: 600; }
  .mono { font-family: ui-monospace, Consolas, monospace; }
  .empty { margin: 0; color: var(--doge-subtle); font-size: 0.78rem; }
  .feed { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .feed li { display: flex; gap: 8px; align-items: baseline; font-size: 0.75rem; padding: 4px 0; border-bottom: 1px dashed var(--doge-border); }
  .lbl { font-weight: 600; white-space: nowrap; }
  .node { color: var(--doge-text); }
  .det { color: var(--doge-subtle); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; flex: 1; }
  .when { margin-left: auto; color: var(--doge-subtle); white-space: nowrap; }
</style>

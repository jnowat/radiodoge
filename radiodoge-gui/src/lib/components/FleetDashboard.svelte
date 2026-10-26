<!--
  v0.3.21 — Multi-gateway dashboard.

  Aggregates this PC's gateway snapshot with any other gateways' `--stats-file`
  snapshots the user points at (e.g. a shared/synced folder each daemon writes to).
  Read-only: nothing here talks to a board or the network.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';

  interface GatewayRow {
    label: string; path: string; local: boolean; ok: boolean; error?: string;
    online?: boolean; updatedAt?: number; port?: string; address?: string | null;
    packetsHeard?: number; txBroadcast?: number; txFailed?: number; outbox?: number;
  }
  interface FleetNode { packets: number; transactions: number; lastSeen: number; heardBy: string[]; bestGateway: string; bestRssi: number }
  interface FleetEvent { gateway: string; at: number; kind: string; node: string; detail: string }
  interface FleetSummary {
    gateways: number; online: number; packetsHeard: number; txReceived: number; txBroadcast: number;
    txDuplicates: number; txFailed: number; acksSent: number; balanceReplies: number;
    outboxBroadcasts: number; outboxReplies: number;
    nodes: Record<string, FleetNode>; events: FleetEvent[];
  }

  const KEY = 'radiodoge.fleetPaths';
  function loadPaths(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem(KEY) ?? '[]');
      return Array.isArray(v) ? v.filter((p) => typeof p === 'string') : [];
    } catch { return []; }
  }
  let paths = $state<string[]>(loadPaths());
  let draft = $state('');
  let rows = $state<GatewayRow[]>([]);
  let summary = $state<FleetSummary | null>(null);
  let now = $state(Math.floor(Date.now() / 1000));
  let error = $state<string | null>(null);

  function save() { localStorage.setItem(KEY, JSON.stringify(paths)); refresh(); }
  function add() {
    const p = draft.trim().replace(/^"(.*)"$/, '$1');
    if (p && !paths.includes(p)) { paths = [...paths, p]; save(); }
    draft = '';
  }
  function remove(p: string) { paths = paths.filter((x) => x !== p); save(); }

  async function refresh() {
    try {
      const r = await invoke<{ now: number; gateways: GatewayRow[]; summary: FleetSummary }>('get_fleet_stats', { paths });
      rows = r.gateways; summary = r.summary; now = r.now; error = null;
    } catch (e) { error = String(e); }
  }
  onMount(() => { refresh(); const id = setInterval(refresh, 5000); return () => clearInterval(id); });

  function ago(t: number): string {
    const s = Math.max(0, now - t);
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    return `${Math.floor(s / 3600)}h ago`;
  }
  const nodes = $derived(summary ? Object.entries(summary.nodes).sort((a, b) => b[1].lastSeen - a[1].lastSeen) : []);
  const rate = $derived.by(() => {
    if (!summary) return null;
    const d = summary.txBroadcast + summary.txFailed;
    return d ? Math.round((summary.txBroadcast * 100) / d) : null;
  });
</script>

<div class="card-doge fleet" aria-label="Multi-gateway dashboard">
  <div class="head">
    <div>
      <div class="title">🛰️ Gateway Fleet</div>
      <div class="sub">Combine several gateways' stats files. Point each daemon's <code>--stats-file</code> at a shared folder, then add the files here.</div>
    </div>
    {#if summary}<span class="pill" class:off={summary.online === 0}>{summary.online}/{summary.gateways} online</span>{/if}
  </div>

  <div class="add">
    <input bind:value={draft} placeholder="Path to another gateway's gateway-stats.json"
      onkeydown={(e) => e.key === 'Enter' && add()} aria-label="Stats file path" />
    <button class="ghost" onclick={add} disabled={!draft.trim()}>Add</button>
  </div>
  {#if error}<div class="err" role="alert">❌ {error}</div>{/if}

  {#if rows.length === 0}
    <p class="empty">No gateways yet. Start the gateway on this PC or add another gateway's stats file.</p>
  {:else}
    <table>
      <thead><tr><th>Gateway</th><th>Status</th><th>Pkts</th><th>TX ok/fail</th><th>Outbox</th><th></th></tr></thead>
      <tbody>
        {#each rows as g (g.path)}
          <tr>
            <td title={g.path}><b>{g.label}</b>{#if g.address && g.address !== g.label} <span class="mono muted">{g.address}</span>{/if}</td>
            {#if g.ok}
              <td>{g.online ? '🟢' : '🟡'} {ago(g.updatedAt ?? 0)}</td>
              <td>{g.packetsHeard}</td><td>{g.txBroadcast}/{g.txFailed}</td>
              <td class:warn={(g.outbox ?? 0) > 0}>{g.outbox}</td>
            {:else}
              <td colspan="4" class="bad" title={g.error}>⚠️ {g.error}</td>
            {/if}
            <td>{#if !g.local}<button class="x" onclick={() => remove(g.path)} aria-label="Remove {g.label}">✕</button>{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if summary && summary.gateways > 0}
    <div class="tiles">
      <div class="tile"><div class="v">{summary.packetsHeard}</div><div class="k">Packets heard</div></div>
      <div class="tile"><div class="v">{summary.txReceived}</div><div class="k">Transactions in</div><div class="h">{summary.txDuplicates} repeats</div></div>
      <div class="tile"><div class="v" style="color: var(--doge-neon)">{summary.txBroadcast}</div><div class="k">Broadcast OK</div>
        <div class="h">{rate === null ? 'no attempts yet' : `${rate}% success`}</div></div>
      <div class="tile"><div class="v">{summary.acksSent}</div><div class="k">ACKs radioed</div></div>
      <div class="tile"><div class="v" class:warn={summary.outboxBroadcasts + summary.outboxReplies > 0}>{summary.outboxBroadcasts} · {summary.outboxReplies}</div><div class="k">Outbox (tx · replies)</div></div>
      <div class="tile"><div class="v">{nodes.length}</div><div class="k">Unique nodes</div></div>
    </div>

    <div class="cols">
      <section>
        <h4>Nodes across the fleet</h4>
        {#if nodes.length === 0}<p class="empty">No nodes heard yet.</p>{:else}
          <table>
            <thead><tr><th>Node</th><th>Pkts</th><th>Best gateway</th><th>Heard by</th><th>Last</th></tr></thead>
            <tbody>
              {#each nodes as [addr, n] (addr)}
                <tr><td class="mono">{addr}</td><td>{n.packets}</td>
                  <td>{n.bestGateway}{n.bestRssi ? ` (${n.bestRssi} dBm)` : ''}</td>
                  <td>{n.heardBy.length}</td><td>{ago(n.lastSeen)}</td></tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>
      <section>
        <h4>Recent activity</h4>
        {#if summary.events.length === 0}<p class="empty">Nothing relayed yet.</p>{:else}
          <ul class="feed">
            {#each summary.events.slice(0, 15) as ev, i (ev.gateway + ev.at + i)}
              <li><span class="gw">{ev.gateway}</span><span class="lbl">{ev.kind}</span><span class="mono">{ev.node}</span><span class="when">{ago(ev.at)}</span></li>
            {/each}
          </ul>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .fleet { display: flex; flex-direction: column; gap: 12px; }
  .head { display: flex; justify-content: space-between; gap: 12px; align-items: flex-start; }
  .title { font-weight: 700; color: var(--doge-yellow); }
  .sub { font-size: var(--fs-xs); color: var(--doge-muted); margin-top: 2px; }
  .pill { font-size: var(--fs-xs); padding: 3px 10px; border-radius: 999px; white-space: nowrap;
    background: rgba(0, 255, 136, 0.1); color: var(--doge-neon); border: 1px solid rgba(0, 255, 136, 0.35); }
  .pill.off { background: rgba(100, 100, 100, 0.1); color: var(--doge-subtle); border-color: var(--doge-border); }
  .add { display: flex; gap: 8px; }
  .add input { flex: 1; min-width: 0; }
  .err, .bad { color: var(--doge-red); font-size: var(--fs-xs); }
  .warn { color: var(--doge-yellow); }
  .muted { color: var(--doge-subtle); }
  .empty { margin: 0; color: var(--doge-subtle); font-size: var(--fs-sm); }
  table { width: 100%; border-collapse: collapse; font-size: var(--fs-xs); }
  th, td { text-align: left; padding: 5px 6px; border-bottom: 1px solid var(--doge-border); }
  th { color: var(--doge-subtle); font-weight: 600; }
  .mono { font-family: ui-monospace, Consolas, monospace; }
  .x { background: none; border: none; color: var(--doge-subtle); cursor: pointer; }
  .x:hover { color: var(--doge-red); }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 8px; }
  .tile { border: 1px solid var(--doge-border); border-radius: 10px; padding: 10px 12px; background: rgba(255, 255, 255, 0.02); }
  .v { font-size: var(--fs-xl); font-weight: 800; font-variant-numeric: tabular-nums; }
  .k { font-size: var(--fs-xs); color: var(--doge-muted); margin-top: 2px; }
  .h { font-size: var(--fs-2xs); color: var(--doge-subtle); margin-top: 4px; }
  .cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 12px; }
  h4 { margin: 0 0 6px; font-size: var(--fs-sm); color: var(--doge-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .feed { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .feed li { display: flex; gap: 8px; font-size: var(--fs-xs); padding: 4px 0; border-bottom: 1px dashed var(--doge-border); }
  .gw { color: var(--doge-blue); font-weight: 600; }
  .lbl { font-weight: 600; }
  .when { margin-left: auto; color: var(--doge-subtle); }
</style>

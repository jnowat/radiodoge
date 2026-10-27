<!--
  v0.3.22 — Multi-channel scan (ROADMAP: multi-channel scanning).
  Hops the board across frequency/SF presets, listens on each, and shows what it
  heard. The board always returns to the saved LoRa settings afterwards.
-->
<script lang="ts">
  import { errorMessage } from '$lib/errors';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { connection } from '$lib/stores/connection.svelte';

  interface ScanChannel { label: string; frequencyMhz: number; spreadingFactor: number; bandwidthKhz: number }
  interface ChannelResult { packets: number; nodes: Record<string, number>; bestRssi: number | null }
  interface Row { channel: ScanChannel; result?: ChannelResult; error?: string }
  interface ScanOutcome { results: Row[]; cancelled: boolean; restored: boolean;
    home: { frequencyMhz: number; spreadingFactor: number; bandwidthKhz: number } }

  let region = $state<'us915' | 'eu868'>('us915');
  let plan = $state<ScanChannel[]>([]);
  let picked = $state<Record<string, boolean>>({});
  let dwell = $state(15);
  let running = $state(false);
  let progress = $state<{ index: number; total: number; label?: string } | null>(null);
  let outcome = $state<ScanOutcome | null>(null);
  let error = $state<string | null>(null);

  async function loadPlan() {
    plan = await invoke<ScanChannel[]>('scan_plan', { region });
    picked = Object.fromEntries(plan.map((c) => [c.label, true]));
  }
  onMount(() => {
    loadPlan().catch((e) => (error = errorMessage(e)));
    const un = listen<{ index: number; total: number; label?: string }>('scan-progress', (e) => (progress = e.payload));
    return () => { un.then((f) => f()); };
  });

  const chosen = $derived(plan.filter((c) => picked[c.label]));
  const eta = $derived(chosen.length * (dwell + 2));

  async function start() {
    running = true; error = null; outcome = null; progress = null;
    try {
      outcome = await invoke<ScanOutcome>('scan_channels', { channels: chosen, dwellSecs: dwell });
    } catch (e) { error = errorMessage(e); }
    finally { running = false; progress = null; }
  }
  function cancel() { invoke('cancel_scan').catch(() => {}); }
  const best = $derived.by(() => {
    if (!outcome) return null;
    const ok = outcome.results.filter((r) => r.result && r.result.packets > 0);
    ok.sort((a, b) => (b.result!.bestRssi ?? -999) - (a.result!.bestRssi ?? -999));
    return ok[0]?.channel.label ?? null;
  });
</script>

<section class="card-doge scan" aria-label="Channel scan">
  <div class="head">
    <h3>📻 Channel scan</h3>
    <p>Listen on several frequency/SF combinations to find RadioDoge nodes on other settings.
      The board goes back to your saved LoRa settings afterwards. It only decodes RadioDoge packets, so a Meshtastic preset finds RadioDoge nodes on that channel, not Meshtastic traffic.</p>
  </div>
  <div class="row">
    <label>Region
      <select bind:value={region} onchange={loadPlan} disabled={running}>
        <option value="us915">US 915</option><option value="eu868">EU 868</option>
      </select></label>
    <label>Listen per channel
      <select bind:value={dwell} disabled={running}>
        {#each [5, 15, 30, 60, 120] as s}<option value={s}>{s}s</option>{/each}
      </select></label>
    <span class="muted">≈ {Math.ceil(eta / 60)} min for {chosen.length} channels</span>
  </div>
  <div class="chips">
    {#each plan as c (c.label)}
      <label class="chip" class:on={picked[c.label]}>
        <input type="checkbox" bind:checked={picked[c.label]} disabled={running} />
        {c.label} <span class="muted">{c.frequencyMhz} MHz · {c.bandwidthKhz} kHz</span>
      </label>
    {/each}
  </div>
  <div class="row">
    {#if running}
      <button class="btn-ghost" onclick={cancel}>⏹ Stop scan</button>
      {#if progress && progress.index < progress.total}<span>Listening on <b>{progress.label}</b> ({progress.index + 1}/{progress.total})…</span>{/if}
    {:else}
      <button class="btn-doge" onclick={start} disabled={!connection.isConnected || chosen.length === 0}>▶ Start scan</button>
      {#if !connection.isConnected}<span class="muted">Connect a board first.</span>{/if}
    {/if}
  </div>
  {#if error}<div class="err" role="alert">❌ {error}</div>{/if}
  {#if outcome}
    <p class:err={!outcome.restored} class="muted">
      {outcome.cancelled ? 'Scan stopped. ' : ''}{outcome.restored
        ? `Board returned to ${outcome.home.frequencyMhz} MHz SF${outcome.home.spreadingFactor}.`
        : '⚠️ Could not confirm the board returned to your saved settings. Re-apply them in Settings.'}
    </p>
    <table>
      <thead><tr><th>Channel</th><th>Packets</th><th>Nodes</th><th>Best RSSI</th></tr></thead>
      <tbody>
        {#each outcome.results as r (r.channel.label)}
          <tr class:best={r.channel.label === best}>
            <td>{r.channel.label}{r.channel.label === best ? ' ⭐' : ''}</td>
            {#if r.result}
              <td>{r.result.packets}</td>
              <td class="mono">{Object.keys(r.result.nodes).join(', ') || '—'}</td>
              <td>{r.result.bestRssi != null ? `${r.result.bestRssi} dBm` : '—'}</td>
            {:else}<td colspan="3" class="err">{r.error}</td>{/if}
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  .scan { display: flex; flex-direction: column; gap: 10px; }
  h3 { margin: 0; color: var(--doge-yellow); font-size: var(--fs-md); }
  .head p { margin: 4px 0 0; font-size: var(--fs-xs); color: var(--doge-muted); }
  .row { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; font-size: var(--fs-sm); }
  label { display: flex; gap: 6px; align-items: center; font-size: var(--fs-sm); }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chip { border: 1px solid var(--doge-border); border-radius: 999px; padding: 3px 10px; cursor: pointer; }
  .chip.on { border-color: var(--doge-yellow); }
  .chip input { margin: 0; }
  .muted { color: var(--doge-subtle); font-size: var(--fs-xs); }
  .err { color: var(--doge-red); font-size: var(--fs-xs); }
  table { width: 100%; border-collapse: collapse; font-size: var(--fs-xs); }
  th, td { text-align: left; padding: 5px 6px; border-bottom: 1px solid var(--doge-border); }
  th { color: var(--doge-subtle); }
  tr.best td { color: var(--doge-neon); }
  .mono { font-family: ui-monospace, Consolas, monospace; }
</style>

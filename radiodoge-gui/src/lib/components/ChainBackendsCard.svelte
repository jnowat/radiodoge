<script lang="ts">
  /**
   * Settings card: which services the app uses to read balances and broadcast
   * transactions. Order matters; the first backend that answers wins.
   */
  import { BACKEND_PRESETS, applyBackendSpec, loadBackendSpec } from '$lib/settings/chainBackends';

  let spec = $state(loadBackendSpec());
  let busy = $state(false);
  let message = $state<{ kind: 'ok' | 'err'; text: string } | null>(null);

  async function apply(next = spec) {
    busy = true;
    message = null;
    try {
      await applyBackendSpec(next);
      spec = next.trim();
      message = { kind: 'ok', text: spec ? `Using: ${spec}` : 'Using the default order (Core, then BlockCypher).' };
    } catch (e) {
      message = { kind: 'err', text: String(e) };
    } finally {
      busy = false;
    }
  }
</script>

<div class="card-doge" aria-label="Chain backends">
  <h3 class="title">⛓️ Chain backends</h3>
  <p class="hint">
    Where balances, UTXOs and broadcasts go. Comma-separated, tried in order: <code>core</code> (your Dogecoin Core
    RPC), <code>blockcypher</code>, <code>blockbook</code>, or a URL such as <code>core=http://127.0.0.1:22555</code>.
    Core credentials come from <code>RADIODOGE_RPC_USER</code>/<code>RADIODOGE_RPC_PASSWORD</code> or Core's
    <code>.cookie</code> file.
  </p>

  <div class="presets" role="group" aria-label="Presets">
    {#each BACKEND_PRESETS as p (p.label)}
      <button type="button" class="btn-ghost preset" class:sel={spec === p.spec} disabled={busy} onclick={() => apply(p.spec)}>
        {p.label}
      </button>
    {/each}
  </div>

  <form class="row" onsubmit={(e) => { e.preventDefault(); apply(); }}>
    <label class="sr-only" for="backend-spec">Custom backend list</label>
    <input id="backend-spec" class="input-doge mono" placeholder="core,blockcypher" bind:value={spec} disabled={busy} spellcheck="false" />
    <button type="submit" class="btn-doge" disabled={busy}>{busy ? 'Applying…' : 'Apply'}</button>
  </form>

  {#if message}
    <p class="msg {message.kind}" role={message.kind === 'err' ? 'alert' : 'status'}>{message.text}</p>
  {/if}
</div>

<style>
  .title { margin: 0 0 6px; font-size: 0.95rem; font-weight: 700; }
  .hint { margin: 0 0 12px; font-size: 0.78rem; color: var(--doge-muted); line-height: 1.5; }
  .hint code { font-family: var(--font-mono); font-size: 0.74rem; color: var(--doge-text); }
  .presets { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 10px; }
  .preset { font-size: 0.75rem; padding: 5px 10px; }
  .preset.sel { border-color: var(--doge-yellow); color: var(--doge-yellow); }
  .row { display: flex; gap: 8px; }
  .row input { flex: 1; }
  .mono { font-family: var(--font-mono); }
  .msg { margin: 10px 0 0; font-size: 0.78rem; }
  .msg.ok { color: var(--doge-neon); }
  .msg.err { color: var(--doge-red); }
  .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
</style>

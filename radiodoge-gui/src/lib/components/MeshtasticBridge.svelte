<script lang="ts">
  // v0.3.25 — Meshtastic bridge mode (firmware FW12 raw LoRa). Opt-in and volatile:
  // the board's saved RadioDoge settings never change, raw receive times out on its
  // own, and the board leaves it automatically before any RadioDoge transmission.
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onDestroy } from 'svelte';

  type Decoded = { kind: string; from?: string; text?: string; rssi: number; snr: number };
  type Heard = Decoded & { at: number };

  let freqKhz = $state(906875);
  let minutes = $state(10);
  let active = $state(false);
  let busy = $state(false);
  let message = $state('');
  let status = $state<{ ok: boolean; text: string } | null>(null);
  let heard = $state<Heard[]>([]);
  let offTimer: ReturnType<typeof setTimeout> | null = null;

  const unlisten = listen<{ command: number; payloadHex: string }>('radio-packet', async (ev) => {
    if (ev.payload.command === 0x2c) active = ev.payload.payloadHex === '01';
    if (ev.payload.command !== 0x2d) return;
    const d = await invoke<Decoded | null>('meshtastic_bridge_decode', { payloadHex: ev.payload.payloadHex }).catch(() => null);
    if (d) heard = [{ ...d, at: Date.now() }, ...heard].slice(0, 50);
  });
  onDestroy(() => {
    unlisten.then((u) => u());
    if (offTimer) clearTimeout(offTimer);
  });

  async function toggle() {
    busy = true;
    status = null;
    try {
      const timeoutS = Math.max(1, Math.min(60, minutes)) * 60;
      active = await invoke<boolean>('meshtastic_bridge_listen', { enable: !active, freqKhz, timeoutS });
      if (offTimer) clearTimeout(offTimer);
      if (active) offTimer = setTimeout(() => (active = false), timeoutS * 1000);
      status = { ok: true, text: active ? `Listening on LongFast for ${minutes} min.` : 'Back to normal RadioDoge receive.' };
    } catch (e) {
      status = { ok: false, text: String(e) };
    } finally {
      busy = false;
    }
  }

  async function send() {
    busy = true;
    status = null;
    try {
      const ok = await invoke<boolean>('meshtastic_bridge_send_text', { text: message, freqKhz });
      status = ok ? { ok: true, text: 'Sent to LongFast.' } : { ok: false, text: 'The board reported the transmit failed.' };
      if (ok) message = '';
    } catch (e) {
      status = { ok: false, text: String(e) };
    } finally {
      busy = false;
    }
  }
</script>

<div class="card-doge bridge">
  <div class="head">
    <h3>🌉 Meshtastic bridge <span class="tag">experimental</span></h3>
    <span class="dot" class:on={active} aria-label={active ? 'Listening' : 'Idle'}></span>
  </div>
  <p class="hint">
    Hear and talk to the public Meshtastic LongFast channel through this board's raw LoRa mode
    (firmware v0.4.4 / FW12). Temporary: saved radio settings are untouched, and the board
    returns to RadioDoge on its own. Not yet tested on hardware.
  </p>
  <div class="row">
    <label>Frequency (kHz) <input type="number" bind:value={freqKhz} min="150000" max="960000" step="125" /></label>
    <label>Minutes <input type="number" bind:value={minutes} min="1" max="60" /></label>
    <button class="btn-doge" onclick={toggle} disabled={busy}>{active ? 'Stop listening' : 'Listen'}</button>
  </div>
  <div class="row">
    <input class="grow" type="text" maxlength="200" placeholder="Message to LongFast" aria-label="Message to LongFast" bind:value={message} />
    <button class="btn-doge" onclick={send} disabled={busy || !message.trim()}>Send</button>
  </div>
  {#if status}<p class="status" class:err={!status.ok} role="status">{status.text}</p>{/if}
  {#if heard.length}
    <ul class="heard">
      {#each heard as h (h.at)}
        <li>
          <span class="mono">{h.from ?? '—'}</span>
          <span class="txt">{h.kind === 'undecodable' ? 'frame on another channel' : h.kind === 'radiodoge' ? `RadioDoge ${h.text}` : h.text}</span>
          <span class="sig">{h.rssi} dBm · {h.snr} dB</span>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .bridge { margin-top: var(--sp-4); }
  .head { display: flex; align-items: center; justify-content: space-between; }
  h3 { margin: 0; font-size: var(--fs-md); }
  .tag { font-size: var(--fs-2xs); color: var(--doge-muted); border: 1px solid currentColor; border-radius: var(--radius-sm); padding: 0 6px; margin-left: 6px; font-weight: 500; }
  .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--doge-subtle); }
  .dot.on { background: var(--doge-neon-dim); }
  .hint { margin: 6px 0 10px; font-size: var(--fs-sm); color: var(--doge-muted); }
  .row { display: flex; gap: 8px; align-items: end; flex-wrap: wrap; margin-bottom: 8px; }
  label { display: flex; flex-direction: column; font-size: var(--fs-xs); color: var(--doge-muted); gap: 2px; }
  input { width: 9em; }
  .grow { flex: 1; min-width: 12em; }
  .status { font-size: var(--fs-sm); margin: 4px 0; color: var(--doge-neon-dim); }
  .status.err { color: var(--doge-red); }
  .heard { list-style: none; margin: 8px 0 0; padding: 0; max-height: 220px; overflow: auto; font-size: var(--fs-sm); }
  .heard li { display: flex; gap: 10px; padding: 4px 0; border-top: 1px solid var(--doge-border); }
  .mono { font-family: var(--font-mono); color: var(--doge-yellow); }
  .txt { flex: 1; word-break: break-word; }
  .sig { color: var(--doge-muted); font-size: var(--fs-xs); white-space: nowrap; }
</style>

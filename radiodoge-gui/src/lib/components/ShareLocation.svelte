<script lang="ts">
  // v0.3.19 — Broadcast this node's GPS position to the mesh (ROADMAP: GPS location embedding).
  // Opt-in only: nothing is sent until the user presses Broadcast.
  import { invoke } from '@tauri-apps/api/core';
  import { open } from '@tauri-apps/plugin-shell';
  import { mapUrl } from '$lib/types';
  import { bridge } from '$lib/device/connection-bridge';

  // Android has no desktop serial loop: build the packet in Rust, write it over USB/BLE.
  let isAndroidPlatform = $state(false);
  bridge.isAndroid().then((v) => (isAndroidPlatform = v)).catch(() => {});

  let lat = $state('');
  let lon = $state('');
  let accuracy = $state<number | null>(null);
  let locating = $state(false);
  let sending = $state(false);
  let status = $state<{ ok: boolean; text: string } | null>(null);

  const parsed = $derived.by(() => {
    const a = Number(lat), o = Number(lon);
    if (lat.trim() === '' || lon.trim() === '' || !Number.isFinite(a) || !Number.isFinite(o)) return null;
    if (a < -90 || a > 90 || o < -180 || o > 180) return null;
    return { lat: a, lon: o, accuracyM: accuracy ?? undefined };
  });

  function useDeviceLocation() {
    status = null;
    if (!('geolocation' in navigator)) {
      status = { ok: false, text: 'This device has no location service. Enter coordinates by hand.' };
      return;
    }
    locating = true;
    navigator.geolocation.getCurrentPosition(
      (p) => {
        lat = p.coords.latitude.toFixed(5);
        lon = p.coords.longitude.toFixed(5);
        accuracy = Math.round(p.coords.accuracy);
        locating = false;
      },
      (e) => {
        locating = false;
        status = { ok: false, text: `Location unavailable (${e.message || 'denied'}). Enter coordinates by hand.` };
      },
      { enableHighAccuracy: true, timeout: 15000, maximumAge: 60000 },
    );
  }

  async function broadcast() {
    if (!parsed) return;
    sending = true;
    status = null;
    try {
      const args = { lat: parsed.lat, lon: parsed.lon, accuracyM: parsed.accuracyM ?? null };
      let text: string;
      if (isAndroidPlatform) {
        const [t, bytes] = await invoke<[string, number[]]>('mobile_build_position', args);
        await bridge.mobileSendBytes(new Uint8Array(bytes));
        text = t;
      } else {
        text = await invoke<string>('share_position', args);
      }
      status = { ok: true, text: `Broadcast ${text}` };
    } catch (e) {
      status = { ok: false, text: String(e) };
    } finally {
      sending = false;
    }
  }
</script>

<section class="card-doge share">
  <div class="head">
    <h3>📍 Share my location</h3>
    <p>For disaster response: other nodes see this position in their neighbor list.
      <b>LoRa is not encrypted.</b> Anyone in range can read it, so only share when you mean to.</p>
  </div>
  <div class="row">
    <label>Latitude <input bind:value={lat} oninput={() => (accuracy = null)} placeholder="e.g. 44.97781" inputmode="decimal" /></label>
    <label>Longitude <input bind:value={lon} oninput={() => (accuracy = null)} placeholder="e.g. -93.26504" inputmode="decimal" /></label>
    <button class="ghost" onclick={useDeviceLocation} disabled={locating}>{locating ? '⏳ Locating…' : '🛰️ Use device location'}</button>
  </div>
  <div class="row">
    {#if parsed}
      <button class="ghost link" onclick={() => open(mapUrl(parsed)).catch(() => {})}>Preview on map ↗</button>
      {#if accuracy != null}<span class="muted">±{accuracy} m</span>{/if}
    {:else if lat || lon}
      <span class="bad">Latitude must be −90…90 and longitude −180…180.</span>
    {/if}
    <button class="primary" onclick={broadcast} disabled={!parsed || sending}>{sending ? 'Sending…' : '📡 Broadcast position'}</button>
  </div>
  {#if status}
    <p class="status" class:bad={!status.ok} role="status">{status.ok ? '✅' : '❌'} {status.text}</p>
  {/if}
</section>

<style>
  .share { margin-top: 16px; display: flex; flex-direction: column; gap: 10px; }
  .head h3 { margin: 0; font-size: 0.95rem; }
  .head p { margin: 4px 0 0; font-size: 0.75rem; color: var(--doge-muted); line-height: 1.45; }
  .row { display: flex; gap: 10px; align-items: flex-end; flex-wrap: wrap; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 0.72rem; color: var(--doge-muted); }
  input {
    width: 150px; padding: 7px 10px; border-radius: 8px; border: 1px solid var(--doge-border);
    background: rgba(0,0,0,0.25); color: var(--doge-text); font-family: var(--font-mono); font-size: 0.85rem;
  }
  button { padding: 7px 14px; border-radius: 8px; font-size: 0.82rem; cursor: pointer; }
  button:disabled { opacity: 0.45; cursor: default; }
  .ghost { border: 1px solid var(--doge-border); background: transparent; color: var(--doge-yellow); }
  .link { color: var(--doge-blue); }
  .primary { margin-left: auto; border: none; background: var(--doge-yellow); color: #111; font-weight: 700; }
  .muted { font-size: 0.75rem; color: var(--doge-muted); }
  .bad { color: var(--doge-red); font-size: 0.78rem; }
  .status { margin: 0; font-size: 0.8rem; }
</style>

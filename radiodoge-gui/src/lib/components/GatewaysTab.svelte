<script lang="ts">
  // Gateways tab: this PC's live gateway stats plus the multi-gateway fleet view.
  // Start/stop and gateway settings stay in Settings; this tab is read-only.
  import { connection } from '$lib/stores/connection.svelte';
  import { bridge } from '$lib/device/connection-bridge';
  import GatewayDashboard from './GatewayDashboard.svelte';
  import FleetDashboard from './FleetDashboard.svelte';
  import EmptyState from './EmptyState.svelte';
  let isAndroid = $state<boolean | null>(null);
  $effect(() => {
    let cancelled = false;
    bridge.isAndroid().then((v) => { if (!cancelled) isAndroid = v; }).catch(() => {});
    return () => { cancelled = true; };
  });
</script>

<div class="tab-page wide gateways">
  <header class="tab-header"><div>
    <h2 class="tab-title">🛰️ Gateways</h2>
    <p class="tab-sub">Live stats for the gateway on this PC and any other gateways you track. Start or stop the gateway in Settings.</p></div>
  </header>
  {#if connection.gatewayOnline}
    <GatewayDashboard />
  {:else}
    <EmptyState icon="📡" title="Gateway is not running on this PC" hint="Turn on gateway mode in Settings to relay transactions heard over LoRa." />
  {/if}
  {#if isAndroid === false}
    <FleetDashboard />
  {/if}
</div>

<style>
  .gateways { display: flex; flex-direction: column; gap: var(--sp-4); }
  .gateways .tab-header { margin-bottom: 0; }
</style>

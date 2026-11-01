<script lang="ts">
  /**
   * App navigation.
   *
   * Wide windows (desktop / Windows app): a grouped left sidebar with a live
   * device-status card at the bottom. Narrow windows (Android, small windows):
   * a compact icon bar across the top. Tabs that need a board are disabled
   * until one is connected.
   */
  import { connection } from '$lib/stores/connection.svelte';
  import SignalBars from './SignalBars.svelte';

  interface Props {
    activeTab: string;
    onTabChange: (tab: string) => void;
  }

  let { activeTab, onTabChange }: Props = $props();

  type Tab = { id: string; icon: string; label: string; tooltip: string; needsBoard: boolean };

  const groups: { title: string; tabs: Tab[] }[] = [
    {
      title: 'Device',
      tabs: [
        { id: 'connect', icon: '🔌', label: 'Connect', tooltip: 'Connect to a Heltec board over USB serial or BLE', needsBoard: false },
        { id: 'dashboard', icon: '📡', label: 'Dashboard', tooltip: 'Live radio stats: RSSI, SNR, packet counts', needsBoard: true },
        { id: 'mesh', icon: '🕸️', label: 'Mesh', tooltip: 'Nodes heard on the LoRa network', needsBoard: false },
        { id: 'receive', icon: '📥', label: 'Radio log', tooltip: 'Live TX/RX packet log', needsBoard: true },
        { id: 'gateways', icon: '🛰️', label: 'Gateways', tooltip: 'Gateway relay stats and the multi-gateway fleet', needsBoard: false },
      ],
    },
    {
      title: 'Wallet',
      tabs: [
        { id: 'wallet', icon: '👛', label: 'Wallet', tooltip: 'Create or import a Dogecoin wallet; balance and receive QR', needsBoard: true },
        { id: 'send', icon: '📤', label: 'Send', tooltip: 'Sign a transaction and relay it over LoRa', needsBoard: true },
        { id: 'history', icon: '📜', label: 'History', tooltip: 'Recent sends and their confirmations', needsBoard: false },
        { id: 'addrbook', icon: '📒', label: 'Addresses', tooltip: 'Saved, labelled Dogecoin addresses', needsBoard: false },
      ],
    },
    {
      title: 'App',
      tabs: [{ id: 'settings', icon: '⚙️', label: 'Settings', tooltip: 'LoRa radio, gateway, chain backends, theme', needsBoard: false }],
    },
  ];

  const statusLabel = $derived(
    connection.isConnected
      ? 'Connected'
      : connection.isReconnecting
        ? 'Reconnecting…'
        : connection.isConnecting
          ? 'Connecting…'
          : 'Not connected',
  );
  const statusKind = $derived(
    connection.isConnected ? 'ok' : connection.isConnecting || connection.isReconnecting ? 'busy' : 'off',
  );
  const nodeAddr = $derived(
    connection.nodeAddress
      ? `${connection.nodeAddress.region}.${connection.nodeAddress.community}.${connection.nodeAddress.node}`
      : null,
  );
</script>

<nav class="nav" aria-label="Main navigation">
  <div class="brand">
    <img src="/doge-radio.png" alt="" width="34" height="34" class="brand-logo" />
    <div class="brand-text">
      <div class="brand-name">RadioDoge</div>
      <div class="brand-sub">Dogecoin over LoRa</div>
    </div>
  </div>

  <div class="groups">
    {#each groups as group (group.title)}
      <div class="group" role="group" aria-label={group.title}>
        <div class="group-title">{group.title}</div>
        {#each group.tabs as tab (tab.id)}
          {@const isActive = activeTab === tab.id}
          {@const isDisabled = tab.needsBoard && !connection.isConnected}
          <button
            type="button"
            class="tab-btn"
            class:active={isActive}
            disabled={isDisabled}
            aria-current={isActive ? 'page' : undefined}
            title={isDisabled ? 'Connect a board first' : tab.tooltip}
            onclick={() => onTabChange(tab.id)}
          >
            <span class="tab-icon" aria-hidden="true">{tab.icon}</span>
            <span class="tab-label">{tab.label}</span>
            {#if tab.id === 'mesh' && connection.addrConflict}
              <span class="dot warn" title="Address conflict detected"></span>
            {/if}
          </button>
        {/each}
      </div>
    {/each}
  </div>

  <button
    type="button"
    class="status-card status-{statusKind}"
    onclick={() => onTabChange('connect')}
    title="Open connection panel"
    aria-live="polite"
  >
    <div class="status-row">
      <span class="dot {statusKind}" aria-hidden="true"></span>
      <span class="status-label">{statusLabel}</span>
      {#if connection.isConnected}
        <span class="status-bars"><SignalBars rssi={connection.stats?.rssi ?? -120} connected={true} size="sm" /></span>
      {/if}
    </div>
    {#if connection.isConnected}
      <div class="status-detail">
        <span class="mono">{connection.portName}</span>{#if nodeAddr}<span> · node {nodeAddr}</span>{/if}
      </div>
      <div class="chips">
        {#if connection.gatewayMode}<span class="chip ok" title="Board is in gateway mode">Gateway</span>{/if}
        {#if connection.gatewayOnline}<span class="chip accent" title="radiodoge-cli daemon is running">Daemon</span>{/if}
        {#if connection.firmwareVersion}<span class="chip">{connection.firmwareVersion}</span>{/if}
      </div>
    {:else if connection.error}
      <div class="status-detail err">{connection.error}</div>
    {/if}
  </button>
</nav>

<style>
  .nav {
    display: flex;
    flex-direction: column;
    width: 228px;
    flex-shrink: 0;
    height: 100%;
    background: var(--doge-nav);
    border-right: 1px solid var(--doge-border);
    padding: 16px 12px 12px;
    gap: 16px;
    overflow-y: auto;
    scrollbar-width: thin;
  }
  .brand { display: flex; align-items: center; gap: 10px; padding: 0 6px; }
  .brand-logo { border-radius: 9px; filter: drop-shadow(0 0 6px rgba(245, 197, 24, 0.45)); }
  .brand-name {
    font-weight: 800;
    font-size: 1.02rem;
    letter-spacing: 0.02em;
    background: linear-gradient(90deg, var(--doge-yellow), var(--doge-orange));
    -webkit-background-clip: text;
    background-clip: text;
    -webkit-text-fill-color: transparent;
  }
  .brand-sub { font-size: var(--fs-2xs); color: var(--doge-muted); letter-spacing: 0.04em; }

  .groups { display: flex; flex-direction: column; gap: 14px; flex: 1; }
  .group { display: flex; flex-direction: column; gap: 2px; }
  .group-title {
    font-size: 0.64rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--doge-subtle);
    padding: 0 10px 4px;
  }
  .tab-btn {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 38px;
    padding: 7px 10px;
    border: 1px solid transparent;
    border-radius: 9px;
    background: transparent;
    color: var(--doge-muted);
    font: inherit;
    font-size: 0.86rem;
    text-align: left;
    cursor: pointer;
    transition: background 0.12s, color 0.12s, border-color 0.12s;
    touch-action: manipulation;
  }
  .tab-btn:hover:not(:disabled) { background: var(--doge-card-hover); color: var(--doge-text); }
  .tab-btn.active {
    background: rgba(245, 197, 24, 0.12);
    border-color: rgba(245, 197, 24, 0.28);
    color: var(--doge-yellow);
    font-weight: 600;
  }
  .tab-btn:disabled { opacity: 0.4; cursor: not-allowed; }
  .tab-btn:focus-visible { outline: 2px solid var(--doge-yellow); outline-offset: 1px; }
  .tab-icon { width: 20px; text-align: center; font-size: var(--fs-md); }
  .tab-label { flex: 1; white-space: nowrap; }

  .dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; background: var(--doge-subtle); }
  .dot.ok { background: var(--doge-neon); box-shadow: 0 0 8px var(--doge-neon); }
  .dot.busy { background: var(--doge-yellow); animation: pulse-glow 1s ease-in-out infinite; }
  .dot.off { background: var(--doge-red); }
  .dot.warn { background: var(--doge-orange); }

  .status-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 100%;
    padding: 10px 12px;
    border-radius: 12px;
    border: 1px solid var(--doge-border);
    background: var(--doge-card);
    color: var(--doge-text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .status-card:hover { border-color: var(--doge-border-active); }
  .status-card:focus-visible { outline: 2px solid var(--doge-yellow); }
  .status-ok { border-color: rgba(0, 255, 136, 0.3); }
  .status-row { display: flex; align-items: center; gap: 8px; }
  .status-label { flex: 1; font-size: var(--fs-sm); font-weight: 600; }
  .status-detail { font-size: var(--fs-2xs); color: var(--doge-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .status-detail.err { color: var(--doge-red); white-space: normal; }
  .mono { font-family: var(--font-mono); }
  .chips { display: flex; flex-wrap: wrap; gap: 4px; }
  .chip {
    font-size: var(--fs-2xs);
    font-weight: 700;
    letter-spacing: 0.04em;
    padding: 2px 7px;
    border-radius: 999px;
    border: 1px solid var(--doge-border);
    color: var(--doge-muted);
  }
  .chip.ok { color: var(--doge-neon); border-color: rgba(0, 255, 136, 0.3); }
  .chip.accent { color: var(--doge-yellow); border-color: rgba(245, 197, 24, 0.3); }

  /* Narrow windows / phones: horizontal icon bar on top. */
  @media (max-width: 860px) {
    .nav {
      flex-direction: row;
      align-items: center;
      width: 100%;
      height: calc(56px + env(safe-area-inset-top, 0px));
      padding: env(safe-area-inset-top, 0px) 10px 0;
      border-right: none;
      border-bottom: 1px solid var(--doge-border);
      overflow-x: auto;
      overflow-y: hidden;
      gap: 8px;
      scrollbar-width: none;
    }
    .nav::-webkit-scrollbar { display: none; }
    .brand-text, .group-title, .status-detail, .chips, .status-bars { display: none; }
    .groups, .group { flex-direction: row; gap: 2px; }
    .groups { flex: 1; }
    .tab-btn { width: auto; min-height: 44px; }
    .status-card { width: auto; flex-shrink: 0; padding: 6px 10px; }
  }
  @media (max-width: 640px) {
    .tab-label, .status-label { display: none; }
  }
</style>

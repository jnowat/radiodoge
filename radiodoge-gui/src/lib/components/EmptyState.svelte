<script lang="ts">
  // Shared empty / loading placeholder so every tab's "nothing here yet" card
  // has the same spacing, typography and screen-reader behaviour.
  import type { Snippet } from 'svelte';
  interface Props {
    icon?: string;
    title: string;
    hint?: string;
    loading?: boolean;
    size?: 'md' | 'lg';
    children?: Snippet;
  }
  let { icon = '', title, hint = '', loading = false, size = 'md', children }: Props = $props();
</script>

<div class="card-doge empty-state" class:lg={size === 'lg'} role="status" aria-live="polite" aria-busy={loading}>
  {#if loading}
    <span class="spinner" aria-hidden="true"></span>
  {:else if icon}
    <div class="icon" aria-hidden="true">{icon}</div>
  {/if}
  <p class="title">{title}</p>
  {#if hint}<p class="hint">{hint}</p>{/if}
  {#if children}
    <div class="actions">
      {@render children()}
    </div>
  {/if}
</div>

<style>
  .empty-state { display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: 6px; padding: 32px 20px; text-align: center; color: var(--doge-muted); }
  .empty-state.lg { padding: 60px 20px; }
  .icon { font-size: var(--fs-xl); line-height: 1; margin-bottom: 4px; }
  .lg .icon { font-size: 3.5rem; margin-bottom: 10px; }
  .title { margin: 0; font-size: var(--fs-base); color: var(--doge-text); font-weight: 600; }
  .hint { margin: 0; font-size: var(--fs-xs); opacity: 0.75; max-width: 46ch; line-height: 1.5; }
  .actions { margin-top: 12px; display: flex; flex-wrap: wrap; gap: 8px; justify-content: center; }
  .spinner { width: 22px; height: 22px; border-radius: 50%; border: 3px solid var(--doge-border);
    border-top-color: var(--doge-gold, #c2a633); animation: spin 0.8s linear infinite; margin-bottom: 4px; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @media (prefers-reduced-motion: reduce) { .spinner { animation: none; } }
</style>

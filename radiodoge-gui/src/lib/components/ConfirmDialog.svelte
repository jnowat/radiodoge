<script lang="ts">
  /**
   * Accessible modal confirmation dialog built on <dialog>.
   * Escape or the backdrop cancels; focus starts on Cancel so Enter never
   * confirms by accident.
   */
  import type { Snippet } from 'svelte';

  interface Props {
    open: boolean;
    title: string;
    confirmLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
    children: Snippet;
  }
  let { open, title, confirmLabel = 'Confirm', danger = false, onConfirm, onCancel, children }: Props = $props();

  let dlg: HTMLDialogElement | undefined = $state();
  let cancelBtn: HTMLButtonElement | undefined = $state();

  $effect(() => {
    if (!dlg) return;
    if (open && !dlg.open) {
      dlg.showModal();
      cancelBtn?.focus();
    } else if (!open && dlg.open) {
      dlg.close();
    }
  });
</script>

<dialog
  bind:this={dlg}
  class="dlg"
  aria-labelledby="confirm-title"
  oncancel={(e) => { e.preventDefault(); onCancel(); }}
  onclick={(e) => { if (e.target === dlg) onCancel(); }}
>
  <div class="body">
    <h2 id="confirm-title">{title}</h2>
    {@render children()}
    <div class="actions">
      <button bind:this={cancelBtn} type="button" class="btn-ghost" onclick={onCancel}>Cancel</button>
      <button type="button" class="btn-doge" class:danger onclick={onConfirm}>{confirmLabel}</button>
    </div>
  </div>
</dialog>

<style>
  .dlg {
    border: 1px solid var(--doge-border);
    border-radius: 16px;
    background: var(--doge-card);
    color: var(--doge-text);
    padding: 0;
    width: min(460px, calc(100vw - 32px));
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  .dlg::backdrop { background: rgba(0, 0, 0, 0.6); backdrop-filter: blur(2px); }
  .body { padding: 22px; }
  h2 { margin: 0 0 14px; font-size: 1.1rem; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 18px; }
  .danger { background: var(--doge-red); }
</style>

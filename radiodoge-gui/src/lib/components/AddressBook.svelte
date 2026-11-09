<script lang="ts">
  import EmptyState from './EmptyState.svelte';
  import { errorMessage } from '$lib/errors';
  /**
   * AddressBook tab — save and manage labelled Dogecoin addresses locally.
   * v0.3.8: Entries persisted via Tauri to address_book.json in app data dir.
   */
  import { invoke } from '@tauri-apps/api/core';
  import type { AddressBookEntry } from '$lib/types';
  import { parsePayment, looksLikeDogeAddress } from '$lib/dogeForm';

  let entries = $state<AddressBookEntry[]>([]);
  let isLoading = $state(true);

  // ── Add entry form ─────────────────────────────────────────────────────────
  let showForm = $state(false);
  let newLabel = $state('');
  let newAddress = $state('');
  let newNotes = $state('');
  let formError = $state<string | null>(null);
  let isSaving = $state(false);

  let copiedId = $state<string | null>(null);

  async function loadBook() {
    isLoading = true;
    try {
      const raw = await invoke<{ id: string; label: string; address: string; createdAt: number; notes?: string }[]>('load_address_book');
      entries = raw as AddressBookEntry[];
    } catch { entries = []; } finally { isLoading = false; }
  }

  /**
   * Error from the last write, if it failed.
   *
   * `persist()` used to swallow every error, so a book that could not be written
   * — read-only directory, disk full — was reported to the user as saved and
   * silently reverted to the old contents on the next launch.
   */
  let persistError = $state<string | null>(null);

  async function persist(): Promise<boolean> {
    try {
      await invoke('save_address_book', { entries });
      persistError = null;
      return true;
    } catch (e: unknown) {
      persistError = e instanceof Error ? e.message : errorMessage(e);
      return false;
    }
  }

  let importInput: HTMLInputElement | undefined = $state();
  let ioBusy = $state(false);
  let ioMsg = $state<string | null>(null);

  function exportBook() {
    ioMsg = null;
    const blob = new Blob([JSON.stringify(entries, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'radiodoge-address-book.json';
    a.click();
    URL.revokeObjectURL(url);
    ioMsg = `Exported ${entries.length} entr${entries.length === 1 ? 'y' : 'ies'}.`;
  }

  async function importBook(file: File | null) {
    if (!file) return;
    ioBusy = true;
    ioMsg = null;
    formError = null;
    try {
      const text = await file.text();
      const raw = JSON.parse(text) as unknown;
      if (!Array.isArray(raw)) throw new Error('File must be a JSON array of address-book entries.');
      const next: AddressBookEntry[] = [];
      for (const row of raw) {
        if (!row || typeof row !== 'object') throw new Error('Each entry must be an object.');
        const e = row as Record<string, unknown>;
        const address = String(e.address ?? '').trim();
        const label = String(e.label ?? '').trim();
        if (!label) throw new Error('Every entry needs a label.');
        if (!looksLikeDogeAddress(address) && !(address.startsWith('D') && address.length >= 25 && address.length <= 35)) {
          throw new Error(`Invalid address in import: ${address || '(empty)'}`);
        }
        next.push({
          id: String(e.id ?? Date.now() + Math.random()),
          label,
          address,
          createdAt: typeof e.createdAt === 'number' ? e.createdAt : Math.floor(Date.now() / 1000),
          notes: typeof e.notes === 'string' && e.notes.trim() ? e.notes.trim() : undefined,
        });
      }
      const previous = entries;
      entries = next;
      if (!await persist()) {
        entries = previous;
        ioMsg = persistError;
      } else {
        ioMsg = `Imported ${next.length} entr${next.length === 1 ? 'y' : 'ies'}.`;
      }
    } catch (e: unknown) {
      ioMsg = e instanceof Error ? e.message : errorMessage(e);
    } finally {
      ioBusy = false;
      if (importInput) importInput.value = '';
    }
  }

  async function addEntry() {
    const label = newLabel.trim();
    if (!label) { formError = 'Label is required.'; return; }
    const pasted = newAddress.trim();
    const parsed = parsePayment(pasted);
    const addr = parsed ? parsed.address : pasted;
    if (parsed?.label && !newNotes.trim()) {
      newNotes = parsed.label;
    }
    if (!looksLikeDogeAddress(addr) && !(addr.startsWith('D') && addr.length >= 25 && addr.length <= 35)) {
      formError = 'Invalid Dogecoin address (must start with "D").';
      return;
    }
    isSaving = true;
    formError = null;
    const entry: AddressBookEntry = {
      id: String(Date.now()),
      label,
      address: addr,
      createdAt: Math.floor(Date.now() / 1000),
      notes: newNotes.trim() || undefined,
    };
    const previous = entries;
    entries = [entry, ...entries];
    if (!await persist()) {
      // Put the list back rather than showing an entry that is not on disk.
      entries = previous;
      formError = `Could not save the address book: ${persistError}`;
      isSaving = false;
      return;
    }
    newLabel = ''; newAddress = ''; newNotes = '';
    showForm = false;
    isSaving = false;
  }

  async function removeEntry(id: string) {
    const previous = entries;
    entries = entries.filter(e => e.id !== id);
    if (!await persist()) {
      entries = previous;
    }
  }

  async function copyAddress(address: string, id: string) {
    try {
      await navigator.clipboard.writeText(address);
      copiedId = id;
      setTimeout(() => { copiedId = null; }, 2000);
    } catch {}
  }

  $effect(() => { loadBook(); });
</script>

<div style="padding: 0 0 24px 0;">
  <!-- ── Header ──────────────────────────────────────────────────────────── -->
  <div class="tab-header">
    <div>
      <h2 class="tab-title">📒 Address Book</h2>
      <p class="tab-sub">
        Save labelled Dogecoin addresses · stored locally
      </p>
    </div>
    <div style="display: flex; gap: 8px; flex-wrap: wrap; justify-content: flex-end;">
      <button
        type="button"
        class="btn-ghost"
        style="padding: 7px 12px; font-size: var(--fs-sm);"
        onclick={exportBook}
        disabled={entries.length === 0 || ioBusy}
      >Export JSON</button>
      <button
        type="button"
        class="btn-ghost"
        style="padding: 7px 12px; font-size: var(--fs-sm);"
        onclick={() => importInput?.click()}
        disabled={ioBusy}
      >Import JSON</button>
      <input
        bind:this={importInput}
        type="file"
        accept="application/json,.json"
        hidden
        onchange={(e) => importBook(e.currentTarget.files?.[0] ?? null)}
      />
      <button
        onclick={() => { showForm = !showForm; formError = null; }}
        style="padding: 7px 16px; border: 1px solid var(--doge-yellow); background: rgba(245,197,24,0.08); color: var(--doge-yellow); border-radius: 8px; cursor: pointer; font-size: var(--fs-sm);"
      >
        {showForm ? '✕ Cancel' : '+ Add Address'}
      </button>
    </div>
  </div>
  {#if ioMsg}
    <p style="margin: 0 0 8px; font-size: var(--fs-xs); color: var(--doge-muted);" role="status">{ioMsg}</p>
  {/if}

  <!-- ── Add form ───────────────────────────────────────────────────────── -->
  {#if showForm}
    <div class="card-doge" style="margin-bottom: 18px;">
      <h3 style="margin: 0 0 14px 0; font-size: var(--fs-md); font-weight: 700; color: var(--doge-yellow);">Add New Address</h3>
      <div style="display: flex; flex-direction: column; gap: 10px;">
        <input
          type="text"
          aria-label="Contact label"
          placeholder="Label (e.g. 'My main wallet')"
          bind:value={newLabel}
          class="field-control"
        />
        <input
          type="text"
          aria-label="Dogecoin address"
          placeholder="Dogecoin address or dogecoin: URI"
          bind:value={newAddress}
          onpaste={(e) => {
            const text = e.clipboardData?.getData('text');
            if (!text) return;
            const p = parsePayment(text);
            if (!p) return;
            e.preventDefault();
            newAddress = p.address;
            if (p.label && !newNotes.trim()) newNotes = p.label;
            if (p.label && !newLabel.trim()) newLabel = p.label;
          }}
          class="field-control mono"
        />
        <input
          type="text"
          aria-label="Notes"
          placeholder="Notes (optional)"
          bind:value={newNotes}
          class="field-control"
        />
        {#if persistError && !formError}
          <p style="margin: 0; font-size: var(--fs-sm); color: #ff6060;" role="alert">
            Could not save the address book: {persistError}
          </p>
        {/if}
        {#if formError}
          <p style="margin: 0; font-size: var(--fs-sm); color: #ff6060;">{formError}</p>
        {/if}
        <button
          onclick={addEntry}
          disabled={isSaving}
          style="align-self: flex-start; padding: 8px 18px; border: 1px solid var(--doge-yellow); background: rgba(245,197,24,0.1); color: var(--doge-yellow); border-radius: 8px; cursor: pointer; font-size: var(--fs-base);"
        >
          {isSaving ? '⏳ Saving…' : '💾 Save'}
        </button>
      </div>
    </div>
  {/if}

  <!-- ── Entries ────────────────────────────────────────────────────────── -->
  {#if isLoading}
    <EmptyState loading title="Loading saved addresses…" />
  {:else if entries.length === 0}
    <EmptyState icon="📭" title="No saved addresses yet" hint='Click &quot;+ Add Address&quot; to save a Dogecoin address with a label.' />
  {:else}
    <div class="card-doge" style="padding: 0; overflow: hidden;">
      <table style="width: 100%; border-collapse: collapse;">
        <thead>
          <tr style="border-bottom: 1px solid var(--doge-border); background: rgba(255,255,255,0.02);">
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">Label</th>
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">Address</th>
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;">Notes</th>
            <th style="padding: 10px 14px; text-align: left; font-size: var(--fs-xs); color: var(--doge-muted); font-weight: 600;"></th>
          </tr>
        </thead>
        <tbody>
          {#each entries as entry (entry.id)}
            <tr style="border-bottom: 1px solid rgba(255,255,255,0.04);">
              <td style="padding: 10px 14px; font-size: var(--fs-base); font-weight: 600; color: var(--doge-text);">{entry.label}</td>
              <td style="padding: 10px 14px;">
                <div style="display: flex; align-items: center; gap: 6px;">
                  <span style="font-family: var(--font-mono); font-size: var(--fs-sm); color: var(--doge-yellow);">
                    {entry.address.slice(0,10)}…{entry.address.slice(-6)}
                  </span>
                  <button
                    onclick={() => copyAddress(entry.address, entry.id)}
                    style="padding: 2px 7px; border: 1px solid var(--doge-border); background: transparent; color: var(--doge-muted); border-radius: 4px; cursor: pointer; font-size: var(--fs-2xs);"
                    title="Copy full address"
                  >
                    {copiedId === entry.id ? '✅' : '📋'}
                  </button>
                </div>
              </td>
              <td style="padding: 10px 14px; font-size: var(--fs-sm); color: var(--doge-muted);">{entry.notes ?? '—'}</td>
              <td style="padding: 10px 14px;">
                <button
                  onclick={() => removeEntry(entry.id)}
                  style="padding: 4px 9px; border: 1px solid rgba(255,60,60,0.3); background: transparent; color: #ff8080; border-radius: 5px; cursor: pointer; font-size: var(--fs-xs);"
                  title="Remove entry"
                >
                  🗑️
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

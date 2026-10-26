import { invoke } from '@tauri-apps/api/core';

/** localStorage key for the user's chain-backend spec (empty = app default). */
const KEY = 'radiodoge.chainBackends';

export const BACKEND_PRESETS = [
  { label: 'Default (Core, then BlockCypher)', spec: '' },
  { label: 'Local Dogecoin Core only', spec: 'core' },
  { label: 'BlockCypher only', spec: 'blockcypher' },
  { label: 'Core, BlockCypher, Blockbook', spec: 'core,blockcypher,blockbook' },
];

export function loadBackendSpec(): string {
  try {
    return localStorage.getItem(KEY) ?? '';
  } catch {
    return '';
  }
}

/** Validate + apply a spec in the Rust side, then persist it. Throws the backend's error text. */
export async function applyBackendSpec(spec: string): Promise<void> {
  const trimmed = spec.trim();
  await invoke('set_chain_backends', { spec: trimmed });
  try {
    if (trimmed) localStorage.setItem(KEY, trimmed);
    else localStorage.removeItem(KEY);
  } catch {
    /* storage unavailable: setting still applies for this session */
  }
}

/** Re-apply the saved spec at startup. Errors are swallowed (falls back to the default list). */
export async function restoreBackendSpec(): Promise<void> {
  const spec = loadBackendSpec();
  if (spec) await applyBackendSpec(spec).catch(() => {});
}

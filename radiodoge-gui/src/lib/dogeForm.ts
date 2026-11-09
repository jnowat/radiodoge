/** Pure Dogecoin send-form helpers (no I/O). Heuristic only — Rust `is_valid_address` is authoritative. */

const BASE58 = /^[1-9A-HJ-NP-Za-km-z]+$/;

/** Fast UI check: mainnet P2PKH-shaped string (D…, 34 chars, Base58 alphabet). */
export function looksLikeDogeAddress(address: string): boolean {
  const a = address.trim();
  return a.length === 34 && a.startsWith('D') && BASE58.test(a);
}

/** Parse a required positive finite DOGE amount. */
export function parsePositiveDoge(raw: string): number | null {
  const n = Number.parseFloat(raw.trim());
  if (!Number.isFinite(n) || n <= 0) return null;
  return n;
}

/**
 * Parse an optional fee box: empty => recommended (null), otherwise a finite number >= 0.
 * Returns `{ ok:false }` when the text is present but not a valid non-negative number.
 */
export function parseOptionalFeeDoge(raw: string): { ok: true; fee: number | null } | { ok: false } {
  const t = raw.trim();
  if (!t) return { ok: true, fee: null };
  const n = Number.parseFloat(t);
  if (!Number.isFinite(n) || n < 0) return { ok: false };
  return { ok: true, fee: n };
}

export function isValidOptionalFee(raw: string): boolean {
  return parseOptionalFeeDoge(raw).ok;
}

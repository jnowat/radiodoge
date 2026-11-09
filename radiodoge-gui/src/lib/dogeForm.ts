/** Pure Dogecoin send-form helpers (no I/O). Heuristic only - Rust `is_valid_address` is authoritative. */

const BASE58 = /^[1-9A-HJ-NP-Za-km-z]+$/;

/** Fast UI check: mainnet P2PKH-shaped string (D., 34 chars, Base58 alphabet). */
export function looksLikeDogeAddress(address: string): boolean {
  const a = address.trim();
  return a.length === 34 && a.startsWith('D') && BASE58.test(a);
}

/**
 * Loose structural check used while parsing BIP21 URIs (mirrors
 * `radiodoge_core::qr::looks_like_doge_address`: D + 26..=34 alphanumerics).
 * Full Base58Check still happens in Rust before a send.
 */
function looksLikeDogeAddressLoose(address: string): boolean {
  const a = address.trim();
  return (
    a.startsWith('D') &&
    a.length >= 26 &&
    a.length <= 34 &&
    /^[0-9A-Za-z]+$/.test(a)
  );
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

/** Default network fee suggested when signing from a UTXO snapshot (no rate fetch). */
export const DEFAULT_OFFLINE_FEE_DOGE = 0.01;

/**
 * Offline signing requires an explicit fee (empty = recommended only works online).
 * Accepts a finite number > 0.
 */
export function parseRequiredFeeDoge(raw: string): { ok: true; fee: number } | { ok: false } {
  const parsed = parseOptionalFeeDoge(raw);
  if (!parsed.ok || parsed.fee == null || parsed.fee <= 0) return { ok: false };
  return { ok: true, fee: parsed.fee };
}

export function isValidRequiredFee(raw: string): boolean {
  return parseRequiredFeeDoge(raw).ok;
}

/** Preview amount + fee against snapshot DOGE available. */
export function offlineSpendPreview(
  amountDoge: number | null,
  feeRaw: string,
  availableDoge: number,
): {
  amount: number | null;
  fee: number | null;
  total: number | null;
  available: number;
  ok: boolean;
  shortfall: number | null;
} {
  const feeParsed = parseRequiredFeeDoge(feeRaw);
  const fee = feeParsed.ok ? feeParsed.fee : null;
  const amount = amountDoge != null && Number.isFinite(amountDoge) && amountDoge > 0 ? amountDoge : null;
  if (amount == null || fee == null) {
    return { amount, fee, total: null, available: availableDoge, ok: false, shortfall: null };
  }
  const total = amount + fee;
  const shortfall = total > availableDoge ? total - availableDoge : null;
  return { amount, fee, total, available: availableDoge, ok: shortfall == null, shortfall };
}

/** Result of interpreting pasted/scanned payment text (mirrors Rust `ParsedPayment`). */
export interface ParsedPaymentText {
  address: string;
  amount: number | null;
  label: string | null;
}

/** Percent-decode a BIP21 query component (+ and %XX). */
function percentDecode(s: string): string {
  try {
    return decodeURIComponent(s.replace(/\+/g, ' '));
  } catch {
    return s.replace(/\+/g, ' ');
  }
}

/**
 * Parse a bare Dogecoin address or BIP21 `dogecoin:` URI into address / amount / label.
 * Mirrors `radiodoge_core::qr::parse_payment`. Returns `null` when no plausible address is found.
 */
export function parsePayment(text: string): ParsedPaymentText | null {
  const trimmed = text.trim();
  if (!trimmed) return null;

  let withoutScheme: string;
  const colon = trimmed.indexOf(':');
  if (colon >= 0) {
    const scheme = trimmed.slice(0, colon);
    if (scheme.toLowerCase() !== 'dogecoin') return null;
    withoutScheme = trimmed.slice(colon + 1);
  } else {
    withoutScheme = trimmed;
  }

  const q = withoutScheme.indexOf('?');
  const addrPart = (q >= 0 ? withoutScheme.slice(0, q) : withoutScheme).trim();
  const query = q >= 0 ? withoutScheme.slice(q + 1) : null;
  if (!looksLikeDogeAddressLoose(addrPart)) return null;

  let amount: number | null = null;
  let label: string | null = null;
  if (query) {
    for (const pair of query.split('&')) {
      const eq = pair.indexOf('=');
      if (eq < 0) continue;
      const key = pair.slice(0, eq).toLowerCase();
      const val = pair.slice(eq + 1);
      if (key === 'amount') {
        const n = Number.parseFloat(val);
        if (Number.isFinite(n) && n > 0) amount = n;
      } else if ((key === 'label' || key === 'message') && label == null) {
        const decoded = percentDecode(val);
        if (decoded) label = decoded;
      }
    }
  }

  return { address: addrPart, amount, label };
}

/**
 * Build a BIP21 `dogecoin:` URI for sharing (QR / clipboard).
 * Omits amount/label when not provided. Amount must be finite and > 0.
 */
export function formatPaymentUri(
  address: string,
  opts?: { amount?: number | null; label?: string | null },
): string | null {
  const a = address.trim();
  if (!looksLikeDogeAddressLoose(a)) return null;
  const params: string[] = [];
  const amount = opts?.amount;
  if (amount != null && Number.isFinite(amount) && amount > 0) {
    params.push(`amount=${amount}`);
  }
  const label = opts?.label?.trim();
  if (label) {
    params.push(`label=${encodeURIComponent(label)}`);
  }
  return params.length ? `dogecoin:${a}?${params.join('&')}` : `dogecoin:${a}`;
}

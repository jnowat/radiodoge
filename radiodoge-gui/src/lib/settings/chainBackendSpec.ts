/** Pure helpers for the chain-backend spec string (no I/O).
 *
 * The Rust side (`radiodoge_core::backend::parse_backends`) is authoritative;
 * these helpers give the Settings UI fast, local feedback that matches it.
 */

export const KNOWN_BACKENDS = ['core', 'rpc', 'dogecoind', 'blockcypher', 'blockbook'] as const;

export type KnownBackend = (typeof KNOWN_BACKENDS)[number];

/** Trim, drop empties, lowercase names; keep `name=arg` args as typed. */
export function normalizeBackendSpec(spec: string): string {
  return spec
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
    .map((item) => {
      const eq = item.indexOf('=');
      if (eq === -1) return item.toLowerCase();
      const name = item.slice(0, eq).trim().toLowerCase();
      const arg = item.slice(eq + 1).trim();
      return arg ? `${name}=${arg}` : name;
    })
    .join(',');
}

export type BackendSpecIssue =
  | { kind: 'empty' }
  | { kind: 'unknown'; name: string }
  | { kind: 'blockbook-url'; token: string };

/** Validate a (possibly raw) spec the same way the Rust parser would reject it. */
export function validateBackendSpec(spec: string): BackendSpecIssue | null {
  const normalized = normalizeBackendSpec(spec);
  if (!normalized) return { kind: 'empty' };
  for (const item of normalized.split(',')) {
    const eq = item.indexOf('=');
    const name = (eq === -1 ? item : item.slice(0, eq)).toLowerCase();
    const arg = eq === -1 ? '' : item.slice(eq + 1);
    if (!(KNOWN_BACKENDS as readonly string[]).includes(name)) {
      return { kind: 'unknown', name };
    }
    if (name === 'blockbook' && !arg) {
      return { kind: 'blockbook-url', token: item };
    }
  }
  return null;
}

export function backendSpecErrorMessage(issue: BackendSpecIssue): string {
  switch (issue.kind) {
    case 'empty':
      return 'No backends configured';
    case 'unknown':
      return `Unknown backend '${issue.name}' (expected core, blockcypher or blockbook=URL)`;
    case 'blockbook-url':
      return 'blockbook needs a URL: blockbook=https://host/api/v2';
  }
}

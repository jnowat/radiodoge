/** Turn catch values into short, readable messages (and optional next-step hints). */

/** Mirrors `radiodoge_core::errors::HINTS` (first needle hit wins; case-insensitive). */
const HINTS: ReadonlyArray<{ needles: readonly string[]; hint: string }> = [
  {
    needles: ['no serial port open', 'connect first', 'not connected to a board'],
    hint: 'Connect a board first (Connect tab in the app, or --port on the CLI).',
  },
  {
    needles: ['access is denied', 'permission denied', 'resource busy', 'port is busy'],
    hint: 'Another program (Arduino IDE, PuTTY, a second RadioDoge window) may have the port open. Close it and try again.',
  },
  {
    needles: ['device is not connected', 'device has been removed', 'broken pipe', 'device not configured'],
    hint: 'The board went away. Check the USB cable (some USB-C cables are power-only) and reconnect.',
  },
  {
    needles: ['no space left', 'os error 28', 'disk is full'],
    hint: 'The disk is full. Free up some space and try again.',
  },
  {
    needles: ['os error 2)', 'no such file', 'cannot find the path', 'cannot find the file'],
    hint: 'Check that the file or folder path exists and is spelled correctly.',
  },
  {
    needles: ['429', 'too many requests', 'rate limit'],
    hint: 'The block explorer is rate-limiting requests. Wait a minute and retry.',
  },
  {
    needles: ['connection refused', 'actively refused'],
    hint: 'The chain backend refused the connection. If you use a local Dogecoin Core node, make sure it is running and the RPC settings are right.',
  },
  {
    needles: ['error sending request', 'dns error', 'failed to lookup address', 'network is unreachable', 'no such host'],
    hint: "Couldn't reach the network. Check your internet connection and try again.",
  },
  {
    needles: ['certificate', 'ssl', 'tls', 'native cert', 'certificate verify', 'invalid peer certificate'],
    hint: 'TLS/SSL to the chain backend failed. Check the clock, proxy/antivirus HTTPS inspection, or try another backend in Settings.',
  },
  {
    needles: ['usb permission', 'permission has not been granted', 'user denied permission'],
    hint: 'Android needs USB permission for the board. Unplug/replug and tap Allow on the permission prompt.',
  },
  {
    needles: ['multipart requires', 'firmware too old', 'requires fw', 'fw11 or newer'],
    hint: 'This send needs newer Heltec firmware (FW11+ for multipart). Flash the latest RadioDoge firmware and reconnect.',
  },
  {
    needles: ['error decoding response', 'expected value', 'invalid json', 'eof while parsing'],
    hint: 'The chain backend returned something that was not valid JSON. Try again, or switch backends in Settings.',
  },
  {
    needles: ['semaphore timeout', 'the semaphore timeout period has expired', 'os error 121'],
    hint: 'The USB serial link stalled. Unplug the board, wait a second, plug it back in, and reconnect.',
  },
  {
    needles: ['device is not functioning', 'the device does not recognize the command', 'os error 31'],
    hint: 'Windows lost the USB device. Try a different cable or USB port, then reconnect.',
  },
  {
    needles: ['element not found', 'os error 433'],
    hint: 'That COM port is gone. Refresh the port list and pick the Heltec again.',
  },
  {
    needles: ['gateway daemon exited', 'daemon exited', 'gateway failed to start'],
    hint: 'The gateway process stopped. Check gateway-daemon.log next to gateway-stats.json, then toggle gateway mode again.',
  },
  {
    needles: ['timed out', 'timeout'],
    hint: "No answer in time. Check the board is powered and running RadioDoge firmware (or that you're online), then retry.",
  },
  {
    needles: ['insufficient funds', 'insufficient balance', 'not enough funds'],
    hint: "The balance can't cover the amount plus the network fee. Lower the amount or add funds.",
  },
  {
    needles: ['decryption failed', 'aead', 'wrong passphrase', 'bad passphrase'],
    hint: 'The passphrase looks wrong (or the wallet file is damaged). Check it and try again.',
  },
  {
    needles: ['mnemonic', 'invalid word', 'word count'],
    hint: 'Check the recovery phrase: 12 or 24 words, spelled exactly, separated by single spaces.',
  },
  {
    needles: ['invalid wif', 'invalid checksum', 'invalid address', 'base58'],
    hint: 'Check the key or address for typos. RadioDoge uses mainnet P2PKH addresses that start with D.',
  },
];

/** A short next step for a recognised error, if any. */
export function hintFor(raw: string): string | null {
  const lower = raw.toLowerCase();
  for (const { needles, hint } of HINTS) {
    if (needles.some((n) => lower.includes(n))) return hint;
  }
  return null;
}

/** Original message plus `Hint: …` when one applies (messages that already carry a hint are unchanged). */
export function friendlyMessage(raw: string): string {
  if (raw.includes('Hint:')) return raw;
  const hint = hintFor(raw);
  return hint ? `${raw}\nHint: ${hint}` : raw;
}

// Turn anything a `catch` receives into a short, readable message.
//
// Tauri commands reject with plain strings, JS code throws `Error`s, and some
// plugins reject with `{ message }` objects; `String(obj)` on the latter shows
// "[object Object]". This keeps every error banner human-readable, and appends
// the same next-step hints the Rust CLI uses when one matches.
export function errorMessage(e: unknown): string {
  let msg: string;
  if (e instanceof Error) msg = e.message;
  else if (typeof e === 'string') msg = e;
  else if (e && typeof e === 'object' && typeof (e as { message?: unknown }).message === 'string') {
    msg = (e as { message: string }).message;
  } else if (e && typeof e === 'object') {
    try { msg = JSON.stringify(e); } catch { msg = String(e); }
  } else msg = String(e);
  msg = msg.replace(/^(Uncaught\s+)?(Error:\s*)+/i, '').trim();
  if (!msg) msg = 'Unknown error';
  return friendlyMessage(msg);
}

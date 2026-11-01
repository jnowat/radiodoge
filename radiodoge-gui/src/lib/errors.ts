// Turn anything a `catch` receives into a short, readable message.
//
// Tauri commands reject with plain strings, JS code throws `Error`s, and some
// plugins reject with `{ message }` objects; `String(obj)` on the latter shows
// "[object Object]". This keeps every error banner human-readable.
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
  return msg || 'Unknown error';
}

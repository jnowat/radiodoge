// Run: npm run test:unit  (Node 22.18+/24 strips the TypeScript types natively)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { errorMessage, hintFor, friendlyMessage } from '../src/lib/errors.ts';

test('Error instances use their message', () => {
  assert.equal(errorMessage(new Error('port busy')), 'port busy');
});
test('plain strings (Tauri command rejections) pass through', () => {
  assert.equal(errorMessage('COM3 not found'), 'COM3 not found');
});
test('{ message } objects no longer become [object Object]', () => {
  assert.equal(errorMessage({ message: 'BLE timeout' }), 'BLE timeout\nHint: No answer in time. Check the board is powered and running RadioDoge firmware (or that you\'re online), then retry.');
});
test('other objects are JSON-encoded', () => {
  assert.equal(errorMessage({ code: 5 }), '{"code":5}');
});
test('duplicate "Error:" prefixes are stripped', () => {
  assert.equal(errorMessage('Error: Error: bad frame'), 'bad frame');
  assert.equal(errorMessage(new Error('Error: nested')), 'nested');
});
test('empty or nullish values give a fallback', () => {
  assert.equal(errorMessage(''), 'Unknown error');
  assert.equal(errorMessage(undefined), 'undefined');
  assert.equal(errorMessage(null), 'null');
});

test('hintFor matches Rust needles (case-insensitive, first hit)', () => {
  assert.match(hintFor('No serial port open - connect first') ?? '', /Connect a board first/);
  assert.match(hintFor('Access is denied. (os error 5)') ?? '', /Arduino IDE/);
  assert.match(hintFor('HTTP 429 Too Many Requests') ?? '', /rate-limiting/);
  assert.match(hintFor('invalid peer certificate: UnknownIssuer') ?? '', /TLS\/SSL/);
  assert.match(hintFor('USB permission has not been granted') ?? '', /Allow/);
  assert.match(hintFor('multipart requires FW11 or newer') ?? '', /FW11/);
  assert.equal(hintFor('something odd'), null);
});

test('friendlyMessage appends Hint once', () => {
  const s = friendlyMessage('connection refused');
  assert.match(s, /Hint: The chain backend refused/);
  assert.equal(friendlyMessage(s), s);
});

test('errorMessage appends hints for known Tauri rejections', () => {
  const s = errorMessage('No serial port open');
  assert.match(s, /^No serial port open/);
  assert.match(s, /Hint: Connect a board first/);
});

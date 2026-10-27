// Run: npm run test:unit  (Node 22.18+/24 strips the TypeScript types natively)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { errorMessage } from '../src/lib/errors.ts';

test('Error instances use their message', () => {
  assert.equal(errorMessage(new Error('port busy')), 'port busy');
});
test('plain strings (Tauri command rejections) pass through', () => {
  assert.equal(errorMessage('COM3 not found'), 'COM3 not found');
});
test('{ message } objects no longer become [object Object]', () => {
  assert.equal(errorMessage({ message: 'BLE timeout' }), 'BLE timeout');
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

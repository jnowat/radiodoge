import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  looksLikeDogeAddress,
  parsePositiveDoge,
  parseOptionalFeeDoge,
  isValidOptionalFee,
} from '../src/lib/dogeForm.ts';

test('accepts a well-shaped mainnet address', () => {
  assert.equal(looksLikeDogeAddress('DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L'), true);
});

test('rejects wrong length, prefix, alphabet, and whitespace-only', () => {
  assert.equal(looksLikeDogeAddress('Dshort'), false);
  assert.equal(looksLikeDogeAddress('AH5yaieqoZN36fDVciNyRueRGvGLR3mr7L'), false);
  assert.equal(looksLikeDogeAddress('DH5yaieqoZN36fDVciNyRueRGvGLR3mr7O'), false); // O not in Base58
  assert.equal(looksLikeDogeAddress(''), false);
});

test('trims before checking address', () => {
  assert.equal(looksLikeDogeAddress('  DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L  '), true);
});

test('parsePositiveDoge requires a finite amount > 0', () => {
  assert.equal(parsePositiveDoge('1.5'), 1.5);
  assert.equal(parsePositiveDoge('0'), null);
  assert.equal(parsePositiveDoge('-1'), null);
  assert.equal(parsePositiveDoge('nope'), null);
  assert.equal(parsePositiveDoge(''), null);
});

test('parseOptionalFeeDoge: empty means recommended', () => {
  assert.deepEqual(parseOptionalFeeDoge(''), { ok: true, fee: null });
  assert.deepEqual(parseOptionalFeeDoge('  '), { ok: true, fee: null });
  assert.deepEqual(parseOptionalFeeDoge('0.01'), { ok: true, fee: 0.01 });
  assert.deepEqual(parseOptionalFeeDoge('0'), { ok: true, fee: 0 });
  assert.equal(parseOptionalFeeDoge('-1').ok, false);
  assert.equal(parseOptionalFeeDoge('x').ok, false);
  assert.equal(isValidOptionalFee(''), true);
  assert.equal(isValidOptionalFee('nope'), false);
});

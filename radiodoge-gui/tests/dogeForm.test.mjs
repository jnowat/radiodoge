import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  looksLikeDogeAddress,
  parsePositiveDoge,
  parseOptionalFeeDoge,
  isValidOptionalFee,
  parseRequiredFeeDoge,
  isValidRequiredFee,
  DEFAULT_OFFLINE_FEE_DOGE,
  offlineSpendPreview,
  parsePayment,
  formatPaymentUri,
  p2pkhTxSize,
  feeDogeFromRate,
  formatFeeDoge,
} from '../src/lib/dogeForm.ts';

const ADDR = 'DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L';

test('accepts a well-shaped mainnet address', () => {
  assert.equal(looksLikeDogeAddress(ADDR), true);
});

test('rejects wrong length, prefix, alphabet, and whitespace-only', () => {
  assert.equal(looksLikeDogeAddress('Dshort'), false);
  assert.equal(looksLikeDogeAddress('AH5yaieqoZN36fDVciNyRueRGvGLR3mr7L'), false);
  assert.equal(looksLikeDogeAddress('DH5yaieqoZN36fDVciNyRueRGvGLR3mr7O'), false); // O not in Base58
  assert.equal(looksLikeDogeAddress(''), false);
});

test('trims before checking address', () => {
  assert.equal(looksLikeDogeAddress(`  ${ADDR}  `), true);
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

test('parsePayment: bare address', () => {
  const p = parsePayment(ADDR);
  assert.deepEqual(p, { address: ADDR, amount: null, label: null });
  assert.deepEqual(parsePayment(`  ${ADDR}\n`), { address: ADDR, amount: null, label: null });
});

test('parsePayment: BIP21 amount + label/message', () => {
  assert.deepEqual(parsePayment(`dogecoin:${ADDR}?amount=4.2`), {
    address: ADDR,
    amount: 4.2,
    label: null,
  });
  assert.deepEqual(parsePayment(`DOGECOIN:${ADDR}`), {
    address: ADDR,
    amount: null,
    label: null,
  });
  assert.deepEqual(parsePayment(`dogecoin:${ADDR}?amount=10&label=Coffee%20Fund`), {
    address: ADDR,
    amount: 10,
    label: 'Coffee Fund',
  });
  assert.deepEqual(parsePayment(`dogecoin:${ADDR}?message=much+wow`), {
    address: ADDR,
    amount: null,
    label: 'much wow',
  });
});

test('parsePayment: rejects bad scheme / amount / address', () => {
  assert.equal(parsePayment(`bitcoin:${ADDR}`), null);
  assert.equal(parsePayment('https://example.com'), null);
  assert.equal(parsePayment(''), null);
  assert.equal(parsePayment('dogecoin:not-an-address'), null);
  // invalid amount is ignored; address still returns
  assert.deepEqual(parsePayment(`dogecoin:${ADDR}?amount=notanumber`), {
    address: ADDR,
    amount: null,
    label: null,
  });
  assert.deepEqual(parsePayment(`dogecoin:${ADDR}?amount=-5`), {
    address: ADDR,
    amount: null,
    label: null,
  });
});

test('formatPaymentUri builds bare and query forms', () => {
  assert.equal(formatPaymentUri(ADDR), `dogecoin:${ADDR}`);
  assert.equal(formatPaymentUri(ADDR, { amount: 1.5 }), `dogecoin:${ADDR}?amount=1.5`);
  assert.equal(
    formatPaymentUri(ADDR, { amount: 2, label: 'Coffee Fund' }),
    `dogecoin:${ADDR}?amount=2&label=Coffee%20Fund`,
  );
  assert.equal(formatPaymentUri('not-an-address'), null);
  assert.equal(formatPaymentUri(ADDR, { amount: 0 }), `dogecoin:${ADDR}`);
});

test('parseRequiredFeeDoge rejects empty and non-positive', () => {
  assert.equal(parseRequiredFeeDoge('').ok, false);
  assert.equal(parseRequiredFeeDoge('0').ok, false);
  assert.equal(parseRequiredFeeDoge('-0.01').ok, false);
  assert.deepEqual(parseRequiredFeeDoge('0.01'), { ok: true, fee: 0.01 });
  assert.equal(isValidRequiredFee(''), false);
  assert.equal(isValidRequiredFee(String(DEFAULT_OFFLINE_FEE_DOGE)), true);
  assert.equal(DEFAULT_OFFLINE_FEE_DOGE, 0.01);
});

test('offlineSpendPreview totals amount+fee against snapshot', () => {
  const ok = offlineSpendPreview(1, '0.01', 5);
  assert.equal(ok.ok, true);
  assert.equal(ok.total, 1.01);
  assert.equal(ok.shortfall, null);
  const short = offlineSpendPreview(5, '0.01', 5);
  assert.equal(short.ok, false);
  assert.ok(short.shortfall != null && short.shortfall > 0);
  const missingFee = offlineSpendPreview(1, '', 5);
  assert.equal(missingFee.ok, false);
  assert.equal(missingFee.fee, null);
});

test('p2pkhTxSize mirrors the Rust estimate', () => {
  assert.equal(p2pkhTxSize(1, 2), 226);
  assert.equal(p2pkhTxSize(3, 2), 522);
  assert.equal(p2pkhTxSize(0, 0), 192); // clamped to 1 in / 1 out
});

test('feeDogeFromRate charges per started kB with the 0.01 floor', () => {
  assert.equal(feeDogeFromRate(2_000_000, 226), 0.02);
  assert.equal(feeDogeFromRate(2_000_000, 1001), 0.04);
  assert.equal(feeDogeFromRate(10, 226), 0.01);
  assert.equal(feeDogeFromRate(0, 0), 0.01);
  assert.equal(feeDogeFromRate(-1, 226), null);
  assert.equal(feeDogeFromRate(Number.NaN, 226), null);
});

test('formatFeeDoge trims trailing zeros', () => {
  assert.equal(formatFeeDoge(0.01), '0.01');
  assert.equal(formatFeeDoge(10), '10');
  assert.equal(formatFeeDoge(0.12345678), '0.12345678');
  assert.equal(formatFeeDoge(0), '0');
});

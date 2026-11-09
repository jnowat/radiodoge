// Run: npm run test:unit
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  normalizeBackendSpec,
  validateBackendSpec,
  backendSpecErrorMessage,
} from '../src/lib/settings/chainBackendSpec.ts';

test('normalize lowercases names and trims', () => {
  assert.equal(normalizeBackendSpec(' Core , BlockCypher '), 'core,blockcypher');
});

test('normalize keeps blockbook URL args', () => {
  assert.equal(
    normalizeBackendSpec('blockbook=https://Example/api/v2'),
    'blockbook=https://Example/api/v2',
  );
});

test('empty / whitespace is empty', () => {
  assert.equal(normalizeBackendSpec('  ,  '), '');
  assert.equal(validateBackendSpec('').kind, 'empty');
});

test('aliases core/rpc/dogecoind are accepted', () => {
  assert.equal(validateBackendSpec('rpc'), null);
  assert.equal(validateBackendSpec('dogecoind,blockcypher'), null);
});

test('unknown backend is rejected with a Rust-like message', () => {
  const issue = validateBackendSpec('foo');
  assert.equal(issue?.kind, 'unknown');
  assert.match(backendSpecErrorMessage(issue), /Unknown backend 'foo'/);
});

test('blockbook without URL is rejected', () => {
  const issue = validateBackendSpec('blockbook');
  assert.equal(issue?.kind, 'blockbook-url');
  assert.match(backendSpecErrorMessage(issue), /blockbook needs a URL/);
});

test('blockbook with URL is ok', () => {
  assert.equal(validateBackendSpec('blockbook=https://host/api/v2'), null);
});

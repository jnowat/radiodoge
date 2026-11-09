// Run: npm run test:unit
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  nodeAddressToString,
  mapUrl,
  rssiToBars,
  commandName,
} from '../src/lib/types/index.ts';

test('nodeAddressToString joins region.community.node', () => {
  assert.equal(nodeAddressToString({ region: 10, community: 0, node: 1 }), '10.0.1');
});

test('mapUrl builds an OpenStreetMap pin link', () => {
  const url = mapUrl({ lat: 43.0389, lon: -87.9065 });
  assert.match(url, /^https:\/\/www\.openstreetmap\.org\/\?mlat=43\.03890&mlon=-87\.90650#map=16\//);
});

test('rssiToBars maps dBm thresholds to 1-5', () => {
  assert.equal(rssiToBars(-40), 5);
  assert.equal(rssiToBars(-50), 5);
  assert.equal(rssiToBars(-65), 4);
  assert.equal(rssiToBars(-80), 3);
  assert.equal(rssiToBars(-95), 2);
  assert.equal(rssiToBars(-120), 1);
});

test('commandName covers classic and FW12-14 opcodes', () => {
  assert.equal(commandName(0x02), 'PING');
  assert.equal(commandName(0x10), 'DOGE TX');
  assert.equal(commandName(0x28), 'BLE TOGGLE');
  assert.equal(commandName(0x29), 'RX ACK');
  assert.equal(commandName(0x2E), 'DISCOVER');
  assert.equal(commandName(0x30), 'RX SIGNAL');
  assert.equal(commandName(0x99), 'CMD(0x99)');
});

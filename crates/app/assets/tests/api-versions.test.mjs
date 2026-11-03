// The API releases of checkout UI extensions the local checkout runs, and what it refuses.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { API_VERSIONS, OWNER_TYPES, refusalOf } from '../api-versions.js';

const ext = (apiVersion, metafields) => ({ handle: 'x', apiVersion, metafields });

test('runs 2026-10 and the 2027-01 release candidate, nothing else', () => {
  assert.deepEqual(Object.keys(API_VERSIONS), ['2026-10', '2027-01']);
  for (const version of ['2026-10', '2027-01']) assert.equal(refusalOf(ext(version)), null, version);
});

test('refuses the API versions before 2026-10, the ones after 2027-01 and the unknown', () => {
  for (const version of ['2025-07', '2025-10', '2026-01', '2026-07', '2027-04', 'unstable', 'rc', '', undefined]) {
    assert.match(refusalOf(ext(version)) ?? '', /is not supported: the local checkout runs 2026-10 and 2027-01/, String(version));
  }
});

test('owner_type is optional on 2026-10 and required on 2027-01', () => {
  const without = [{ namespace: 'custom', key: 'a' }, { namespace: 'custom', key: 'b', ownerType: 'CART' }];
  assert.equal(refusalOf(ext('2026-10', without)), null);
  assert.equal(
    refusalOf(ext('2027-01', without)),
    'API version 2027-01 requires owner_type on every metafield declaration: custom.a has none.',
  );
  assert.equal(refusalOf(ext('2027-01', [{ namespace: 'custom', key: 'b', ownerType: 'CART' }])), null);
  assert.equal(refusalOf(ext('2027-01', [])), null, 'no declaration, nothing to require');
  assert.equal(refusalOf(ext('2027-01')), null);
});

test('an owner_type must name a resource, on both versions', () => {
  assert.deepEqual(OWNER_TYPES, ['CART', 'COMPANY', 'COMPANY_LOCATION', 'CUSTOMER', 'PRODUCT', 'PRODUCTVARIANT', 'SHOP']);
  const declared = [{ namespace: 'custom', key: 'a', ownerType: 'PRODUCT_VARIANT' }];
  for (const version of ['2026-10', '2027-01']) {
    assert.match(refusalOf(ext(version, declared)), /owner_type of custom\.a must be one of CART, COMPANY/);
  }
});

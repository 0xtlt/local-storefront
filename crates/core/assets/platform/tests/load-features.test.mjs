import assert from 'node:assert/strict';
import { test } from 'node:test';

import { page } from './harness.mjs';

/** The queue `content_for_header` installs before the loader arrives. */
const bootstrap = () => {
  const queue = (...args) => queue.q.push(args);
  queue.q = [];
  return queue;
};

const load = (shopify) =>
  page({
    scripts: ['load-features.js'],
    shopify: { loadFeatures: bootstrap(), autoloadFeatures: bootstrap(), ...shopify },
  });

const turn = () => new Promise((resolve) => setTimeout(resolve, 0));

test('features asked for before the loader ran are loaded when it arrives', async () => {
  const queue = bootstrap();
  let loaded;
  queue([{ name: 'consent-tracking-api', version: '0.1' }], (error) => (loaded = [error]));
  const { Shopify } = page({ scripts: ['load-features.js'], shopify: { loadFeatures: queue, country: 'US' } });
  await turn();
  assert.deepEqual(loaded, [undefined]);
  assert.equal(typeof Shopify.customerPrivacy.setTrackingConsent, 'function');
});

test('a feature hosted by Shopify reports an error instead of hanging', async () => {
  const { Shopify } = load({ country: 'US' });
  const seen = [];
  Shopify.loadFeatures(
    [{ name: 'model-viewer-ui', version: '1.0', onLoad: (error) => seen.push(['onLoad', error?.length]) }],
    (error) => seen.push(['callback', error?.length]),
  );
  await turn();
  assert.deepEqual(seen, [
    ['onLoad', 1],
    ['callback', 1],
  ]);
});

test('where consent is required, nothing is allowed until the visitor answers', async () => {
  const { Shopify, record } = load({ country: 'FR' });
  Shopify.loadFeatures([{ name: 'consent-tracking-api', version: '0.1' }]);
  const privacy = Shopify.customerPrivacy;
  const collected = record('visitorConsentCollected');

  assert.equal(privacy.shouldShowBanner(), true);
  assert.equal(privacy.analyticsProcessingAllowed(), false);
  assert.equal(privacy.getTrackingConsent(), 'no_interaction');
  assert.deepEqual(privacy.currentVisitorConsent(), {
    analytics: '',
    marketing: '',
    preferences: '',
    sale_of_data: '',
  });

  let called = false;
  privacy.setTrackingConsent({ analytics: true, marketing: false, preferences: true }, () => (called = true));
  assert.equal(called, true);
  assert.equal(privacy.shouldShowBanner(), false);
  assert.equal(privacy.analyticsProcessingAllowed(), true);
  assert.equal(privacy.marketingAllowed(), false);
  assert.equal(privacy.userCanBeTracked(), false);
  assert.equal(privacy.currentVisitorConsent().marketing, 'no');
  assert.equal(collected.length, 1);
  assert.equal(collected[0].detail.analyticsAllowed, true);
  assert.equal(collected[0].detail.marketingAllowed, false);
});

test('where consent is not required, everything is allowed and no banner is due', () => {
  const { Shopify } = load({ country: 'US' });
  Shopify.loadFeatures([{ name: 'consent-tracking-api', version: '0.1' }]);
  const privacy = Shopify.customerPrivacy;
  assert.equal(privacy.shouldShowBanner(), false);
  assert.equal(privacy.analyticsProcessingAllowed(), true);
  assert.equal(privacy.marketingAllowed(), true);
  assert.equal(privacy.saleOfDataAllowed(), true);
  assert.equal(privacy.getRegion(), 'US');
});

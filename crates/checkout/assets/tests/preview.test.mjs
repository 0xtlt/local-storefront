// The stand-in server of the preview: the engine that applies operations, and the routes the
// real server has.

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { complete, update } from '../preview/mock-engine.mjs';
import { embed, startPreview } from '../preview/serve.mjs';
import { engineCheckout } from './harness.mjs';

const apply = (state, ...operations) => {
  const result = update(state, operations);
  assert.equal(result.error, undefined, result.error);
  return result.state;
};

test('applies the buyer\'s operations', () => {
  let state = apply(engineCheckout(), { type: 'email', value: 'a@b.c' }, { type: 'shippingAddress', address: { countryCode: 'FR', city: 'Paris' } });
  assert.equal(state.buyer.email, 'a@b.c');
  assert.equal(state.deliveryGroups[0].options.length, 2);
  state = apply(state, { type: 'deliveryOption', group: 'shipping', handle: 'express' }, { type: 'paymentMethod', handle: 'cod' });
  assert.equal(state.cost.total.amount, 85);
  assert.equal(state.selectedPayment, 'cod');
});

test('discount codes take money off, free shipping makes the option free', () => {
  let state = apply(engineCheckout(), { type: 'discountCode', action: 'add', code: 'save10' });
  assert.equal(state.cost.discounts.amount, 7.3);
  assert.equal(state.lines[0].discountAllocations[0].amount.amount, 4.9);
  assert.deepEqual(state.discountCodes, [{ code: 'SAVE10', applicable: true }]);
  state = apply(state, { type: 'discountCode', action: 'add', code: 'nope' }, { type: 'shippingAddress', address: { countryCode: 'FR' } }, { type: 'deliveryOption', group: 'shipping', handle: 'standard' }, { type: 'discountCode', action: 'add', code: 'freeship' });
  assert.deepEqual(state.discountCodes.map((entry) => entry.applicable), [true, false, true]);
  assert.equal(state.cost.shipping.amount, 0);
  assert.equal(state.deliveryGroups[0].options[0].originalCost.amount, 4.9);
  state = apply(state, { type: 'discountCode', action: 'remove', code: 'SAVE10' });
  assert.equal(state.cost.discounts.amount, 0);
});

test('cart lines, attributes, note and metafields', () => {
  let state = apply(
    engineCheckout(),
    { type: 'cartLines', change: { type: 'addCartLine', merchandiseId: 'gid://shopify/ProductVariant/202', quantity: 1 } },
    { type: 'cartLines', change: { type: 'updateCartLine', id: 'gid://shopify/CartLine/1', quantity: 3 } },
    { type: 'attribute', change: { type: 'updateAttribute', key: 'gift', value: 'true' } },
    { type: 'note', change: { type: 'updateNote', note: 'ring twice' } },
    { type: 'metafield', change: { type: 'updateCartMetafield', namespace: 'n', key: 'k', valueType: 'boolean', value: 'true' } },
  );
  assert.equal(state.lines.find((line) => line.variantId === 101).quantity, 3);
  assert.equal(state.lines.find((line) => line.variantId === 202).quantity, 3, 'the tote was already in the cart: 2 + 1');
  assert.deepEqual(state.attributes, [{ key: 'gift', value: 'true' }]);
  assert.equal(state.note, 'ring twice');
  assert.deepEqual(state.metafields, [{ namespace: 'n', key: 'k', type: 'boolean', value: 'true' }]);
  state = apply(state, { type: 'cartLines', change: { type: 'removeCartLine', id: 'gid://shopify/CartLine/2', quantity: 3 } }, { type: 'attribute', change: { type: 'removeAttribute', key: 'gift' } });
  assert.equal(state.lines.length, 1);
  assert.deepEqual(state.attributes, []);
});

test('refuses what it cannot apply and keeps the checkout as it was', () => {
  const state = engineCheckout();
  assert.match(update(state, [{ type: 'email', value: 'a@b.c' }, { type: 'deliveryOption', group: 'shipping', handle: 'drone' }]).error, /No delivery option/);
  assert.equal(state.buyer.email, null);
  assert.match(update(state, [{ type: 'teleport' }]).error, /Unknown operation/);
  assert.match(update(state, [{ type: 'discountCode', action: 'add', code: '' }]).error, /code/);
  assert.match(update(state, [{ type: 'cartLines', change: { type: 'removeCartLine', id: 'nope', quantity: 1 } }]).error, /No cart line/);
});

test('a Function can refuse the note, and completing needs an email and a payment method', () => {
  const state = apply(engineCheckout(), { type: 'note', change: { type: 'updateNote', note: 'Forbidden thing' } });
  assert.equal(state.errors[0].function, 'order-rules');
  const refused = complete(state);
  assert.deepEqual(refused.errors.errors.map((error) => error.target), ['$.cart', '$.cart.buyerIdentity.email', '$.cart.paymentMethods']);
  const ready = apply(engineCheckout(), { type: 'email', value: 'a@b.c' }, { type: 'paymentMethod', handle: 'card' });
  const done = complete(ready).state;
  assert.equal(done.status, 'completed');
  assert.equal(done.order.number, '#1001');
});

test('puts state in a script tag safely', () => {
  const text = embed({ note: '</script><script>alert(1)</script> & ' + String.fromCharCode(0x2028) });
  assert.ok(!text.includes('<'));
  assert.ok(!text.includes('&'));
  assert.ok(!text.includes(String.fromCharCode(0x2028)));
  assert.equal(JSON.parse(text).note, '</script><script>alert(1)</script> & ' + String.fromCharCode(0x2028));
});

let preview;
let base;
before(async () => {
  preview = await startPreview({ port: 0 });
  base = `http://127.0.0.1:${preview.port}`;
});
after(() => preview.close());

test('serves the shell with the state and the asset base filled in', async () => {
  const page = await fetch(`${base}/checkouts/cn/preview`);
  assert.equal(page.status, 200);
  const html = await page.text();
  assert.match(html, /<link rel="stylesheet" href="\/__lsf\/checkout\/assets\/checkout\.css"/);
  assert.match(html, /<script type="module" src="\/__lsf\/checkout\/assets\/checkout\.js">/);
  const state = JSON.parse(html.match(/<script type="application\/json" id="lsf-checkout-state">(.*?)<\/script>/s)[1]);
  assert.equal(state.token, 'preview');
  assert.equal(state.extensions.length, 2);
  assert.ok(!html.includes('<!--LSF_'));
  assert.equal((await fetch(`${base}/checkouts/cn/preview?variant=shop`).then((response) => response.text())).includes('"variant":"shop"'), true);
});

test('runs an extension on another API version when asked, until the checkout is reset', async () => {
  const versions = async (query) =>
    (await fetch(`${base}/checkouts/cn/preview.json${query}`).then((response) => response.json())).extensions.map((ext) => `${ext.handle}@${ext.apiVersion}`);
  assert.deepEqual(await versions(''), ['gift-message@2026-10', 'order-notice@2027-01']);
  assert.deepEqual(await versions('?apiVersion=order-notice:2026-10'), ['gift-message@2026-10', 'order-notice@2026-10']);
  assert.deepEqual(await versions(''), ['gift-message@2026-10', 'order-notice@2026-10'], 'it stays');
  await fetch(`${base}/checkouts/cn/preview?reset`);
  assert.deepEqual(await versions(''), ['gift-message@2026-10', 'order-notice@2027-01']);
});

test('serves the assets and the bundles of the extensions, and nothing else', async () => {
  for (const file of ['checkout.js', 'checkout.css', 'sandbox.html', 'sandbox.js', 'extension-host.js', 'api-versions.js']) {
    assert.equal((await fetch(`${base}/__lsf/checkout/assets/${file}`)).status, 200, file);
  }
  const js = await fetch(`${base}/__lsf/extensions/gift-message/gift-message.js`);
  assert.equal(js.status, 200);
  assert.match(js.headers.get('content-type'), /javascript/);
  assert.equal((await fetch(`${base}/__lsf/extensions/gift-message/missing.js`)).status, 404);
  assert.equal((await fetch(`${base}/__lsf/checkout/assets/..%2F..%2FCargo.toml`)).status, 404);
  assert.equal((await fetch(`${base}/__lsf/checkout/assets/%2e%2e/Cargo.toml`)).status, 404);
});

test('updates and completes like the real routes', async () => {
  await fetch(`${base}/checkouts/cn/preview?reset`);
  const post = (path, body) => fetch(`${base}/checkouts/cn/preview${path}`, { method: 'POST', body: JSON.stringify(body) });
  const bad = await post('/update', { operations: [{ type: 'teleport' }] });
  assert.equal(bad.status, 400);
  assert.match((await bad.json()).error, /Unknown operation/);
  const refused = await post('/complete', {});
  assert.equal(refused.status, 422);
  assert.ok((await refused.json()).errors.length >= 2);
  const updated = await post('/update', { operations: [{ type: 'email', value: 'a@b.c' }, { type: 'paymentMethod', handle: 'card' }] });
  assert.equal((await updated.json()).buyer.email, 'a@b.c');
  const done = await post('/complete', {});
  assert.equal(done.status, 200);
  assert.match((await done.json()).redirect, /\/checkouts\/cn\/preview\/thank-you$/);
  const thanks = await fetch(`${base}/checkouts/cn/preview/thank-you`).then((response) => response.text());
  assert.match(thanks, /"status":"completed"/);
  assert.equal((await fetch(`${base}/checkouts/cn/preview.json`).then((response) => response.json())).status, 'completed');
});

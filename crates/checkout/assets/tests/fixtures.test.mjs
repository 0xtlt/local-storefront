// The two fixture extensions, as built bundles: run through the whole sandbox (boot, API,
// components) against states of the checkout. `gift-message` is on API 2026-10 and draws the
// DOM by hand; `order-notice` is a Preact bundle on the 2027-01 release candidate.

import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { FakeEvent } from './fake-dom.mjs';
import { refusalOf } from '../api-versions.js';
import { bundle, engineCheckout, fixtures, init, pageState, plain, sandboxWindow, settle } from './harness.mjs';
import { update } from '../preview/mock-engine.mjs';

const SANDBOX = ['sandbox-api.js', 'sandbox-components.js', 'sandbox.js'];
const url = (handle) => `http://localhost/__lsf/extensions/${handle}/${handle}.js`;
const next = (...operations) => pageState(update(engineCheckout(), operations).state);

/** Boots the sandbox for an extension and waits for its target to render. */
async function boot(handle, target, options = {}) {
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url(handle)]: bundle(handle) } });
  win.receive(init({ handle, target, ...options }));
  for (let i = 0; i < 4; i++) await settle();
  return win;
}

const toml = (handle) => readFileSync(join(fixtures, handle, 'shopify.extension.toml'), 'utf8');
const field = (text, name) => text.match(new RegExp(`^\\s*${name}\\s*=\\s*"([^"]*)"`, 'm'))?.[1];
const targetsOf = (text) => [...text.matchAll(/^\s*target\s*=\s*"([^"]+)"/gm)].map((match) => match[1]);

/** The `[[extensions.metafields]]` of a toml, as the state of the page spells them. */
const metafieldsOf = (text) =>
  text
    .split(/^\s*\[\[extensions\.metafields\]\]\s*$/m)
    .slice(1)
    .map((block) => block.split(/^\s*\[/m)[0])
    .map((block) => ({ namespace: field(block, 'namespace'), key: field(block, 'key'), ownerType: field(block, 'owner_type') }));

test('each fixture is an app extension with a toml and a bundle named after its handle', () => {
  // The fixture app also holds Functions: only its UI extensions are bundles.
  const handles = readdirSync(fixtures)
    .filter((handle) => field(toml(handle), 'type') === 'ui_extension')
    .sort();
  assert.deepEqual(handles, ['gift-message', 'order-notice']);
  for (const handle of handles) {
    const text = toml(handle);
    assert.equal(field(text, 'handle'), handle);
    assert.equal(field(text, 'type'), 'ui_extension');
    assert.ok(field(text, 'api_version'));
    assert.ok(bundle(handle).startsWith('//'), 'the bundle says what it is');
    assert.match(bundle(handle).replace(/^(\/\/.*\n)+/, ''), /^\(\(\) ?=> ?\{/, 'an IIFE, as esbuild leaves it');
  }
  assert.equal(field(toml('gift-message'), 'api_version'), '2026-10');
  assert.equal(field(toml('order-notice'), 'api_version'), '2027-01');
});

test('each fixture declares a metafield the way its API version asks for', () => {
  const gift = { apiVersion: field(toml('gift-message'), 'api_version'), metafields: metafieldsOf(toml('gift-message')) };
  const notice = { apiVersion: field(toml('order-notice'), 'api_version'), metafields: metafieldsOf(toml('order-notice')) };
  // 2026-10 lets a declaration go without owner_type: gift-message does.
  assert.deepEqual(gift.metafields, [{ namespace: '$app:gift', key: 'wrapped', ownerType: undefined }]);
  assert.equal(refusalOf(gift), null);
  // 2027-01 requires it: order-notice has it, and gift-message would not run there.
  assert.deepEqual(notice.metafields, [{ namespace: '$app:notice', key: 'dismissed', ownerType: 'CART' }]);
  assert.equal(refusalOf(notice), null);
  assert.match(refusalOf({ ...gift, apiVersion: '2027-01' }), /requires owner_type.*\$app:gift\.wrapped/);
  assert.equal(refusalOf({ ...notice, apiVersion: '2026-10' }), null, 'owner_type is allowed on 2026-10');
});

test('a bundle registers the targets its toml declares, and no others', () => {
  for (const handle of ['gift-message', 'order-notice']) {
    const names = [];
    const win = sandboxWindow();
    win.run(`globalThis.shopify = { extend: (name, fn) => { globalThis.__names = (globalThis.__names || []).concat(name); } };`);
    win.run(bundle(handle));
    names.push(...plain(win.context.__names));
    assert.deepEqual(names.sort(), targetsOf(toml(handle)).sort(), handle);
  }
});

test('gift-message draws its block with the current API and follows the checkout', async () => {
  const win = await boot('gift-message', 'purchase.checkout.block.render', { ext: { settings: { max_length: 80 } } });
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  const root = win.document.body.firstChild;
  assert.equal(root.localName, 's-stack');
  const [heading, gift, message, banner, total] = root.childNodes;
  message.value = ''; // what a real text area holds
  assert.equal(heading.textContent, 'Gift options');
  assert.equal(gift.getAttribute('label'), 'This order is a gift');
  assert.equal(message.getAttribute('maxlength'), '80', 'the setting reaches the extension');
  assert.equal(total.textContent, 'Order total: €73.00');
  assert.deepEqual(win.posts('interceptors'), [{ lsf: true, type: 'interceptors', count: 1 }]);

  // The cost changes: the extension's signal subscriber redraws.
  win.receive({ type: 'state', state: next({ type: 'discountCode', action: 'add', code: 'SAVE10' }) });
  assert.equal(total.textContent, 'Order total: €65.70');

  // The buyer ticks the box: attributes and a cart metafield are written.
  gift.checked = true;
  gift.dispatchEvent(new FakeEvent('change'));
  const [first] = win.posts('change');
  assert.deepEqual(first.change, { type: 'updateAttribute', key: 'gift', value: 'true' });
  win.receive({ type: 'result', id: first.id, result: { type: 'success' } });
  await settle();
  const second = win.posts('change')[1];
  assert.equal(second.method, 'applyMetafieldChange');
  assert.deepEqual(second.change.metafield, { namespace: '$app:gift', key: 'wrapped', type: 'boolean', value: 'true' });

  // Without a message the buyer cannot pay.
  win.receive({ type: 'state', state: next({ type: 'attribute', change: { type: 'updateAttribute', key: 'gift', value: 'true' } }) });
  win.receive({ type: 'intercept', id: 'i1', canBlockProgress: true });
  await settle();
  const answer = win.posts('intercepted').at(-1);
  assert.equal(answer.results[0].behavior, 'block');
  assert.equal(answer.results[0].reason, 'The gift message is empty.');
  assert.equal(banner.hidden, false);
  // Nor can it block a buyer who cannot be blocked.
  win.receive({ type: 'intercept', id: 'i2', canBlockProgress: false });
  await settle();
  assert.equal(win.posts('intercepted').at(-1).results[0].behavior, 'allow');

  // With a message, it saves it and lets the buyer through.
  message.value = 'Happy birthday';
  message.dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(win.posts('change').at(-1).change, { type: 'updateAttribute', key: 'gift_message', value: 'Happy birthday' });
  win.receive({ type: 'intercept', id: 'i3', canBlockProgress: true });
  await settle();
  assert.equal(win.posts('intercepted').at(-1).results[0].behavior, 'allow');
});

test('gift-message shows each line it is placed on', async () => {
  const win = await boot('gift-message', 'purchase.checkout.cart-line-item.render-after', { ctx: { lineId: 'gid://shopify/CartLine/2' } });
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  const [badge, line] = win.document.body.firstChild.childNodes;
  assert.equal(line.textContent, 'Canvas tote × 2 · SKU TOTE-01');
  assert.equal(badge.hidden, false);
  win.receive({ type: 'state', ctx: { lineId: 'gid://shopify/CartLine/2' }, state: next({ type: 'cartLines', change: { type: 'updateCartLine', id: 'gid://shopify/CartLine/2', quantity: 5 } }) });
  assert.equal(line.textContent, 'Canvas tote × 5 · SKU TOTE-01');
});

test('gift-message thanks the buyer on the thank-you page', async () => {
  const done = pageState({ ...update(engineCheckout(), [{ type: 'attribute', change: { type: 'updateAttribute', key: 'gift_message', value: 'Enjoy' } }]).state, status: 'completed', order: { id: 'gid://shopify/Order/1', number: '#1001' } });
  const win = await boot('gift-message', 'purchase.thank-you.block.render', { state: done });
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  const banner = win.document.body.firstChild;
  assert.equal(banner.getAttribute('heading'), 'Your gift message');
  assert.equal(banner.textContent, '“Enjoy” — order #1001');
  assert.equal(banner.hidden, false);
});

/** Boots order-notice, a Preact bundle, and waits for Preact to draw. */
const notice = (target, options = {}) =>
  boot('order-notice', target, { apiVersion: '2027-01', ...options, ext: { capabilities: { api_access: true }, ...options.ext } });

test('order-notice draws with Preact and follows the checkout through signals', async () => {
  const win = await notice('purchase.checkout.reductions.render-after', { ext: { settings: { threshold: 100, code: 'FREESHIP' } } });
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  const [banner] = win.document.body.childNodes;
  assert.equal(banner.localName, 's-banner');
  assert.equal(banner.getAttribute('heading'), 'Free shipping from €100.00');
  assert.equal(banner.getAttribute('tone'), 'info');
  const [sentence, progress, actions] = banner.firstChild.childNodes;
  assert.equal(sentence.textContent, 'You are €27.00 away from free shipping.');
  assert.equal(progress.localName, 's-progress');
  assert.equal(progress.getAttribute('value'), '73');
  assert.equal(progress.getAttribute('max'), '100');
  const [apply] = actions.childNodes;
  assert.equal(apply.getAttribute('variant'), 'secondary');
  assert.equal(apply.textContent, 'Apply FREESHIP');

  // Preact reads `shopify.cost.totalAmount.value` while it renders: a new total redraws it.
  win.receive({ type: 'state', state: next({ type: 'cartLines', change: { type: 'updateCartLine', id: 'gid://shopify/CartLine/2', quantity: 6 } }) });
  await settle();
  assert.equal(sentence.textContent, 'Shipping is free on this order.');
  assert.equal(banner.getAttribute('tone'), 'success');
  assert.equal(progress.getAttribute('value'), '100');

  apply.click();
  const request = win.posts('change').at(-1);
  assert.deepEqual([request.method, request.change], ['applyDiscountCodeChange', { type: 'addDiscountCode', code: 'FREESHIP' }]);
  win.receive({ type: 'result', id: request.id, result: { type: 'error', message: 'Code not valid' } });
  await settle();
  assert.equal(banner.getAttribute('heading'), 'Code not valid');
  assert.equal(banner.getAttribute('tone'), 'critical');
});

test('order-notice reads the cart metafield it declares with an owner_type, and writes it to dismiss', async () => {
  const win = await notice('purchase.checkout.reductions.render-after');
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  assert.equal(win.document.body.childNodes.length, 1);
  const dismiss = win.document.body.firstChild.firstChild.childNodes[2].childNodes[1];
  assert.equal(dismiss.textContent, 'Dismiss');
  dismiss.click();
  const request = win.posts('change').at(-1);
  assert.equal(request.method, 'applyMetafieldChange');
  assert.deepEqual(request.change, {
    type: 'updateCartMetafield',
    metafield: { namespace: '$app:notice', key: 'dismissed', type: 'boolean', value: 'true' },
  });

  // The checkout now holds it as a cart metafield: the notice goes.
  const stored = { type: 'metafield', change: { type: 'updateCartMetafield', namespace: '$app:notice', key: 'dismissed', valueType: 'boolean', value: 'true' } };
  win.receive({ type: 'state', state: next(stored) });
  await settle();
  assert.equal(win.document.body.childNodes.length, 0);
  // Another metafield of the cart does not dismiss it.
  const other = { ...stored, change: { ...stored.change, key: 'other' } };
  const again = await notice('purchase.checkout.reductions.render-after', { state: next(other) });
  assert.equal(again.document.body.childNodes.length, 1);
});

test('order-notice reads and writes the note from the shipping step', async () => {
  const state = pageState({ ...update(engineCheckout(), [{ type: 'shippingAddress', address: { countryCode: 'FR' } }]).state, note: 'ring twice' });
  const win = await notice('purchase.checkout.shipping-option-list.render-after', { state });
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  const [info, field] = win.document.body.firstChild.childNodes;
  assert.equal(info.textContent, '2 shipping options for this order');
  assert.equal(field.localName, 's-text-area', 'multiline');
  assert.equal(field.getAttribute('value'), 'ring twice');
  for (const [typed, change] of [['leave at the door', { type: 'updateNote', note: 'leave at the door' }], ['', { type: 'removeNote' }]]) {
    field.value = typed;
    field.dispatchEvent(new FakeEvent('change'));
    assert.deepEqual(win.posts('change').at(-1).change, change);
  }
});

test('reports a bundle that is missing, and a target it did not register', async () => {
  const missing = sandboxWindow({ scripts: SANDBOX, bundles: {} });
  missing.receive(init({ handle: 'gift-message', target: 'purchase.checkout.block.render' }));
  for (let i = 0; i < 3; i++) await settle();
  assert.match(missing.posts('error')[0].message, /shopify app build/);

  const wrong = await boot('order-notice', 'purchase.checkout.block.render', { apiVersion: '2027-01' });
  assert.match(wrong.posts('error')[0].message, /did not register purchase\.checkout\.block\.render.*registered: purchase\.checkout\.reductions\.render-after, purchase\.checkout\.shipping-option-list\.render-after/);
});

test('the frame is as tall as the extension and says when it changes', async () => {
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url('gift-message')]: bundle('gift-message') } });
  win.document.body.heightForTests = 120;
  win.receive(init({ handle: 'gift-message', target: 'purchase.checkout.block.render' }));
  for (let i = 0; i < 3; i++) await settle();
  assert.deepEqual(win.posts('resize'), [{ lsf: true, type: 'resize', height: 120 }]);
  assert.deepEqual(win.posts('ready'), [{ lsf: true, type: 'ready' }]);
});

test('ignores messages that do not come from the page', async () => {
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url('gift-message')]: bundle('gift-message') } });
  win.receive(init({ handle: 'gift-message', target: 'purchase.checkout.block.render' }), { postMessage() {} });
  for (let i = 0; i < 3; i++) await settle();
  assert.equal(win.document.body.childNodes.length, 0);
  assert.deepEqual(win.posts('resize'), []);
});

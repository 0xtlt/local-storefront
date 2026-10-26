// The two fixture extensions, as built bundles: run through the whole sandbox (boot, API,
// components or legacy host) against states of the checkout.

import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { FakeEvent } from './fake-dom.mjs';
import { bundle, engineCheckout, fixtures, init, pageState, plain, sandboxWindow, settle } from './harness.mjs';
import { update } from '../preview/mock-engine.mjs';

const SANDBOX = ['sandbox-api.js', 'sandbox-components.js', 'sandbox-legacy.js', 'sandbox.js'];
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
    assert.match(bundle(handle).replace(/^(\/\/.*\n)+/, ''), /^\(\(\) => \{/, 'an IIFE, as esbuild leaves it');
  }
  assert.equal(field(toml('gift-message'), 'api_version'), '2025-10');
  assert.equal(field(toml('order-notice'), 'api_version'), '2025-07');
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

test('order-notice draws a remote-ui tree and updates it', async () => {
  const settings = { threshold: 100, code: 'FREESHIP' };
  const win = await boot('order-notice', 'purchase.checkout.reductions.render-after', { apiVersion: '2025-07', ext: { settings, capabilities: { api_access: true } } });
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  const banner = win.document.body.firstChild;
  assert.equal(banner.localName, 's-banner');
  assert.equal(banner.getAttribute('heading'), 'Free shipping from €100.00');
  assert.equal(banner.getAttribute('tone'), 'info');
  const stack = banner.firstChild;
  assert.equal(stack.localName, 's-stack');
  const [sentence, progress, button] = stack.childNodes;
  assert.equal(sentence.textContent, 'You are €27.00 away from free shipping.');
  assert.equal(progress.localName, 's-progress');
  assert.equal(progress.getAttribute('value'), '73');
  assert.equal(button.localName, 's-button');
  assert.equal(button.getAttribute('variant'), 'secondary');
  assert.equal(button.textContent, 'Apply FREESHIP');

  win.receive({ type: 'state', state: next({ type: 'cartLines', change: { type: 'updateCartLine', id: 'gid://shopify/CartLine/2', quantity: 6 } }) });
  assert.equal(sentence.textContent, 'Shipping is free on this order.');
  assert.equal(banner.getAttribute('tone'), 'success');
  assert.equal(progress.getAttribute('value'), '100');

  button.click();
  const request = win.posts('change').at(-1);
  assert.deepEqual([request.method, request.change], ['applyDiscountCodeChange', { type: 'addDiscountCode', code: 'FREESHIP' }]);
  win.receive({ type: 'result', id: request.id, result: { type: 'error', message: 'Code not valid' } });
  await settle();
  assert.equal(banner.getAttribute('heading'), 'Code not valid');
  assert.equal(banner.getAttribute('tone'), 'critical');
});

test('order-notice reads and writes the note from the shipping step', async () => {
  const state = pageState({ ...update(engineCheckout(), [{ type: 'shippingAddress', address: { countryCode: 'FR' } }]).state, note: 'ring twice' });
  const win = await boot('order-notice', 'purchase.checkout.shipping-option-list.render-after', { apiVersion: '2025-07', state, ext: { capabilities: { api_access: true } } });
  assert.equal(win.posts('error').length, 0, JSON.stringify(win.posts('error')));
  const [info, field] = win.document.body.firstChild.childNodes;
  assert.equal(info.textContent, '2 shipping options for this order');
  assert.equal(field.localName, 's-text-area', 'multiline');
  assert.equal(field.value, 'ring twice');
  field.value = 'leave at the door';
  field.dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(win.posts('change').at(-1).change, { type: 'updateNote', note: 'leave at the door' });
  field.value = '';
  field.dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(win.posts('change').at(-1).change, { type: 'removeNote' });
});

test('reports a bundle that is missing, and a target it did not register', async () => {
  const missing = sandboxWindow({ scripts: SANDBOX, bundles: {} });
  missing.receive(init({ handle: 'gift-message', target: 'purchase.checkout.block.render' }));
  for (let i = 0; i < 3; i++) await settle();
  assert.match(missing.posts('error')[0].message, /shopify app build/);

  const wrong = await boot('order-notice', 'purchase.checkout.block.render', { apiVersion: '2025-07' });
  assert.match(wrong.posts('error')[0].message, /did not register purchase\.checkout\.block\.render.*registered: purchase\.checkout\.reductions\.render-after, purchase\.checkout\.shipping-option-list\.render-after/);
});

test('an extension registered under an older name of its target still runs', async () => {
  const handle = 'gift-message';
  const source = bundle(handle).replace('"purchase.checkout.block.render"', '"Checkout::Dynamic::Render"');
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url(handle)]: source } });
  win.receive(init({ handle, target: 'purchase.checkout.block.render', names: ['purchase.checkout.block.render', 'Checkout::Dynamic::Render'] }));
  for (let i = 0; i < 4; i++) await settle();
  assert.equal(win.posts('error').length, 0);
  assert.equal(win.document.body.firstChild.localName, 's-stack');
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

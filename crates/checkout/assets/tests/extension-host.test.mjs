// The page side of extensions: the iframes, the messages, the buyer journey.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createExtensionHost, sessionToken, stateForExtensions } from '../extension-host.js';
import { FakeDocument } from './fake-dom.mjs';
import { checkoutState, settle } from './harness.mjs';

const SANDBOX = 'http://localhost/__lsf/checkout/assets/sandbox.html';

function setup({ extensions, storage, state } = {}) {
  const document = new FakeDocument();
  const listeners = [];
  const window = {
    addEventListener: (type, listener) => listeners.push(listener),
    removeEventListener: () => {},
    opened: [],
    open(...args) {
      this.opened.push(args);
    },
  };
  let current = { ...(state ?? checkoutState()), extensions: extensions ?? checkoutState().extensions };
  const dispatched = [];
  const log = { calls: [], error: (...args) => log.calls.push(['error', ...args]), warn: (...args) => log.calls.push(['warn', ...args]), debug: (...args) => log.calls.push(['debug', ...args]) };
  const host = createExtensionHost({
    document,
    window,
    sandboxUrl: SANDBOX,
    getState: () => current,
    dispatch: async (operations) => {
      dispatched.push(operations);
      if (dispatched.failWith) throw new Error(dispatched.failWith);
    },
    storage,
    console: log,
  });
  const slot = document.createElement('div');
  document.body.appendChild(slot);
  /** Gives each iframe of the instances a window that remembers what it was sent. */
  const windows = new Map();
  const connect = () => {
    for (const instance of host.instances()) {
      if (instance.iframe.contentWindow) continue;
      const posted = [];
      instance.iframe.contentWindow = { posted, postMessage: (message) => posted.push(message) };
      windows.set(instance.ext.handle + instance.target, instance.iframe.contentWindow);
    }
  };
  const from = (instance, message) => host.onMessage({ source: instance.iframe.contentWindow, data: { lsf: true, ...message } });
  return { host, document, slot, window, dispatched, log, windows, connect, from, setState: (next) => (current = next), state: () => current };
}

const BLOCK = 'purchase.checkout.block.render';

test('shows extensions only where they target, one frame for each', () => {
  const { host, slot, connect } = setup();
  assert.equal(host.hasTarget(BLOCK), true);
  assert.equal(host.hasTarget('purchase.checkout.reductions.render-after'), true);
  assert.equal(host.hasTarget('purchase.checkout.footer.render-after'), false);
  host.attach(slot, BLOCK, { key: 'block' });
  assert.equal(host.instances().length, 1);
  const [frame] = host.instances();
  assert.equal(frame.ext.handle, 'gift-message');
  const iframe = frame.iframe;
  assert.equal(iframe.getAttribute('sandbox'), 'allow-scripts', 'nothing more than scripts');
  assert.equal(iframe.getAttribute('src'), SANDBOX);
  assert.equal(iframe.getAttribute('title'), `gift-message (${BLOCK})`);
  assert.equal(slot.firstChild.getAttribute('data-extension'), 'gift-message');
  connect();
});

test('loads the extensions of the two API versions it runs, and says why it leaves out the others', () => {
  const extensions = [
    { handle: 'stable', apiVersion: '2026-10', script: '/s.js', targets: [BLOCK] },
    { handle: 'candidate', apiVersion: '2027-01', script: '/c.js', targets: [BLOCK] },
    { handle: 'react', apiVersion: '2025-07', script: '/r.js', targets: [BLOCK] },
    { handle: 'unversioned', script: '/u.js', targets: [BLOCK] },
  ];
  const { host, slot } = setup({ extensions });
  host.attach(slot, BLOCK);
  assert.deepEqual(host.instances().map((instance) => instance.ext.handle), ['stable', 'candidate']);
  assert.deepEqual(host.refusals().map((refusal) => refusal.handle), ['react', 'unversioned']);
  assert.match(host.refusals()[0].reason, /API version 2025-07 is not supported.*2026-10 and 2027-01/);
  assert.match(host.refusals()[1].reason, /API version \(none\)/);
});

test('an extension that only another API version allows is not loaded either', () => {
  const metafields = [{ namespace: 'custom', key: 'note' }];
  const on = (apiVersion) => setup({ extensions: [{ handle: 'meta', apiVersion, script: '/m.js', targets: [BLOCK], metafields }] }).host;
  assert.equal(on('2026-10').hasTarget(BLOCK), true);
  assert.deepEqual(on('2026-10').refusals(), []);
  assert.equal(on('2027-01').hasTarget(BLOCK), false);
  assert.match(on('2027-01').refusals()[0].reason, /requires owner_type on every metafield declaration: custom\.note/);
});

test('extensions without a bundle are left out', () => {
  const { host } = setup({ extensions: [{ handle: 'nobuild', apiVersion: '2026-10', targets: [BLOCK] }] });
  assert.equal(host.hasTarget(BLOCK), false);
});

test('answers a frame that is ready with everything it needs', () => {
  const { host, slot, connect, from, windows } = setup();
  host.attach(slot, 'purchase.checkout.cart-line-item.render-after', { key: 'l1', lineId: 'gid://shopify/CartLine/1' });
  connect();
  const [frame] = host.instances();
  from(frame, { type: 'ready' });
  const [init] = windows.get('gift-messagepurchase.checkout.cart-line-item.render-after').posted;
  assert.equal(init.lsf, true);
  assert.equal(init.type, 'init');
  assert.equal(init.handle, 'gift-message');
  assert.equal(init.target, 'purchase.checkout.cart-line-item.render-after');
  assert.equal(init.script, 'http://localhost/__lsf/extensions/gift-message/gift-message.js', 'an absolute URL, for a frame of another origin');
  assert.equal(init.apiVersion, '2026-10');
  assert.equal('names' in init, false);
  assert.deepEqual(init.ctx, { key: 'l1', lineId: 'gid://shopify/CartLine/1' });
  assert.deepEqual(init.ext.settings, { max_length: 80 });
  assert.deepEqual(init.ext.capabilities, { api_access: true, block_progress: true });
  assert.equal(init.ext.translations.en.total, 'Order total: {{amount}}');
  assert.equal(init.variant, 'standard');
  assert.equal(init.state.token, 'preview');
  assert.equal('runs' in init.state, false, 'what Functions saw is not for extensions');
  assert.equal('extensions' in init.state, false);
  assert.equal('diagnostics' in init.state, false);
});

test('tells each extension its own API version', () => {
  const { host, slot, connect, from, windows } = setup();
  host.attach(slot, 'purchase.checkout.reductions.render-after');
  connect();
  from(host.instances()[0], { type: 'ready' });
  const [init] = windows.get('order-noticepurchase.checkout.reductions.render-after').posted;
  assert.equal(init.apiVersion, '2027-01');
  assert.equal('mode' in init, false);
});

test('sends the new state to the frames that are ready', () => {
  const { host, slot, connect, from, windows } = setup();
  host.attach(slot, BLOCK);
  host.attach(slot, 'purchase.checkout.reductions.render-after');
  connect();
  const [ready, notReady] = host.instances();
  from(ready, { type: 'ready' });
  host.update({ ...checkoutState(), note: 'new', runs: [1] });
  const posted = windows.get('gift-message' + BLOCK).posted;
  assert.equal(posted.at(-1).type, 'state');
  assert.equal(posted.at(-1).state.note, 'new');
  assert.equal('runs' in posted.at(-1).state, false);
  assert.deepEqual(notReady.iframe.contentWindow.posted, []);
});

test('ignores messages from anything but its frames', () => {
  const { host, slot, connect, dispatched } = setup();
  host.attach(slot, BLOCK);
  connect();
  host.onMessage({ source: { postMessage() {} }, data: { lsf: true, type: 'change', id: 1, method: 'applyNoteChange', change: { type: 'removeNote' } } });
  host.onMessage({ source: host.instances()[0].iframe.contentWindow, data: { type: 'change' } });
  host.onMessage({ source: host.instances()[0].iframe.contentWindow, data: null });
  assert.deepEqual(dispatched, []);
});

test('turns an extension\'s change into operations and answers', async () => {
  const { host, slot, connect, from, dispatched } = setup();
  host.attach(slot, BLOCK);
  connect();
  const [frame] = host.instances();
  from(frame, { type: 'change', id: 7, method: 'applyAttributeChange', change: { type: 'updateAttribute', key: 'gift', value: 'true' } });
  await settle();
  assert.deepEqual(dispatched, [[{ type: 'attribute', change: { type: 'updateAttribute', key: 'gift', value: 'true' } }]]);
  assert.deepEqual(frame.iframe.contentWindow.posted.at(-1), { lsf: true, type: 'result', id: 7, result: { type: 'success' } });

  from(frame, { type: 'change', id: 8, method: 'applyGiftCardChange', change: { type: 'addGiftCard', code: 'x' } });
  await settle();
  assert.equal(frame.iframe.contentWindow.posted.at(-1).result.type, 'error');
  assert.equal(dispatched.length, 1, 'what the checkout cannot do is not sent');

  dispatched.failWith = 'No such line';
  from(frame, { type: 'change', id: 9, method: 'applyNoteChange', change: { type: 'removeNote' } });
  await settle();
  assert.deepEqual(frame.iframe.contentWindow.posted.at(-1).result, { type: 'error', message: 'No such line' });
});

test('answers address changes in the shape of their API', async () => {
  const { host, slot, connect, from } = setup();
  host.attach(slot, BLOCK);
  connect();
  const [frame] = host.instances();
  from(frame, { type: 'change', id: 1, method: 'applyShippingAddressChange', change: { type: 'updateShippingAddress', address: { city: 'Paris' } } });
  await settle();
  assert.deepEqual(frame.iframe.contentWindow.posted.at(-1).result, { type: 'success', errors: null });
});

test('sizes the frame, grows it for an overlay and shows its errors', () => {
  const { host, slot, connect, from, log } = setup();
  host.attach(slot, BLOCK);
  connect();
  const [frame] = host.instances();
  from(frame, { type: 'resize', height: 87.2 });
  assert.equal(frame.iframe.style.height, '88px');
  from(frame, { type: 'overlay', open: true });
  assert.match(frame.iframe.className, /lsf-ext-overlay/);
  from(frame, { type: 'overlay', open: false });
  assert.doesNotMatch(frame.iframe.className, /lsf-ext-overlay/);
  from(frame, { type: 'error', message: 'boom' });
  from(frame, { type: 'error', message: 'again' });
  assert.equal(slot.firstChild.firstChild.className, 'lsf-ext-error');
  assert.equal(slot.firstChild.firstChild.textContent, `gift-message (${BLOCK}): again`);
  assert.equal(slot.firstChild.querySelectorAll('.lsf-ext-error').length, 1, 'one banner');
  assert.equal(log.calls.filter(([level]) => level === 'error').length, 2);
  from(frame, { type: 'dismiss' });
  assert.equal(slot.firstChild.style.display, 'none');
});

test('keeps storage apart for each extension', async () => {
  const items = new Map();
  const storage = { getItem: (key) => items.get(key) ?? null, setItem: (key, value) => items.set(key, value), removeItem: (key) => items.delete(key) };
  const { host, slot, connect, from } = setup({ storage });
  host.attach(slot, BLOCK);
  connect();
  const [frame] = host.instances();
  from(frame, { type: 'storage', id: 1, op: 'write', key: 'k', value: { a: 1 } });
  assert.deepEqual([...items.keys()], ['lsf-extension:gift-message:k']);
  from(frame, { type: 'storage', id: 2, op: 'read', key: 'k' });
  assert.deepEqual(frame.iframe.contentWindow.posted.at(-1).result, { a: 1 });
  from(frame, { type: 'storage', id: 3, op: 'delete', key: 'k' });
  from(frame, { type: 'storage', id: 4, op: 'read', key: 'k' });
  assert.equal(frame.iframe.contentWindow.posted.at(-1).result, null);
});

test('hands out a session token that looks like the real one', () => {
  const { state } = setup();
  const token = sessionToken({ ...state(), buyer: { customer: { id: 'gid://shopify/Customer/1' } } }, { handle: 'gift-message' }, 1_700_000_000_000);
  const [header, payload, signature] = token.split('.');
  assert.deepEqual(JSON.parse(atob(header.replace(/-/g, '+').replace(/_/g, '/'))), { alg: 'HS256', typ: 'JWT' });
  const claims = JSON.parse(atob(payload.replace(/-/g, '+').replace(/_/g, '/')));
  assert.equal(claims.dest, 'http://127.0.0.1:9293');
  assert.equal(claims.iss, 'http://127.0.0.1:9293/admin');
  assert.equal(claims.sub, 'gid://shopify/Customer/1');
  assert.equal(claims.sid, 'preview');
  assert.equal(claims.exp - claims.iat, 60);
  assert.ok(signature);
  const { host, slot, connect, from } = setup();
  host.attach(slot, BLOCK);
  connect();
  from(host.instances()[0], { type: 'sessionToken', id: 5 });
  assert.equal(host.instances()[0].iframe.contentWindow.posted.at(-1).result.split('.').length, 3);
});

test('asks the interceptors before the buyer pays, and tells them how it went', async () => {
  const { host, slot, connect, from } = setup();
  host.attach(slot, BLOCK);
  host.attach(slot, 'purchase.checkout.reductions.render-after');
  connect();
  const [gift, notice] = host.instances();
  from(gift, { type: 'ready' });
  from(notice, { type: 'ready' });
  from(gift, { type: 'interceptors', count: 1 });
  const pending = host.intercept();
  const asked = gift.iframe.contentWindow.posted.find((message) => message.type === 'intercept');
  assert.equal(asked.canBlockProgress, true, 'gift-message has the capability');
  assert.equal(notice.iframe.contentWindow.posted.some((message) => message.type === 'intercept'), false, 'nobody asks who has no interceptor');
  from(gift, { type: 'intercepted', id: asked.id, results: [{ behavior: 'block', reason: 'Empty message', errors: [{ message: 'Write one' }] }, { behavior: 'allow' }] });
  assert.deepEqual(await pending, { blocks: [{ handle: 'gift-message', reason: 'Empty message', errors: [{ message: 'Write one' }] }] });
  assert.deepEqual(gift.iframe.contentWindow.posted.at(-1), { lsf: true, type: 'perform', behavior: 'block' });

  const second = host.intercept();
  from(gift, { type: 'intercepted', id: gift.iframe.contentWindow.posted.findLast((message) => message.type === 'intercept').id, results: [{ behavior: 'allow' }] });
  assert.deepEqual(await second, { blocks: [] });
  assert.equal(gift.iframe.contentWindow.posted.at(-1).behavior, 'allow');
});

test('says an extension without the capability cannot block', async () => {
  const { host, slot, connect, from } = setup({ extensions: [{ handle: 'weak', apiVersion: '2026-10', script: '/w.js', targets: [BLOCK], capabilities: { api_access: true } }] });
  host.attach(slot, BLOCK);
  connect();
  const [frame] = host.instances();
  from(frame, { type: 'ready' });
  from(frame, { type: 'interceptors', count: 1 });
  const pending = host.intercept();
  assert.equal(frame.iframe.contentWindow.posted.find((message) => message.type === 'intercept').canBlockProgress, false);
  from(frame, { type: 'intercepted', id: frame.iframe.contentWindow.posted.find((message) => message.type === 'intercept').id, results: [] });
  await pending;
});

test('removes the frames when their slot goes', () => {
  const { host, slot, connect } = setup();
  host.attach(slot, BLOCK);
  connect();
  const [frame] = host.instances();
  host.detach(slot);
  assert.equal(host.instances().length, 0);
  assert.equal(frame.iframe.parentNode, null);
});

test('reloads a frame on request and opens only safe links', () => {
  const { host, slot, connect, from, window } = setup();
  host.attach(slot, BLOCK);
  connect();
  const [frame] = host.instances();
  from(frame, { type: 'ready' });
  from(frame, { type: 'reload' });
  assert.equal(frame.ready, false, 'it says ready again once it has loaded');
  from(frame, { type: 'navigate', url: 'https://example.com/a' });
  from(frame, { type: 'navigate', url: 'javascript:alert(1)' });
  from(frame, { type: 'navigate', url: '/cart', target: '_self' });
  assert.deepEqual(window.opened, [['https://example.com/a', '_blank', 'noopener'], ['/cart', '_self', 'noopener']]);
});

test('shows what extensions see of the state', () => {
  const state = { ...checkoutState(), diagnostics: ['x'] };
  const visible = stateForExtensions(state);
  assert.deepEqual(Object.keys(state).filter((key) => !(key in visible)).sort(), ['diagnostics', 'extensions', 'runs']);
});

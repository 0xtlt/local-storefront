// The page as a whole: the buyer's gestures become operations, the replies are drawn, and
// extensions take part. The server is the mock engine of the preview.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { startCheckout } from '../checkout.js';
import { op } from '../operations.js';
import { complete, update } from '../preview/mock-engine.mjs';
import { FakeDocument, FakeEvent } from './fake-dom.mjs';
import { engineCheckout, pageState, settle } from './harness.mjs';

const SANDBOX = 'http://localhost/__lsf/checkout/assets/sandbox.html';
const publicState = (state) => {
  const { selection, ...rest } = state;
  return rest;
};

/** A page on a checkout kept by the mock engine; `requests` is what it was asked. */
function app({ variant = 'standard', mutate, hold, conflict, href = 'http://localhost/checkouts/cn/preview' } = {}) {
  let engine = engineCheckout();
  engine.variant = variant;
  if (mutate) mutate(engine);
  const document = new FakeDocument();
  const root = document.createElement('div');
  document.body.appendChild(root);
  const requests = [];
  const location = { href, assigned: [], assign(url) { this.assigned.push(url); } };
  const window = { addEventListener() {}, removeEventListener() {}, open() {} };
  const reply = (status, body) => ({ ok: status < 400, status, json: async () => body });
  const fetch = async (url, init) => {
    const body = JSON.parse(init.body);
    requests.push({ url, body });
    await hold?.(url, body);
    if (url.endsWith('/update')) {
      // Someone else changed the checkout: nothing is applied and the reply is the checkout now.
      const other = conflict?.(engine, body.operations);
      if (other) {
        engine = other;
        return reply(409, publicState(engine));
      }
      const result = update(engine, body.operations);
      if (result.error) return reply(400, { error: result.error });
      engine = result.state;
      return reply(200, publicState(engine));
    }
    if (url.endsWith('/complete')) {
      const result = complete(engine);
      if (result.errors) {
        engine = result.errors;
        return reply(422, publicState(engine));
      }
      engine = result.state;
      return reply(200, { redirect: 'http://localhost/checkouts/cn/preview/thank-you' });
    }
    return reply(404, { error: 'no such route' });
  };
  const checkout = startCheckout({ root, initial: publicState(engine), document, window, location, fetch, storage: undefined, sandboxUrl: SANDBOX });
  const query = (selector) => root.querySelector(selector);
  return { checkout, document, window, root, requests, location, query, text: (selector) => query(selector)?.textContent, operations: () => requests.filter((r) => r.url.endsWith('/update')).flatMap((r) => r.body.operations), engine: () => engine };
}

const ADDRESS = { lastName: 'Martin', address1: '1 rue de Rivoli', city: 'Paris', countryCode: 'FR' };
async function fillIn(page, { delivery = true, payment = true } = {}) {
  const { actions } = page.checkout;
  await actions.commit('email', 'a@b.c');
  for (const [name, value] of Object.entries(ADDRESS)) actions.draft(name, value);
  await actions.commit('zip', '75001');
  if (delivery) await actions.chooseDelivery('shipping', 'standard');
  if (payment) await actions.choosePayment('card');
}

test('draws the checkout from the state in the page', () => {
  const page = app();
  assert.match(page.root.querySelector('.lsf-app').getAttribute('class'), /lsf-open/);
  assert.equal(page.document.body.getAttribute('data-variant'), 'standard');
  assert.equal(page.text('.lsf-pay'), 'Pay now');
  assert.equal(page.root.querySelectorAll('.lsf-line').length, 2);
});

test('the email goes to the server when the field is left, once', async () => {
  const page = app();
  await page.checkout.actions.commit('email', 'a@b.c');
  assert.deepEqual(page.operations(), [{ type: 'email', value: 'a@b.c' }]);
  assert.equal(page.checkout.getState().buyer.email, 'a@b.c');
  await page.checkout.actions.commit('email', 'a@b.c');
  assert.equal(page.operations().length, 1, 'nothing changed, nothing sent');
});

test('the address goes as a whole, an emptied field as null', async () => {
  const page = app();
  const { actions } = page.checkout;
  actions.draft('lastName', 'Martin');
  actions.draft('city', 'Paris');
  await actions.commit('countryCode', 'FR');
  assert.deepEqual(page.operations()[0], {
    type: 'shippingAddress',
    address: { firstName: null, lastName: 'Martin', company: null, address1: null, address2: null, city: 'Paris', countryCode: 'FR', provinceCode: null, zip: null, phone: null },
  });
  await actions.commit('city', '');
  assert.equal(page.operations().at(-1).address.city, null);
  await actions.commit('phone', '+33 1 23');
  assert.deepEqual(page.operations().slice(-2).map((operation) => operation.type), ['shippingAddress', 'phone']);
});

test('delivery methods show once the country is known, and can be chosen', async () => {
  const page = app();
  assert.ok(page.query('.lsf-placeholder'));
  await page.checkout.actions.commit('countryCode', 'FR');
  assert.equal(page.root.querySelectorAll('.lsf-radio-card').length >= 2, true);
  await page.checkout.actions.chooseDelivery('shipping', 'express');
  assert.deepEqual(page.operations().at(-1), { type: 'deliveryOption', group: 'shipping', handle: 'express' });
  assert.match(page.text('.lsf-total'), /€85\.00/);
  await page.checkout.actions.choosePayment('cod');
  assert.equal(page.text('.lsf-pay'), 'Complete order');
});

test('a discount code that applies is kept, one that does not is refused and removed', async () => {
  const page = app();
  const { actions, ui } = page.checkout;
  actions.draft('discount', 'save10');
  await actions.addDiscount();
  assert.deepEqual(page.operations(), [{ type: 'discountCode', action: 'add', code: 'save10' }]);
  assert.equal(ui.discountInput, '', 'the field is emptied');
  assert.equal(page.root.querySelectorAll('.lsf-code').length, 1);
  assert.match(page.text('.lsf-summary'), /−€7\.30/);

  actions.draft('discount', 'bogus');
  await actions.addDiscount();
  assert.deepEqual(page.operations().slice(-2), [{ type: 'discountCode', action: 'add', code: 'bogus' }, { type: 'discountCode', action: 'remove', code: 'BOGUS' }]);
  assert.equal(ui.discountError, 'Enter a valid discount code');
  assert.equal(page.root.querySelectorAll('.lsf-code').length, 1, 'it does not stay');
  assert.match(page.text('.lsf-discount-form'), /Enter a valid discount code/);

  await actions.removeDiscount('SAVE10');
  assert.equal(page.root.querySelectorAll('.lsf-code').length, 0);
});

test('a code applied with Enter leaves the field empty and Apply disabled, though the field keeps the focus', async () => {
  const page = app();
  const field = page.query('.lsf-discount-form .lsf-input');
  field.focus();
  field.value = 'save10';
  page.checkout.actions.draft('discount', 'save10');
  await page.checkout.actions.addDiscount();
  assert.equal(page.document.activeElement, field);
  assert.equal(field.value, '');
  assert.equal(page.query('.lsf-apply').hasAttribute('disabled'), true);

  field.value = 'bogus';
  page.checkout.actions.draft('discount', 'bogus');
  await page.checkout.actions.addDiscount();
  assert.equal(field.value, 'bogus', 'a refused code stays, to be corrected');
});

test('an empty discount field sends nothing', async () => {
  const page = app();
  await page.checkout.actions.addDiscount();
  assert.deepEqual(page.requests, []);
});

test('a failed operation shows its message', async () => {
  const page = app();
  await page.checkout.actions.chooseDelivery('shipping', 'drone');
  assert.match(page.text('[data-banner="failure"]'), /No delivery option drone/);
  await page.checkout.actions.choosePayment('card');
  assert.equal(page.query('[data-banner="failure"]'), null, 'it goes with the next gesture');
});

test('a refused operation that comes with the fresh checkout shows that checkout first', async () => {
  // Another tab sets the phone while this one chooses a payment method: the server refuses.
  const page = app({
    conflict: (engine, operations) => {
      if (!operations.some((operation) => operation.type === 'paymentMethod')) return null;
      return update(engine, [op.phone('+33102030405')]).state;
    },
  });
  const frames = connect(page);
  for (const frame of frames) say(page, frame, { type: 'ready' });
  assert.equal(page.checkout.getState().buyer?.phone ?? '', '');
  await page.checkout.actions.choosePayment('card');

  const now = page.checkout.getState();
  assert.equal(now.buyer.phone, '+33102030405', 'the page took the checkout the server sent');
  assert.equal(now.selectedPayment ?? null, null, 'what was refused is not in it');
  assert.match(page.text('[data-banner="failure"]'), /not applied/);
  assert.equal(page.requests.length, 1, 'nothing else was asked');
  for (const frame of frames) {
    const states = frame.iframe.contentWindow.posted.filter((message) => message.type === 'state');
    assert.equal(states.at(-1).state.buyer.phone, '+33102030405', frame.ext.handle);
  }
});

test('paying with something missing shows what, and does not ask the server', async () => {
  const page = app();
  await page.checkout.actions.pay();
  assert.equal(page.requests.length, 0);
  const errors = page.root.querySelectorAll('.lsf-field-error').map((node) => node.textContent);
  assert.ok(errors.includes('Enter an email'));
  assert.ok(errors.includes('Select a payment method'));
  assert.equal(page.checkout.ui.busy, false);
});

test('paying completes the order and goes to the thank-you page', async () => {
  const page = app();
  await fillIn(page);
  await page.checkout.actions.pay();
  assert.equal(page.requests.at(-1).url, '/checkouts/cn/preview/complete');
  assert.deepEqual(page.location.assigned, ['http://localhost/checkouts/cn/preview/thank-you']);
  assert.equal(page.engine().status, 'completed');
  assert.equal(page.checkout.ui.busy, false);
});

test('paying waits for the updates on their way, then pays for the state they made', async () => {
  let release;
  let holding = false;
  const held = new Promise((resolve) => { release = resolve; });
  const page = app({ hold: async (url) => { if (holding && url.endsWith('/update')) await held; } });
  await fillIn(page);
  holding = true;
  const sent = page.checkout.send([{ type: 'note', change: { type: 'updateNote', note: 'late' } }]);
  const paying = page.checkout.actions.pay();
  await settle();
  await settle();
  assert.equal(page.requests.some((request) => request.url.endsWith('/complete')), false, 'the update has not answered: nothing is paid');
  assert.equal(page.checkout.ui.busy, true);
  holding = false;
  release();
  await sent;
  await paying;
  const urls = page.requests.map((request) => request.url.split('/').pop());
  assert.deepEqual(urls.slice(-2), ['update', 'complete']);
  assert.equal(page.engine().note, 'late', 'the order carries the note the update made');
  assert.equal(page.location.assigned.length, 1);
});

test('paying sends what the buyer typed and has not left the field of, before the order', async () => {
  const page = app();
  await fillIn(page);
  page.checkout.actions.draft('email', 'typed@b.c');
  page.checkout.actions.draft('city', 'Lyon');
  await page.checkout.actions.pay();
  const urls = page.requests.map((request) => request.url.split('/').pop());
  assert.equal(urls.at(-1), 'complete');
  assert.equal(urls.at(-2), 'update');
  const last = page.requests.at(-2).body.operations;
  assert.deepEqual(last.map((operation) => operation.type), ['email', 'shippingAddress']);
  assert.equal(page.engine().buyer.email, 'typed@b.c');
  assert.equal(page.engine().shippingAddress.city, 'Lyon');
});

test('paying asks the interceptors about the state the updates made, not the one on screen', async () => {
  let release;
  let holding = false;
  const held = new Promise((resolve) => { release = resolve; });
  const page = app({ hold: async (url) => { if (holding && url.endsWith('/update')) await held; } });
  await fillIn(page);
  const gift = connect(page).find((frame) => frame.target === 'purchase.checkout.block.render');
  say(page, gift, { type: 'ready' });
  say(page, gift, { type: 'interceptors', count: 1 });
  const seen = [];
  gift.iframe.contentWindow.postMessage = (message) => {
    gift.iframe.contentWindow.posted.push(message);
    if (message.type === 'intercept') {
      seen.push(page.checkout.getState().note);
      queueMicrotask(() => say(page, gift, { type: 'intercepted', id: message.id, results: [{ behavior: 'allow' }] }));
    }
  };
  holding = true;
  const sent = page.checkout.send([{ type: 'note', change: { type: 'updateNote', note: 'fresh' } }]);
  const paying = page.checkout.actions.pay();
  await settle();
  assert.deepEqual(seen, [], 'not asked while an update is pending');
  holding = false;
  release();
  await sent;
  await paying;
  assert.deepEqual(seen, ['fresh']);
});

test('a checkout the server refuses stays open with its errors', async () => {
  const page = app();
  await fillIn(page);
  await page.checkout.send([{ type: 'note', change: { type: 'updateNote', note: 'this is forbidden' } }]);
  await page.checkout.actions.pay();
  assert.deepEqual(page.location.assigned, []);
  assert.match(page.text('[data-banner="errors"]'), /cannot contain the word “forbidden”/);
});

test('operations are sent one after the other, in order', async () => {
  const page = app();
  const first = page.checkout.send([{ type: 'email', value: 'one@x.y' }]);
  const second = page.checkout.send([{ type: 'email', value: 'two@x.y' }]);
  await Promise.all([first, second]);
  assert.deepEqual(page.operations().map((operation) => operation.value), ['one@x.y', 'two@x.y']);
  assert.equal(page.checkout.getState().buyer.email, 'two@x.y');
});

test('a draft gives way once the server agrees, and to the server when it changes the field', async () => {
  const page = app();
  const { actions, ui } = page.checkout;
  actions.draft('email', 'typed@x.y');
  assert.equal(ui.draft.email, 'typed@x.y');
  await actions.commit('email', 'typed@x.y');
  assert.equal('email' in ui.draft, false);
  await page.checkout.send([{ type: 'shippingAddress', address: { city: 'Lyon', countryCode: 'FR' } }]);
  assert.equal(page.query('[data-field="city"] input').value, 'Lyon');
});

test('the summary opens and closes', () => {
  const page = app();
  const { actions } = page.checkout;
  assert.equal(page.query('.lsf-summary').getAttribute('data-open'), 'false');
  actions.toggleSummary();
  assert.equal(page.query('.lsf-summary').getAttribute('data-open'), 'true');
  actions.toggleSummary();
  assert.equal(page.query('.lsf-summary').getAttribute('data-open'), 'false');
});

test('the badge opens the dialog of the Functions as a modal, and its close button closes it', () => {
  const page = app();
  const { ui } = page.checkout;
  const dialog = page.query('.lsf-fn-dialog');
  assert.equal(dialog.open, false);
  assert.equal(dialog.childNodes.length, 0, 'nothing in it while it is closed');
  page.query('.lsf-fn-badge').click();
  assert.equal(ui.functionsOpen, true);
  assert.equal(dialog.modal, true, 'opened with showModal()');
  assert.equal(page.query('.lsf-fn-run').getAttribute('data-run'), 'volume-discount');
  page.query('.lsf-fn-close').click();
  assert.equal(dialog.open, false);
  assert.equal(ui.functionsOpen, false);
  assert.equal(dialog.childNodes.length, 0);
  assert.equal(page.document.activeElement, page.query('.lsf-fn-badge'), 'the focus goes back to the badge');
});

test('the dialog closing by itself (Escape) resets the page, and the focus goes back to the badge', () => {
  const page = app();
  page.checkout.actions.toggleFunctions(true);
  const dialog = page.query('.lsf-fn-dialog');
  // What the browser does on Escape: the dialog closes and says so.
  dialog.close();
  assert.equal(page.checkout.ui.functionsOpen, false);
  assert.equal(page.query('.lsf-fn-frame'), null);
  assert.equal(page.document.activeElement, page.query('.lsf-fn-badge'));
  // It opens again, the same element.
  page.checkout.actions.toggleFunctions(true);
  assert.equal(page.query('.lsf-fn-dialog'), dialog);
  assert.equal(dialog.open, true);
});

test('a click on the backdrop closes the dialog, not one that started inside it', () => {
  const page = app();
  page.checkout.actions.toggleFunctions(true);
  const dialog = page.query('.lsf-fn-dialog');
  const frame = page.query('.lsf-fn-frame');
  // A selection of the JSON that ends on the backdrop: the click lands on the dialog.
  frame.dispatchEvent(new FakeEvent('pointerdown', { bubbles: true }));
  dialog.click();
  assert.equal(dialog.open, true);
  frame.click();
  assert.equal(dialog.open, true, 'a click inside does nothing');
  dialog.dispatchEvent(new FakeEvent('pointerdown', { bubbles: true }));
  dialog.click();
  assert.equal(dialog.open, false);
  assert.equal(page.checkout.ui.functionsOpen, false);
});

test('the run shown in the dialog stays across updates, and the dialog element with it', async () => {
  const page = app();
  const { actions } = page.checkout;
  actions.toggleFunctions(true);
  const dialog = page.query('.lsf-fn-dialog');
  page.query('[data-run="order-rules"]').click();
  assert.equal(page.query('.lsf-fn-run').getAttribute('data-run'), 'order-rules');
  await actions.commit('email', 'a@b.c');
  assert.equal(page.query('.lsf-fn-dialog'), dialog, 'an update does not replace the dialog');
  assert.equal(dialog.open, true);
  assert.equal(page.query('.lsf-fn-run').getAttribute('data-run'), 'order-rules');
  assert.equal(page.query('[aria-current="true"]').getAttribute('data-run'), 'order-rules');
  page.query('[data-view="overview"]').click();
  assert.ok(page.query('.lsf-fn-overview'));
  assert.equal(page.query('.lsf-fn-run'), null);
});

test('Copy puts the JSON on the clipboard and says so for a moment', async () => {
  const page = app();
  const written = [];
  page.window.navigator = { clipboard: { writeText: async (text) => written.push(text) } };
  page.checkout.actions.toggleFunctions(true);
  page.query('.lsf-fn-copy').click();
  await settle();
  assert.equal(written[0], JSON.stringify(page.checkout.getState().runs[0].input, null, 2));
  assert.equal(page.query('.lsf-fn-copy').textContent, 'Copied');
  // No clipboard (a page that is not secure): nothing happens.
  delete page.window.navigator;
  await page.checkout.actions.copyJson('x', '{}');
  assert.notEqual(page.checkout.ui.copied, 'x');
});

test('the Shop skin is drawn from the state', () => {
  const page = app({ variant: 'shop' });
  assert.equal(page.document.body.getAttribute('data-variant'), 'shop');
  assert.ok(page.query('.lsf-shop-logo'));
  assert.match(page.text('.lsf-pay'), /^Pay now · €73\.00$/);
});

// --- Extensions ----------------------------------------------------------------------------

/** Lets the frames of the page talk: they are given a window, and say they are ready. */
function connect(page) {
  const frames = page.checkout.host.instances();
  for (const instance of frames) {
    if (instance.iframe.contentWindow) continue;
    const posted = [];
    instance.iframe.contentWindow = { posted, postMessage: (message) => posted.push(message) };
  }
  return frames;
}
const say = (page, instance, message) => page.checkout.host.onMessage({ source: instance.iframe.contentWindow, data: { lsf: true, ...message } });

test('puts extension frames where their targets are, with the state', () => {
  const page = app();
  const frames = connect(page);
  const placed = frames.map((frame) => `${frame.ext.handle}@${frame.target}`);
  assert.ok(placed.includes('gift-message@purchase.checkout.block.render'));
  assert.equal(placed.filter((entry) => entry === 'gift-message@purchase.checkout.cart-line-item.render-after').length, 2, 'one for each line');
  assert.ok(placed.includes('order-notice@purchase.checkout.reductions.render-after'));
  assert.ok(placed.includes('order-notice@purchase.checkout.shipping-option-list.render-after'));
  assert.ok(!placed.some((entry) => entry.includes('thank-you')), 'the thank-you targets wait for the thank-you page');
  const lines = frames.filter((frame) => frame.target.endsWith('cart-line-item.render-after'));
  assert.deepEqual(lines.map((frame) => frame.ctx.lineId), ['gid://shopify/CartLine/1', 'gid://shopify/CartLine/2']);
  assert.equal(page.query('.lsf-slot[data-target="purchase.checkout.block.render"]').childNodes.length, 1);
});

test('does not load an extension on an API version it does not run, and says why', () => {
  const page = app({ mutate: (engine) => { engine.extensions[1].apiVersion = '2025-07'; } });
  const placed = connect(page).map((frame) => frame.ext.handle);
  assert.ok(placed.includes('gift-message'));
  assert.ok(!placed.includes('order-notice'));
  assert.equal(page.query('.lsf-slot[data-target="purchase.checkout.reductions.render-after"]'), null, 'no slot where only that extension targets');
  assert.match(page.text('.lsf-diagnostics'), /order-notice: not loaded\. API version 2025-07 is not supported/);
});

test('keeps its frames when the page is drawn again', async () => {
  const page = app();
  const before = connect(page).map((frame) => frame.iframe);
  await page.checkout.actions.commit('email', 'a@b.c');
  await page.checkout.actions.commit('countryCode', 'FR');
  const after = page.checkout.host.instances().map((frame) => frame.iframe);
  for (const iframe of before) assert.ok(after.includes(iframe) && iframe.parentNode, 'a frame is never rebuilt: that would reload it');
});

test('an extension a reloaded scenario no longer selects loses its frame and its say over the payment', async () => {
  const page = app();
  await fillIn(page);
  const frames = connect(page);
  const gift = frames.find((frame) => frame.ext.handle === 'gift-message' && frame.target === 'purchase.checkout.block.render');
  const slot = page.query('.lsf-slot[data-target="purchase.checkout.block.render"]');
  say(page, gift, { type: 'ready' });
  say(page, gift, { type: 'interceptors', count: 1 });
  gift.iframe.contentWindow.postMessage = (message) => {
    gift.iframe.contentWindow.posted.push(message);
    if (message.type === 'intercept') {
      queueMicrotask(() => say(page, gift, { type: 'intercepted', id: message.id, results: [{ behavior: 'block', reason: 'Blocked by the old selection.' }] }));
    }
  };

  // The scenario now selects another extension for the same target: the slot stays, the frame does not.
  const engine = page.engine();
  const gone = engine.extensions.find((ext) => ext.handle === 'gift-message');
  engine.extensions = engine.extensions
    .filter((ext) => ext !== gone)
    .concat({ ...gone, handle: 'replacement', script: '/__lsf/extensions/replacement/replacement.js', targets: ['purchase.checkout.block.render'] });
  await page.checkout.actions.commit('email', 'other@b.c');
  assert.deepEqual(
    [...slot.childNodes].map((frame) => frame.getAttribute('data-extension')),
    ['replacement'],
  );
  assert.equal(gift.iframe.parentNode, null);
  assert.ok(!page.checkout.host.instances().includes(gift));

  await page.checkout.actions.pay();
  assert.equal(page.query('[data-banner="blocked"]'), null, 'the extension that left cannot block the order');
  assert.equal(page.location.assigned.length, 1);
});

test('tells every frame about the new state', async () => {
  const page = app();
  const frames = connect(page);
  for (const frame of frames) say(page, frame, { type: 'ready' });
  await page.checkout.actions.commit('email', 'a@b.c');
  for (const frame of frames) {
    const states = frame.iframe.contentWindow.posted.filter((message) => message.type === 'state');
    assert.equal(states.at(-1).state.buyer.email, 'a@b.c', frame.ext.handle);
  }
});

test('what an extension changes reaches the checkout and every other extension', async () => {
  const page = app();
  const frames = connect(page);
  for (const frame of frames) say(page, frame, { type: 'ready' });
  const [gift] = frames;
  say(page, gift, { type: 'change', id: 1, method: 'applyNoteChange', change: { type: 'updateNote', note: 'from an extension' } });
  await settle();
  await settle();
  assert.equal(page.checkout.getState().note, 'from an extension');
  assert.deepEqual(gift.iframe.contentWindow.posted.find((message) => message.type === 'result').result, { type: 'success' });
  const notice = frames.find((frame) => frame.ext.handle === 'order-notice');
  assert.equal(notice.iframe.contentWindow.posted.filter((message) => message.type === 'state').at(-1).state.note, 'from an extension');
});

test('a field the buyer leaves does not undo an address change an extension has on its way', async () => {
  let release;
  let updates = 0;
  const gate = new Promise((resolve) => (release = resolve));
  const page = app({ hold: async (url) => { if (url.endsWith('/update') && ++updates === 2) await gate; } });
  const [gift] = connect(page);
  await page.checkout.send([{ type: 'shippingAddress', address: { lastName: 'Martin', address1: '1 rue', city: 'Paris', countryCode: 'FR', zip: '75001' } }]);
  // The extension's change is at the server when the buyer leaves the zip field.
  say(page, gift, { type: 'change', id: 1, method: 'applyShippingAddressChange', change: { type: 'updateShippingAddress', address: { city: 'Lyon' } } });
  await settle();
  const buyer = page.checkout.actions.commit('zip', '69001');
  release();
  await buyer;
  await settle();
  const address = page.checkout.getState().shippingAddress;
  assert.equal(address.city, 'Lyon', 'the city of the extension stays');
  assert.equal(address.zip, '69001', 'the zip of the buyer is sent');
  assert.equal(address.address1, '1 rue');
});

test('an extension that blocks the buyer stops the payment', async () => {
  const page = app();
  await fillIn(page);
  const frames = connect(page);
  const gift = frames.find((frame) => frame.target === 'purchase.checkout.block.render');
  say(page, gift, { type: 'ready' });
  say(page, gift, { type: 'interceptors', count: 1 });
  gift.iframe.contentWindow.postMessage = (message) => {
    gift.iframe.contentWindow.posted.push(message);
    if (message.type === 'intercept') {
      queueMicrotask(() => say(page, gift, { type: 'intercepted', id: message.id, results: [{ behavior: 'block', reason: 'The gift message is empty.', errors: [{ message: 'Write a message for the recipient.' }] }] }));
    }
  };
  await page.checkout.actions.pay();
  assert.equal(page.requests.some((request) => request.url.endsWith('/complete')), false);
  assert.deepEqual(page.location.assigned, []);
  assert.match(page.text('[data-banner="blocked"]'), /The gift message is empty\.Write a message for the recipient\./);
  assert.equal(page.checkout.ui.busy, false);

  gift.iframe.contentWindow.postMessage = (message) => {
    gift.iframe.contentWindow.posted.push(message);
    if (message.type === 'intercept') queueMicrotask(() => say(page, gift, { type: 'intercepted', id: message.id, results: [{ behavior: 'allow' }] }));
  };
  await page.checkout.actions.pay();
  assert.equal(page.location.assigned.length, 1, 'allowed, it pays');
  assert.equal(page.query('[data-banner="blocked"]'), null);
});

test('the thank-you page runs the thank-you extensions, not the checkout ones', () => {
  const page = app({
    mutate: (engine) => {
      engine.status = 'completed';
      engine.order = { id: 'gid://shopify/Order/1', number: '#1001' };
    },
  });
  const placed = connect(page).map((frame) => `${frame.ext.handle}@${frame.target}`);
  assert.deepEqual(placed, ['gift-message@purchase.thank-you.block.render']);
});

test('the state is read with the currency of the checkout', () => {
  const page = app();
  assert.deepEqual(page.checkout.getState().cost.total, { amount: 73, currencyCode: 'EUR' });
  assert.deepEqual(pageState(page.engine()).lines[0].total, { amount: 49, currencyCode: 'EUR' });
});

test('a buyer in French keeps the language in the requests, the links and the page', async () => {
  const page = app({
    mutate: (engine) => {
      engine.localization = { language: { isoCode: 'fr' }, rootUrl: '/fr' };
    },
  });
  assert.equal(page.document.documentElement.lang, 'fr');
  await page.checkout.actions.commit('email', 'a@b.c');
  assert.match(page.requests[0].url, /^\/fr\/checkouts\/cn\/preview\/update$/);
  const hrefs = [...page.root.querySelectorAll('a')].map((link) => link.getAttribute('href'));
  assert.ok(hrefs.some((href) => href.endsWith('/fr/cart')), hrefs.join(' '));
  assert.ok(hrefs.every((href) => !href.endsWith('.myshopify.com/cart')), hrefs.join(' '));

  // Without a language of the buyer, the page is in the one of the shop and under no root.
  const plain = app();
  assert.equal(plain.document.documentElement.lang, 'en');
  await plain.checkout.actions.commit('email', 'a@b.c');
  assert.equal(plain.requests[0].url, '/checkouts/cn/preview/update');
});

test('?targets=all shows the panel of the targets and the thank-you page, and nothing else does', () => {
  const plain = app();
  assert.equal(plain.query('[data-section="targets"]'), null);
  assert.equal(plain.query('[data-section="thank-you-preview"]'), null);

  const every = app({ href: 'http://localhost/checkouts/cn/preview?targets=all' });
  assert.ok(every.query('[data-section="targets"]'));
  assert.ok(every.query('[data-section="thank-you-preview"]'));
  // The sample extensions declare targets of the checkout page: each is listed.
  assert.match(every.text('[data-section="targets"]'), /purchase\.checkout\.block\.render/);
  assert.match(every.text('[data-section="targets"]'), /shown|needs/);
});

test('a checkout with nothing to ship has no address and no shipping method, unless every target is shown', () => {
  const digital = (href) => app({ href, mutate: (engine) => engine.lines.forEach((line) => { line.requiresShipping = false; }) });
  const plain = digital('http://localhost/checkouts/cn/preview');
  assert.equal(plain.query('[data-section="delivery"]'), null);
  assert.equal(plain.query('[data-section="shipping"]'), null);
  assert.ok(plain.query('[data-section="contact"]'));

  const every = digital('http://localhost/checkouts/cn/preview?targets=all');
  assert.ok(every.query('[data-section="delivery"]'), 'every target needs the steps of a cart that ships');
  assert.ok(every.query('[data-section="shipping"]'));
});

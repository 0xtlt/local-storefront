import assert from 'node:assert/strict';
import { test } from 'node:test';

import { page, settle } from './harness.mjs';

const LINES = 'shopify:cart:lines-update';
const load = (options) => page({ scripts: ['standard-actions.js'], ...options });

test('Shopify.actions exposes the three actions and cannot be replaced', () => {
  const { Shopify } = load();
  assert.deepEqual(Object.keys(Shopify.actions), ['getCart', 'updateCart', 'openCart']);
  assert.throws(() => {
    'use strict';
    Shopify.actions.getCart = null;
  });
  // getCart is not configurable.
  assert.equal(Shopify.actions.getCart.configure, undefined);
  assert.equal(Shopify.actions.updateCart.isDefault(), true);
});

test('getCart is null until a cart exists, then reports the cart', async () => {
  const empty = load();
  assert.deepEqual(await empty.Shopify.actions.getCart(), { cart: null });
  assert.equal(empty.server.state.requests.length, 0);

  const { Shopify, server } = load({ cookie: 'cart=local-token' });
  server.state.lines.push({ key: '101:key', id: 101, quantity: 2 });
  const [first, second] = await Promise.all([Shopify.actions.getCart(), Shopify.actions.getCart()]);
  assert.deepEqual(first, {
    cart: {
      id: 'gid://shopify/Cart/local-token',
      totalQuantity: 2,
      cost: { totalAmount: { amount: '59.98', currencyCode: 'CAD' } },
      lines: {
        nodes: [
          {
            id: 'gid://shopify/CartLine/101:key?cart=local-token',
            quantity: 2,
            cost: { totalAmount: { amount: '59.98', currencyCode: 'CAD' } },
          },
        ],
      },
      discountCodes: [],
    },
  });
  assert.deepEqual(second, first);
  // Concurrent reads share one request.
  assert.equal(server.state.requests.length, 1);
});

test('updateCart adds a line, emits the event as it starts and resolves with the cart', async () => {
  const { Shopify, server, record, document, window } = load();
  const events = record(LINES);
  let requestsWhenEmitted;
  document.addEventListener(LINES, () => (requestsWhenEmitted = server.state.requests.length));
  const call = Shopify.actions.updateCart({ lines: [{ merchandiseId: 101, quantity: 1 }] });

  // The event is out as the call starts, before anything is written.
  assert.equal(events.length, 1);
  assert.equal(requestsWhenEmitted, 0);
  const [event] = events;
  assert.equal(event.action, 'add');
  assert.equal(event.context, 'standard-action');
  assert.equal(event.bubbles, true);
  assert.deepEqual(event.lines, [{ merchandiseId: 'gid://shopify/ProductVariant/101', quantity: 1 }]);

  const result = await call;
  assert.equal(result.cart.totalQuantity, 1);
  assert.equal(result.userErrors, undefined);
  assert.equal(result.cart.lines.nodes.length, 1);
  // The event reports the same result, with the lines as a plain array.
  const reported = await event.promise;
  assert.deepEqual(reported.cart.lines, result.cart.lines.nodes);
  assert.deepEqual({ ...reported.cart, lines: undefined }, { ...result.cart, lines: undefined });
  assert.deepEqual(server.state.requests[0].body, { id: 101, quantity: 1 });

  // Nothing configured and no cart the default recognises: the page reloads, once.
  await settle();
  assert.equal(window.location.reloads, 1);
});

test('updateCart updates and removes lines, sets the note and replaces the attributes', async () => {
  const { Shopify, server, record } = load({ cookie: 'cart=local-token' });
  server.state.lines.push({ key: '101:key', id: 101, quantity: 1 }, { key: '202:key', id: 202, quantity: 1 });
  server.state.attributes = { gift: 'yes', wrap: 'red' };
  const lines = record(LINES);
  const notes = record('shopify:cart:note-update');
  const attributes = record('shopify:cart:attributes-update');
  const discounts = record('shopify:cart:discount-update');

  // Lines are named by the id the cart reports, or by their key in the Ajax cart.
  const [firstLine] = (await Shopify.actions.getCart()).cart.lines.nodes;
  assert.equal(firstLine.id, 'gid://shopify/CartLine/101:key?cart=local-token');
  const result = await Shopify.actions.updateCart(
    {
      lines: [
        { id: firstLine.id, quantity: 3 },
        { id: '202:key', quantity: 0 },
      ],
      note: 'Ring the bell',
      attributes: [{ key: 'gift', value: 'no' }],
      discountCodes: ['SUMMER'],
    },
    { event: { context: 'cart', detail: { source: 'test' } } },
  );

  assert.deepEqual(
    lines.map((event) => [event.action, event.context, event.lines, event.detail]),
    [
      ['update', 'cart', [{ id: firstLine.id, quantity: 3 }], { source: 'test' }],
      ['remove', 'cart', [{ id: '202:key', quantity: 0 }], { source: 'test' }],
    ],
  );
  assert.equal(notes[0].note, 'Ring the bell');
  assert.deepEqual(attributes[0].attributes, [{ key: 'gift', value: 'no' }]);
  assert.deepEqual(discounts[0].discountCodes, [{ code: 'SUMMER' }]);
  assert.equal(discounts[0].context, undefined);

  assert.deepEqual(server.state.lines, [{ key: '101:key', id: 101, quantity: 3 }]);
  assert.equal(server.state.note, 'Ring the bell');
  // The payload is the complete set of attributes: `wrap` is gone.
  assert.deepEqual(server.state.attributes, { gift: 'no' });
  assert.equal(result.cart.totalQuantity, 3);
  // Discounts are not simulated.
  assert.deepEqual(result.cart.discountCodes, [{ applicable: false, code: 'SUMMER' }]);
});

test('a change the cart refuses resolves with userErrors and no error event', async () => {
  const { Shopify, record } = load();
  const errors = record('shopify:cart:error');
  const events = record(LINES);
  const result = await Shopify.actions.updateCart({
    lines: [
      { merchandiseId: 'gid://shopify/ProductVariant/101', quantity: 1 },
      { merchandiseId: 202, quantity: 5 },
      { merchandiseId: 999, quantity: 1 },
    ],
  });
  assert.deepEqual(result.userErrors, [
    { code: 'INVALID', field: ['lines', '1', 'quantity'], message: 'All 1 are in your cart.' },
    { code: 'INVALID', field: ['lines', '2', 'merchandiseId'], message: 'Cannot find variant' },
  ]);
  assert.equal(result.cart.totalQuantity, 1);
  assert.equal(errors.length, 0);
  assert.deepEqual((await events[0].promise).userErrors, result.userErrors);
});

test('a request that fails rejects and emits shopify:cart:error', async () => {
  const { Shopify, server, record } = load();
  const errors = record('shopify:cart:error');
  const events = record(LINES);
  server.state.failing = true;
  await assert.rejects(Shopify.actions.updateCart({ lines: [{ merchandiseId: 101, quantity: 1 }] }), {
    message: 'Failed to fetch',
  });
  assert.equal(errors.length, 1);
  assert.equal(errors[0].code, 'SERVICE_UNAVAILABLE');
  assert.equal(errors[0].error, 'Failed to fetch');
  await assert.rejects(events[0].promise);
});

test('an invalid payload and an aborted call reject without an error event', async () => {
  const { Shopify, record } = load();
  const errors = record('shopify:cart:error');
  const events = record(LINES);
  await assert.rejects(Shopify.actions.updateCart({}), { name: 'ValidationError' });
  await assert.rejects(Shopify.actions.updateCart({ lines: [{ quantity: 1 }] }), {
    name: 'ValidationError',
  });
  assert.equal(events.length, 0);

  const controller = new AbortController();
  controller.abort();
  await assert.rejects(
    Shopify.actions.updateCart({ lines: [{ merchandiseId: 101, quantity: 1 }] }, { signal: controller.signal }),
    { name: 'AbortError' },
  );
  assert.equal(errors.length, 0);
});

test('only the first configuration takes effect, and it replaces the reload', async () => {
  const { Shopify, document, window } = load();
  const target = new EventTarget();
  const seen = [];
  target.addEventListener(LINES, (event) => seen.push(event));
  const onDocument = [];
  document.addEventListener(LINES, (event) => onDocument.push(event));
  const calls = [];

  assert.equal(
    Shopify.actions.updateCart.configure({
      eventTarget: (meta) => {
        calls.push(meta);
        return target;
      },
      async handler(defaultHandler, payload, options) {
        const result = await defaultHandler();
        return { ...result, detail: { handledBy: 'theme', sawOptions: options.marker, lines: payload.lines.length } };
      },
    }),
    true,
  );
  assert.equal(Shopify.actions.updateCart.configure({ handler: () => ({}) }), false);
  assert.equal(Shopify.actions.updateCart.isDefault(), false);

  const result = await Shopify.actions.updateCart(
    { lines: [{ merchandiseId: 101, quantity: 2 }] },
    { marker: 'kept' },
  );
  assert.deepEqual(calls, [{ type: LINES, action: 'add' }]);
  assert.equal(seen.length, 1);
  // The target is not in the document: nothing bubbles there.
  assert.equal(onDocument.length, 0);
  assert.deepEqual(result.detail, { handledBy: 'theme', sawOptions: 'kept', lines: 1 });
  assert.equal(result.cart.totalQuantity, 2);
  assert.deepEqual((await seen[0].promise).detail, result.detail);

  await settle();
  assert.equal(window.location.reloads, 0);
});

test('the default refreshes a Horizon-style cart from an event instead of reloading', async () => {
  const { Shopify, record, window } = load({
    elements: {
      'cart-icon': { renderCartBubble() {} },
      'cart-items-component': { updateQuantity() {} },
    },
  });
  const updates = record('cart:update');
  await Shopify.actions.updateCart({ lines: [{ merchandiseId: 101, quantity: 2 }] });
  await settle();
  assert.equal(updates.length, 1);
  assert.deepEqual(updates[0].detail, {
    resource: null,
    sourceId: 'external',
    data: { source: 'external', itemCount: 2 },
  });
  assert.equal(window.location.reloads, 0);
});

test('openCart opens a drawer it recognises, goes to the cart page otherwise, or does what it is told', async () => {
  const plain = load();
  assert.equal(await plain.Shopify.actions.openCart(), undefined);
  assert.equal(plain.window.location.href, '/cart');

  let opened = 0;
  const drawer = load({ elements: { 'cart-drawer': { open: () => (opened += 1) } } });
  await drawer.Shopify.actions.openCart();
  assert.equal(opened, 1);
  assert.equal(drawer.window.location.href, 'http://shop.test/');

  const configured = load();
  let handled = 0;
  assert.equal(configured.Shopify.actions.openCart.configure({ handler: () => void (handled += 1) }), true);
  await configured.Shopify.actions.openCart();
  assert.equal(handled, 1);
  assert.equal(configured.window.location.href, 'http://shop.test/');
});

test('requests follow the locale root of the storefront', async () => {
  const { Shopify, server } = load({ cookie: 'cart=local-token', shopify: { routes: { root: '/fr/' } } });
  await Shopify.actions.getCart();
  assert.equal(server.state.requests[0].url, '/fr/cart.js');
});

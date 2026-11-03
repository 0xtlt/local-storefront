// The customer account: its pages, where each target of customer account UI extensions sits, and
// where `shopify.navigation.navigate` leads.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ACCOUNT_TARGETS } from '../extension-host.js';
import { orderStatus, renderAccount } from '../account-views.js';
import { resolveNavigation, startAccount } from '../customer-account.js';
import { withCurrency } from '../format.js';
import { h } from '../vdom.js';
import { FakeDocument } from './fake-dom.mjs';
import { checkoutState } from './harness.mjs';

const walk = (node, visit) => {
  visit(node);
  for (const child of node.children) walk(child, visit);
};
const findAll = (tree, predicate) => {
  const found = [];
  walk(tree, (node) => predicate(node) && found.push(node));
  return found;
};
const text = (node) => (node.tag === '#text' ? node.text : node.children.map(text).join(''));
const section = (tree, name) => findAll(tree, (node) => node.props['data-section'] === name);

const LINES = checkoutState().lines;
const ORDER = {
  id: 'gid://shopify/Order/42',
  legacyId: 42,
  name: '#1042',
  confirmationNumber: 'ABC123XYZ',
  processedAt: '2024-03-04T00:00:00+00:00',
  financialStatus: 'paid',
  fulfillmentStatus: 'partial',
  lines: LINES,
  cost: { currencyCode: 'USD', subtotal: { amount: 73 }, shipping: { amount: 5 }, tax: { amount: 0 }, total: { amount: 78 } },
  shippingAddress: { firstName: 'Jane', lastName: 'Doe', address1: '12 Maple Street', city: 'Portland', countryCode: 'US' },
  fulfillments: [{ id: 'gid://shopify/Fulfillment/421', status: 'SUCCESS', lineIds: [LINES[0].id] }],
  unfulfilledLineIds: [LINES[1].id],
  returns: [{ id: 'gid://shopify/Return/421', status: 'OPEN', lines: [{ lineId: LINES[0].id, quantity: 1 }] }],
};

function accountState(overrides = {}) {
  return withCurrency({
    surface: 'customer-account',
    page: 'orders',
    root: '/account',
    shop: checkoutState().shop,
    localization: checkoutState().localization,
    customer: {
      id: 'gid://shopify/Customer/7', email: 'jane@example.com', firstName: 'Jane', lastName: 'Doe',
      addresses: [{ id: 'a1', default: true, address: ORDER.shippingAddress }],
    },
    buyer: { customer: { id: 'gid://shopify/Customer/7' } },
    orders: [ORDER],
    order: null,
    company: null,
    profiles: [{ email: 'jane@example.com', name: 'Jane Doe' }, { email: 'alex@example.com', name: 'Alex Morgan' }],
    extensions: [],
    ...overrides,
  });
}

/** The page, with a slot (a node) at every target, and what each slot was asked for. */
function render(state, { ui = {}, preview = null } = {}) {
  const asked = [];
  const slot = (target, instance) => {
    asked.push([target, instance]);
    return h('div', { class: 'lsf-slot', 'data-target': target });
  };
  const actions = new Proxy({}, { get: () => () => {} });
  const tree = renderAccount({ state, ui: { toasts: [], ...ui }, slot, actions, here: '/account', preview });
  return { tree, asked, targets: asked.map(([target]) => target) };
}

test('the orders: a card for each, with its actions, between the announcement, the block and the footer', () => {
  const { tree, asked, targets } = render(accountState());
  assert.deepEqual(targets, [
    'customer-account.order-index.announcement.render',
    'customer-account.order-index.block.render',
    'customer-account.order.action.menu-item.render',
    'customer-account.footer.render-after',
  ]);
  assert.equal(asked[2][1].orderId, ORDER.id, 'the menu item knows its order');
  const card = findAll(tree, (node) => node.props['data-order'] === 42)[0];
  assert.match(text(card), /Return in progress.*Order #1042.*3 items/);
  assert.equal(findAll(card, (node) => node.tag === 'a')[0].props.href, '/account/orders/42');
});

test('the status of an order: its shipments, what is not shipped, its returns, and every target of the page', () => {
  const { tree, asked, targets } = render(accountState({ page: 'order', order: ORDER }));
  for (const target of ACCOUNT_TARGETS.filter((name) => name.includes('.order-status.'))) {
    assert.ok(targets.includes(target), `${target} is on the page`);
  }
  const of = (target, key) => asked.filter(([name]) => name === target).map(([, instance]) => instance[key]);
  assert.deepEqual(of('customer-account.order-status.fulfillment-details.render-after', 'fulfillmentId'), ['gid://shopify/Fulfillment/421']);
  assert.deepEqual(of('customer-account.order-status.return-details.render-after', 'returnId'), ['gid://shopify/Return/421']);
  assert.deepEqual(of('customer-account.order-status.cart-line-item.render-after', 'lineId'), LINES.map((line) => line.id));
  assert.match(text(section(tree, 'fulfillment')[0]), /Shipment 1/);
  assert.match(text(section(tree, 'unfulfilled')[0]), /Not shipped yet/);
  assert.match(text(section(tree, 'return')[0]), /Return 1.*open/);
  assert.match(text(tree), /Confirmation ABC123XYZ/);
});

test('the profile: the targets of the customer, and those of each location of a B2B customer', () => {
  const company = {
    id: 'gid://shopify/Company/1', name: 'Northwind Hotels', externalId: 'NW-001',
    locations: [
      { id: 'gid://shopify/CompanyLocation/1', name: 'Portland', current: true, address: ORDER.shippingAddress, staff: [{ name: 'Alex Morgan', email: 'alex@example.com', title: 'Buyer' }] },
      { id: 'gid://shopify/CompanyLocation/2', name: 'Seattle', staff: [] },
    ],
  };
  const { tree, asked, targets } = render(accountState({ page: 'profile', company }));
  for (const target of ACCOUNT_TARGETS.filter((name) => name.includes('.profile.'))) {
    assert.ok(targets.includes(target), `${target} is on the page`);
  }
  const locations = asked.filter(([name]) => name === 'customer-account.profile.company-location-staff.render-after').map(([, instance]) => instance.locationId);
  assert.deepEqual(locations, company.locations.map((location) => location.id));
  assert.match(text(section(tree, 'location')[0]), /Portland.*Buying for.*Alex Morgan · Buyer/);
  // Not a B2B customer: no company.
  assert.ok(!render(accountState({ page: 'profile' })).targets.includes('customer-account.profile.company-details.render-after'));
});

test('a full page is the page of its extension, for an order or not', () => {
  const page = render(accountState({ page: 'page', extensionPage: { handle: 'loyalty', path: '/rewards' } }));
  assert.deepEqual(page.asked[0], ['customer-account.page.render', { key: 'page:loyalty:/rewards', handle: 'loyalty', path: '/rewards', orderId: undefined, order: null }]);
  const ordered = render(accountState({ page: 'order-page', order: ORDER, extensionPage: { handle: 'returns', path: '/' } }));
  assert.equal(ordered.asked[0][0], 'customer-account.order.page.render');
  assert.equal(ordered.asked[0][1].order, 42);
  assert.equal(ordered.asked[0][1].orderId, ORDER.id);
});

test('logged out, the page lists the customers to log in as, and renders no extension', () => {
  const { tree, targets } = render(accountState({ customer: null, buyer: { customer: null } }));
  assert.deepEqual(targets, []);
  const links = findAll(tree, (node) => node.tag === 'a' && node.props['data-customer']).map((node) => node.props.href);
  assert.deepEqual(links, [
    '/__lsf/login?customer=jane%40example.com&return_to=%2Faccount',
    '/__lsf/login?customer=alex%40example.com&return_to=%2Faccount',
  ]);
});

test('the dialog of an order action holds the action of the extension whose menu item was pressed', () => {
  const { asked } = render(accountState(), { ui: { action: { handle: 'ask', orderId: ORDER.id } } });
  assert.deepEqual(asked.at(-1), ['customer-account.order.action.render', { key: `action:ask:${ORDER.id}`, handle: 'ask', orderId: ORDER.id }]);
});

test('what the customer sees of an order', () => {
  assert.equal(orderStatus({ cancelledAt: '2024-01-01' }).label, 'Cancelled');
  assert.equal(orderStatus({ returns: [{}] }).label, 'Return in progress');
  assert.equal(orderStatus({ fulfillmentStatus: 'fulfilled' }).label, 'Delivered');
  assert.equal(orderStatus({ fulfillmentStatus: 'unfulfilled' }).label, 'Confirmed');
});

test('where navigation leads', () => {
  const at = (url, from = {}) => resolveNavigation(url, { root: '/fr/account', from, handles: ['loyalty', 'returns'] });
  assert.equal(at('shopify:customer-account/orders'), '/fr/account');
  assert.equal(at('shopify:customer-account/profile'), '/fr/account/profile');
  assert.equal(at('shopify:customer-account/orders/42'), '/fr/account/orders/42');
  assert.equal(at('shopify:customer-account/orders/gid://shopify/Order/42'), '/fr/account/orders/42');
  assert.equal(at('extension://loyalty'), '/fr/account/pages/loyalty', 'another extension');
  assert.equal(at('extension:returns/start'), '/fr/account/pages/returns/start');
  assert.equal(at('extension://rewards', { handle: 'loyalty' }), '/fr/account/pages/loyalty/rewards', 'a route of its own');
  assert.equal(at('/rewards/3', { handle: 'loyalty' }), '/fr/account/pages/loyalty/rewards/3');
  assert.equal(at('extension://rewards', { handle: 'returns', order: 42 }), '/fr/account/orders/42/pages/returns/rewards');
  assert.equal(at('extension://rewards'), null, 'not on a full page');
  assert.equal(at('shopify:admin/orders'), null);
  assert.equal(at('https://example.com/'), null);
});

test('the page follows what the extensions ask: a dialog, a toast, a navigation', () => {
  const document = new FakeDocument();
  const root = document.createElement('div');
  document.body.appendChild(root);
  const assigned = [];
  const listeners = [];
  const window = { addEventListener: (type, listener) => listeners.push(listener), removeEventListener: () => {}, fetch: async () => ({ ok: true, text: async () => '' }) };
  const extensions = [{ handle: 'ask', apiVersion: '2026-10', script: '/a.js', targets: ['customer-account.order.action.menu-item.render', 'customer-account.order.action.render'] }];
  const page = startAccount({
    root, initial: accountState({ extensions }), document, window,
    location: { href: 'http://localhost/account', assign: (href) => assigned.push(href) },
    sandboxUrl: 'http://localhost/sandbox.html',
    console: { warn: () => {}, error: () => {}, debug: () => {} },
  });
  const [menu] = page.host.instances();
  assert.equal(menu.target, 'customer-account.order.action.menu-item.render');
  menu.iframe.contentWindow = { postMessage: () => {} };
  const from = (instance, data) => page.host.onMessage({ source: instance.iframe.contentWindow, data: { lsf: true, ...data } });
  from(menu, { type: 'action' });
  assert.deepEqual(page.ui.action, { handle: 'ask', orderId: ORDER.id });
  const dialog = page.host.instances().find((instance) => instance.target === 'customer-account.order.action.render');
  assert.ok(dialog, 'the dialog holds the action');
  dialog.iframe.contentWindow = { postMessage: () => {} };
  from(dialog, { type: 'toast', content: 'Sent' });
  assert.deepEqual(page.ui.toasts.map((toast) => toast.content), ['Sent']);
  from(dialog, { type: 'close' });
  assert.equal(page.ui.action, null);
  from(menu, { type: 'navigate', url: 'shopify:customer-account/profile' });
  assert.deepEqual(assigned, ['/account/profile']);
});

test('every target of customer accounts has a place in the views', () => {
  assert.equal(ACCOUNT_TARGETS.length, 24);
});

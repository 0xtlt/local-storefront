// lsf's admin around an embedded app (`admin/admin.js`): what it answers the app's frame, what it
// draws, and what it says goes wrong.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { FakeEvent } from './fake-dom.mjs';
import { APP, adminConfig, adminPage, plain, settle, tick } from './admin-harness.mjs';
import { adminHref, appPathOf, searchQuery } from '../admin/admin.js';

const json = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });

/** An admin page whose frame said hello. */
async function connected(options) {
  const page = await adminPage(options);
  page.bridge({ type: 'hello', version: 'lsf-1', apiKey: 'client-1', scriptOrigin: 'http://127.0.0.1:9292' });
  return page;
}

test('builds the search of the resource picker in Shopify\'s syntax', () => {
  assert.equal(searchQuery({ type: 'product' }, ''), '');
  assert.equal(searchQuery({ type: 'product' }, ' mug '), '(mug)');
  assert.equal(
    searchQuery({ type: 'product', filter: { query: 'vendor:Acme', draft: false, archived: false, hidden: false } }, 'mug'),
    '(vendor:Acme) AND (mug) AND -status:draft AND -status:archived AND published_status:published',
  );
  assert.equal(searchQuery({ type: 'collection', filter: { draft: false } }, ''), '', 'draft and archived are of products');
});

test('writes the admin\'s URL of the app\'s path, without the launch\'s parameters', () => {
  assert.equal(appPathOf('/app?embedded=1&hmac=x&host=y&id_token=z&locale=fr&session=s&shop=a&timestamp=1&tab=2'), '/app?tab=2');
  assert.equal(appPathOf('/myapp/settings#top', 'https://example.com/myapp'), '/settings#top', 'the path of the app\'s URL is the app\'s root');
  assert.equal(appPathOf('/myapp', 'https://example.com/myapp/'), '/');
  assert.equal(adminHref('/admin/apps/x', '/app/settings?tab=2', { shop: 'other.myshopify.com' }), '/admin/apps/x/app/settings?tab=2&shop=other.myshopify.com');
  assert.equal(adminHref('/admin/apps/x', '/'), '/admin/apps/x');
});

test('welcomes lsf\'s App Bridge from the app\'s origin, with the shop, the user and the scopes', async () => {
  const page = await connected();
  const [welcome] = page.sent('welcome');
  assert.deepEqual(plain(welcome), {
    lsf: 'admin', v: 1, type: 'welcome', shop: 'local-supply-co.myshopify.com', locale: 'en', clientId: 'client-1',
    user: { id: 1, name: 'Shop Owner', email: 'owner@local-supply-co.myshopify.com', accountAccess: 'Account owner', locale: 'en' },
    scopes: { granted: ['read_products'], required: ['read_products'], optional: [] },
    origin: 'http://127.0.0.1:9292',
  });
  assert.ok(page.toFrame.every(({ origin }) => origin === APP), 'posted to the app\'s origin only');

  // Another origin in the frame (a page the app went to), or another window: not answered.
  const other = await adminPage();
  other.send({ lsf: 'app-bridge', type: 'hello' }, { origin: 'http://evil.example' });
  other.send({ lsf: 'app-bridge', type: 'hello' }, { source: {} });
  assert.equal(other.sent('welcome').length, 0);
});

test('says when the app runs Shopify\'s App Bridge, which cannot talk to it', async () => {
  const page = await adminPage();
  page.send({ type: 'dispatch', payload: { type: 'APP::CLIENT::INITIALIZE', clientInterface: { name: 'app-bridge-cdn' } }, source: { apiKey: 'client-1', shop: 'x' } });
  const banner = page.query('[data-banner="app-bridge"]');
  assert.match(banner.textContent, /This page runs Shopify's App Bridge \(cdn\.shopify\.com\/shopifycloud\/app-bridge\.js\), which talks to Shopify's admin only/);
  assert.match(banner.textContent, /<script src="http:\/\/127\.0\.0\.1:9292\/__lsf\/admin\/app-bridge\.js"><\/script>/);
  assert.match(banner.getAttribute('class'), /lsf-banner-critical/);
  await tick();
  await tick();
  assert.equal(page.all('[data-banner]').length, 1, 'the wait for an App Bridge does not add a second banner');
});

test('says when no App Bridge answers, and takes it back when one does', async () => {
  const page = await adminPage();
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.match(page.query('[data-banner="app-bridge"]').textContent, /No App Bridge answered from the app's page\. Is the backend running at http:\/\/localhost:3000/);
  page.bridge({ type: 'hello', scriptOrigin: 'http://127.0.0.1:9292' });
  assert.equal(page.query('[data-banner="app-bridge"]'), null);
});

test('says when the app loaded lsf\'s App Bridge from another host name than the admin\'s', async () => {
  const page = await adminPage();
  page.bridge({ type: 'hello', scriptOrigin: 'http://localhost:9292' });
  assert.match(page.query('[data-banner="origin"]').textContent, /from http:\/\/localhost:9292, but this admin is at http:\/\/127\.0\.0\.1:9292: they cannot talk/);
  assert.equal(page.sent('welcome').length, 0);
});

test('mints the ID tokens of the frame with lsf', async () => {
  const page = await connected({
    fetch: async (url, init) => (url === '/__lsf/admin/session-token' ? json({ token: `token-for-${JSON.parse(init.body).user}` }) : json({}, 404)),
  });
  page.bridge({ type: 'request', id: 7, method: 'idToken', params: null });
  await settle();
  await settle();
  assert.deepEqual(plain(page.sent('response')[0]), { lsf: 'admin', v: 1, type: 'response', id: 7, ok: true, value: 'token-for-1' });
  assert.deepEqual(JSON.parse(page.requests[0].init.body), { shop: 'local-supply-co.myshopify.com', user: 1 });

  page.bridge({ type: 'request', id: 8, method: 'teleport', params: null });
  await settle();
  assert.match(page.sent('response')[1].error, /lsf's admin does not answer teleport/);
});

test('draws the title bar the app asks for, and sends its clicks back', async () => {
  const page = await connected();
  page.bridge({
    type: 'title',
    title: 'Edit product',
    breadcrumb: { index: 0, label: 'Products' },
    primary: { index: 1, label: 'Save', disabled: false },
    secondary: [{ index: 2, label: 'Preview' }, { group: true, label: 'More', actions: [{ index: 3, label: 'Duplicate' }] }],
    accessory: { label: 'Draft', tone: 'warning' },
  });
  assert.equal(page.query('.lsf-titlebar-title').textContent, 'Edit product');
  assert.equal(page.document.title, 'Edit product · Local Supply Co.');
  assert.equal(page.query('.lsf-titlebar-accessory .lsf-badge-warning').textContent, 'Draft');
  const buttons = page.all('.lsf-titlebar-actions button');
  assert.deepEqual(buttons.map((button) => button.textContent), ['Preview', 'Duplicate', 'Save'], 'the group is a menu of its own');
  assert.equal(page.query('.lsf-menu-group summary').textContent, 'More');
  assert.match(buttons[2].getAttribute('class'), /lsf-button-primary/);
  buttons[1].click();
  page.query('.lsf-titlebar-back').click();
  assert.deepEqual(page.sent('event').map((event) => plain(event.detail)), [{ index: 3 }, { index: 0 }]);

  page.bridge({ type: 'title', title: '', breadcrumb: null, primary: null, secondary: [], accessory: null });
  assert.equal(page.query('.lsf-titlebar-title').textContent, 'Admin backend', 'no title: the app\'s name');
});

test('lists the app\'s navigation, follows the app\'s path in its URL, and marks where it is', async () => {
  const page = await connected({ config: adminConfig({ query: { shop: 'other.myshopify.com' } }) });
  page.bridge({ type: 'nav', items: [{ label: 'Templates', path: '/app/templates' }, { label: 'Settings', path: '/app/settings' }], home: '/app' });
  const links = page.all('.lsf-nav-item');
  assert.deepEqual(links.map((link) => link.getAttribute('href')), [
    '/admin/apps/admin-backend/app/templates?shop=other.myshopify.com',
    '/admin/apps/admin-backend/app/settings?shop=other.myshopify.com',
  ]);
  page.bridge({ type: 'history', path: '/app/settings/emails?embedded=1&id_token=x&tab=2', mode: 'push' });
  assert.equal(page.replaced.at(-1), '/admin/apps/admin-backend/app/settings/emails?tab=2&shop=other.myshopify.com', 'replaced, never pushed: the frame has the history');
  assert.equal(page.all('.lsf-nav-item[aria-current]').map((link) => link.textContent).join(), 'Settings');
  assert.equal(page.query('.lsf-nav-app').hasAttribute('aria-current'), false);

  links[0].click();
  page.query('.lsf-nav-app').click();
  assert.deepEqual(page.sent('event').map((event) => [event.name, plain(event.detail)]), [
    ['navigate', { path: '/app/templates' }],
    ['navigate', { home: true }],
  ]);
});

test('without an App Bridge to follow them, nav items open the app there again', async () => {
  const page = await adminPage();
  page.admin.renderNav({ items: [{ label: 'Settings', path: '/app/settings' }], home: null });
  page.query('.lsf-nav-item').click();
  page.query('.lsf-nav-app').click();
  assert.deepEqual(page.location.assigned, ['/admin/apps/admin-backend/app/settings', '/admin/apps/admin-backend']);
});

test('shows the save bar, sends Save and Discard, and confirms a discard when asked', async () => {
  const page = await connected();
  page.bridge({ type: 'saveBar', visible: true, save: { disabled: false, loading: true }, discard: { disabled: false, loading: false }, discardConfirmation: true });
  assert.equal(page.query('.lsf-savebar').hidden, false);
  assert.ok(page.query('.lsf-savebar-save').hasAttribute('disabled'), 'saving: the button waits');
  assert.ok(page.query('.lsf-admin').classList.contains('lsf-saving'));
  page.query('.lsf-savebar-discard').click();
  await settle();
  const dialog = page.query('dialog.lsf-confirm');
  assert.equal(dialog.querySelector('h2').textContent, 'Discard your changes?');
  dialog.querySelectorAll('button')[0].click();
  await settle();
  assert.equal(page.sent('event').length, 0, 'kept editing');
  page.query('.lsf-savebar-discard').click();
  await settle();
  page.query('dialog.lsf-confirm').querySelectorAll('button')[1].click();
  await settle();
  page.bridge({ type: 'saveBar', visible: true, save: { disabled: false, loading: false }, discard: {}, discardConfirmation: false });
  page.query('.lsf-savebar-save').click();
  assert.deepEqual(page.sent('event').map((event) => event.name), ['saveBar.discard', 'saveBar.save']);
  page.bridge({ type: 'saveBar', visible: false });
  assert.equal(page.query('.lsf-savebar').hidden, true);
});

test('asks the merchant before leaving with unsaved changes', async () => {
  const page = await connected();
  page.bridge({ type: 'request', id: 1, method: 'leaveConfirmation', params: null });
  await settle();
  const dialog = page.query('dialog.lsf-confirm');
  assert.equal(dialog.querySelector('h2').textContent, 'Leave this page?');
  assert.ok(page.query('.lsf-savebar').classList.contains('lsf-shake'));
  dialog.querySelectorAll('button')[1].click();
  await settle();
  assert.equal(page.sent('response')[0].value, true);

  // A section of the admin, with the save bar shown.
  page.bridge({ type: 'saveBar', visible: true, save: {}, discard: {} });
  const section = page.query('.lsf-nav-section');
  const event = new FakeEvent('click', { bubbles: true, cancelable: true });
  section.dispatchEvent(event);
  assert.equal(event.defaultPrevented, true);
  await settle();
  page.query('dialog.lsf-confirm').querySelectorAll('button')[1].click();
  await settle();
  assert.deepEqual(page.location.assigned, ['/admin/products']);
});

test('shows toasts, which go after their duration and say so to the frame', async () => {
  const page = await connected();
  page.bridge({ type: 'toast', id: 't1', message: 'Saved', duration: 10, isError: false, action: 'Undo' });
  page.bridge({ type: 'toast', id: 't2', message: 'Failed', duration: 10000, isError: true, action: null });
  assert.deepEqual(page.all('.lsf-toast-message').map((toast) => toast.textContent), ['Saved', 'Failed']);
  assert.match(page.query('[data-toast="t2"]').getAttribute('class'), /lsf-toast-error/);
  page.query('.lsf-toast-action').click();
  page.bridge({ type: 'toast-hide', id: 't2' });
  await tick();
  await tick();
  assert.equal(page.all('.lsf-toast').length, 0);
  assert.deepEqual(page.sent('event').map((event) => [event.name, event.detail.id]), [['toast.action', 't1'], ['toast.dismiss', 't2'], ['toast.dismiss', 't1']]);
});

test('shows the loading bar and dims the admin while the app shows a modal', async () => {
  const page = await connected();
  page.bridge({ type: 'loading', on: true });
  assert.equal(page.query('.lsf-progress').hidden, false);
  page.bridge({ type: 'modal', open: true });
  assert.ok(page.query('.lsf-admin').classList.contains('lsf-modal-open'));
  // The frame's page went away: what it asked for goes with it.
  page.bridge({ type: 'unload' });
  assert.equal(page.query('.lsf-progress').hidden, true);
  assert.equal(page.query('.lsf-admin').classList.contains('lsf-modal-open'), false);
});

test('follows shopify:admin links to lsf\'s page of them, and other links out of the admin', async () => {
  const page = await connected({ config: adminConfig({ query: { shop: 'other.myshopify.com' } }) });
  page.bridge({ type: 'navigate', url: 'shopify:/admin/products/123', target: '_top' });
  page.bridge({ type: 'navigate', url: 'shopify:/admin/orders', target: '_blank' });
  page.bridge({ type: 'navigate', url: 'https://example.org/docs', target: '_top' });
  page.bridge({ type: 'navigate', url: 'javascript:alert(1)', target: '_top' });
  assert.deepEqual(page.location.assigned, ['/admin/products/123?shop=other.myshopify.com', 'https://example.org/docs']);
  assert.deepEqual(plain(page.opened), [{ url: '/admin/orders?shop=other.myshopify.com', target: '_blank' }]);
});

test('lists what lsf does not simulate that the page called', async () => {
  const page = await connected();
  page.bridge({ type: 'unsupported', name: 'shopify.scanner.capture' });
  page.bridge({ type: 'unsupported', name: 'shopify.reviews.request' });
  assert.match(page.query('[data-banner="unsupported"]').textContent, /lsf does not simulate: shopify\.scanner\.capture, shopify\.reviews\.request\./);
});

const PRODUCTS = [
  {
    id: 'gid://shopify/Product/1',
    title: 'Mug',
    status: 'ACTIVE',
    vendor: 'Acme',
    totalInventory: 5,
    hasOnlyDefaultVariant: false,
    images: [{ id: 'gid://shopify/MediaImage/1', originalSrc: 'http://cdn/mug.png', altText: null }],
    variants: [
      { id: 'gid://shopify/ProductVariant/11', title: 'Oat', price: '22.00', product: { id: 'gid://shopify/Product/1' } },
      { id: 'gid://shopify/ProductVariant/12', title: 'Slate', price: '22.00', product: { id: 'gid://shopify/Product/1' } },
    ],
  },
  {
    id: 'gid://shopify/Product/2',
    title: 'Tote',
    status: 'DRAFT',
    vendor: 'Acme',
    totalInventory: 0,
    hasOnlyDefaultVariant: true,
    images: [],
    variants: [{ id: 'gid://shopify/ProductVariant/21', title: 'Default Title', price: '15.00', product: { id: 'gid://shopify/Product/2' } }],
  },
];

/** The admin with lsf's resources endpoint answering PRODUCTS (for the query `id:…`, those). */
async function picking(options) {
  const asked = [];
  const page = await connected({
    fetch: async (url) => {
      const params = new URL(url, 'http://127.0.0.1:9292').searchParams;
      asked.push(Object.fromEntries(params));
      const query = params.get('query') ?? '';
      const ids = [...query.matchAll(/(?:variant_)?id:(\d+)/g)].map((match) => match[1]);
      const resources = ids.length ? PRODUCTS.filter((product) => ids.some((id) => product.id.endsWith(`/${id}`) || product.variants.some((v) => v.id.endsWith(`/${id}`)))) : PRODUCTS;
      return json({ resources, pageInfo: { hasNextPage: false, endCursor: null } });
    },
  });
  page.bridge({ type: 'request', id: 1, method: 'resourcePicker', params: options });
  await settle();
  await settle();
  const dialog = page.query('dialog.lsf-picker');
  const rows = () => dialog.querySelectorAll('.lsf-picker-row');
  const check = (index, on = true) => {
    const box = rows()[index].querySelector('input');
    box.checked = on;
    box.dispatchEvent(new FakeEvent('change'));
  };
  const answer = async () => {
    await settle();
    return page.sent('response')[0];
  };
  return { page, dialog, rows, check, answer, asked };
}

test('the resource picker lists the shop\'s products with their variants, and answers what was picked', async () => {
  const { dialog, rows, check, answer, asked } = await picking({ type: 'product', multiple: true, filter: { query: 'vendor:Acme', archived: false } });
  assert.equal(dialog.querySelector('h2').textContent, 'Add products');
  assert.equal(asked[0].type, 'product');
  assert.equal(asked[0].query, '(vendor:Acme) AND -status:archived');
  assert.deepEqual(rows().map((row) => row.querySelector('.lsf-picker-title').textContent), ['Mug', 'Oat', 'Slate', 'Tote'], 'a product of one variant has no variant rows');
  assert.equal(rows()[3].querySelector('.lsf-badge').textContent, 'Draft');
  check(2);
  check(3);
  assert.equal(dialog.querySelector('.lsf-picker-count').textContent, '2 products selected');
  assert.ok(rows()[0].querySelector('input').indeterminate, 'one variant of two');
  dialog.querySelectorAll('.lsf-dialog-actions button')[1].click();
  const response = await answer();
  assert.deepEqual(
    response.value.map((product) => [product.title, product.variants.map((variant) => variant.title)]),
    [['Mug', ['Slate']], ['Tote', ['Default Title']]],
  );
  assert.equal(response.value[0].images[0].originalSrc, 'http://cdn/mug.png');
});

test('the resource picker picks one unless told more, and answers null when cancelled', async () => {
  const single = await picking({ type: 'product', action: 'select' });
  assert.equal(single.dialog.querySelector('h2').textContent, 'Select product');
  single.check(0);
  single.check(3);
  assert.equal(single.dialog.querySelector('.lsf-picker-count').textContent, '1 product selected');
  single.dialog.querySelectorAll('.lsf-dialog-actions button')[0].click();
  assert.equal((await single.answer()).value, null);
});

test('the resource picker of variants answers variants, the preselected ones fetched by id', async () => {
  const { check, dialog, answer, asked } = await picking({ type: 'variant', multiple: 2, selectionIds: [{ id: 'gid://shopify/ProductVariant/12' }] });
  assert.equal(asked[0].query, 'variant_id:12');
  assert.equal(dialog.querySelector('.lsf-picker-count').textContent, '1 variant selected');
  check(1);
  dialog.querySelectorAll('.lsf-dialog-actions button')[1].click();
  const response = await answer();
  assert.deepEqual(response.value.map((variant) => variant.title), ['Oat', 'Slate'], 'in the order of their product');
  assert.equal(response.value[0].product.id, 'gid://shopify/Product/1');
});

test('the picker of the app\'s items answers their ids', async () => {
  const page = await connected();
  page.bridge({
    type: 'request',
    id: 3,
    method: 'picker',
    params: { heading: 'Templates', multiple: true, headers: [{ content: 'Uses', type: 'number' }], items: [{ id: 'a', heading: 'Welcome', data: [3], selected: true }, { id: 'b', heading: 'Promo', data: [1], badges: [{ content: 'Draft', tone: 'info' }] }] },
  });
  await settle();
  const dialog = page.query('dialog.lsf-picker');
  assert.equal(dialog.querySelector('h2').textContent, 'Templates');
  const boxes = dialog.querySelectorAll('tbody input');
  boxes[1].checked = true;
  boxes[1].dispatchEvent(new FakeEvent('change'));
  dialog.querySelectorAll('.lsf-dialog-actions button')[1].click();
  await settle();
  assert.deepEqual(plain(page.sent('response')[0].value), ['a', 'b']);
});

test('a page lsf has no page for marks its section, not the app', async () => {
  const page = await adminPage({ frame: false, path: '/admin/products/12', config: adminConfig({ frame: undefined }) });
  assert.ok(page.query('.lsf-nav-section').hasAttribute('aria-current'));
  assert.equal(page.query('.lsf-nav-app').hasAttribute('aria-current'), false);
});

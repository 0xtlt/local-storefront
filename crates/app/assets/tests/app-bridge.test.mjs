// lsf's App Bridge (`admin/app-bridge.js`): the `shopify` global in the app's frame, and what it
// says to lsf's admin around it.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ADMIN, APP, LAUNCH, appFrame, element, plain, settle, tick } from './admin-harness.mjs';

/** A frame the admin welcomed. */
function connected(options) {
  const frame = appFrame(options);
  frame.welcome();
  return frame;
}

/** A fetch that records each request and answers `respond(request)` (200 `{}` by default). */
function recordingFetch(respond = () => new Response('{}')) {
  const requests = [];
  const fetch = async (request) => {
    requests.push(request);
    return respond(request, requests.length);
  };
  return { fetch, requests };
}

test('defines the shopify global first, so that Shopify\'s script finds it and does nothing', () => {
  const frame = appFrame();
  const { shopify } = frame;
  // The first line of the CDN script: `if ("then" in (self.shopify?.ready || {})) return;`.
  assert.ok('then' in (frame.window.shopify?.ready || {}));
  assert.equal(shopify.lsf.version, 'lsf-1');
  assert.equal(shopify.origin, ADMIN);
  assert.deepEqual(plain(shopify.environment), { mobile: false, embedded: true, pos: false, intent: false });
  for (const name of ['config', 'origin', 'ready', 'setSignals', 'environment', 'loading', 'idToken', 'user', 'toast', 'resourcePicker',
    'scanner', 'modal', 'saveBar', 'pos', 'intents', 'webVitals', 'support', 'reviews', 'scopes', 'picker', 'app', 'shopifyQL', 'sidekick', 'tools']) {
    assert.ok(name in shopify, `shopify.${name}`);
  }
  // The elements of App Bridge are defined, and what it draws in the admin is hidden in the page.
  for (const name of ['ui-modal', 's-app-window', 'ui-save-bar', 'ui-title-bar', 'ui-nav-menu', 's-app-nav']) {
    assert.ok(frame.window.customElements.get(name), name);
  }
  const style = frame.document.head.querySelector('style[data-lsf-app-bridge]');
  assert.match(style.textContent, /ui-title-bar, ui-nav-menu, s-app-nav, ui-save-bar/);
  assert.match(style.textContent, /s-page > \[slot="primary-action"\]/);
});

test('a script that finds Shopify\'s App Bridge already there leaves it, and says why', () => {
  const frame = appFrame({ window: { shopify: { ready: Promise.resolve(), config: {} } } });
  assert.equal(frame.shopify.lsf, undefined, 'Shopify\'s global is kept');
  assert.match(frame.errors[0], /Shopify's App Bridge was loaded before lsf's/);
});

test('reads its configuration where Shopify\'s does: the page\'s scripts, meta tags, query and session', () => {
  const session = new Map();
  const frame = appFrame({
    session,
    build(document) {
      // What AppProvider renders after lsf's script.
      document.body.append(element(document, 'script', { src: 'https://cdn.shopify.com/shopifycloud/app-bridge.js', 'data-api-key': 'client-1' }));
      document.head.append(element(document, 'meta', { name: 'shopify-disabled-features', content: 'auto-redirect, web-vitals' }));
      document.head.append(element(document, 'meta', { name: 'Shopify-App-Origins', content: 'https://api.example.com' }));
    },
  });
  const config = plain(frame.shopify.config);
  assert.equal(config.apiKey, 'client-1');
  assert.equal(config.shop, 'local-supply-co.myshopify.com');
  assert.equal(config.locale, 'fr');
  assert.equal(config.host, 'YWRtaW4uc2hvcGlmeS5jb20vc3RvcmUvbG9jYWwtc3VwcGx5LWNv');
  assert.deepEqual(config.disabledFeatures, ['auto-redirect', 'web-vitals']);
  assert.deepEqual(config.appOrigins, ['https://api.example.com']);
  assert.equal(JSON.parse(session.get('app-bridge-config')).shop, 'local-supply-co.myshopify.com', 'kept for the next page');

  // A page of the app without the admin's parameters (after a client-side navigation and a reload).
  const later = appFrame({ session, href: `${APP}/app/settings` });
  assert.equal(later.shopify.config.shop, 'local-supply-co.myshopify.com');
  assert.equal(later.shopify.config.apiKey, 'client-1');
});

test('says hello to its parent, and listens to lsf\'s admin only', async () => {
  const frame = appFrame();
  const [hello] = frame.posted;
  assert.equal(hello.targetOrigin, '*', 'any parent may say why it cannot answer');
  assert.deepEqual(Object.keys(hello.message).sort(), ['apiKey', 'lsf', 'scriptOrigin', 'type', 'v', 'version']);
  assert.equal(hello.message.scriptOrigin, ADMIN);

  // An admin on another origin is not lsf's: nothing it says is taken, and the console says why.
  frame.welcome.call(null);
  const impostor = appFrame();
  impostor.receive({ type: 'welcome', shop: 'evil.myshopify.com' }, 'http://localhost:9292');
  assert.match(impostor.errors.join('\n'), /the admin around this page is at http:\/\/localhost:9292, but this script comes from http:\/\/127\.0\.0\.1:9292/);
  let ready = false;
  impostor.shopify.ready.then(() => (ready = true));
  await settle();
  assert.equal(ready, false);

  // What is asked before the welcome waits for it, then goes to the admin's origin alone.
  const early = appFrame();
  early.shopify.toast.show('Saved');
  assert.equal(early.posts('toast').length, 0);
  early.welcome();
  await early.shopify.ready;
  assert.equal(early.last('toast').message, 'Saved');
  assert.ok(early.posted.filter(({ message }) => message.type !== 'hello').every(({ targetOrigin }) => targetOrigin === ADMIN));
});

test('idToken() asks the admin, and fails after a minute without an answer', async () => {
  const frame = connected();
  const token = frame.shopify.idToken();
  await frame.answer({ idToken: 'eyJ.token.sig' })();
  assert.equal(await token, 'eyJ.token.sig');
  const empty = frame.shopify.idToken();
  await frame.answer({ idToken: '' })();
  await assert.rejects(empty, /idToken unavailable/);
});

test('fetch() adds the session token to the app\'s requests, as App Bridge v4 decides which', async () => {
  const { fetch, requests } = recordingFetch();
  const frame = connected({
    fetch,
    build(document) {
      document.head.append(element(document, 'meta', { name: 'shopify-app-origins', content: 'https://api.example.com' }));
    },
  });
  const answer = frame.answer({ idToken: () => `token-${frame.posts('request').length}` });
  const call = async (url, init) => {
    const response = frame.window.fetch(url, init);
    await settle();
    await answer();
    await response;
    return requests.at(-1);
  };

  const own = await call('/api/products');
  assert.equal(own.url, `${APP}/api/products`);
  assert.equal(own.headers.get('authorization'), 'Bearer token-1');
  assert.equal(own.headers.get('x-requested-with'), 'XMLHttpRequest');
  assert.equal(own.headers.get('accept-language'), 'fr');
  // The page's host on any port, its subdomains, and `appOrigins`.
  assert.match((await call('http://localhost:8080/x')).headers.get('authorization'), /^Bearer /);
  assert.match((await call('http://api.localhost:3000/x')).headers.get('authorization'), /^Bearer /);
  assert.match((await call('https://api.example.com/x')).headers.get('authorization'), /^Bearer /);
  // Another site, or another protocol: untouched.
  const other = await call('https://example.org/x');
  assert.equal(other.headers.get('authorization'), null);
  assert.equal(other.headers.get('x-requested-with'), null);
  assert.equal((await call('https://localhost:3000/x')).headers.get('authorization'), null);
  // A token of the app's own is kept.
  assert.equal((await call('/x', { headers: { Authorization: 'Bearer mine' } })).headers.get('authorization'), 'Bearer mine');
});

test('fetch() asks a new token once when the backend says so, and follows a reauthorization', async () => {
  const { fetch, requests } = recordingFetch((request, count) =>
    count === 1 ? new Response('', { status: 401, headers: { 'X-Shopify-Retry-Invalid-Session-Request': '1' } }) : new Response('ok'),
  );
  const frame = connected({ fetch });
  const answer = frame.answer({ idToken: () => `token-${frame.posts('request').length}` });
  const response = frame.window.fetch('/api/x');
  for (let i = 0; i < 4; i++) {
    await settle();
    await answer();
  }
  assert.equal(await (await response).text(), 'ok');
  assert.deepEqual(requests.map((request) => request.headers.get('authorization')), ['Bearer token-1', 'Bearer token-2']);

  const reauthorize = connected({
    fetch: async () => new Response('', { status: 401, headers: { 'X-Shopify-API-Request-Failure-Reauthorize-Url': '/admin/charges/1' } }),
  });
  let settled = false;
  reauthorize.window.fetch('/api/billing', { headers: { Authorization: 'Bearer t' } }).then(() => (settled = true));
  await settle();
  await settle();
  assert.deepEqual(reauthorize.last('navigate'), { lsf: 'app-bridge', v: 1, type: 'navigate', url: `${APP}/admin/charges/1`, target: '_top' });
  assert.equal(settled, false, 'the page goes, the fetch never answers');
});

test('fetch() of Direct API access rejects, and disabledFeatures: fetch leaves fetch alone', async () => {
  const { fetch } = recordingFetch();
  const frame = connected({ fetch });
  await assert.rejects(frame.window.fetch('shopify:admin/api/graphql.json', { method: 'POST' }), /lsf does not simulate shopify\.fetch\(\) of shopify:admin\/… URLs \(Direct API access\)/);
  assert.equal(frame.last('unsupported').name, 'shopify.fetch() of shopify:admin/… URLs (Direct API access)');

  const untouched = connected({
    fetch,
    build(document) {
      document.head.append(element(document, 'meta', { name: 'shopify-disabled-features', content: 'fetch' }));
    },
  });
  assert.equal(untouched.window.fetch, fetch);
});

test('toast.show() returns an id, and calls back on the action and when the toast goes', async () => {
  const frame = connected();
  const calls = [];
  const id = frame.shopify.toast.show('Product saved', { action: 'Undo', isError: true, duration: 2000, onAction: () => calls.push('action'), onDismiss: () => calls.push('dismiss') });
  assert.match(id, /^lsf-toast-\d+$/);
  assert.deepEqual(frame.last('toast'), { lsf: 'app-bridge', v: 1, type: 'toast', id, message: 'Product saved', duration: 2000, isError: true, action: 'Undo' });
  frame.event('toast.action', { id });
  frame.event('toast.dismiss', { id });
  frame.event('toast.dismiss', { id });
  assert.deepEqual(calls, ['action', 'dismiss']);
  frame.shopify.toast.hide(id);
  assert.equal(frame.last('toast-hide').id, id);
  assert.equal(frame.last('toast', { duration: 5000 }).type, 'toast');
  frame.shopify.toast.show('Plain');
  assert.equal(frame.last('toast').duration, 5000, 'five seconds by default');
  assert.equal(frame.last('toast').action, null);
});

test('loading() says when it starts and stops, once each', () => {
  const frame = connected();
  frame.shopify.loading(true);
  frame.shopify.loading(true);
  frame.shopify.loading(false);
  frame.shopify.loading();
  assert.deepEqual(frame.posts('loading').map((message) => message.on), [true, false]);
});

test('user() and scopes.query() answer what the admin said', async () => {
  const frame = connected();
  assert.deepEqual(plain(await frame.shopify.user()), { id: 1, name: 'Shop Owner', email: 'owner@local-supply-co.myshopify.com', accountAccess: 'Account owner', locale: 'en' });
  assert.deepEqual(plain(await frame.shopify.scopes.query()), { granted: ['read_products'], required: ['read_products'], optional: ['write_products'] });
});

test('the title bar comes from ui-title-bar, and its buttons are clicked from the admin', async () => {
  let clicked = '';
  const frame = connected({
    build(document) {
      const button = (label, attributes) => {
        const created = element(document, 'button', attributes, [label]);
        created.addEventListener('click', () => (clicked = label));
        return created;
      };
      document.body.append(
        element(document, 'ui-title-bar', { title: 'Products' }, [
          button('Back', { variant: 'breadcrumb' }),
          button('Save', { variant: 'primary', disabled: '' }),
          button('Delete', { tone: 'critical' }),
          element(document, 'section', { label: 'More actions' }, [button('Duplicate'), button('Archive')]),
        ]),
      );
    },
  });
  const title = frame.last('title');
  assert.equal(title.title, 'Products');
  assert.equal(title.breadcrumb.label, 'Back');
  assert.deepEqual([title.primary.label, title.primary.disabled], ['Save', true]);
  assert.deepEqual([title.secondary[0].label, title.secondary[0].tone], ['Delete', 'critical']);
  assert.equal(title.secondary[1].group, true);
  assert.deepEqual(title.secondary[1].actions.map((action) => action.label), ['Duplicate', 'Archive']);
  frame.event('title.action', { index: title.secondary[1].actions[1].index });
  assert.equal(clicked, 'Archive');

  // A change of the page sends the title again; the same title is not sent twice.
  const before = frame.posts('title').length;
  frame.document.querySelector('ui-title-bar').setAttribute('title', 'All products');
  await frame.mutated();
  await frame.mutated();
  assert.equal(frame.posts('title').length, before + 1);
  assert.equal(frame.last('title').title, 'All products');
});

test('the title bar comes from s-page and its slots, and from document.title without one', () => {
  const frame = connected({
    build(document) {
      document.body.append(
        element(document, 's-page', { heading: 'Edit product' }, [
          element(document, 's-button', { slot: 'primary-action' }, ['Save']),
          element(document, 's-button', { slot: 'secondary-actions' }, ['Preview']),
          element(document, 's-button', { slot: 'secondary-actions', commandfor: 'more' }, ['More actions']),
          element(document, 's-menu', { id: 'more' }, [element(document, 's-button', {}, ['Import']), element(document, 's-button', {}, ['Export'])]),
          element(document, 's-link', { slot: 'breadcrumb-actions', href: '/app' }, ['Home']),
          element(document, 's-badge', { slot: 'accessory', tone: 'warning' }, ['Draft']),
        ]),
      );
    },
  });
  const title = frame.last('title');
  assert.equal(title.title, 'Edit product');
  assert.equal(title.primary.label, 'Save');
  assert.equal(title.breadcrumb.href, '/app');
  assert.deepEqual(title.accessory, { label: 'Draft', tone: 'warning' });
  assert.equal(title.secondary[0].label, 'Preview');
  assert.deepEqual(title.secondary[1].actions.map((action) => action.label), ['Import', 'Export']);
  assert.ok(frame.document.getElementById('more').hasAttribute('data-lsf-title-bar-menu'), 'the menu is the title bar\'s, hidden in the page');

  const plainPage = connected({ build: (document) => document.setAttribute('title', 'Reports') });
  assert.equal(plainPage.last('title').title, 'Reports');
  assert.equal(plainPage.last('title').primary, null);
});

test('the navigation comes from s-app-nav, and the admin\'s nav items click its links', async () => {
  const clicked = [];
  const frame = connected({
    build(document) {
      const link = (href, label, attributes = {}) => {
        const created = element(document, 'a', { href, ...attributes }, [label]);
        created.addEventListener('click', (event) => {
          event.preventDefault();
          clicked.push(href);
        });
        return created;
      };
      document.body.append(
        element(document, 's-app-nav', {}, [
          link('/app', 'Home', { rel: 'home' }),
          link('/app/templates', 'Templates'),
          element(document, 's-link', { href: 'app:/app/settings?tab=1' }, ['Settings']),
          link('https://example.org/help', 'Help'),
        ]),
      );
    },
  });
  const nav = frame.last('nav');
  assert.deepEqual(plain(nav.items), [
    { label: 'Templates', path: '/app/templates' },
    { label: 'Settings', path: '/app/settings?tab=1' },
  ]);
  assert.equal(nav.home, '/app');
  assert.match(frame.warnings.join('\n'), /Dropped nav item with unsupported href "https:\/\/example.org\/help"/);

  frame.event('navigate', { path: '/app/templates' });
  frame.event('navigate', { home: true });
  await settle();
  assert.deepEqual(clicked, ['/app/templates', '/app']);
  frame.event('navigate', { path: '/app/nowhere' });
  await settle();
  assert.deepEqual(frame.location.assigned, ['/app/nowhere'], 'no link of its own: the frame goes there');
});

test('a data-save-bar form shows the save bar while it differs from what it was', async () => {
  let submitted = 0;
  const frame = connected({
    build(document) {
      const form = element(document, 'form', { 'data-save-bar': '', 'data-discard-confirmation': '' }, [
        element(document, 'input', { name: 'title', value: 'Mug' }),
      ]);
      form.querySelector('input').value = 'Mug';
      form.requestSubmit = () => {
        submitted++;
        frame.fire('submit', { target: form, defaultPrevented: true });
      };
      form.reset = () => {
        form.querySelector('input').value = 'Mug';
        frame.fire('reset', { target: form });
      };
      document.body.append(form);
    },
  });
  const input = frame.document.querySelector('input');
  assert.deepEqual(plain(frame.last('saveBar')), { lsf: 'app-bridge', v: 1, type: 'saveBar', visible: false });
  input.value = 'Big mug';
  frame.fire('input', {});
  await tick();
  const bar = frame.last('saveBar');
  assert.equal(bar.visible, true);
  assert.equal(bar.discardConfirmation, true);
  input.value = 'Mug';
  frame.fire('input', {});
  await tick();
  assert.equal(frame.last('saveBar').visible, false, 'back to what it was: no save bar');

  input.value = 'Big mug';
  frame.fire('input', {});
  await tick();
  frame.event('saveBar.save', {});
  await tick();
  assert.equal(submitted, 1);
  assert.equal(frame.last('saveBar').visible, false, 'saved: what it has now is what it is compared with');

  input.value = 'Huge mug';
  frame.fire('change', {});
  await tick();
  frame.event('saveBar.discard', {});
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(input.value, 'Mug');
  assert.equal(frame.last('saveBar').visible, false);
});

test('ui-save-bar is shown by shopify.saveBar, and its buttons answer the admin\'s', async () => {
  const clicks = [];
  const frame = connected({
    build(document) {
      const bar = element(document, 'ui-save-bar', { id: 'bar', discardconfirmation: '' }, [
        element(document, 'button', { variant: 'primary', loading: '' }, ['Save']),
        element(document, 'button', {}, ['Discard']),
      ]);
      for (const button of bar.querySelectorAll('button')) button.addEventListener('click', () => clicks.push(button.textContent));
      document.body.append(bar);
    },
  });
  const bar = frame.document.getElementById('bar');
  const events = [];
  bar.addEventListener('show', () => events.push('show'));
  bar.addEventListener('hide', () => events.push('hide'));
  await frame.shopify.saveBar.show('bar');
  await tick();
  assert.equal(bar.showing, true);
  assert.deepEqual(plain(frame.last('saveBar')), {
    lsf: 'app-bridge', v: 1, type: 'saveBar', visible: true, id: 'bar',
    save: { disabled: false, loading: true }, discard: { disabled: false, loading: false }, discardConfirmation: true,
  });
  frame.event('saveBar.save', {});
  frame.event('saveBar.discard', {});
  assert.deepEqual(clicks, ['Save', 'Discard']);
  await frame.shopify.saveBar.toggle('bar');
  await tick();
  assert.equal(frame.last('saveBar').visible, false);
  assert.deepEqual(events, ['show', 'hide']);
  await assert.rejects(frame.shopify.saveBar.show('nope'), /the page has no <ui-save-bar id="nope">/);
});

test('with the save bar shown, a link asks the admin first, and goes once the merchant leaves', async () => {
  const frame = connected({
    build(document) {
      document.body.append(element(document, 'ui-save-bar', { id: 'bar' }), element(document, 'a', { href: 'shopify://admin/orders' }, ['Orders']));
    },
  });
  await frame.shopify.saveBar.show('bar');
  await settle();
  const link = frame.document.querySelector('a');
  const blocked = frame.click(link);
  assert.equal(blocked.defaultPrevented, true);
  await settle();
  const [ask] = frame.posts('request').filter((request) => request.method === 'leaveConfirmation');
  assert.ok(ask, 'the admin is asked');
  assert.equal(frame.posts('navigate').length, 0);
  frame.receive({ type: 'response', id: ask.id, ok: true, value: true });
  await settle();
  assert.equal(frame.last('navigate').url, 'shopify:/admin/orders', 'left: the link goes');
  assert.equal(frame.document.getElementById('bar').showing, false);
  assert.equal(await Promise.race([frame.shopify.saveBar.leaveConfirmation().then(() => 'resolved'), settle().then(() => 'pending')]), 'resolved', 'no save bar: at once');
});

test('links to the admin, to the app and to the whole window go where App Bridge sends them', async () => {
  const frame = connected({
    build(document) {
      document.body.append(
        element(document, 'a', { href: 'shopify://admin/products/123', target: '_top' }, ['Product']),
        element(document, 'a', { href: 'shopify:admin/orders', target: '_blank' }, ['Orders']),
        element(document, 'a', { href: 'shopify:customers' }, ['Wrong']),
        element(document, 'a', { href: 'app:/app/settings' }, ['Settings']),
        element(document, 'a', { href: 'https://example.org/docs', target: '_top' }, ['Docs']),
        element(document, 'a', { href: '/app/plain' }, ['Plain']),
        element(document, 'a', { href: 'extension:my-extension' }, ['Extension']),
      );
    },
  });
  const [product, orders, wrong, settings, docs, plainLink, extension] = frame.document.querySelectorAll('a');
  assert.equal(frame.click(product).defaultPrevented, true);
  assert.deepEqual(frame.last('navigate'), { lsf: 'app-bridge', v: 1, type: 'navigate', url: 'shopify:/admin/products/123', target: '_top' });
  frame.click(orders);
  assert.equal(frame.last('navigate').target, '_blank');
  assert.throws(() => frame.click(wrong), /Invalid URL: expected '\/admin\/\*', received: '\/customers'/);
  frame.click(settings);
  assert.deepEqual(frame.location.assigned, ['/app/settings']);
  frame.click(docs);
  assert.deepEqual(frame.last('navigate'), { lsf: 'app-bridge', v: 1, type: 'navigate', url: 'https://example.org/docs', target: '_top' });
  // A plain link nothing routes reloads the frame without its session token: said, not prevented.
  assert.equal(frame.click(plainLink).defaultPrevented, false);
  assert.match(frame.warnings.join('\n'), /<a href="\/app\/plain"> reloads the app's frame without the session token/);
  frame.click(extension);
  assert.match(frame.warnings.join('\n'), /lsf does not simulate shopify\.navigation to extension: URLs/);

  frame.window.open('shopify://admin/customers', '_top');
  assert.equal(frame.last('navigate').url, 'shopify:/admin/customers');
  frame.window.open('https://example.org', '_blank');
  assert.deepEqual(plain(frame.opened.at(-1)), { url: 'https://example.org', target: '_blank' });
});

test('an s-link without Polaris dispatches shopify:navigate for the router, and goes when none follows it', () => {
  const frame = connected({
    build(document) {
      document.body.append(element(document, 's-link', { href: '/app/routed' }, ['Routed']), element(document, 's-link', { href: '/app/plain' }, ['Plain']));
    },
  });
  const routed = [];
  const [first, second] = frame.document.querySelectorAll('s-link');
  // AppProvider's listener: it reads the href and navigates.
  frame.document.addEventListener('shopify:navigate', (event) => {
    if (event.target === first) routed.push(event.target.getAttribute('href'));
  });
  frame.click(first);
  assert.deepEqual(routed, ['/app/routed']);
  assert.deepEqual(frame.location.assigned, []);
  frame.click(second);
  assert.deepEqual(frame.location.assigned, [`${APP}/app/plain`]);
});

test('the app\'s path is reported when it changes, so that the admin\'s URL follows it', () => {
  const frame = connected();
  assert.equal(frame.posts('history')[0].path, new URL(LAUNCH).pathname + new URL(LAUNCH).search);
  frame.window.history.pushState(null, '', '/app/products?page=2');
  assert.deepEqual(frame.last('history'), { lsf: 'app-bridge', v: 1, type: 'history', path: '/app/products?page=2', mode: 'push' });
  frame.window.history.replaceState(null, '', '/app/products?page=3');
  assert.equal(frame.last('history').mode, 'replace');
});

test('resourcePicker() answers the admin\'s selection, with its selection property, or undefined', async () => {
  const frame = connected();
  const picking = frame.shopify.resourcePicker({ type: 'product', multiple: true, filter: { variants: false }, onSomething() {} });
  const products = [{ id: 'gid://shopify/Product/1', title: 'Mug', variants: [] }];
  await frame.answer({ resourcePicker: () => products })();
  const selected = await picking;
  assert.deepEqual(plain(selected), products);
  assert.equal(selected.selection, selected);
  const request = frame.posts('request').find((entry) => entry.method === 'resourcePicker');
  assert.deepEqual(request.params, { type: 'product', multiple: true, filter: { variants: false } }, 'the options, without functions');

  const cancelled = frame.shopify.resourcePicker({ type: 'collection' });
  await frame.answer({ resourcePicker: null })();
  assert.equal(await cancelled, undefined);
  await assert.rejects(frame.shopify.resourcePicker({ type: 'order' }), /type is one of product, variant, collection/);

  const { selected: ids } = await frame.shopify.picker({ heading: 'Templates', items: [{ id: 't1', heading: 'One' }] });
  await frame.answer({ picker: ['t1'] })();
  assert.deepEqual(plain(await ids), ['t1']);
});

test('ui-modal opens over the app, dims the admin, and is shown by its id', async () => {
  const frame = connected({
    build(document) {
      document.body.append(
        element(document, 'ui-modal', { id: 'confirm', variant: 'small' }, [
          element(document, 'p', {}, ['Delete it?']),
          element(document, 'ui-title-bar', { title: 'Delete product' }, [element(document, 'button', { variant: 'primary' }, ['Delete']), element(document, 'button', {}, ['Cancel'])]),
        ]),
      );
    },
  });
  const modal = frame.document.getElementById('confirm');
  const events = [];
  modal.addEventListener('show', () => events.push('show'));
  modal.addEventListener('hide', () => events.push('hide'));
  await frame.shopify.modal.show('confirm');
  assert.ok(modal.hasAttribute('data-lsf-open'));
  assert.equal(frame.last('modal').open, true);
  const root = modal.shadowRoot;
  assert.equal(root.querySelector('.title').textContent, 'Delete product');
  assert.deepEqual(root.querySelectorAll('footer button').map((button) => button.textContent), ['Cancel', 'Delete'], 'the primary action last');
  assert.equal(modal.getAttribute('data-lsf-variant'), 'small');
  await frame.shopify.modal.toggle('confirm');
  assert.equal(modal.hasAttribute('data-lsf-open'), false);
  assert.equal(frame.last('modal').open, false);
  assert.deepEqual(events, ['show', 'hide']);
  await assert.rejects(frame.shopify.modal.show('nope'), /has no <ui-modal>, <s-modal> or <s-app-window> with id "nope"/);

  // A modal with src shows that page of the app, in a frame named for the App Bridge it shares.
  modal.setAttribute('src', '/app/modal');
  await modal.show();
  const iframe = modal.shadowRoot.querySelector('iframe');
  assert.equal(iframe.getAttribute('src'), '/app/modal');
  assert.match(iframe.getAttribute('name'), /^lsf-modal:\d+$/);
  await modal.hide();

  // Polaris's s-modal opens itself.
  const polaris = connected({
    build(document) {
      const sModal = element(document, 's-modal', { id: 'p' });
      sModal.showOverlay = () => (sModal.opened = true);
      document.body.append(sModal);
    },
  });
  await polaris.shopify.modal.show('p');
  assert.equal(polaris.document.getElementById('p').opened, true);
});

test('what lsf does not simulate rejects (or warns) and says so, here and to the admin', async () => {
  const frame = connected();
  const { shopify } = frame;
  const rejected = [
    ['scanner.capture', () => shopify.scanner.capture()],
    ['pos.cart.fetch', () => shopify.pos.cart.fetch()],
    ['pos.close', () => shopify.pos.close()],
    ['pos.device', () => shopify.pos.device()],
    ['pos.location', () => shopify.pos.location()],
    ['intents.invoke', () => shopify.intents.invoke('create:shopify/Product')],
    ['webVitals.onReport', () => shopify.webVitals.onReport(() => {})],
    ['support.registerHandler', () => shopify.support.registerHandler(() => {})],
    ['reviews.request', () => shopify.reviews.request()],
    ['scopes.request', () => shopify.scopes.request(['write_products'])],
    ['scopes.revoke', () => shopify.scopes.revoke(['write_products'])],
    ['app.extensions', () => shopify.app.extensions()],
    ['shopifyQL.available', () => shopify.shopifyQL.available()],
    ['sidekick.isEnabled', () => shopify.sidekick.isEnabled()],
    ['sidekick.launch', () => shopify.sidekick.launch('hi')],
  ];
  for (const [name, call] of rejected) {
    await assert.rejects(call(), new RegExp(`^Error: lsf does not simulate shopify\\.${name.replace('.', '\\.')}$`), name);
  }
  assert.equal(typeof shopify.tools.register('lookup', () => ({})), 'function');
  assert.equal(typeof shopify.pos.cart.subscribe(() => {}), 'function');
  for (const name of ['tools.register', 'pos.cart.subscribe', 'scanner.capture']) {
    assert.ok(frame.warnings.includes(`lsf does not simulate shopify.${name}`), name);
    assert.ok(frame.posts('unsupported').some((message) => message.name === `shopify.${name}`), name);
  }
  assert.equal(shopify.intents.request.value, null, 'not in an intent: as Shopify says it');
});

test('outside an admin, it sends the page to lsf\'s admin, unless auto-redirect is disabled', () => {
  const frame = appFrame({
    framed: false,
    build(document) {
      document.body.append(element(document, 'script', { src: 'https://cdn.shopify.com/shopifycloud/app-bridge.js', 'data-api-key': 'client-1' }));
    },
  });
  assert.deepEqual(frame.location.assigned, [`${ADMIN}/admin/apps/client-1/app?shop=local-supply-co.myshopify.com`]);
  assert.deepEqual(frame.posted, []);

  const stays = appFrame({
    framed: false,
    build(document) {
      document.head.append(element(document, 'meta', { name: 'shopify-api-key', content: 'client-1' }), element(document, 'meta', { name: 'shopify-disabled-features', content: 'auto-redirect' }));
    },
  });
  assert.deepEqual(stays.location.assigned, []);
  return assert.rejects(stays.shopify.idToken(), /this page is not open in lsf's admin, which answers App Bridge here\. Open it at http:\/\/127\.0\.0\.1:9292\/admin\/apps\/client-1\?shop=local-supply-co\.myshopify\.com/);
});

test('the bounce page of Shopify\'s libraries comes back with an ID token', async () => {
  const reload = `${APP}/app/settings?embedded=1&shop=local-supply-co.myshopify.com`;
  const frame = connected({ href: `${APP}/auth/session-token?shop=local-supply-co.myshopify.com&shopify-reload=${encodeURIComponent(reload)}` });
  await settle();
  await frame.answer({ idToken: 'fresh' })();
  await settle();
  assert.deepEqual(frame.location.assigned, [`${reload}&id_token=fresh`]);

  const elsewhere = connected({ href: `${APP}/auth/session-token?shopify-reload=${encodeURIComponent('https://evil.example/x')}` });
  await settle();
  assert.match(elsewhere.errors.join('\n'), /\?shopify-reload must be same-origin/);
  assert.deepEqual(elsewhere.location.assigned, []);
});

test('a page of a modal with src shares the App Bridge of the frame that opened it', () => {
  const opener = connected();
  const modalPage = appFrame({ window: { name: 'lsf-modal:1' } });
  // The fake parent of the modal page is not the opener: give it the opener's global.
  assert.ok(modalPage.shopify.lsf, 'without a same-origin opener, it runs its own');
  const shared = appFrame({ window: { name: 'lsf-modal:2', parent: { shopify: opener.shopify, postMessage() {} } } });
  assert.equal(shared.shopify, opener.shopify);
});

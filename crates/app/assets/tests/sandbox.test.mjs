// The frame of an extension (`sandbox.js`): what the bundle throws reaches the page with its
// message and its stack, wherever it throws.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { checkoutState, init, plain, sandboxWindow, settle } from './harness.mjs';

const SANDBOX = ['sandbox-api.js', 'sandbox-components.js', 'sandbox.js'];
const TARGET = 'purchase.checkout.block.render';
const url = (handle) => `http://localhost/__lsf/extensions/${handle}/${handle}.js`;

async function boot(handle, source, ext) {
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url(handle)]: source } });
  win.receive(init({ handle, target: TARGET, ext }));
  for (let i = 0; i < 4; i++) await settle();
  return win;
}

test('the bundle runs from a blob: URL of the frame, with the text the page sent, so that the browser does not hide its errors', async () => {
  const win = await boot('fine', `shopify.extend(${JSON.stringify(TARGET)}, () => {});`);
  const script = win.document.head.childNodes.find((node) => node.localName === 'script');
  assert.match(script.src, /^blob:/);
  assert.equal(script.crossOrigin, undefined, 'no CORS: the server does not allow it');
  assert.deepEqual(win.posts('error'), []);
});

test('a bundle the page could not fetch says so', async () => {
  const win = sandboxWindow({ scripts: SANDBOX });
  win.receive(init({ handle: 'gone', target: TARGET }));
  for (let i = 0; i < 4; i++) await settle();
  const [error] = win.posts('error');
  assert.match(error.message, /^Could not load http:\/\/localhost\/__lsf\/extensions\/gone\/gone\.js\. Build it with `lsf app build gone` \(or `--build`\)/);
  assert.equal(win.document.head.childNodes.some((node) => node.localName === 'script'), false);
});

test('a bundle that throws as it loads says what it threw, and that it registered nothing', async () => {
  const win = await boot('loads', 'throw new Error("no config");');
  const errors = win.posts('error');
  assert.equal(errors.length, 2, JSON.stringify(errors));
  assert.equal(errors[0].message, 'no config');
  assert.match(errors[0].stack, /Error: no config\n\s+at .*loads\.js/);
  assert.equal(
    errors[1].message,
    `The bundle threw while it loaded (no config), and did not register ${TARGET} with shopify.extend (it registered: nothing).`,
  );
  assert.equal(errors[1].stack, errors[0].stack, 'with the stack of the cause');
});

test('a bundle that registers nothing without throwing says so alone', async () => {
  const win = await boot('empty', '');
  assert.deepEqual(win.posts('error').map((error) => error.message), [
    `The bundle did not register ${TARGET} with shopify.extend (it registered: nothing).`,
  ]);
});

test('what a listener throws and a promise nobody catches reach the page with their stack', async () => {
  const win = await boot('later', `shopify.extend(${JSON.stringify(TARGET)}, () => {});`);
  const thrown = new Error('a click handler threw');
  win.fire('error', { message: 'Uncaught Error: a click handler threw', error: thrown });
  win.fire('unhandledrejection', { reason: new Error('nobody caught this') });
  win.fire('unhandledrejection', { reason: 'a plain string' });
  // What the browser hides of a script of another origin still says something.
  win.fire('error', { message: 'Script error.', error: null });
  const errors = win.posts('error');
  assert.deepEqual(errors.map((error) => error.message), [
    'a click handler threw',
    'Unhandled rejection: nobody caught this',
    'Unhandled rejection: a plain string',
    'Script error.',
  ]);
  assert.equal(errors[0].stack, thrown.stack);
  assert.match(errors[1].stack, /Error: nobody caught this/);
  assert.equal(errors[2].stack, undefined);
});

test('the frame is in the language the extension renders in', async () => {
  const frame = async (locale, translations, shop = locale) => {
    const state = structuredClone(checkoutState());
    state.shop = { ...state.shop, locale: shop };
    state.localization = { ...state.localization, language: { isoCode: locale } };
    const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url('i18n')]: '' } });
    win.receive(init({ handle: 'i18n', target: TARGET, state, ext: { translations } }));
    for (let i = 0; i < 4; i++) await settle();
    return win.document.documentElement.lang;
  };
  // French strings for a French buyer; English ones for a buyer whose language it lacks.
  assert.equal(await frame('fr', { fr: { hi: 'Bonjour' }, en: { hi: 'Hello' } }), 'fr');
  assert.equal(await frame('de', { fr: { hi: 'Bonjour' }, en: { hi: 'Hello' } }), 'en');
  // No strings: its text is in its code, in the shop's primary language more likely than the
  // buyer's.
  assert.equal(await frame('fr', {}, 'en'), 'en');
  assert.equal(await frame('fr', {}, 'fr'), 'fr');
});

test('without network_access, a fetch out of the shop rejects and says why; with it, it is sent', async () => {
  const source = `shopify.extend(${JSON.stringify(TARGET)}, async () => {
    globalThis.results = [];
    for (const target of ['http://127.0.0.1:9664/data', '/apps/proxy/data', new URL('http://localhost/x')]) {
      try {
        const response = await fetch(target);
        results.push('answered ' + response.status);
      } catch (error) {
        results.push(error.name + ': ' + error.message);
      }
    }
  });`;
  const closed = await boot('closed', source, { capabilities: { api_access: true } });
  const [external, ...shop] = plain(closed.run('results'));
  assert.match(external, /^TypeError: fetch\(http:\/\/127\.0\.0\.1:9664\/data\) needs the network_access capability/);
  assert.deepEqual(shop, ['answered 200', 'answered 200'], 'the shop itself (an app proxy, the Storefront API) is not external');
  assert.deepEqual(closed.fetched, ['/apps/proxy/data', 'http://localhost/x']);
  assert.match(closed.posts('log')[0].args[0], /needs the network_access capability/);

  const open = await boot('open', source, { capabilities: { network_access: true } });
  assert.deepEqual(plain(open.run('results')), ['answered 200', 'answered 200', 'answered 200']);
  assert.deepEqual(open.posts('log'), []);
});

test('in a customer account extension, the global navigation is shopify.navigation, and navigates', async () => {
  const target = 'customer-account.profile.block.render';
  const source = `shopify.extend(${JSON.stringify(target)}, () => {
    globalThis.same = navigation === shopify.navigation;
    navigation.navigate('shopify:customer-account/orders');
  });`;
  const win = sandboxWindow({ scripts: ['sandbox-api.js', 'sandbox-account-api.js', 'sandbox-components.js', 'sandbox.js'], bundles: { [url('nav')]: source } });
  // The browser's Navigation API of the frame, which the extension must not get.
  win.context.navigation = { navigate() {} };
  win.receive(init({ handle: 'nav', target }));
  for (let i = 0; i < 4; i++) await settle();
  assert.deepEqual(win.posts('error'), []);
  assert.equal(win.run('same'), true);
  assert.deepEqual(win.posts('navigate'), [{ lsf: true, type: 'navigate', url: 'shopify:customer-account/orders' }]);
});

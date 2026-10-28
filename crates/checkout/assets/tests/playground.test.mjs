// The checkout UI extensions of the playground app (`examples/checkout-playground/extensions/ui-*`):
// each toml is well formed, each bundle registers the targets its toml declares, and each one
// that should work boots in the sandbox at every target without an error. The `ui-broken-*`
// ones fail the way their case says.

import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { refusalOf } from '../api-versions.js';
import { RENDERED_TARGETS } from '../extension-host.js';
import { assets, checkoutState, init, plain, sandboxWindow, settle } from './harness.mjs';

const SANDBOX = ['sandbox-api.js', 'sandbox-components.js', 'sandbox.js'];
const extensions = join(assets, '..', '..', '..', 'examples', 'checkout-playground', 'extensions');
const handles = readdirSync(extensions).filter((name) => name.startsWith('ui-')).sort();
const toml = (handle) => readFileSync(join(extensions, handle, 'shopify.extension.toml'), 'utf8');
const bundlePath = (handle) => join(extensions, handle, 'dist', `${handle}.js`);
const bundle = (handle) => readFileSync(bundlePath(handle), 'utf8');
const url = (handle) => `http://localhost/__lsf/extensions/${handle}/${handle}.js`;

/** Built from its sources (a target names its `module`), by lsf: its bundle is minified. */
const fromSources = (handle) => /^\s*module\s*=/m.test(toml(handle));
const field = (text, name) => text.match(new RegExp(`^\\s*${name}\\s*=\\s*"([^"]*)"`, 'm'))?.[1];
const targetsOf = (text) => [...text.matchAll(/^\s*target\s*=\s*"([^"]+)"/gm)].map((match) => match[1]);
const metafieldsOf = (text) =>
  text
    .split(/^\s*\[\[extensions\.metafields\]\]\s*$/m)
    .slice(1)
    .map((block) => block.split(/^\s*\[/m)[0])
    .map((block) => ({ namespace: field(block, 'namespace'), key: field(block, 'key'), ownerType: field(block, 'owner_type') }));
const manifest = (handle) => {
  const text = toml(handle);
  return { handle, apiVersion: field(text, 'api_version'), metafields: metafieldsOf(text), targets: targetsOf(text) };
};

/** The broken extensions, and how each one is broken. */
const NOT_LOADED = {
  'ui-broken-old-api': /API version 2025-07 is not supported/,
  'ui-broken-no-api-version': /API version \(none\) is not supported/,
  'ui-broken-missing-owner-type': /requires owner_type.*\$app:playground\.no_owner/,
  'ui-broken-bad-owner-type': /owner_type of \$app:playground\.bad_owner must be one of/,
};
const NOT_BUILT = ['ui-broken-not-built'];
const FAILS_AT_RUN = {
  // The error that it registered nothing says what the bundle threw.
  'ui-broken-throws-on-load': /threw while it loaded \(ui-broken-throws-on-load: the bundle throws before it registers its target\.\), and did not register purchase\.checkout\.block\.render/,
  'ui-broken-throws-on-render': /Cannot read properties of undefined/,
  'ui-broken-unregistered-target': /registered: purchase\.checkout\.footer\.render-after/,
};
// Run in the browser only: they wait seconds, or reach for the network.
const NOT_BOOTED = ['ui-broken-slow'];

test('the playground has many UI extensions, each with a toml named after its handle', () => {
  assert.ok(handles.length >= 20, `${handles.length} extensions`);
  for (const handle of handles) {
    const text = toml(handle);
    assert.equal(field(text, 'handle'), handle);
    assert.equal(field(text, 'type'), 'ui_extension');
    assert.ok(field(text, 'description'), `${handle} says what it is for`);
    assert.ok(targetsOf(text).length > 0, `${handle} has targets`);
    assert.equal(existsSync(bundlePath(handle)), !NOT_BUILT.includes(handle), `${handle}: dist/${handle}.js`);
  }
});

test('the working extensions run on a supported API version and render where the page has a place', () => {
  for (const handle of handles) {
    const reason = refusalOf(manifest(handle));
    if (NOT_LOADED[handle]) {
      assert.match(reason ?? '', NOT_LOADED[handle], handle);
      continue;
    }
    assert.equal(reason, null, handle);
    if (handle.startsWith('ui-broken-')) continue;
    for (const target of manifest(handle).targets) assert.ok(RENDERED_TARGETS.includes(target), `${handle}: ${target}`);
  }
  // Both releases are used, and a metafield is declared the way each one wants.
  const versions = new Set(handles.map((handle) => manifest(handle).apiVersion));
  assert.ok(versions.has('2026-10') && versions.has('2027-01'));
  assert.equal(manifest('ui-metafield-writer').metafields[0].ownerType, 'CART');
  assert.equal(manifest('ui-metafield-legacy').metafields[0].ownerType, undefined);
});

test('each bundle says what it shows and registers the targets its toml declares', () => {
  for (const handle of handles.filter((name) => !NOT_BUILT.includes(name))) {
    const source = bundle(handle);
    if (!fromSources(handle)) {
      assert.match(source, /^\/\/ ui-/, `${handle} starts with a comment that says what it shows`);
    }
    const win = sandboxWindow();
    win.run('globalThis.__names = []; globalThis.shopify = { extend: (name) => { globalThis.__names.push(name); } };');
    let thrown = null;
    try {
      win.run(source);
    } catch (error) {
      thrown = error;
    }
    const names = plain(win.context.__names).sort();
    if (handle === 'ui-broken-throws-on-load') {
      assert.match(String(thrown?.message), /throws before it registers/);
      assert.deepEqual(names, []);
    } else if (handle === 'ui-broken-unregistered-target') {
      assert.deepEqual(names, ['purchase.checkout.footer.render-after']);
    } else {
      assert.equal(thrown, null, `${handle}: ${thrown?.message}`);
      assert.deepEqual(names, [...manifest(handle).targets].sort(), handle);
    }
  }
});

/** Boots an extension at a target and waits for it to draw. */
async function boot(handle, target, ctx) {
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url(handle)]: bundle(handle) } });
  // What a browser frame has and the test window lacks: the network is down.
  Object.assign(win.context, {
    fetch: () => Promise.reject(new TypeError('Failed to fetch')),
    AbortController: class {
      signal = {};
      abort() {}
    },
    clearTimeout,
    atob,
  });
  const ext = { capabilities: {}, translations: {} };
  const locales = join(extensions, handle, 'locales');
  if (existsSync(locales)) {
    for (const file of readdirSync(locales)) {
      const language = file.replace(/(\.default)?\.json$/, '');
      ext.translations[language] = JSON.parse(readFileSync(join(locales, file), 'utf8'));
      if (file.includes('.default.')) ext.defaultLocale = language;
    }
  }
  win.receive(init({ handle, target, apiVersion: manifest(handle).apiVersion ?? '2026-10', ctx, ext }));
  for (let i = 0; i < 6; i++) await settle();
  // Storage and the session token wait for the page: answer them.
  for (const request of [...win.posts('storage'), ...win.posts('sessionToken')]) {
    win.receive({ type: 'result', id: request.id, result: request.type === 'sessionToken' ? 'e30.eyJzdWIiOiJ4In0.x' : null });
  }
  for (let i = 0; i < 6; i++) await settle();
  return win;
}

test('each working extension draws at each of its targets without an error', async () => {
  const state = checkoutState();
  const ctxFor = (target) => (/cart-line-item/.test(target) ? { lineId: state.lines[0].id } : {});
  for (const handle of handles) {
    if (NOT_LOADED[handle] || NOT_BUILT.includes(handle) || FAILS_AT_RUN[handle] || NOT_BOOTED.includes(handle)) continue;
    for (const target of manifest(handle).targets.filter((name) => RENDERED_TARGETS.includes(name))) {
      const win = await boot(handle, target, ctxFor(target));
      assert.deepEqual(win.posts('error'), [], `${handle} at ${target}`);
      const drawn = win.document.body.childNodes.length > 0;
      assert.equal(drawn, handle !== 'ui-broken-renders-nothing', `${handle} at ${target} draws something`);
    }
  }
});

test('the broken extensions fail the way their case says', async () => {
  for (const [handle, message] of Object.entries(FAILS_AT_RUN)) {
    const win = await boot(handle, 'purchase.checkout.block.render', {});
    const errors = win.posts('error').map((post) => post.message).join(' | ');
    assert.match(errors, message, handle);
  }
});

test('the interceptors block the order the way their scenarios say', async () => {
  const ask = async (win, canBlockProgress) => {
    win.receive({ type: 'intercept', id: 'i', canBlockProgress });
    for (let i = 0; i < 4; i++) await settle();
    return win.posts('intercepted').at(-1).results;
  };
  const terms = await boot('ui-terms-gate', 'purchase.checkout.actions.render-before', {});
  assert.equal((await ask(terms, true))[0].behavior, 'block', 'the terms are not accepted');
  const limit = await boot('ui-cart-limit-gate', 'purchase.checkout.block.render', {});
  const units = checkoutState().lines.reduce((sum, line) => sum + line.quantity, 0);
  assert.equal((await ask(limit, true))[0].behavior, units > 5 ? 'block' : 'allow');
  const nocap = await boot('ui-intercept-no-capability', 'purchase.checkout.block.render', {});
  assert.equal((await ask(nocap, false))[0].behavior, 'allow', 'no block_progress: the buyer goes on');
});

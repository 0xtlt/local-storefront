// The checkout UI extensions of the playground that lsf builds from their sources, without Node
// (`examples/checkout-playground/extensions/ui-preact*`): the bundle lsf writes draws what the
// bundle Shopify's CLI 5.0.0 builds from the same sources draws
// (`crates/checkout/tests/fixtures/shopify-cli/<handle>.js`), asks the page for the same things,
// and does the same when the buyer acts on it. The two are not the same text: each is minified by
// its own tool.

import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { FakeEvent } from './fake-dom.mjs';
import { assets, checkoutState, init, plain, sandboxWindow, settle } from './harness.mjs';

const SANDBOX = ['sandbox-api.js', 'sandbox-components.js', 'sandbox.js'];
const extensions = join(assets, '..', '..', '..', 'examples', 'checkout-playground', 'extensions');
const shopifyCli = join(assets, '..', 'tests', 'fixtures', 'shopify-cli');

/** Each extension, the target it draws at, and what the buyer does there. */
const CASES = {
  'ui-preact': {
    target: 'purchase.checkout.block.render',
    // Ticks the checkbox, as a buyer: the extension asks the page to set a cart metafield.
    act(win) {
      const checkbox = win.document.body.querySelector('s-checkbox');
      assert.ok(checkbox, 'a checkbox');
      checkbox.checked = true;
      checkbox.dispatchEvent(new FakeEvent('change', { bubbles: true }));
    },
  },
  'ui-preact-typescript': {
    target: 'purchase.checkout.cart-line-list.render-after',
    // Shows the lines: a signal changes, Preact draws them.
    act(win) {
      const button = win.document.body.querySelector('s-button');
      assert.ok(button, 'a button');
      button.click();
    },
  },
};

/** Boots a bundle at a target, as the page does, and waits for it to draw. */
async function boot(handle, target, source) {
  const script = `http://localhost/__lsf/extensions/${handle}/${handle}.js`;
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [script]: source } });
  Object.assign(win.context, {
    fetch: () => Promise.reject(new TypeError('Failed to fetch')),
    AbortController: class {
      signal = {};
      abort() {}
    },
    clearTimeout,
    atob,
  });
  const ext = { capabilities: { api_access: true }, translations: {} };
  const locales = join(extensions, handle, 'locales');
  if (existsSync(locales)) {
    for (const file of readdirSync(locales)) {
      const language = file.replace(/(\.default)?\.json$/, '');
      ext.translations[language] = JSON.parse(readFileSync(join(locales, file), 'utf8'));
      if (file.includes('.default.')) ext.defaultLocale = language;
    }
  }
  win.receive(init({ handle, target, state: checkoutState(), ext }));
  for (let i = 0; i < 6; i++) await settle();
  return win;
}

/** What the extension shows, and what it asked the page, but for the messages that only say it is alive. */
function seen(win) {
  const asked = plain(win.posted)
    .filter((message) => !['ready', 'size', 'resize', 'heartbeat'].includes(message.type))
    .map(({ id, ...message }) => message);
  return { body: win.document.body.outerHTML, asked };
}

for (const [handle, { target, act }] of Object.entries(CASES)) {
  test(`${handle}, built by lsf, does what Shopify's CLI 5.0.0 build of it does`, async () => {
    const ours = await boot(handle, target, readFileSync(join(extensions, handle, 'dist', `${handle}.js`), 'utf8'));
    const theirs = await boot(handle, target, readFileSync(join(shopifyCli, `${handle}.js`), 'utf8'));
    assert.deepEqual(ours.posts('error'), [], 'lsf');
    assert.deepEqual(theirs.posts('error'), [], 'Shopify CLI');
    assert.ok(ours.document.body.childNodes.length > 0, `${handle} draws something`);
    if (handle === 'ui-preact') {
      // The template translates with an element among the replacements.
      assert.match(
        ours.document.body.outerHTML,
        /Welcome to the <s-text type="emphasis">purchase\.checkout\.block\.render<\/s-text> extension!/,
      );
    }
    assert.deepEqual(seen(ours), seen(theirs), 'when it is drawn');

    act(ours);
    act(theirs);
    for (let i = 0; i < 6; i++) await settle();
    assert.deepEqual(seen(ours), seen(theirs), 'after the buyer acted');
    assert.notDeepEqual(seen(ours).body + JSON.stringify(seen(ours).asked), '', 'it did something');
  });
}

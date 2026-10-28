// The frame of an extension (`sandbox.js`): what the bundle throws reaches the page with its
// message and its stack, wherever it throws.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { init, sandboxWindow, settle } from './harness.mjs';

const SANDBOX = ['sandbox-api.js', 'sandbox-components.js', 'sandbox.js'];
const TARGET = 'purchase.checkout.block.render';
const url = (handle) => `http://localhost/__lsf/extensions/${handle}/${handle}.js`;

async function boot(handle, source) {
  const win = sandboxWindow({ scripts: SANDBOX, bundles: { [url(handle)]: source } });
  win.receive(init({ handle, target: TARGET }));
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
  assert.match(error.message, /^Could not load http:\/\/localhost\/__lsf\/extensions\/gone\/gone\.js\. Run `shopify app build`/);
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

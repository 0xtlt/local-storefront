// The script of the home page of `lsf checkout`: the filter of the scenarios, the check of the
// Functions it asks for, and the load of each checkout UI extension in a frame of the sandbox.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { FakeDocument } from './fake-dom.mjs';
import { init as sandboxInit, sandboxWindow } from './harness.mjs';
import { applyFilter, countText, markScenarios, openLink, probe, reorder, richText, rowMatches, sortRows, start, verdict } from '../home.js';

const SANDBOX = ['sandbox-api.js', 'sandbox-components.js', 'sandbox.js'];

const row = (search, status = 'ok', uses = 'functions ui') => ({ search, status, uses: uses.split(' ') });

test('a search keeps the rows that have every word', () => {
  const tees = row('three tees in french tees tees.json volume-discount gift-message');
  assert.equal(rowMatches(tees, { query: '' }), true);
  assert.equal(rowMatches(tees, { query: 'Tees' }), true);
  assert.equal(rowMatches(tees, { query: 'french volume' }), true);
  assert.equal(rowMatches(tees, { query: 'french  shop' }), false);
  // A handle is a word like the others.
  assert.equal(rowMatches(tees, { query: 'gift-message' }), true);
});

test('the chips keep the rows with problems, or that use Functions or UI extensions', () => {
  const ok = row('a', 'ok', 'functions');
  const warning = row('b', 'warning', 'ui');
  const error = row('c', 'error', '');
  const problems = { chip: 'problems' };
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, problems)), [false, true, true]);
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, { chip: 'functions' })), [true, false, false]);
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, { chip: 'ui' })), [false, true, false]);
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, { chip: 'all' })), [true, true, true]);
  // A search and a chip together.
  assert.equal(rowMatches(warning, { query: 'b', chip: 'problems' }), true);
  assert.equal(rowMatches(warning, { query: 'a', chip: 'problems' }), false);
});

test('the count says how many rows are shown', () => {
  assert.equal(countText(82, 82), '82 scenarios');
  assert.equal(countText(3, 82), '3 of 82 scenarios shown');
  assert.equal(countText(1, 1), '1 scenario');
});

/** A list of rows as the page has them: elements with `data-*` and an id. */
function fakeList(rows) {
  const children = rows.map(([id, search, status, uses]) => ({ id, hidden: false, dataset: { scenario: id, search, status, uses } }));
  return {
    children,
    querySelectorAll: () => [...children],
    querySelector: (selector) => children.find((child) => selector === `[id="${child.id}"]`) ?? null,
    appendChild(child) {
      children.splice(children.indexOf(child), 1);
      children.push(child);
    },
  };
}

test('the filter hides the rows that do not match and counts the others', () => {
  const list = fakeList([
    ['a', 'guest tees', 'ok', 'functions'],
    ['b', 'b2b tees', 'error', 'ui'],
    ['c', 'abroad', 'warning', 'functions ui'],
  ]);
  assert.deepEqual(applyFilter(list, { query: 'tees', chip: 'all' }), { shown: 2, total: 3 });
  assert.deepEqual(list.children.map((child) => child.hidden), [false, false, true]);
  assert.deepEqual(applyFilter(list, { query: '', chip: 'problems' }), { shown: 2, total: 3 });
  assert.deepEqual(list.children.map((child) => child.hidden), [true, false, false]);
  assert.deepEqual(applyFilter(list, { query: '', chip: 'all' }), { shown: 3, total: 3 });
});

test('the rows take the order the check gives them', () => {
  const list = fakeList([['scenario-a'], ['scenario-b'], ['scenario-c']]);
  reorder(list, ['scenario-c', 'scenario-a', 'scenario-gone']);
  assert.deepEqual(list.children.map((child) => child.id), ['scenario-b', 'scenario-c', 'scenario-a']);
});

/** A page with the problems still to check, and the window that answers the check. */
function fakePage(answer) {
  const line = { textContent: 'Running…', classList: new Set() };
  line.classList.add = line.classList.add.bind(line.classList);
  const section = {
    dataset: { check: 'pending', checkUrl: '/__lsf/checkouts/check' },
    outerHTML: '<section>pending</section>',
    querySelector: (selector) => (selector === '[data-check-line]' ? line : null),
  };
  const rows = { 'scenario-crash': { outerHTML: '<li>before</li>' } };
  const document = {
    querySelector: (selector) => (selector === '[data-section="problems"]' ? section : null),
    getElementById: (id) => rows[id] ?? null,
    querySelectorAll: () => [],
  };
  const asked = [];
  const window = {
    AbortController,
    setTimeout,
    clearTimeout,
    fetch: async (url) => {
      asked.push(url);
      if (answer instanceof Error) throw answer;
      return { ok: true, json: async () => answer };
    },
  };
  return { document, window, section, line, rows, asked };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 10));

test('what the check found takes the place of the problems and of the rows that fail', async () => {
  const page = fakePage({
    state: 'done',
    problems: '<section data-check="done">found</section>',
    rows: { 'scenario-crash': '<li data-status="error">after</li>', 'scenario-gone': '<li></li>' },
    order: ['scenario-crash'],
  });
  start(page.document, page.window);
  await settle();
  assert.deepEqual(page.asked, ['/__lsf/checkouts/check']);
  assert.equal(page.section.outerHTML, '<section data-check="done">found</section>');
  assert.equal(page.rows['scenario-crash'].outerHTML, '<li data-status="error">after</li>');
});

test('a check that fails says so, and leaves the page as it is', async () => {
  const page = fakePage(new Error('connection refused'));
  start(page.document, page.window);
  await settle();
  assert.equal(page.section.outerHTML, '<section>pending</section>');
  assert.match(page.line.textContent, /Could not check the Functions of the scenarios: connection refused/);
  assert.ok(page.line.classList.has('is-error'));

  const refused = fakePage({ state: 'error', error: 'the app cannot be read' });
  start(refused.document, refused.window);
  await settle();
  assert.match(refused.line.textContent, /the app cannot be read/);
});

test('a page whose check already ran asks for nothing', async () => {
  const page = fakePage({});
  page.section.dataset.check = 'done';
  start(page.document, page.window);
  await settle();
  assert.deepEqual(page.asked, []);
});

// --- The load of the checkout UI extensions ---------------------------------------------------

const TARGET = 'purchase.checkout.block.render';
const BUNDLE_URL = 'http://127.0.0.1:9292/__lsf/extensions/probed/probed.js';

/**
 * A page that loads one bundle in a frame. `behave(frame, send)` plays the frame: `send(message)`
 * posts a message of it to the page, and `frame.onMessage` hears what the page posts to it.
 */
function probePage(behave) {
  const listeners = new Set();
  const frames = [];
  const window = {
    setTimeout,
    clearTimeout,
    addEventListener: (type, listener) => type === 'message' && listeners.add(listener),
    removeEventListener: (type, listener) => listeners.delete(listener),
  };
  const document = {
    baseURI: 'http://127.0.0.1:9292/',
    createElement: () => {
      const frame = {
        attributes: {},
        style: {},
        attached: false,
        setAttribute(name, value) {
          this.attributes[name] = value;
        },
        remove() {
          this.attached = false;
        },
      };
      frame.contentWindow = { postMessage: (message) => frame.onMessage?.(message) };
      return frame;
    },
    body: {
      appendChild(frame) {
        frame.attached = true;
        frames.push(frame);
        // As a browser does: what a frame posts reaches the page later.
        const send = (message) =>
          setImmediate(() => {
            for (const listener of [...listeners]) listener({ source: frame.contentWindow, data: { lsf: true, ...message } });
          });
        behave(frame, send);
        return frame;
      },
    },
  };
  const row = { id: 'ui-probed', dataset: { uiExtension: 'probed', targets: TARGET, probe: '/__lsf/extensions/probed/probed.js', api: '2026-10' } };
  return { document, window, frames, listeners, row };
}

/** A page whose frame runs the real sandbox, with `bundles` behind their URLs. */
function sandboxedProbe(bundles) {
  let win = null;
  const page = probePage((frame, send) => {
    win = sandboxWindow({ scripts: SANDBOX, bundles, onPost: send });
    frame.onMessage = (message) => win.receive(message);
  });
  return { ...page, sandbox: () => win };
}

test('a bundle that registers its targets is fine, and its frame goes away', async () => {
  const page = sandboxedProbe({ [BUNDLE_URL]: `shopify.extend(${JSON.stringify(TARGET)}, () => {});` });
  const result = await probe(page.document, page.window, 'sandbox.html', page.row, 2000);
  assert.deepEqual(result.registered, [TARGET]);
  assert.equal(result.loaded, true);
  assert.deepEqual(result.errors, []);
  assert.equal(verdict(result.targets, result.registered, result.errors, result.loaded), null);
  assert.equal(page.frames.length, 1);
  assert.equal(page.frames[0].attached, false, 'the frame is removed');
  assert.equal(page.listeners.size, 0, 'the page stops listening');
  // The probe only loads: the target was not rendered.
  assert.deepEqual(page.sandbox().posts('probed'), [{ lsf: true, type: 'probed', loaded: true, targets: [TARGET] }]);
});

test('a bundle that does not load is an error that says why, without waiting for the timeout', async () => {
  const page = sandboxedProbe({});
  const started = Date.now();
  const result = await probe(page.document, page.window, 'sandbox.html', page.row, 2000);
  assert.ok(Date.now() - started < 1000, 'answered at once');
  assert.equal(result.loaded, false);
  assert.deepEqual(result.registered, []);
  assert.match(result.errors[0], /^Could not load http:\/\/127\.0\.0\.1:9292\/__lsf\/extensions\/probed\/probed\.js/);
  const problem = verdict(result.targets, result.registered, result.errors, result.loaded);
  assert.equal(problem.severity, 'error');
  assert.match(problem.message, /^its bundle does not load: Could not load .*`shopify app build`/);
  assert.equal(page.frames[0].attached, false);
  assert.equal(page.listeners.size, 0);
});

test('a frame that reported an error and never answers keeps the error', async () => {
  // As when the bundle goes away between the page and its load, in a sandbox that does not answer.
  const page = probePage((frame, send) => {
    frame.onMessage = (message) => {
      if (message.type === 'init') send({ type: 'error', message: 'Could not load the bundle' });
    };
    send({ type: 'ready' });
  });
  const result = await probe(page.document, page.window, 'sandbox.html', page.row, 50);
  assert.equal(result.registered, null);
  assert.deepEqual(result.errors, ['Could not load the bundle']);
  const problem = verdict(result.targets, result.registered, result.errors, result.loaded);
  assert.equal(problem.severity, 'error');
  assert.match(problem.message, /Could not load the bundle/);
  assert.equal(page.frames[0].attached, false, 'the frame is removed after the timeout');
  assert.equal(page.listeners.size, 0);

  // What the browser hides of an error ("Script error.") is still an error.
  assert.equal(verdict([TARGET], null, ['Script error.']).severity, 'error');
});

test('a frame that says nothing in time is a warning that cannot tell', async () => {
  const page = probePage(() => {});
  const result = await probe(page.document, page.window, 'sandbox.html', page.row, 50);
  assert.equal(result.registered, null);
  assert.deepEqual(result.errors, []);
  const problem = verdict(result.targets, result.registered, result.errors, result.loaded);
  assert.equal(problem.severity, 'warning');
  assert.match(problem.message, /^could not tell whether its bundle loads/);
  assert.equal(page.frames[0].attached, false);
  assert.equal(page.listeners.size, 0);
});

test('the frame is removed when it cannot even be added, and a late answer changes nothing', async () => {
  const page = probePage(() => {
    throw new Error('no body');
  });
  const result = await probe(page.document, page.window, 'sandbox.html', page.row, 50);
  assert.equal(result.loaded, false);
  assert.deepEqual(result.errors, ['no body']);
  assert.equal(page.frames[0].attached, false);
  assert.equal(page.listeners.size, 0);

  // An answer after the verdict is not heard: the listener is gone.
  let send;
  const late = probePage((frame, post) => {
    send = post;
  });
  const timedOut = await probe(late.document, late.window, 'sandbox.html', late.row, 20);
  send({ type: 'probed', loaded: true, targets: [TARGET] });
  await settle();
  assert.equal(timedOut.registered, null);
});

test('what a bundle registers, against what its extension declares', () => {
  assert.equal(verdict([TARGET], [TARGET], []), null);
  assert.equal(verdict([TARGET], [TARGET], ['Script error.']).severity, 'warning');
  assert.match(verdict([TARGET, 'b'], [TARGET], []).message, /does not register b \(it registers purchase/);
  assert.match(verdict([TARGET], [], []).message, /registers no target: `shopify\.extend` is never called/);
  assert.match(verdict([TARGET], [], ['boom']).message, /throws while its bundle loads \(boom\): it registers no target/);
});

test('outside the home page, the sandbox renders the target and says nothing of a probe', async () => {
  let rendered = false;
  const win = sandboxWindow({
    scripts: SANDBOX,
    bundles: { [BUNDLE_URL]: `shopify.extend(${JSON.stringify(TARGET)}, () => { globalThis.__rendered = true; });` },
  });
  win.receive({ ...sandboxInit({ handle: 'probed', target: TARGET }), script: BUNDLE_URL });
  for (let i = 0; i < 4; i++) await settle();
  rendered = win.context.__rendered === true;
  assert.equal(rendered, true);
  assert.deepEqual(win.posts('probed'), []);
  assert.deepEqual(win.posts('error'), []);

  // A bundle that does not load is an error, and still no probe answer.
  const missing = sandboxWindow({ scripts: SANDBOX });
  missing.receive({ ...sandboxInit({ handle: 'probed', target: TARGET }), script: BUNDLE_URL });
  for (let i = 0; i < 4; i++) await settle();
  assert.match(missing.posts('error')[0].message, /^Could not load /);
  assert.deepEqual(missing.posts('probed'), []);
});

// --- Messages, and the scenarios of a broken bundle ------------------------------------------

test('a message shows what is between backticks as code, and never as HTML', () => {
  const document = new FakeDocument();
  const nodes = richText(document, '`purchase.cart-transform.run` is a <b>legacy</b> target, `x');
  assert.equal(nodes[0].localName, 'code');
  assert.equal(nodes[0].textContent, 'purchase.cart-transform.run');
  assert.deepEqual(nodes.slice(1), [' is a <b>legacy</b> target, ', '`x']);
});

test('a scenario that runs a broken bundle becomes a warning, and moves up', () => {
  const document = new FakeDocument();
  const list = fakeList([
    ['a', '', 'ok', ''],
    ['b', '', 'ok', ''],
    ['c', '', 'error', ''],
    ['d', '', 'warning', ''],
  ]);
  const [a, b, c] = list.children;
  const ui = { a: 'other', b: 'probed other', c: 'probed', d: '' };
  for (const row of list.children) {
    row.dataset.uiExtensions = ui[row.id];
    row.dot = { replaceWith: (node) => (row.dot = node) };
    row.querySelector = (selector) => (selector === '.lsf-dot' ? row.dot : null);
  }
  const page = { querySelectorAll: (selector) => (selector === '[data-scenario]' ? list.children : []), createElement: (tag) => document.createElement(tag) };
  markScenarios(page, 'probed');
  assert.equal(a.dataset.status, 'ok');
  assert.equal(b.dataset.status, 'warning');
  assert.equal(b.dot.className, 'lsf-dot is-warning');
  assert.equal(c.dataset.status, 'error', 'an error stays one');
  sortRows(list);
  assert.deepEqual(list.children.map((row) => row.id), ['c', 'b', 'd', 'a']);
});

test('a problem the page finds in a bundle opens the checkout of the first scenario that runs it', () => {
  const document = new FakeDocument();
  const href = '/checkout?scenario=ui-broken-%3Cloader%3E';
  const link = openLink(document, { id: 'ui-probed', dataset: { open: href, openIn: 'ui-broken-<loader>' } });
  assert.equal(link.getAttribute('href'), href);
  assert.equal(link.getAttribute('class'), 'lsf-hp-where lsf-hp-open');
  assert.match(link.textContent, /^Open in checkout ui-broken-<loader>$/);
  assert.equal(link.querySelector('.lsf-hp-in').textContent, 'ui-broken-<loader>', 'the name is text');
  assert.equal(link.getAttribute('title'), 'Opens a fresh checkout of the scenario ui-broken-<loader>');

  // No scenario runs it: the store's cart.
  const cart = openLink(document, { id: 'ui-probed', dataset: { open: '/checkout', openIn: '' } });
  assert.equal(cart.getAttribute('href'), '/checkout');
  assert.equal(cart.querySelector('.lsf-hp-in').textContent, "store's cart");
  // A row without a checkout to open: its row on the page.
  assert.equal(openLink(document, { id: 'ui-probed', dataset: {} }).getAttribute('href'), '#ui-probed');
});

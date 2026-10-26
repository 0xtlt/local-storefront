// What the sandbox tests share: a page for the sandbox scripts (classic scripts, run in a
// `vm` context the way a browser would run them), a state of the checkout and the fixture
// extensions. Values made inside the context have other prototypes than the test's: compare
// them through `plain()`.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { FakeDocument, FakeElement, FakeEvent, settle } from './fake-dom.mjs';
import { withCurrency } from '../format.js';
import { createCheckout } from '../preview/mock-engine.mjs';

export const assets = join(dirname(fileURLToPath(import.meta.url)), '..');
export const fixtures = join(assets, '..', 'tests', 'fixtures', 'app', 'extensions');
export const read = (name) => readFileSync(join(assets, name), 'utf8');
export const plain = (value) => JSON.parse(JSON.stringify(value ?? null));
export { settle };

export const sample = JSON.parse(read('preview/sample-state.json'));

/** The checkout the sample starts with, as the mock engine keeps it. */
export const engineCheckout = () => createCheckout(sample, { origin: 'http://127.0.0.1:9293' });

/** What the engine's state looks like to the page: no bookkeeping, the currency written in. */
export function pageState({ selection, ...state }, overrides = {}) {
  return withCurrency({ ...state, ...overrides });
}

/** The checkout the sample starts with, as the page sees it. */
export function checkoutState(overrides = {}) {
  return pageState(engineCheckout(), overrides);
}

/** The bundle `shopify app build` left for `handle`. */
export const bundle = (handle) => readFileSync(join(fixtures, handle, 'dist', `${handle}.js`), 'utf8');

/**
 * A browser window for the sandbox scripts. `scripts` are the files of the sandbox to run, in
 * order; `bundles` maps the URL of an extension script to its source. Returns the window and
 * what it posted to the page; `receive(message)` plays a message of the page.
 */
export function sandboxWindow({ scripts = ['sandbox-api.js'], bundles = {}, customElements } = {}) {
  const posted = [];
  const listeners = new Map();
  const document = new FakeDocument();
  const parent = { postMessage: (message) => posted.push(JSON.parse(JSON.stringify(message))) };
  const context = vm.createContext({
    document,
    parent,
    console,
    Intl,
    Event: FakeEvent,
    Promise,
    setTimeout,
    JSON,
    addEventListener: (type, listener) => listeners.set(type, [...(listeners.get(type) ?? []), listener]),
    ResizeObserver: class {
      observe() {}
    },
  });
  context.globalThis = context;
  if (customElements) {
    context.customElements = customElements;
    context.HTMLElement = class extends FakeElement {
      constructor() {
        super(document, 'x-element');
      }
      // A shadow root that hands out one stand-in element for each selector asked.
      attachShadow() {
        const parts = new Map();
        return {
          set innerHTML(value) {
            this.html = value;
          },
          querySelector: (selector) => {
            if (!parts.has(selector)) parts.set(selector, document.createElement('div'));
            return parts.get(selector);
          },
        };
      }
    };
  }

  // A script put into the head runs the bundle behind its `src`, then says it loaded.
  const append = document.head.appendChild.bind(document.head);
  document.head.appendChild = (node) => {
    append(node);
    if (node.localName !== 'script') return node;
    const source = bundles[node.src];
    setImmediate(() => {
      if (source === undefined) return node.onerror?.();
      vm.runInContext(source, context, { filename: node.src });
      return node.onload?.();
    });
    return node;
  };
  Object.defineProperty(FakeElement.prototype, 'src', {
    configurable: true,
    get() {
      return this._src;
    },
    set(value) {
      this._src = value;
    },
  });

  for (const script of scripts) vm.runInContext(read(script), context, { filename: script });
  return {
    context,
    document,
    posted,
    /** Plays a message of the page. */
    receive(message, source = parent) {
      for (const listener of listeners.get('message') ?? []) listener({ source, data: { lsf: true, ...message } });
    },
    posts: (type) => posted.filter((message) => message.type === type),
    run: (source) => vm.runInContext(source, context),
  };
}

/** The `init` message of the page for an extension on a target. */
export function init({ handle, target, apiVersion = '2025-10', state = checkoutState(), ctx = {}, ext = {}, names }) {
  return {
    type: 'init',
    handle,
    target,
    names: names ?? [target],
    script: `http://localhost/__lsf/extensions/${handle}/${handle}.js`,
    apiVersion,
    mode: apiVersion < '2025-10' ? 'legacy' : 'modern',
    ctx,
    variant: 'standard',
    ext: { settings: {}, capabilities: { api_access: true, block_progress: true }, translations: {}, ...ext },
    state,
  };
}

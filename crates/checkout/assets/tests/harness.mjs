// What the tests share: a page for the sandbox scripts (classic scripts, run in a `vm` context
// the way a browser would run them), a state of the checkout, the fixture extensions, and the
// checkout page on the mock engine. Values made inside the context have other prototypes than
// the test's: compare them through `plain()`.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import { FakeDocument, FakeElement, FakeEvent, settle } from './fake-dom.mjs';
import { withCurrency } from '../format.js';
import { startCheckout } from '../checkout.js';
import { complete, createCheckout, update } from '../preview/mock-engine.mjs';

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

const publicState = ({ selection, ...rest }) => rest;

/**
 * A checkout page on a checkout kept by the mock engine; `requests` is what it was asked. `start`
 * is operations applied before the page loads (a login made elsewhere), `mutate` changes the
 * engine's checkout first, `hold(url, body)` delays a request, and `conflict(engine, operations)`
 * returns the checkout someone else made (a 409 reply) or nothing.
 */
export function checkoutPage({ start = [], variant = 'standard', mutate, hold, conflict, href = 'http://localhost/checkouts/cn/preview', window: more = {} } = {}) {
  let engine = engineCheckout();
  engine.variant = variant;
  if (start.length) engine = update(engine, start).state;
  if (mutate) mutate(engine);
  const document = new FakeDocument();
  const root = document.createElement('div');
  document.body.appendChild(root);
  const requests = [];
  const location = { href, assigned: [], assign(url) { this.assigned.push(url); } };
  const window = { addEventListener() {}, removeEventListener() {}, open() {}, ...more };
  const reply = (status, body) => ({ ok: status < 400, status, json: async () => body });
  const fetch = async (url, init) => {
    const body = JSON.parse(init.body);
    requests.push({ url, body });
    await hold?.(url, body);
    if (url.endsWith('/update')) {
      // Someone else changed the checkout: nothing is applied and the reply is the checkout now.
      const other = conflict?.(engine, body.operations);
      if (other) {
        engine = other;
        return reply(409, publicState(engine));
      }
      const result = update(engine, body.operations);
      if (result.error) return reply(400, { error: result.error });
      engine = result.state;
      return reply(200, publicState(engine));
    }
    if (url.endsWith('/complete')) {
      const result = complete(engine);
      if (result.errors) {
        engine = result.errors;
        return reply(422, publicState(engine));
      }
      engine = result.state;
      return reply(200, { redirect: 'http://localhost/checkouts/cn/preview/thank-you' });
    }
    return reply(404, { error: 'no such route' });
  };
  const sandboxUrl = 'http://localhost/__lsf/checkout/assets/sandbox.html';
  const checkout = startCheckout({ root, initial: publicState(engine), document, window, location, fetch, storage: undefined, sandboxUrl });
  const query = (selector) => root.querySelector(selector);
  return { checkout, document, window, root, requests, location, query, text: (selector) => query(selector)?.textContent, operations: () => requests.filter((r) => r.url.endsWith('/update')).flatMap((r) => r.body.operations), engine: () => engine };
}

/** The bundle `shopify app build` left for `handle`. */
export const bundle = (handle) => readFileSync(join(fixtures, handle, 'dist', `${handle}.js`), 'utf8');

/**
 * A browser window for the sandbox scripts. `scripts` are the files of the sandbox to run, in
 * order; `bundles` maps the URL of an extension script to its source, which `receive` puts in an
 * `init` that has none, as the page does; `onPost` hears each message the frame posts to the
 * page. Returns the window and what it posted to the page; `receive(message)` plays a message of
 * the page.
 */
export function sandboxWindow({ scripts = ['sandbox-api.js'], bundles = {}, customElements, onPost } = {}) {
  const posted = [];
  const listeners = new Map();
  const document = new FakeDocument();
  const parent = {
    postMessage: (message) => {
      const copy = JSON.parse(JSON.stringify(message));
      posted.push(copy);
      onPost?.(copy);
    },
  };
  // `blob:` URLs, as the frame makes them for the text of a bundle.
  const blobs = new Map();
  class Blob {
    constructor(parts) {
      this.text = parts.join('');
    }
  }
  const URL = {
    createObjectURL: (blob) => {
      const url = `blob:null/${blobs.size + 1}`;
      blobs.set(url, blob.text);
      return url;
    },
    revokeObjectURL: (url) => blobs.delete(url),
  };
  const context = vm.createContext({
    Blob,
    URL,
    document,
    parent,
    console,
    Intl,
    Event: FakeEvent,
    Promise,
    setTimeout,
    queueMicrotask,
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

  // A script put into the head runs the blob behind its `src`, then says it loaded. As in a
  // browser, what the bundle throws goes to the `error` listeners of the window, and the script
  // still loaded. Stacks name the script by its `sourceURL`.
  const append = document.head.appendChild.bind(document.head);
  document.head.appendChild = (node) => {
    append(node);
    if (node.localName !== 'script') return node;
    const source = blobs.get(node.src);
    setImmediate(() => {
      if (source === undefined) return node.onerror?.();
      const filename = /\/\/# sourceURL=(.*)$/.exec(source)?.[1] ?? node.src;
      try {
        vm.runInContext(source, context, { filename });
      } catch (error) {
        for (const listener of listeners.get('error') ?? []) listener({ message: `Uncaught ${error}`, error });
      }
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
    /** Plays an event of the window (`error`, `unhandledrejection`). */
    fire(type, event) {
      for (const listener of listeners.get(type) ?? []) listener(event);
    },
    /** Plays a message of the page. */
    receive(message, source = parent) {
      const data = { lsf: true, ...message };
      if (data.type === 'init' && !('source' in data)) data.source = bundles[data.script] ?? null;
      for (const listener of listeners.get('message') ?? []) listener({ source, data });
    },
    posts: (type) => posted.filter((message) => message.type === type),
    run: (source) => vm.runInContext(source, context),
  };
}

/** The `init` message of the page for an extension on a target. */
export function init({ handle, target, apiVersion = '2026-10', state = checkoutState(), ctx = {}, ext = {} }) {
  return {
    type: 'init',
    handle,
    target,
    script: `http://localhost/__lsf/extensions/${handle}/${handle}.js`,
    apiVersion,
    ctx,
    variant: 'standard',
    ext: { settings: {}, capabilities: { api_access: true, block_progress: true }, translations: {}, ...ext },
    state,
  };
}

// A page just big enough to run the platform scripts in Node: a document that dispatches
// events and holds a cookie, and a cart server that behaves like the cart endpoints of lsf.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = (name) => readFileSync(join(here, '..', name), 'utf8');

/** A cart server with two variants: 101 (always available) and 202 (one in stock). */
export function cartServer() {
  const variants = { 101: { price: 2999, stock: Infinity }, 202: { price: 1000, stock: 1 } };
  const state = { lines: [], note: null, attributes: {}, requests: [], failing: false };

  const cart = () => ({
    token: 'local-token',
    note: state.note,
    attributes: state.attributes,
    currency: 'CAD',
    item_count: state.lines.reduce((sum, line) => sum + line.quantity, 0),
    total_price: state.lines.reduce((sum, line) => sum + line.quantity * variants[line.id].price, 0),
    items: state.lines.map((line) => ({
      key: line.key,
      id: line.id,
      quantity: line.quantity,
      final_line_price: line.quantity * variants[line.id].price,
    })),
  });

  const json = (status, body) => ({ ok: status < 400, status, json: async () => body });

  const handle = (path, body) => {
    if (path === '/cart.js') return json(200, cart());
    if (path === '/cart/add.js') {
      const variant = variants[body.id];
      if (!variant) return json(404, { description: 'Cannot find variant' });
      const existing = state.lines.find((line) => line.id === body.id);
      const quantity = (existing?.quantity ?? 0) + body.quantity;
      if (quantity > variant.stock) {
        return json(422, { description: `All ${variant.stock} are in your cart.` });
      }
      if (existing) existing.quantity = quantity;
      else state.lines.push({ key: `${body.id}:key`, id: body.id, quantity: body.quantity });
      return json(200, {});
    }
    if (path === '/cart/change.js') {
      const line = state.lines.find((candidate) => candidate.key === body.id);
      if (!line) return json(400, { description: 'no valid id or line parameter' });
      if (body.quantity <= 0) state.lines.splice(state.lines.indexOf(line), 1);
      else line.quantity = body.quantity;
      return json(200, cart());
    }
    if (path === '/cart/update.js') {
      if (body.note !== undefined) state.note = body.note;
      for (const [key, value] of Object.entries(body.attributes ?? {})) {
        if (value === null) delete state.attributes[key];
        else state.attributes[key] = value;
      }
      return json(200, cart());
    }
    return json(404, {});
  };

  const fetch = async (url, init = {}) => {
    if (init.signal?.aborted) throw new DOMException('The operation was aborted.', 'AbortError');
    const body = init.body ? JSON.parse(init.body) : undefined;
    state.requests.push({ url, method: init.method ?? 'GET', body });
    if (state.failing) throw new TypeError('Failed to fetch');
    // A locale prefix does not change what the endpoint does.
    return handle(url.split('?')[0].replace(/^\/fr(?=\/)/, ''), body);
  };

  return { state, fetch };
}

/** A fresh page with the given platform scripts loaded. */
export function page({ scripts, cookie = '', shopify = {}, elements = {}, fetch } = {}) {
  const server = cartServer();
  const document = new EventTarget();
  document.cookie = cookie;
  document.querySelector = (selector) => elements[selector] ?? null;
  document.querySelectorAll = () => [];
  document.getElementById = () => null;

  const window = {
    Shopify: { routes: { root: '/' }, ...shopify },
    location: {
      href: 'http://shop.test/',
      reloads: 0,
      reload() {
        this.reloads += 1;
      },
    },
    customElements: { get: () => undefined },
    localStorage: (() => {
      const items = new Map();
      return {
        getItem: (key) => (items.has(key) ? items.get(key) : null),
        setItem: (key, value) => items.set(key, String(value)),
      };
    })(),
  };
  window.window = window;
  window.document = document;

  // Run in this realm, with the page's globals passed in, so that what the scripts return
  // can be compared with plain objects.
  for (const name of scripts) {
    new Function('window', 'document', 'fetch', source(name))(window, document, fetch ?? server.fetch);
  }

  /** Every event of the given type dispatched on the document from now on. */
  const record = (type) => {
    const events = [];
    document.addEventListener(type, (event) => events.push(event));
    return events;
  };

  return { window, document, server, record, Shopify: window.Shopify };
}

/** Lets timers scheduled with `setTimeout(…, 0)` run. */
export const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

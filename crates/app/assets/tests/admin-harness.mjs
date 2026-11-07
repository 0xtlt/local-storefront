// What the tests of lsf's admin share: the app's frame with lsf's App Bridge (a classic script,
// run in a `vm` context the way a browser runs it), and the admin page around it, on the fake DOM.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import vm from 'node:vm';
import { FakeDocument, FakeElement, FakeEvent, settle } from './fake-dom.mjs';
import { assets, plain } from './harness.mjs';

export { plain, settle };
export const ADMIN = 'http://127.0.0.1:9292';
export const APP = 'http://localhost:3000';
export const LAUNCH = `${APP}/app?embedded=1&host=YWRtaW4uc2hvcGlmeS5jb20vc3RvcmUvbG9jYWwtc3VwcGx5LWNv&id_token=abc&locale=fr&shop=local-supply-co.myshopify.com`;
const appBridge = readFileSync(join(assets, 'admin', 'app-bridge.js'), 'utf8');

// Every element of a browser can have a shadow root (`s-modal` without Polaris).
FakeElement.prototype.attachShadow ??= function () {
  const root = new FakeElement(this.ownerDocument, '#shadow-root');
  this.shadowRoot = root;
  return root;
};

// `element.click()` in a frame goes through the window's listeners, as in a browser.
const frames = new WeakMap();
const ownClick = FakeElement.prototype.click;
FakeElement.prototype.click = function () {
  const frame = frames.get(this.ownerDocument);
  return frame ? !frame.click(this).defaultPrevented : ownClick.call(this);
};

/** Waits for the timers the code under test set (a `setTimeout(…, 0)` may come after
 * `setImmediate`). */
export const tick = () => new Promise((resolve) => setTimeout(resolve, 5));

class FakeCustomEvent extends FakeEvent {
  constructor(type, init = {}) {
    super(type, init);
    this.detail = init.detail ?? null;
  }
}

/** A `FormData` of the fake DOM: the named `input`s of a form, checkboxes when checked. */
class FakeFormData {
  constructor(form) {
    this.entries = form
      .querySelectorAll('input')
      .filter((input) => input.getAttribute('name') && (input.getAttribute('type') !== 'checkbox' || input.checked))
      .map((input) => [input.getAttribute('name'), input.value ?? input.getAttribute('value') ?? '']);
  }
  [Symbol.iterator]() {
    return this.entries[Symbol.iterator]();
  }
}

/**
 * The app's frame: a page at `href` with lsf's App Bridge loaded from `scriptSrc`, inside lsf's
 * admin (`framed`) or alone. `build(document)` puts the page's elements in first (before the
 * script runs, as the parser would have). Returns the window, what the frame posted to its parent,
 * and helpers to play the admin.
 */
export function appFrame({
  href = LAUNCH,
  scriptSrc = `${ADMIN}/__lsf/admin/app-bridge.js`,
  framed = true,
  build,
  fetch: nativeFetch,
  session = new Map(),
  window: more = {},
} = {}) {
  const document = new FakeDocument();
  const listeners = [];
  const posted = [];
  const errors = [];
  const warnings = [];
  const url = new URL(href);
  const location = {
    get href() {
      return url.href;
    },
    get origin() {
      return url.origin;
    },
    get pathname() {
      return url.pathname;
    },
    get search() {
      return url.search;
    },
    get hash() {
      return url.hash;
    },
    assigned: [],
    assign(to) {
      this.assigned.push(String(to));
    },
    replace(to) {
      this.assigned.push(String(to));
    },
  };
  const history = {
    pushState(_state, _title, to) {
      if (to) url.href = new URL(to, url.href).href;
    },
    replaceState(_state, _title, to) {
      if (to) url.href = new URL(to, url.href).href;
    },
  };
  const parent = {
    postMessage(message, targetOrigin) {
      posted.push({ message: JSON.parse(JSON.stringify(message)), targetOrigin });
    },
  };
  // Custom elements: a class per name, and `document.createElement` makes one.
  const registry = new Map();
  let constructing = null;
  class HTMLElement extends FakeElement {
    constructor() {
      super(document, constructing ?? 'x-element');
    }
  }
  const customElements = {
    // As a browser does, the elements already in the page are upgraded.
    define(name, constructor) {
      registry.set(name, constructor);
      const walk = (node) => {
        for (const child of node.childNodes) {
          if (child.nodeType !== 1) continue;
          if (child.localName === name) Object.setPrototypeOf(child, constructor.prototype);
          walk(child);
        }
      };
      walk(document);
    },
    get: (name) => registry.get(name),
  };
  document.elementFactory = (tag) => {
    const constructor = registry.get(tag);
    if (!constructor) return null;
    return () => {
      constructing = tag;
      try {
        return new constructor();
      } finally {
        constructing = null;
      }
    };
  };
  const observers = [];
  class MutationObserver {
    constructor(callback) {
      this.callback = callback;
      observers.push(this);
    }
    observe() {}
  }
  const opened = [];
  const window = {
    document,
    location,
    history,
    parent,
    name: '',
    console: {
      ...console,
      error: (...args) => errors.push(args.map(String).join(' ')),
      warn: (...args) => warnings.push(args.map(String).join(' ')),
    },
    sessionStorage: {
      getItem: (key) => session.get(key) ?? null,
      setItem: (key, value) => session.set(key, value),
    },
    addEventListener(type, listener, capture = false) {
      listeners.push({ type, listener, capture: Boolean(capture) });
    },
    removeEventListener(type, listener) {
      const at = listeners.findIndex((entry) => entry.type === type && entry.listener === listener);
      if (at >= 0) listeners.splice(at, 1);
    },
    open(to, target) {
      opened.push({ url: to, target });
      return null;
    },
    // The waits of a frame no admin answers do not keep the tests running.
    setTimeout: (callback, ms) => {
      const timer = setTimeout(callback, ms);
      timer.unref?.();
      return timer;
    },
    clearTimeout,
    URL,
    URLSearchParams,
    atob,
    Request,
    Headers,
    Response,
    Event: FakeEvent,
    CustomEvent: FakeCustomEvent,
    FormData: FakeFormData,
    HTMLElement,
    customElements,
    MutationObserver,
    ...(nativeFetch ? { fetch: nativeFetch } : {}),
    ...more,
  };
  window.top = framed ? {} : window;
  if (!framed) window.parent = window;
  const context = vm.createContext(window);
  context.globalThis = context;
  context.self = context;

  // The page as parsed up to the script: the script itself in the head.
  const script = document.createElement('script');
  script.setAttribute('src', scriptSrc);
  script.src = scriptSrc;
  document.head.appendChild(script);
  build?.(document);
  document.currentScript = script;
  vm.runInContext(appBridge, context, { filename: 'app-bridge.js' });
  document.currentScript = null;

  const fire = (type, event) => {
    for (const { listener, type: name } of [...listeners]) if (name === type) listener(event);
  };
  const frame = {
    window: context,
    document,
    location,
    history,
    posted,
    errors,
    warnings,
    opened,
    get shopify() {
      return context.shopify;
    },
    /** What the frame posted to its parent, of a type. */
    posts: (type) => posted.filter(({ message }) => message.type === type).map(({ message }) => message),
    last: (type) => frame.posts(type).at(-1),
    /** A message of the admin, from `origin`. */
    receive(data, origin = ADMIN) {
      fire('message', { source: parent, origin, data: { lsf: 'admin', ...data } });
    },
    welcome(extra = {}) {
      frame.receive({
        type: 'welcome',
        shop: 'local-supply-co.myshopify.com',
        clientId: 'client-1',
        locale: 'fr',
        user: { id: 1, name: 'Shop Owner', email: 'owner@local-supply-co.myshopify.com', accountAccess: 'Account owner', locale: 'en' },
        scopes: { granted: ['read_products'], required: ['read_products'], optional: ['write_products'] },
        ...extra,
      });
    },
    /** Answers the admin gives to requests: `method` → value (a function of the params). */
    answer(answers) {
      const done = new Set();
      return async () => {
        for (const request of frame.posts('request')) {
          if (done.has(request.id) || !(request.method in answers)) continue;
          done.add(request.id);
          const value = answers[request.method];
          const result = typeof value === 'function' ? await value(request.params) : value;
          if (result instanceof Error) frame.receive({ type: 'response', id: request.id, ok: false, error: result.message });
          else frame.receive({ type: 'response', id: request.id, ok: true, value: result });
        }
      };
    },
    event: (name, detail) => frame.receive({ type: 'event', name, detail }),
    /** What changed in the page: the mutation observer's turn, then the timer it set. */
    async mutated() {
      for (const observer of observers) observer.callback([]);
      await new Promise((resolve) => setTimeout(resolve, 5));
    },
    /** A click as a browser dispatches it: the window's capture listeners, the element and its
     * ancestors, then the window's own. Returns the event. */
    click(element, init = {}) {
      const event = new FakeEvent('click', { bubbles: true, cancelable: true, data: init });
      event.target = element;
      for (const { type, listener, capture } of [...listeners]) {
        if (type === 'click' && capture && !event.stopped) listener(event);
      }
      if (!event.stopped) FakeElement.prototype.dispatchEvent.call(element, event);
      for (const { type, listener, capture } of [...listeners]) {
        if (type === 'click' && !capture && !event.stopped) listener(event);
      }
      return event;
    },
    fire,
  };
  frames.set(document, frame);
  return frame;
}

/** An element with attributes and children, for building pages. */
export function element(document, tag, attributes = {}, children = []) {
  const created = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) created.setAttribute(name, value);
  for (const child of children) created.append(child);
  return created;
}

/** The config the admin page gets from lsf, for the app of localhost:3000. */
export function adminConfig(overrides = {}) {
  return {
    shop: 'local-supply-co.myshopify.com',
    shopName: 'Local Supply Co.',
    locale: 'en',
    query: {},
    user: { id: 1, name: 'Shop Owner', firstName: 'Shop', lastName: 'Owner', email: 'owner@local-supply-co.myshopify.com', accountOwner: true, accountAccess: 'Account owner', locale: 'en' },
    app: {
      name: 'Admin backend',
      handle: 'admin-backend',
      clientId: 'client-1',
      base: '/admin/apps/admin-backend',
      scopes: { granted: ['read_products'], required: ['read_products'], optional: [] },
    },
    endpoints: { sessionToken: '/__lsf/admin/session-token', resources: '/__lsf/admin/resources', appBridge: '/__lsf/admin/app-bridge.js' },
    frame: { url: LAUNCH, origin: APP, path: '/app', base: APP },
    ...overrides,
  };
}

/**
 * lsf's admin page (the skeleton of `admin.html`) started by `admin.js` on the fake DOM. `fetch`
 * answers lsf's endpoints. Returns the page, what it posted to the app's frame, and `send(data)`,
 * which plays a message of the frame.
 */
export async function adminPage({ config = adminConfig(), fetch = async () => new Response('{}'), frame = true, path = '/admin/apps/admin-backend/app' } = {}) {
  const { startAdmin } = await import('../admin/admin.js');
  const document = new FakeDocument();
  const el = (tag, attributes, children) => element(document, tag, attributes, children);
  const toFrame = [];
  const contentWindow = {
    postMessage: (message, origin) => toFrame.push({ message: JSON.parse(JSON.stringify(message)), origin }),
  };
  const iframe = el('iframe', { class: 'lsf-frame', src: config.frame?.url ?? '' });
  iframe.contentWindow = contentWindow;
  const progress = el('div', { class: 'lsf-progress' });
  progress.hidden = true;
  const savebar = el('div', { class: 'lsf-savebar' }, [
    el('button', { class: 'lsf-button lsf-savebar-discard' }, ['Discard']),
    el('button', { class: 'lsf-button lsf-savebar-save' }, ['Save']),
  ]);
  savebar.hidden = true;
  document.body.append(
    el('div', { class: 'lsf-admin' }, [
      progress,
      el('header', { class: 'lsf-topbar' }, [el('a', { class: 'lsf-brand', href: config.app.base }, ['lsf']), savebar]),
      el('nav', { class: 'lsf-nav' }, [
        el('a', { class: 'lsf-nav-section', href: '/admin/products' }, ['Products']),
        el('a', { class: 'lsf-nav-app', href: config.app.base }, [config.app.name]),
        el('ul', { class: 'lsf-nav-items' }),
      ]),
      el('main', { class: 'lsf-main' }, [
        el('div', { class: 'lsf-titlebar' }, [
          el('span', { class: 'lsf-titlebar-breadcrumb' }),
          el('h1', { class: 'lsf-titlebar-title' }, [config.app.name]),
          el('span', { class: 'lsf-titlebar-accessory' }),
          el('span', { class: 'lsf-titlebar-actions' }),
        ]),
        el('div', { class: 'lsf-banners' }),
        el('div', { class: 'lsf-content' }, frame ? [iframe] : []),
      ]),
      el('div', { class: 'lsf-toasts' }),
    ]),
  );
  const listeners = [];
  const opened = [];
  const window = {
    addEventListener: (type, listener) => listeners.push({ type, listener }),
    open: (url, target) => opened.push({ url, target }),
  };
  const replaced = [];
  const location = {
    origin: ADMIN,
    pathname: path,
    search: '',
    assigned: [],
    assign(url) {
      this.assigned.push(url);
    },
  };
  const history = { state: null, replaceState: (_state, _title, url) => replaced.push(url) };
  const requests = [];
  const recording = async (url, init) => {
    requests.push({ url, init });
    return fetch(url, init);
  };
  const admin = startAdmin({ window, document, config, location, history, fetch: recording, waitForAppBridge: 20 });
  const send = (data, { origin = APP, source = contentWindow } = {}) => {
    for (const { type, listener } of listeners) if (type === 'message') listener({ source, origin, data });
  };
  return {
    admin,
    document,
    toFrame,
    sent: (type) => toFrame.filter(({ message }) => message.type === type).map(({ message }) => message),
    send,
    /** A message of lsf's App Bridge. */
    bridge: (data) => send({ lsf: 'app-bridge', v: 1, ...data }),
    query: (selector) => document.querySelector(selector),
    all: (selector) => document.querySelectorAll(selector),
    replaced,
    location,
    requests,
    opened,
    iframe,
  };
}

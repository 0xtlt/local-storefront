// lsf's App Bridge: the `shopify` global of App Bridge v4, for an embedded app previewed in lsf's
// admin (`/admin/apps/<app>`), offline. Load it before anything else of the page, in place of
// Shopify's (`https://cdn.shopify.com/shopifycloud/app-bridge.js`):
//
//   <script src="http://127.0.0.1:9292/__lsf/admin/app-bridge.js"></script>
//
// It runs in the app's frame, on the app's origin, and talks to lsf's admin around it with
// `postMessage`: the admin is on the origin this script was loaded from, and each side checks the
// other's origin. The admin mints the ID tokens and draws the title bar, the navigation, the save
// bar, toasts, the loading bar and the resource picker. Modals are drawn here, over the app.
//
// When Shopify's script loads after this one, it finds `shopify.ready` and does nothing: that is
// how a page that keeps Shopify's script tag still runs on this one. What lsf does not simulate
// exists and rejects (or warns) with "lsf does not simulate shopify.<name>".
//
// A classic script, not a module: it must run before the page's other scripts, and read
// `document.currentScript`. The API surface is that of `@shopify/app-bridge-types` 0.7.4; the
// behaviour (which requests get the ID token, which links the admin follows) that of the CDN
// script 0.41.4. See crates/app/docs/admin/design.md, "The admin of an embedded app".
(function (win) {
  'use strict';

  const document = win.document;
  const console = win.console;
  const VERSION = 'lsf-1';
  const SESSION_KEY = 'app-bridge-config';
  const LAUNCH_PARAMS = ['embedded', 'hmac', 'host', 'id_token', 'locale', 'session', 'shop', 'timestamp'];
  const LINKS = ['a', 's-link', 's-button', 's-clickable', 's-clickable-chip'];
  const ADMIN_PROTOCOLS = ['shopify:', 'app:', 'extension:'];
  const TARGETS = ['_self', '_top', '_parent', '_blank'];

  const script = document.currentScript;
  const lsfOrigin = originOf(script && script.src);

  // The proxy of lsf's frame pointed the page's scripts of Shopify's CDN at lsf, and marked them
  // (`#lsf-cdn`). Once a script has started loading, its `src` no longer matters to the browser:
  // the CDN's URL goes back in it, so that React finds the `src` it rendered when it hydrates
  // the page, and warns of no mismatch. This script's own tag now; the others when the parser is
  // done, before the page's deferred and module scripts (React's) run.
  const CDN_MARK = '#lsf-cdn';
  function cdnSource(src) {
    if (!lsfOrigin || typeof src !== 'string' || !src.endsWith(CDN_MARK)) return null;
    const url = src.slice(0, -CDN_MARK.length);
    if (url === lsfOrigin + '/__lsf/admin/app-bridge.js') return 'https://cdn.shopify.com/shopifycloud/app-bridge.js';
    const cdn = lsfOrigin + '/__lsf/cdn/';
    if (url.startsWith(cdn)) return 'https://cdn.shopify.com/' + url.slice(cdn.length);
    return null;
  }
  function restoreCdnSources() {
    for (const element of document.querySelectorAll('script[src]')) {
      const cdn = cdnSource(element.getAttribute('src'));
      if (cdn) element.setAttribute('src', cdn);
    }
  }
  if (script && cdnSource(script.getAttribute('src'))) script.setAttribute('src', cdnSource(script.getAttribute('src')));
  if (document.readyState === 'loading') {
    document.addEventListener('readystatechange', function restore() {
      document.removeEventListener('readystatechange', restore);
      restoreCdnSources();
    });
  } else {
    restoreCdnSources();
  }

  // Another App Bridge is already here: Shopify's (whose `ready` this script also defines, so that
  // the CDN script returns when it loads second), or this script twice.
  try {
    const existing = win.shopify;
    if (existing && existing.ready && typeof existing.ready.then === 'function') {
      if (!existing.lsf) {
        console.error(
          "lsf App Bridge: Shopify's App Bridge was loaded before lsf's, and runs this page. Load lsf's first: " +
            '<script src="' + (script ? script.src : '…/__lsf/admin/app-bridge.js') + '"> before any other script.',
        );
      }
      return;
    }
  } catch (_) {
    // A global that throws on access is not an App Bridge.
  }

  // A page of a modal with `src`, or of an app window: the app's frame opened it, on the app's
  // origin, and its App Bridge is the frame's.
  if (String(win.name || '').startsWith('lsf-modal:')) {
    try {
      if (win.parent && win.parent.shopify && win.parent.shopify.lsf) {
        Object.defineProperty(win, 'shopify', { configurable: true, writable: true, value: win.parent.shopify });
        return;
      }
    } catch (_) {
      // Not the app's origin after all: go on as a frame of its own.
    }
  }

  // ---- helpers ------------------------------------------------------------------------------

  function originOf(url) {
    try {
      return url ? new URL(url).origin : null;
    } catch (_) {
      return null;
    }
  }

  function camel(name) {
    return name.toLowerCase().replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
  }

  function elementChildren(element) {
    return Array.from(element.childNodes || []).filter((node) => node.nodeType === 1);
  }

  function text(element) {
    return String(element.textContent || '').replace(/\s+/g, ' ').trim();
  }

  function attributeNames(element) {
    if (typeof element.getAttributeNames === 'function') return element.getAttributeNames();
    return Array.from(element.attributes || [], (attribute) => attribute.name);
  }

  const warned = new Set();
  function warnOnce(message) {
    if (warned.has(message)) return;
    warned.add(message);
    console.warn(message);
  }

  // ---- configuration (as the CDN script reads it) ------------------------------------------

  const LIST_KEYS = ['disabledFeatures', 'experimentalFeatures', 'appOrigins'];

  function normalized(key, value) {
    if (value === undefined || value === null || value === '') return undefined;
    if (LIST_KEYS.includes(key)) {
      return Array.isArray(value) ? value : String(value).split(',').map((entry) => entry.trim()).filter(Boolean);
    }
    if (key === 'debug' && typeof value === 'string') return { webVitals: value.includes('web-vitals') };
    return value;
  }

  function merge(into, values) {
    for (const [key, value] of Object.entries(values || {})) {
      const entry = normalized(key, value);
      if (entry !== undefined) into[key] = entry;
    }
  }

  function fromSession() {
    try {
      const saved = win.sessionStorage && win.sessionStorage.getItem(SESSION_KEY);
      return saved ? JSON.parse(saved) : {};
    } catch (_) {
      return {};
    }
  }

  // The scripts of App Bridge (Shopify's and this one: their query and `data-*` attributes), and
  // `<script type="shopify/config">`.
  function fromScripts() {
    const values = {};
    const scripts = Array.from(document.querySelectorAll('script'));
    if (script && !scripts.includes(script)) scripts.unshift(script);
    for (const element of scripts) {
      const src = element.getAttribute('src');
      if (src) {
        let url;
        try {
          url = new URL(src, win.location.href);
        } catch (_) {
          continue;
        }
        if (!/\/app-?bridge[/.-]/i.test(url.pathname)) continue;
        url.searchParams.forEach((value, key) => {
          if (value) values[key] = value;
        });
        for (const name of attributeNames(element)) {
          if (name.startsWith('data-')) values[camel(name.slice(5))] = element.getAttribute(name);
        }
      } else if (element.getAttribute('type') === 'shopify/config') {
        try {
          Object.assign(values, JSON.parse(element.textContent || '{}'));
        } catch (error) {
          console.warn('lsf App Bridge: <script type="shopify/config"> is not JSON: ' + error.message);
        }
      }
    }
    return values;
  }

  function fromMeta() {
    const values = {};
    for (const meta of Array.from(document.querySelectorAll('meta'))) {
      const name = meta.getAttribute('name') || '';
      if (!/^shopify-/i.test(name)) continue;
      values[camel(name.slice('shopify-'.length))] = meta.getAttribute('content') || undefined;
    }
    return values;
  }

  function fromQuery() {
    const params = new URL(win.location.href).searchParams;
    return { shop: params.get('shop'), host: params.get('host'), locale: params.get('locale') };
  }

  function readConfig(into) {
    const config = {
      apiKey: '',
      appOrigins: [],
      debug: {},
      disabledFeatures: [],
      experimentalFeatures: [],
      locale: 'en-US',
      host: '',
    };
    merge(config, fromSession());
    merge(config, preset);
    merge(config, fromScripts());
    merge(config, fromMeta());
    merge(config, fromQuery());
    // The shop of `host` (`admin.shopify.com/store/<name>`) when the page has no `shop`.
    if (!config.shop && config.host) {
      try {
        const name = /\/store\/([^/]+)/.exec(win.atob(config.host));
        if (name) config.shop = name[1] + '.myshopify.com';
      } catch (_) {
        // Not base64: no shop from it.
      }
    }
    for (const key of Object.keys(into)) delete into[key];
    Object.assign(into, config);
    try {
      win.sessionStorage && win.sessionStorage.setItem(SESSION_KEY, JSON.stringify(into));
    } catch (_) {
      // No session storage (a sandboxed frame): the page's own parameters are all there is.
    }
    return into;
  }

  // A `shopify.config` the page set before loading App Bridge.
  let preset = {};
  try {
    preset = (win.shopify && win.shopify.config) || {};
  } catch (_) {
    preset = {};
  }

  const config = readConfig({});
  const disabled = (feature) => (config.disabledFeatures || []).includes(feature);

  // ---- the connection to lsf's admin ----------------------------------------------------------

  const framed = (() => {
    try {
      return win.parent && win.parent !== win;
    } catch (_) {
      return true;
    }
  })();

  let welcome = null;
  let resolveReady;
  const ready = new Promise((resolve) => {
    resolveReady = resolve;
  });
  let nextId = 1;
  const pending = new Map();
  const handlers = new Map();
  const outbox = [];

  function adminUrl() {
    const shop = config.shop ? '?shop=' + encodeURIComponent(config.shop) : '';
    return (lsfOrigin || 'http://127.0.0.1:9292') + '/admin/apps/' + (config.apiKey || '<app>') + shop;
  }

  function notInAdmin() {
    return new Error(
      "lsf App Bridge: this page is not open in lsf's admin, which answers App Bridge here. Open it at " + adminUrl() + '.',
    );
  }

  function post(message) {
    if (!framed) return;
    const tagged = Object.assign({ lsf: 'app-bridge', v: 1 }, message);
    if (welcome) win.parent.postMessage(tagged, lsfOrigin);
    else outbox.push(tagged);
  }

  // A request lsf's admin answers. Before the admin said hello, it waits for it, ten seconds.
  function request(method, params, timeout) {
    if (!framed) return Promise.reject(notInAdmin());
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const entry = { resolve, reject, timer: null };
      pending.set(id, entry);
      if (timeout) {
        entry.timer = win.setTimeout(() => {
          pending.delete(id);
          reject(new Error(timeout.message));
        }, timeout.ms);
      }
      if (!welcome) {
        const waited = win.setTimeout(() => {
          if (welcome || !pending.has(id)) return;
          pending.delete(id);
          reject(notInAdmin());
        }, 10000);
        ready.then(() => win.clearTimeout(waited));
      }
      post({ type: 'request', id, method, params: params === undefined ? null : params });
    });
  }

  function on(name, handler) {
    handlers.set(name, handler);
  }

  let originWarned = false;
  win.addEventListener('message', (event) => {
    if (!framed || event.source !== win.parent) return;
    const data = event.data;
    if (!data || typeof data !== 'object' || data.lsf !== 'admin') return;
    if (event.origin !== lsfOrigin) {
      if (!originWarned) {
        originWarned = true;
        console.error(
          "lsf App Bridge: the admin around this page is at " + event.origin + ', but this script comes from ' + lsfOrigin +
            ': open the admin at ' + adminUrl() + ' (the same host name), or load the script from ' + event.origin + '.',
        );
      }
      return;
    }
    if (data.type === 'welcome') {
      if (welcome) return;
      welcome = data;
      if (!config.shop) config.shop = data.shop;
      if (config.apiKey && data.clientId && config.apiKey !== data.clientId) {
        console.warn(
          'lsf App Bridge: the page says its API key is ' + config.apiKey + ', but lsf serves the app ' + data.clientId +
            ': the ID tokens are for ' + data.clientId + ', which the backend checks against its own key.',
        );
      }
      for (const message of outbox.splice(0)) win.parent.postMessage(message, lsfOrigin);
      resolveReady();
      return;
    }
    if (data.type === 'response') {
      const entry = pending.get(data.id);
      if (!entry) return;
      pending.delete(data.id);
      if (entry.timer) win.clearTimeout(entry.timer);
      if (data.ok) entry.resolve(data.value);
      else entry.reject(new Error(data.error || 'lsf admin: the request failed'));
      return;
    }
    if (data.type === 'event') {
      const handler = handlers.get(data.name);
      if (handler) handler(data.detail || {});
    }
  });

  // Hello, until the admin answers: to any parent, since it says nothing secret, so that an admin
  // on another origin can say why it cannot answer. Ten seconds, then the page is not in lsf's admin.
  if (framed) {
    let tries = 0;
    const hello = () => {
      if (welcome) return;
      try {
        win.parent.postMessage(
          { lsf: 'app-bridge', v: 1, type: 'hello', version: VERSION, apiKey: config.apiKey, scriptOrigin: lsfOrigin },
          '*',
        );
      } catch (_) {
        // A parent that cannot be posted to: there is no admin to wait for.
        return;
      }
      if (++tries < 50) win.setTimeout(hello, 200);
    };
    hello();
  }

  // ---- what lsf does not simulate -------------------------------------------------------------

  function notSimulated(name) {
    const message = 'lsf does not simulate shopify.' + name;
    warnOnce(message);
    if (welcome || framed) post({ type: 'unsupported', name: 'shopify.' + name });
    return message;
  }
  const rejecting = (name) =>
    function () {
      return Promise.reject(new Error(notSimulated(name)));
    };
  const warning = (name, value) =>
    function () {
      notSimulated(name);
      return typeof value === 'function' ? value() : value;
    };

  // ---- intents ---------------------------------------------------------------------------------

  // `shopify.intents.invoke('edit:shopify/Product', {value})`, or `({action, type, value, data})`:
  // the admin opens what lsf has of the resource, and `complete` answers when the merchant closes
  // it (`{code: 'closed'}`). An intent lsf does not simulate completes with `{code: 'error'}`,
  // whose message says so.
  async function invokeIntent(query, options) {
    let intent;
    if (typeof query === 'string') {
      const match = /^([a-z]+):(.+)$/.exec(query.trim());
      intent = match ? Object.assign({}, options || {}, { action: match[1], type: match[2] }) : null;
    } else if (query && typeof query === 'object') {
      intent = Object.assign({}, query);
    }
    if (!intent || !intent.action || !intent.type) {
      throw new Error('shopify.intents.invoke: an intent is "<action>:<type>", such as "edit:shopify/Product", or {action, type}');
    }
    if (!framed) throw notInAdmin();
    const params = { action: String(intent.action), type: String(intent.type), value: intent.value === undefined ? null : intent.value, data: plain(intent.data) || null };
    const complete = request('intent', params).then((response) => {
      if (response && response.code === 'error' && response.message) console.warn('lsf App Bridge: ' + response.message);
      return response;
    });
    return { complete };
  }

  // ---- toast, loading, user -------------------------------------------------------------------

  const toasts = new Map();
  let toastCount = 0;
  const toast = {
    show(message, options) {
      const opts = Object.assign({ duration: 5000 }, options || {});
      const id = 'lsf-toast-' + ++toastCount;
      toasts.set(id, { onAction: opts.onAction, onDismiss: opts.onDismiss });
      post({
        type: 'toast',
        id,
        message: String(message),
        duration: Number(opts.duration),
        isError: Boolean(opts.isError),
        action: opts.action ? String(opts.action) : null,
      });
      return id;
    },
    hide(id) {
      post({ type: 'toast-hide', id: String(id) });
    },
  };
  on('toast.action', ({ id }) => {
    const entry = toasts.get(id);
    if (entry && typeof entry.onAction === 'function') entry.onAction();
  });
  on('toast.dismiss', ({ id }) => {
    const entry = toasts.get(id);
    toasts.delete(id);
    if (entry && typeof entry.onDismiss === 'function') entry.onDismiss();
  });

  let loadingOn = false;
  async function loading(isLoading) {
    const on = Boolean(isLoading);
    if (on === loadingOn) return;
    loadingOn = on;
    post({ type: 'loading', on });
  }

  async function user() {
    if (!framed) throw notInAdmin();
    await Promise.race([
      ready,
      new Promise((_, reject) => win.setTimeout(() => reject(notInAdmin()), 10000)),
    ]);
    return Object.assign({}, welcome.user);
  }

  function idToken() {
    return request('idToken', null, {
      ms: 60000,
      message: 'idToken unavailable: host did not respond in time',
    }).then((token) => {
      if (typeof token !== 'string' || token === '') throw new Error('idToken unavailable');
      return token;
    });
  }

  // ---- the resource picker and the picker -----------------------------------------------------

  const RESOURCE_TYPES = ['product', 'variant', 'collection'];

  function plain(value) {
    return JSON.parse(JSON.stringify(value === undefined ? null : value));
  }

  async function resourcePicker(options) {
    if (!options || !RESOURCE_TYPES.includes(options.type)) {
      throw new TypeError('shopify.resourcePicker: type is one of product, variant, collection');
    }
    const selection = await request('resourcePicker', plain(options));
    if (!Array.isArray(selection)) return undefined;
    Object.defineProperty(selection, 'selection', { value: selection, enumerable: false });
    return selection;
  }

  async function picker(options) {
    if (!options || !Array.isArray(options.items)) {
      throw new TypeError('shopify.picker: items is an array of { id, heading }');
    }
    const selected = request('picker', plain(options)).then((ids) => (Array.isArray(ids) ? ids : undefined));
    return { selected };
  }

  // ---- the title bar and the navigation ------------------------------------------------------

  let titleElements = [];

  function action(element, extra) {
    titleElements.push(element);
    return Object.assign(
      {
        index: titleElements.length - 1,
        label: text(element),
        disabled: element.hasAttribute('disabled'),
        loading: element.hasAttribute('loading'),
        tone: element.getAttribute('tone') || null,
        href: element.getAttribute('href') || null,
      },
      extra || {},
    );
  }

  function insideModal(element) {
    return Boolean(element.closest('ui-modal') || element.closest('s-app-window') || element.closest('s-modal'));
  }

  function fromUiTitleBar(bar) {
    const title = { title: bar.getAttribute('title') || document.title || '', breadcrumb: null, primary: null, secondary: [], accessory: null };
    for (const child of elementChildren(bar)) {
      const variant = child.getAttribute('variant');
      if (child.localName === 'section') {
        const actions = elementChildren(child)
          .filter((entry) => entry.localName === 'button' || entry.localName === 'a')
          .map((entry) => action(entry));
        title.secondary.push({ group: true, label: child.getAttribute('label') || '', actions });
      } else if (child.localName === 'button' || child.localName === 'a') {
        if (variant === 'breadcrumb') title.breadcrumb = action(child);
        else if (variant === 'primary') title.primary = action(child);
        else title.secondary.push(action(child));
      }
    }
    return title;
  }

  function fromSPage(page) {
    const title = { title: page.getAttribute('heading') || document.title || '', breadcrumb: null, primary: null, secondary: [], accessory: null };
    for (const child of elementChildren(page)) {
      const slot = child.getAttribute('slot');
      if (slot === 'primary-action' && !title.primary) title.primary = action(child);
      else if (slot === 'breadcrumb-actions' && !title.breadcrumb) title.breadcrumb = action(child);
      else if (slot === 'accessory') title.accessory = { label: text(child), tone: child.getAttribute('tone') || null };
      else if (slot === 'secondary-actions') {
        const menuId = child.getAttribute('commandfor');
        const menu = menuId ? document.getElementById(menuId) : null;
        if (menu && menu.localName === 's-menu') {
          menu.setAttribute('data-lsf-title-bar-menu', '');
          const actions = elementChildren(menu).map((entry) => action(entry));
          title.secondary.push({ group: true, label: text(child), actions });
        } else {
          title.secondary.push(action(child));
        }
      }
    }
    return title;
  }

  function readTitle() {
    titleElements = [];
    const bar = Array.from(document.querySelectorAll('ui-title-bar')).find((element) => !insideModal(element));
    if (bar) return fromUiTitleBar(bar);
    const page = Array.from(document.querySelectorAll('s-page')).find((element) => !insideModal(element));
    if (page) return fromSPage(page);
    return { title: document.title || '', breadcrumb: null, primary: null, secondary: [], accessory: null };
  }

  on('title.action', ({ index }) => {
    const element = titleElements[index];
    if (element) element.click();
  });

  let navElements = [];
  let home = null;

  // A nav item's path in the app: a relative path, an `app:` URL, or a URL of the app's origin.
  function appPath(href) {
    if (!href) return null;
    try {
      const url = /^app:/i.test(href)
        ? new URL(href.replace(/^app:\/{0,2}/i, '/'), win.location.href)
        : new URL(href, win.location.href);
      if (url.origin !== win.location.origin) return null;
      return url.pathname + url.search;
    } catch (_) {
      return null;
    }
  }

  function readNav() {
    const menus = Array.from(document.querySelectorAll('s-app-nav, ui-nav-menu'));
    if (menus.length > 1) {
      warnOnce(
        'lsf App Bridge: ' + menus.length + ' navigation menus (' + menus.map((menu) => menu.localName).join(', ') +
          '): only one <s-app-nav> or <ui-nav-menu> is used, the last.',
      );
    }
    navElements = [];
    home = null;
    const menu = menus[menus.length - 1];
    if (!menu) return { items: [], home: null };
    const items = [];
    for (const link of elementChildren(menu)) {
      if (!['a', 's-link', 's-internal-link'].includes(link.localName)) continue;
      const href = link.getAttribute('href');
      const path = appPath(href);
      if (path === null) {
        warnOnce(
          '[' + menu.localName + '] Dropped nav item with unsupported href "' + href +
            '". App nav only supports relative app paths (e.g. "/settings"), app: URLs, or same-origin http(s) URLs.',
        );
        continue;
      }
      if (String(link.getAttribute('rel') || '').split(/\s+/).includes('home')) {
        home = { element: link, path };
        continue;
      }
      navElements.push({ element: link, path });
      items.push({ label: text(link), path });
    }
    return { items, home: home ? home.path : null };
  }

  // A navigation the admin asks for (a nav item, the app's name): the app's own link is clicked,
  // so that its router follows it; without one, the frame goes there.
  on('navigate', ({ path, home: toHome }) => {
    const entry = toHome ? home : navElements.find((item) => item.path === path);
    leaveConfirmed().then(() => {
      if (entry) entry.element.click();
      else if (path) win.location.assign(path);
      else win.location.assign('/');
    });
  });

  // ---- the save bar ---------------------------------------------------------------------------

  const forms = new Map();
  let shownBar = null;

  function formState(form) {
    const entries = [];
    try {
      for (const [name, value] of new win.FormData(form)) {
        entries.push([name, typeof value === 'string' ? value : value && value.name]);
      }
    } catch (_) {
      return '';
    }
    return JSON.stringify(entries);
  }

  function trackForms() {
    for (const form of Array.from(document.querySelectorAll('form[data-save-bar]'))) {
      if (!forms.has(form)) forms.set(form, formState(form));
    }
    for (const form of Array.from(forms.keys())) {
      if (!document.contains(form) || !form.hasAttribute('data-save-bar')) forms.delete(form);
    }
  }

  function dirtyForms() {
    return Array.from(forms.keys()).filter((form) => formState(form) !== forms.get(form));
  }

  function rememberForm(form) {
    if (forms.has(form)) forms.set(form, formState(form));
  }

  function saveBarState() {
    if (shownBar && shownBar.isConnected !== false && document.contains(shownBar)) {
      const buttons = elementChildren(shownBar).filter((child) => child.localName === 'button');
      const save = buttons.find((button) => button.getAttribute('variant') === 'primary');
      const discard = buttons.find((button) => button.getAttribute('variant') !== 'primary');
      return {
        visible: true,
        id: shownBar.id || null,
        save: { disabled: Boolean(save && save.hasAttribute('disabled')), loading: Boolean(save && save.hasAttribute('loading')) },
        discard: { disabled: Boolean(discard && discard.hasAttribute('disabled')), loading: Boolean(discard && discard.hasAttribute('loading')) },
        discardConfirmation: shownBar.hasAttribute('discardconfirmation') || shownBar.hasAttribute('discard-confirmation'),
      };
    }
    const dirty = dirtyForms();
    if (dirty.length) {
      return {
        visible: true,
        id: null,
        save: { disabled: false, loading: false },
        discard: { disabled: false, loading: false },
        discardConfirmation: dirty.some((form) => form.hasAttribute('data-discard-confirmation')),
      };
    }
    return { visible: false };
  }

  const saveBarVisible = () => saveBarState().visible;

  on('saveBar.save', () => {
    if (shownBar) {
      const button = elementChildren(shownBar).find((child) => child.localName === 'button' && child.getAttribute('variant') === 'primary');
      if (button) button.click();
      return;
    }
    for (const form of dirtyForms()) {
      if (typeof form.requestSubmit === 'function') form.requestSubmit();
      else form.dispatchEvent(new win.Event('submit', { bubbles: true, cancelable: true }));
    }
  });
  on('saveBar.discard', () => {
    if (shownBar) {
      const button = elementChildren(shownBar).find((child) => child.localName === 'button' && child.getAttribute('variant') !== 'primary');
      if (button) button.click();
      return;
    }
    for (const form of dirtyForms()) form.reset();
  });

  function findById(id, kinds) {
    const element = document.getElementById(String(id));
    if (!element || (kinds && !kinds.includes(element.localName))) return null;
    return element;
  }

  const saveBar = {
    async show(id) {
      const bar = findById(id, ['ui-save-bar']);
      if (!bar) throw new Error('shopify.saveBar.show: the page has no <ui-save-bar id="' + id + '">');
      return bar.show();
    },
    async hide(id) {
      const bar = findById(id, ['ui-save-bar']);
      if (!bar) throw new Error('shopify.saveBar.hide: the page has no <ui-save-bar id="' + id + '">');
      return bar.hide();
    },
    async toggle(id) {
      const bar = findById(id, ['ui-save-bar']);
      if (!bar) throw new Error('shopify.saveBar.toggle: the page has no <ui-save-bar id="' + id + '">');
      return bar.toggle();
    },
    leaveConfirmation() {
      return leaveConfirmed();
    },
  };

  // Resolves when the merchant chose to leave (the save bar is then put away), or at once without
  // a save bar; never when they chose to stay.
  function leaveConfirmed() {
    if (!saveBarVisible()) return Promise.resolve();
    return request('leaveConfirmation', null).then((leave) => {
      if (!leave) return new Promise(() => {});
      if (shownBar) {
        const bar = shownBar;
        shownBar = null;
        if (bar.dispatchEvent) bar.dispatchEvent(new win.CustomEvent('hide'));
      }
      for (const form of Array.from(forms.keys())) rememberForm(form);
      leaving = true;
      sync();
    });
  }

  // ---- links, history and window.open (the navigation of App Bridge v4) ---------------------

  let leaving = false;

  // A link's URL: `shopify:admin/products` and `shopify://admin/products` are `shopify:/admin/products`.
  function linkUrl(href) {
    const url = new URL(href, win.location.href);
    if (ADMIN_PROTOCOLS.includes(url.protocol)) {
      const path = '/' + (url.host + url.pathname + url.search).replace(/^\/+/, '');
      return new URL(url.protocol + path);
    }
    return url;
  }

  // Follows a `shopify:` URL in the admin: lsf's admin page of it, which says what lsf has.
  function toAdmin(url, target) {
    const path = url.pathname + url.search;
    if (!path.startsWith('/admin/')) {
      throw new Error("Invalid URL: expected '/admin/*', received: '" + path + "'.");
    }
    post({ type: 'navigate', url: 'shopify:' + path, target });
  }

  function linkOf(node) {
    for (let element = node; element; element = element.parentNode) {
      if (element.nodeType === 1 && LINKS.includes(element.localName) && element.getAttribute('href') !== null) return element;
    }
    return null;
  }

  // The modal a `commandfor` button controls, when Polaris is not there to do it (offline).
  function command(event) {
    for (let element = event.target; element; element = element.parentNode) {
      if (element.nodeType !== 1) continue;
      const id = element.getAttribute('commandfor');
      if (!id) continue;
      const target = document.getElementById(id);
      if (!target || !['ui-modal', 's-modal', 's-app-window'].includes(target.localName)) return;
      if (target.localName === 's-modal' && typeof target.showOverlay === 'function') return;
      const name = (element.getAttribute('command') || '--auto').toLowerCase();
      const run = name === '--show' || name === 'show-modal' ? 'show' : name === '--hide' || name === 'close' ? 'hide' : 'toggle';
      event.preventDefault();
      modal[run](id);
      return;
    }
  }

  win.addEventListener(
    'click',
    (event) => {
      if (event.defaultPrevented) return;
      const link = linkOf(event.target);
      if (!link) return command(event);
      const href = link.getAttribute('href');
      const target = String(link.getAttribute('target') || '_self').toLowerCase();
      let url;
      try {
        url = linkUrl(href);
      } catch (_) {
        return;
      }
      const followed = ADMIN_PROTOCOLS.includes(url.protocol) || url.protocol === 'http:' || url.protocol === 'https:';
      if (!followed) return;
      if (target !== '_blank' && saveBarVisible() && !leaving) {
        event.preventDefault();
        event.stopImmediatePropagation();
        leaveConfirmed().then(() => link.click());
        return;
      }
      if (url.protocol === 'shopify:') {
        event.preventDefault();
        toAdmin(url, target === '_blank' ? '_blank' : '_top');
        return;
      }
      if (url.protocol === 'extension:') {
        event.preventDefault();
        notSimulated('navigation to extension: URLs (' + href + ')');
        return;
      }
      if (url.protocol === 'app:') {
        event.preventDefault();
        const path = url.pathname + url.search;
        if (target === '_blank') win.open(path, '_blank');
        else win.location.assign(path);
        return;
      }
      if (target === '_top' || target === '_parent') {
        event.preventDefault();
        post({ type: 'navigate', url: url.href, target: '_top' });
        return;
      }
      // A link the app's router does not follow reloads the frame at its URL: without the admin's
      // parameters (`shop`, `host`, `embedded`, `id_token`), the server cannot authenticate it,
      // on Shopify as here. Said once the click is done, if nothing prevented it.
      if (link.localName === 'a' && target === '_self' && url.origin === win.location.origin && !url.searchParams.has('shop')) {
        const after = (done) => {
          if (done !== event) return;
          win.removeEventListener('click', after);
          if (!done.defaultPrevented) {
            console.warn(
              'lsf App Bridge: <a href="' + href + '"> reloads the app\'s frame without the session token or the shop: ' +
                "authenticate.admin cannot authenticate that request, on Shopify too. Navigate with the router's <Link> (or <s-link>).",
            );
          }
        };
        win.addEventListener('click', after);
      }
      // An `s-link` (or `s-button`, `s-clickable`) of the app, without Polaris to follow it
      // (offline): `shopify:navigate`, as Polaris dispatches it, for the app's router; the frame
      // follows it when nothing did.
      if (link.localName !== 'a' && target === '_self' && url.origin === win.location.origin && !polarisDefines(link.localName)) {
        if (event.ctrlKey || event.shiftKey || event.altKey || event.metaKey) return;
        event.preventDefault();
        let read = false;
        const getAttribute = link.getAttribute;
        link.getAttribute = function (name) {
          if (name === 'href') read = true;
          return getAttribute.call(this, name);
        };
        let handled;
        try {
          handled = !link.dispatchEvent(new win.CustomEvent('shopify:navigate', { bubbles: true, cancelable: true, composed: true }));
        } finally {
          link.getAttribute = getAttribute;
        }
        if (!read && !handled) win.location.assign(url.href);
      }
    },
    true,
  );

  function polarisDefines(name) {
    try {
      return Boolean(win.customElements && win.customElements.get(name));
    } catch (_) {
      return false;
    }
  }

  win.addEventListener('submit', (event) => {
    const form = event.target;
    if (form && forms.has(form)) {
      // Saved: what the form has now is what it is compared with.
      rememberForm(form);
      leaving = !event.defaultPrevented;
      sync();
    }
  });
  win.addEventListener('reset', (event) => {
    const form = event.target;
    if (form && forms.has(form)) {
      // Discarded: the form has its first values back once the reset is done.
      win.setTimeout(() => {
        rememberForm(form);
        sync();
      }, 0);
    }
  });
  win.addEventListener('input', () => sync());
  win.addEventListener('change', () => sync());
  // The admin forgets what this page asked for (its save bar, its loading bar) once it is gone.
  win.addEventListener('pagehide', () => post({ type: 'unload' }));
  win.addEventListener('beforeunload', (event) => {
    if (leaving || !saveBarVisible()) return;
    event.preventDefault();
    event.returnValue = true;
  });

  function reportLocation(mode) {
    post({ type: 'history', path: win.location.pathname + win.location.search + win.location.hash, mode });
  }

  if (framed && win.history) {
    for (const [name, mode] of [['pushState', 'push'], ['replaceState', 'replace']]) {
      const original = win.history[name];
      if (typeof original !== 'function') continue;
      win.history[name] = function (state, unused, url) {
        const result = original.call(this, state, unused, url);
        reportLocation(mode);
        return result;
      };
    }
    win.addEventListener('popstate', () => reportLocation('replace'));
  }

  const open = win.open;
  win.open = function (url, target, features) {
    if (url !== undefined && url !== null) {
      let parsed;
      try {
        parsed = linkUrl(String(url));
      } catch (_) {
        parsed = null;
      }
      const name = target === undefined || target === null || target === '' ? '_blank' : String(target).toLowerCase();
      if (parsed && parsed.protocol === 'shopify:') {
        toAdmin(parsed, name === '_blank' ? '_blank' : '_top');
        return null;
      }
      if (parsed && parsed.protocol === 'app:') {
        return open.call(this, parsed.pathname + parsed.search, target, features);
      }
      if (parsed && parsed.protocol === 'extension:') {
        notSimulated('navigation to extension: URLs (' + url + ')');
        return null;
      }
      if (parsed && (name === '_top' || name === '_parent') && TARGETS.includes(name)) {
        post({ type: 'navigate', url: parsed.href, target: '_top' });
        return null;
      }
    }
    return open.call(this, url, target, features);
  };

  // ---- fetch ----------------------------------------------------------------------------------

  // Which requests get the ID token: those to the page's host (the port is not compared) or one
  // of its subdomains, over the page's protocol, and those to `appOrigins`. As the CDN script.
  // Never those to lsf, the admin this script comes from (its CDN, `/__lsf/cdn/`, included):
  // on Shopify, the admin and the CDN are never the app's host, but locally lsf and the app
  // share `localhost`, and the token and `X-Requested-With` would make every `fetch()` of
  // lsf's copy of Polaris (its icons) a preflighted request. Unless the page is on lsf's origin
  // itself: then lsf serves the app.
  function sameApp(url) {
    const location = new URL(win.location.href);
    if (lsfOrigin && url.origin === lsfOrigin && location.origin !== lsfOrigin) return false;
    return (
      (url.protocol === location.protocol &&
        (url.hostname === location.hostname || url.hostname.endsWith('.' + location.hostname))) ||
      (config.appOrigins || []).includes(url.origin)
    );
  }

  const nativeFetch = win.fetch;
  if (typeof nativeFetch === 'function' && !disabled('fetch')) {
    win.fetch = async function (input, init) {
      const isRequest = typeof win.Request === 'function' && input instanceof win.Request;
      const raw = isRequest ? input.url : String(input);
      if (/^shopify:/i.test(raw)) {
        throw new Error(notSimulated('fetch() of shopify:admin/… URLs (Direct API access)'));
      }
      const url = new URL(raw, win.location.href);
      const request = isRequest ? new win.Request(input.clone(), init) : new win.Request(url.href, init);
      const ours = sameApp(url);
      const authorize = ours && !request.headers.has('Authorization');
      if (authorize) {
        try {
          request.headers.set('Authorization', 'Bearer ' + (await idToken()));
        } catch (error) {
          console.error('Failed to fetch an idToken', error);
        }
      }
      if (ours && !request.headers.has('X-Requested-With')) request.headers.set('X-Requested-With', 'XMLHttpRequest');
      if (ours && !request.headers.has('Accept-Language') && config.locale) request.headers.set('Accept-Language', config.locale);
      const retry = request.clone();
      let response = await nativeFetch.call(win, request);
      if (authorize && response.headers.get('X-Shopify-Retry-Invalid-Session-Request')) {
        try {
          retry.headers.set('Authorization', 'Bearer ' + (await idToken()));
          response = await nativeFetch.call(win, retry);
        } catch (error) {
          console.error('Failed to fetch an idToken', error);
        }
      }
      const reauthorize = response.headers.get('X-Shopify-API-Request-Failure-Reauthorize-Url');
      if (ours && reauthorize) {
        post({ type: 'navigate', url: new URL(reauthorize, win.location.href).href, target: '_top' });
        return new Promise(() => {});
      }
      // A request of the app failed: the admin asks lsf again what it should say (a backend that
      // sends its Admin API calls to Shopify rather than to lsf, say).
      if (ours && response.status >= 400) post({ type: 'failed', status: response.status });
      return response;
    };
  }

  // ---- Polaris without the internet -------------------------------------------------------------

  // The page's Polaris from Shopify's CDN did not load (the machine is offline): lsf's copy, at the
  // same path under `/__lsf/cdn/`, when lsf serves it (its Polaris mode is `offline`: `--polaris
  // offline`, `PUT /__lsf/polaris`). Once, and never when Polaris is already defined. The
  // components defined late upgrade the elements already in the page.
  const CDN_POLARIS = /^https:\/\/cdn\.shopify\.com\/(shopifycloud\/polaris(?:-[0-9.]+)?\.js)(?:[?#]|$)/;
  let polarisReplaced = false;
  win.addEventListener(
    'error',
    (event) => {
      const failed = event && event.target;
      if (polarisReplaced || !lsfOrigin || !failed || failed.localName !== 'script') return;
      const match = CDN_POLARIS.exec(String(failed.src || ''));
      if (!match || polarisDefines('s-page') || typeof nativeFetch !== 'function') return;
      polarisReplaced = true;
      nativeFetch
        .call(win, lsfOrigin + '/__lsf/polaris')
        .then((response) => response.json())
        .then((polaris) => {
          if (!polaris || polaris.mode !== 'offline' || polarisDefines('s-page')) return;
          const copy = document.createElement('script');
          copy.src = lsfOrigin + '/__lsf/cdn/' + match[1];
          copy.setAttribute('data-lsf-polaris', '');
          (document.head || document.documentElement).appendChild(copy);
          console.warn("lsf App Bridge: " + failed.src + " did not load: lsf's copy of Polaris runs the page (" + copy.src + ').');
        })
        .catch(() => {});
    },
    true,
  );

  // ---- modals and app windows: drawn over the app, in its frame -------------------------------

  const SIZES = { small: '380px', base: '620px', large: '980px' };
  let modalCount = 0;

  const MODAL_STYLE = [
    ':host { display: none; }',
    ':host([data-lsf-open]) { display: flex; position: fixed; inset: 0; z-index: 2147483000; align-items: center; justify-content: center; background: rgba(15, 17, 20, 0.55); font: 14px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; }',
    '.box { display: flex; flex-direction: column; background: #fff; color: #1a1c1f; border-radius: 12px; box-shadow: 0 20px 50px rgba(0,0,0,.3); width: min(var(--lsf-modal-width, 620px), calc(100vw - 32px)); max-height: calc(100vh - 32px); overflow: hidden; }',
    ':host([data-lsf-variant="max"]) .box { width: calc(100vw - 16px); height: calc(100vh - 16px); max-height: none; }',
    'header { display: flex; align-items: center; gap: 8px; padding: 12px 16px; border-bottom: 1px solid #e3e5e8; font-weight: 600; }',
    'header .title { flex: 1; }',
    'button { font: inherit; border: 1px solid #c9ccd1; background: #fff; border-radius: 8px; padding: 6px 12px; cursor: pointer; }',
    'button.primary { background: #1a1c1f; color: #fff; border-color: #1a1c1f; }',
    'button[disabled] { opacity: .5; cursor: default; }',
    'button.close { border: 0; font-size: 18px; line-height: 1; padding: 4px 8px; }',
    '.body { flex: 1; overflow: auto; padding: 16px; }',
    '.body.frame { padding: 0; display: flex; }',
    'iframe { border: 0; flex: 1; width: 100%; min-height: 360px; }',
    'footer { display: flex; justify-content: flex-end; gap: 8px; padding: 12px 16px; border-top: 1px solid #e3e5e8; }',
    'footer:empty { display: none; }',
    '::slotted(ui-title-bar), ::slotted(ui-save-bar) { display: none !important; }',
  ].join('\n');

  function el(tag, attributes, children) {
    const element = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes || {})) element.setAttribute(name, value);
    for (const child of children || []) element.append(child);
    return element;
  }

  // The overlay of a modal: a shadow root over its element, so that the app's own children stay
  // where they are (React keeps them) and only show while it is open.
  class Overlay {
    constructor(host) {
      this.host = host;
      this.root = host.shadowRoot || host.attachShadow({ mode: 'open' });
      this.frame = null;
      this.title = el('span', { class: 'title' });
      this.footer = el('footer');
      this.body = el('div', { class: 'body' }, [el('slot')]);
      const close = el('button', { class: 'close', type: 'button', 'aria-label': 'Close' }, ['×']);
      close.addEventListener('click', () => this.host.hide ? this.host.hide() : hideOverlay(this.host));
      const box = el('div', { class: 'box', role: 'dialog', 'aria-modal': 'true' }, [
        el('header', {}, [this.title, close]),
        this.body,
        this.footer,
      ]);
      const style = el('style');
      style.textContent = MODAL_STYLE;
      this.root.append(style, box);
    }

    // The title and the buttons: those of a `<ui-title-bar>` child, or the `heading` and the
    // slotted actions of an `s-modal`.
    render() {
      const host = this.host;
      const variant = host.getAttribute('variant') || 'base';
      host.setAttribute('data-lsf-variant', variant);
      if (host.style && host.style.setProperty) host.style.setProperty('--lsf-modal-width', SIZES[variant] || SIZES.base);
      const bar = elementChildren(host).find((child) => child.localName === 'ui-title-bar');
      this.title.textContent = (bar && bar.getAttribute('title')) || host.getAttribute('heading') || '';
      this.footer.replaceChildren();
      if (bar) {
        const buttons = elementChildren(bar).filter((child) => child.localName === 'button' || child.localName === 'a');
        buttons.sort((a, b) => (a.getAttribute('variant') === 'primary') - (b.getAttribute('variant') === 'primary'));
        for (const source of buttons) {
          const button = el('button', { type: 'button', class: source.getAttribute('variant') === 'primary' ? 'primary' : '' }, [text(source)]);
          if (source.hasAttribute('disabled')) button.setAttribute('disabled', '');
          button.addEventListener('click', () => source.click());
          this.footer.append(button);
        }
      } else if (host.localName === 's-modal') {
        this.footer.append(el('slot', { name: 'secondary-actions' }), el('slot', { name: 'primary-action' }));
      }
      const src = host.getAttribute('src');
      if (src) {
        if (!this.frame || this.frame.getAttribute('src') !== src) {
          this.frame = el('iframe', { src, name: 'lsf-modal:' + ++modalCount, title: this.title.textContent || 'Modal' });
          this.body.replaceChildren(this.frame);
          this.body.setAttribute('class', 'body frame');
        }
      } else if (this.frame) {
        this.frame = null;
        this.body.replaceChildren(el('slot'));
        this.body.setAttribute('class', 'body');
      }
    }
  }

  const overlays = new WeakMap();
  function overlayOf(host) {
    if (!overlays.has(host)) overlays.set(host, new Overlay(host));
    return overlays.get(host);
  }

  function showOverlay(host) {
    if (host.hasAttribute('data-lsf-open')) return Promise.resolve();
    overlayOf(host).render();
    host.setAttribute('data-lsf-open', '');
    modalShown(host);
    host.dispatchEvent(new win.CustomEvent('show'));
    return Promise.resolve();
  }

  function hideOverlay(host) {
    if (!host.hasAttribute('data-lsf-open')) return Promise.resolve();
    host.removeAttribute('data-lsf-open');
    modalHidden(host);
    host.dispatchEvent(new win.CustomEvent('hide'));
    return Promise.resolve();
  }

  // The modals open in the page, lsf's overlays and Polaris's `s-modal`s alike: while there is
  // one, the admin lifts the frame over the whole window (`modal.lift`), so that the modal is
  // over the whole admin, as on Shopify.
  const shownModals = new Set();
  function modalShown(host) {
    if (shownModals.has(host)) return;
    shownModals.add(host);
    if (shownModals.size === 1) post({ type: 'modal', open: true });
  }
  function modalHidden(host) {
    if (!shownModals.delete(host)) return;
    if (!shownModals.size) post({ type: 'modal', open: false });
  }
  // Polaris draws its own `s-modal`, and says when it shows and hides it.
  document.addEventListener(
    'show',
    (event) => {
      const host = event && event.target;
      if (host && host.localName === 's-modal' && !host.hasAttribute('data-lsf-open')) modalShown(host);
    },
    true,
  );
  document.addEventListener(
    'hide',
    (event) => {
      const host = event && event.target;
      if (host && host.localName === 's-modal' && !host.hasAttribute('data-lsf-open')) modalHidden(host);
    },
    true,
  );

  // The frame over the whole window: the page keeps its box (`left`, `top`, `width`, `height`
  // of the frame in the admin, the frame's `background` and `radius`), and the rest of the frame
  // is transparent, so that the admin shows through it, under the modal's backdrop.
  let lifted = null;
  function lift(box) {
    const html = document.documentElement;
    const body = document.body;
    if (!html || !body || !box) return;
    if (!lifted) {
      lifted = { html: html.getAttribute('style'), body: body.getAttribute('style'), x: win.scrollX || 0, y: win.scrollY || 0 };
    }
    const computed = (element) => (typeof win.getComputedStyle === 'function' ? win.getComputedStyle(element) : null);
    const opaque = (color) => color && color !== 'transparent' && !/^rgba\(.*,\s*0\)$/.test(color);
    const own = computed(body);
    const page = computed(html);
    const background = (own && opaque(own.backgroundColor) && own.backgroundColor) || (page && opaque(page.backgroundColor) && page.backgroundColor) || box.background || '#fff';
    const margin = (side) => (own ? parseFloat(own['margin' + side]) || 0 : 0);
    const px = (value) => Math.max(0, Math.round(Number(value) || 0)) + 'px';
    const important = (declarations) => declarations.map(([name, value]) => name + ': ' + value + ' !important').join('; ');
    // A transparent image, not just a transparent color: with none, the body's background would
    // paint the whole frame (CSS gives the root's canvas the body's background).
    html.setAttribute(
      'style',
      (lifted.html ? lifted.html + '; ' : '') +
        important([['background', 'linear-gradient(transparent, transparent)'], ['overflow', 'hidden']]),
    );
    body.setAttribute(
      'style',
      (lifted.body ? lifted.body + '; ' : '') +
        important([
          ['position', 'fixed'],
          ['left', px(box.left)],
          ['top', px(box.top)],
          ['width', px(box.width - margin('Left') - margin('Right'))],
          ['height', px(box.height - margin('Top') - margin('Bottom'))],
          ['overflow', 'hidden'],
          ['background', background],
          ['border-radius', px(box.radius)],
        ]),
    );
    html.setAttribute('data-lsf-lifted', '');
    body.scrollTop = lifted.y;
    body.scrollLeft = lifted.x;
    post({ type: 'lifted' });
  }
  function unlift() {
    if (!lifted) return;
    const html = document.documentElement;
    const body = document.body;
    const put = (element, style) => (style === null ? element.removeAttribute('style') : element.setAttribute('style', style));
    put(html, lifted.html);
    put(body, lifted.body);
    html.removeAttribute('data-lsf-lifted');
    const { x, y } = lifted;
    lifted = null;
    if (typeof win.scrollTo === 'function') win.scrollTo(x, y);
  }
  on('modal.lift', (box) => {
    if (shownModals.size) lift(box);
  });
  on('modal.unlift', () => unlift());

  function modalElement(id) {
    const element = findById(id, ['ui-modal', 's-modal', 's-app-window']);
    if (!element) throw new Error('shopify.modal: the page has no <ui-modal>, <s-modal> or <s-app-window> with id "' + id + '"');
    return element;
  }

  const modal = {
    async show(id) {
      const element = modalElement(id);
      if (typeof element.showOverlay === 'function') return element.showOverlay();
      if (typeof element.show === 'function') return element.show();
      return showOverlay(element);
    },
    async hide(id) {
      const element = modalElement(id);
      if (typeof element.hideOverlay === 'function') return element.hideOverlay();
      if (typeof element.hide === 'function') return element.hide();
      return hideOverlay(element);
    },
    async toggle(id) {
      const element = modalElement(id);
      if (typeof element.toggleOverlay === 'function') return element.toggleOverlay();
      const open = element.hasAttribute('data-lsf-open');
      return open ? modal.hide(id) : modal.show(id);
    },
  };

  // The elements of App Bridge: `ui-modal`, `s-app-window`, `ui-save-bar`, and those App Bridge
  // reads but does not draw (`ui-title-bar`, `ui-nav-menu`, `s-app-nav`).
  function defineElements() {
    if (!win.customElements || typeof win.HTMLElement !== 'function') return;
    const define = (name, make) => {
      if (!win.customElements.get(name)) win.customElements.define(name, make());
    };
    const modalClass = () =>
      class extends win.HTMLElement {
        static get observedAttributes() {
          return ['src', 'variant', 'heading'];
        }
        attributeChangedCallback() {
          if (this.hasAttribute('data-lsf-open')) overlayOf(this).render();
        }
        get src() {
          return this.getAttribute('src') || undefined;
        }
        set src(value) {
          if (value) this.setAttribute('src', value);
          else this.removeAttribute('src');
        }
        get variant() {
          return this.getAttribute('variant') || 'base';
        }
        set variant(value) {
          this.setAttribute('variant', value);
        }
        get content() {
          return elementChildren(this).find((child) => !['ui-title-bar', 'ui-save-bar'].includes(child.localName));
        }
        get contentWindow() {
          const frame = overlays.has(this) ? overlays.get(this).frame : null;
          return this.hasAttribute('data-lsf-open') && frame ? frame.contentWindow : null;
        }
        show() {
          return showOverlay(this);
        }
        hide() {
          return hideOverlay(this);
        }
        toggle() {
          return this.hasAttribute('data-lsf-open') ? this.hide() : this.show();
        }
      };
    define('ui-modal', modalClass);
    define('s-app-window', () => {
      const base = modalClass();
      return class extends base {
        connectedCallback() {
          this.setAttribute('variant', 'max');
        }
      };
    });
    define('ui-save-bar', () =>
      class extends win.HTMLElement {
        get showing() {
          return shownBar === this;
        }
        get discardConfirmation() {
          return this.hasAttribute('discardconfirmation');
        }
        set discardConfirmation(value) {
          this.toggleAttribute('discardconfirmation', Boolean(value));
        }
        show() {
          if (shownBar !== this) {
            shownBar = this;
            leaving = false;
            this.dispatchEvent(new win.CustomEvent('show'));
            sync();
          }
          return Promise.resolve();
        }
        hide() {
          if (shownBar === this) {
            shownBar = null;
            this.dispatchEvent(new win.CustomEvent('hide'));
            sync();
          }
          return Promise.resolve();
        }
        toggle() {
          return shownBar === this ? this.hide() : this.show();
        }
      },
    );
    for (const name of ['ui-title-bar', 'ui-nav-menu', 's-app-nav']) {
      define(name, () => class extends win.HTMLElement {});
    }
  }

  // What App Bridge draws in the admin is not drawn in the page.
  function hideAdminElements() {
    const style = document.createElement('style');
    style.setAttribute('data-lsf-app-bridge', '');
    style.textContent = [
      'ui-title-bar, ui-nav-menu, s-app-nav, ui-save-bar, s-menu[data-lsf-title-bar-menu] { display: none !important; }',
      // A modal's content shows only while it is open (Polaris hides its own `s-modal`).
      'ui-modal:not([data-lsf-open]), s-app-window:not([data-lsf-open]), s-modal:not(:defined):not([data-lsf-open]) { display: none !important; }',
      's-page > [slot="primary-action"], s-page > [slot="secondary-actions"], s-page > [slot="breadcrumb-actions"], s-page > [slot="accessory"] { display: none !important; }',
    ].join('\n');
    (document.head || document.documentElement).appendChild(style);
  }

  // ---- keeping the admin up to date -----------------------------------------------------------

  const sent = {};
  let scheduled = false;

  function send(type, value) {
    const json = JSON.stringify(value);
    if (sent[type] === json) return;
    sent[type] = json;
    post(Object.assign({ type }, value));
  }

  function sync() {
    if (scheduled) return;
    scheduled = true;
    win.setTimeout(() => {
      scheduled = false;
      flush();
    }, 0);
  }

  function flush() {
    trackForms();
    send('title', readTitle());
    send('nav', readNav());
    const bar = saveBarState();
    if (bar.visible) leaving = false;
    send('saveBar', bar);
  }

  function start() {
    readConfig(config);
    if (!config.shop && welcome) config.shop = welcome.shop;
    if (!config.apiKey) {
      console.error(
        'lsf App Bridge: the page names no API key: give the script data-api-key="<client_id>", or a <meta name="shopify-api-key">.',
      );
    }
    if (!framed) {
      // Outside an admin, App Bridge sends the page to the admin, unless told not to.
      if (!disabled('auto-redirect') && lsfOrigin && config.apiKey && config.shop) {
        const here = new URL(win.location.href);
        const params = new URLSearchParams();
        here.searchParams.forEach((value, key) => {
          if (!LAUNCH_PARAMS.includes(key)) params.append(key, value);
        });
        params.set('shop', config.shop);
        const target = lsfOrigin + '/admin/apps/' + encodeURIComponent(config.apiKey) + here.pathname + '?' + params.toString();
        win.location.assign(target);
      }
      return;
    }
    // The bounce page of Shopify's libraries (`/auth/session-token?shopify-reload=<url>`): the
    // page comes back with an ID token, as App Bridge brings it back.
    const reload = new URL(win.location.href).searchParams.get('shopify-reload');
    if (reload && !new URL(win.location.href).searchParams.get('id_token')) {
      bounce(reload);
      return;
    }
    if (typeof win.MutationObserver === 'function') {
      new win.MutationObserver(sync).observe(document.documentElement, {
        subtree: true,
        childList: true,
        attributes: true,
        characterData: true,
      });
    }
    reportLocation('replace');
    flush();
  }

  async function bounce(reload) {
    const target = new URL(reload, win.location.href);
    if (target.origin !== win.location.origin) {
      console.error('?shopify-reload must be same-origin (' + target.origin + ' !== ' + win.location.origin + ')');
      return;
    }
    try {
      target.searchParams.set('id_token', await idToken());
    } catch (error) {
      console.error('lsf App Bridge: no ID token for ?shopify-reload: ' + error.message);
      return;
    }
    win.location.replace(target.href);
  }

  // ---- the global -----------------------------------------------------------------------------

  const NOT_POS = ['fetch', 'subscribe', 'setCustomer', 'removeCustomer', 'addAddress', 'updateAddress', 'applyCartDiscount',
    'applyCartCodeDiscount', 'removeCartDiscount', 'removeAllDiscounts', 'addCartProperties', 'removeCartProperties',
    'addCustomSale', 'clear', 'addLineItem', 'updateLineItem', 'removeLineItem', 'setLineItemDiscount',
    'removeLineItemDiscount', 'addLineItemProperties', 'removeLineItemProperties'];
  const cart = {};
  for (const name of NOT_POS) {
    cart[name] = name === 'subscribe' ? warning('pos.cart.subscribe', () => () => {}) : rejecting('pos.cart.' + name);
  }

  const shopify = {
    config,
    origin: lsfOrigin,
    ready,
    setSignals() {
      // lsf answers no reactive values: there is nothing to move to the app's signals.
    },
    environment: { mobile: false, embedded: Boolean(framed), pos: false, intent: false },
    loading,
    idToken,
    user,
    toast,
    resourcePicker,
    scanner: { capture: rejecting('scanner.capture') },
    modal,
    saveBar,
    pos: {
      cart,
      close: rejecting('pos.close'),
      device: rejecting('pos.device'),
      location: rejecting('pos.location'),
    },
    intents: {
      request: {
        value: null,
        subscribe() {
          return () => {};
        },
      },
      invoke: invokeIntent,
      register: warning('intents.register', () => () => {}),
    },
    webVitals: { onReport: rejecting('webVitals.onReport') },
    support: { registerHandler: rejecting('support.registerHandler') },
    reviews: { request: rejecting('reviews.request') },
    scopes: {
      async query() {
        if (!framed) throw notInAdmin();
        await ready;
        return plain(welcome.scopes);
      },
      request: rejecting('scopes.request'),
      revoke: rejecting('scopes.revoke'),
    },
    picker,
    app: { extensions: rejecting('app.extensions') },
    shopifyQL: { available: rejecting('shopifyQL.available'), ready: rejecting('shopifyQL.ready') },
    sidekick: {
      isEnabled: rejecting('sidekick.isEnabled'),
      isOpen: rejecting('sidekick.isOpen'),
      open: rejecting('sidekick.open'),
      launch: rejecting('sidekick.launch'),
    },
    tools: {
      register: warning('tools.register', () => () => {}),
      unregister: warning('tools.unregister'),
      clear: warning('tools.clear'),
    },
  };
  Object.defineProperty(shopify, 'lsf', { value: { version: VERSION, admin: lsfOrigin }, enumerable: false });
  Object.defineProperty(win, 'shopify', { configurable: true, writable: true, value: shopify });

  defineElements();
  hideAdminElements();
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})(globalThis);

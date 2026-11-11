// lsf's admin around an embedded app (`/admin/apps/<app>`): the page draws what App Bridge asks
// for (the title bar, the app's navigation, the save bar, toasts, the loading bar, the resource
// picker), mints the ID tokens, and keeps its URL on the app's path. The app runs in the frame,
// on its own origin, with lsf's App Bridge (`/__lsf/admin/app-bridge.js`), which talks to this
// page with `postMessage`; each side checks the other's origin.
//
// It also says what goes wrong: an app that runs Shopify's App Bridge (which talks to Shopify's
// admin only), an app whose page loads no App Bridge, a call lsf does not simulate.

const LAUNCH_PARAMS = ['embedded', 'hmac', 'host', 'id_token', 'locale', 'session', 'shop', 'timestamp'];
const NOUNS = {
  product: ['product', 'products'],
  variant: ['variant', 'variants'],
  collection: ['collection', 'collections'],
};

/** The search the resource picker sends to lsf, in Shopify's syntax: the hidden filter, what the
 * merchant typed, and the product filters (`draft`, `archived`, `hidden`). */
export function searchQuery(options, typed) {
  const filter = options.filter ?? {};
  const parts = [];
  if (filter.query) parts.push(`(${filter.query})`);
  if (typed && typed.trim()) parts.push(`(${typed.trim()})`);
  if (options.type !== 'collection') {
    if (filter.draft === false) parts.push('-status:draft');
    if (filter.archived === false) parts.push('-status:archived');
  }
  if (filter.hidden === false) parts.push('published_status:published');
  return parts.join(' AND ');
}

/** The app's path as the admin's URL shows it: without the parameters of the launch, and without
 * the path of the app's URL (`application_url`) it starts with. */
export function appPathOf(path, base) {
  const url = new URL(path, 'http://app.invalid');
  for (const name of LAUNCH_PARAMS) url.searchParams.delete(name);
  let pathname = url.pathname;
  const prefix = base ? new URL(base).pathname.replace(/\/+$/, '') : '';
  if (prefix && (pathname === prefix || pathname.startsWith(`${prefix}/`))) pathname = pathname.slice(prefix.length) || '/';
  return pathname + url.search + url.hash;
}

/** The admin's URL of a path of the app: `/admin/apps/<app><path>`, with the admin's own
 * parameters (`shop`, `user`, `locale`). */
export function adminHref(base, path, query = {}) {
  const url = new URL(path === '/' ? base : `${base}${path}`, 'http://admin.invalid');
  for (const [name, value] of Object.entries(query)) url.searchParams.set(name, value);
  return url.pathname + url.search + url.hash;
}

/** The numeric id of a GID: `gid://shopify/Product/12` is `12`. */
const numeric = (gid) => String(gid).split('/').pop();

/** Starts the admin on `document`. Everything it touches comes in `env`, so that the tests run
 * it on a fake page. Returns what the tests look at. */
export function startAdmin(env) {
  const { window, document, config, location, history } = env;
  const fetch = env.fetch;
  const frame = document.querySelector('.lsf-frame');
  const root = document.querySelector('.lsf-admin');
  const query = (selector) => document.querySelector(selector);
  const el = (tag, attributes = {}, children = []) => {
    const element = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes)) {
      if (value === false || value === null || value === undefined) continue;
      element.setAttribute(name, value === true ? '' : String(value));
    }
    for (const child of children) element.append(typeof child === 'string' ? document.createTextNode(child) : child);
    return element;
  };
  const appName = config.app?.name ?? '';
  const state = {
    connected: false,
    saveBar: { visible: false },
    title: null,
    nav: { items: [], home: null },
    unsupported: new Set(),
    cdn: false,
    path: config.frame?.path ?? '/',
    toasts: new Map(),
  };

  // ---- talking to the frame ------------------------------------------------------------------

  const toFrame = (message) => {
    if (!frame?.contentWindow || !config.frame) return;
    frame.contentWindow.postMessage({ lsf: 'admin', v: 1, ...message }, config.frame.origin);
  };
  const emit = (name, detail = {}) => toFrame({ type: 'event', name, detail });

  const requests = {
    async idToken() {
      const response = await fetch(config.endpoints.sessionToken, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ shop: config.shop, user: config.user?.id }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error ?? `lsf answered ${response.status}`);
      return body.token;
    },
    resourcePicker: (options) => openResourcePicker(options),
    picker: (options) => openPicker(options),
    async leaveConfirmation() {
      shake();
      return confirmDialog({
        title: 'Leave this page?',
        message: 'The changes you have not saved will be lost.',
        cancel: 'Stay',
        confirm: 'Leave page',
      });
    },
  };

  const notifications = {
    title: (message) => renderTitle(message),
    nav: (message) => renderNav(message),
    saveBar: (message) => renderSaveBar(message),
    loading: ({ on }) => {
      query('.lsf-progress').hidden = !on;
    },
    toast: (message) => showToast(message),
    'toast-hide': ({ id }) => hideToast(id),
    history: ({ path }) => followApp(path),
    navigate: ({ url, target }) => navigate(url, target),
    modal: ({ open }) => root.classList.toggle('lsf-modal-open', Boolean(open)),
    unload: () => frameUnloaded(),
    unsupported: ({ name }) => {
      state.unsupported.add(name);
      banner(
        'unsupported',
        'warning',
        `This page called what lsf does not simulate: ${[...state.unsupported].join(', ')}. It rejected (or warned) in the page; see docs/admin.md for what is simulated.`,
      );
    },
  };

  // Shopify's App Bridge posts to any parent before it knows the admin: what it sends says it.
  const isShopifyAppBridge = (data) =>
    data && typeof data === 'object' && !data.lsf && typeof data.type === 'string' &&
    ['dispatch', 'getState', 'subscribe', 'unsubscribe'].includes(data.type) &&
    data.source && typeof data.source === 'object' && 'apiKey' in data.source;

  async function receive(event) {
    if (!frame || event.source !== frame.contentWindow) return;
    const data = event.data;
    if (isShopifyAppBridge(data)) {
      if (!state.cdn) {
        state.cdn = true;
        clearTimeout(state.waiting);
        banner(
          'app-bridge',
          'critical',
          `This page runs Shopify's App Bridge (cdn.shopify.com/shopifycloud/app-bridge.js), which talks to Shopify's admin only: its ID tokens, toasts, title bar and navigation cannot reach this admin, and its authenticated fetch() calls wait forever. Load lsf's App Bridge first in the page's <head>: <script src="${location.origin}${config.endpoints.appBridge}"></script>. Shopify's script then does nothing. See "Preview the embedded app" in docs/admin.md.`,
        );
      }
      return;
    }
    if (!data || typeof data !== 'object' || data.lsf !== 'app-bridge') return;
    if (event.origin !== config.frame.origin) {
      console.warn(`lsf admin: a message of ${event.origin} in the app's frame, which is ${config.frame.origin}: ignored`);
      return;
    }
    if (data.type === 'hello') {
      clearTimeout(state.waiting);
      removeBanner('app-bridge');
      if (data.scriptOrigin && data.scriptOrigin !== location.origin) {
        banner(
          'origin',
          'critical',
          `The app loaded lsf's App Bridge from ${data.scriptOrigin}, but this admin is at ${location.origin}: they cannot talk. Open the admin at ${data.scriptOrigin}${location.pathname}${location.search}.`,
        );
        return;
      }
      state.connected = true;
      toFrame({
        type: 'welcome',
        shop: config.shop,
        locale: config.locale,
        clientId: config.app?.clientId,
        user: config.user
          ? { id: config.user.id, name: config.user.name, email: config.user.email, accountAccess: config.user.accountAccess, locale: config.user.locale }
          : null,
        scopes: config.app?.scopes ?? { granted: [], required: [], optional: [] },
        origin: location.origin,
      });
      return;
    }
    if (data.type === 'request') {
      const answer = requests[data.method];
      if (!answer) {
        toFrame({ type: 'response', id: data.id, ok: false, error: `lsf's admin does not answer ${data.method}` });
        return;
      }
      try {
        const value = await answer(data.params);
        toFrame({ type: 'response', id: data.id, ok: true, value: value === undefined ? null : value });
      } catch (error) {
        toFrame({ type: 'response', id: data.id, ok: false, error: error?.message ?? String(error) });
      }
      return;
    }
    notifications[data.type]?.(data);
  }
  window.addEventListener('message', receive);

  // The page of the frame went away: what it asked for goes with it, until the next one says it
  // again.
  function frameUnloaded() {
    state.connected = false;
    renderSaveBar({ visible: false });
    query('.lsf-progress').hidden = true;
    root.classList.remove('lsf-modal-open');
  }

  // A page loaded in the frame: its App Bridge says hello soon, or the admin says why not.
  function frameLoaded() {
    clearTimeout(state.waiting);
    state.waiting = setTimeout(() => {
      if (state.connected || state.cdn) return;
      banner(
        'app-bridge',
        'warning',
        `No App Bridge answered from the app's page. Is the backend running at ${config.frame.origin}, and did the page load (an error page has no App Bridge)? A page that loads Shopify's App Bridge cannot load it offline, and online it talks only to Shopify's admin: load lsf's first in the page's <head>: <script src="${location.origin}${config.endpoints.appBridge}"></script>. And the app must let this admin frame it: Content-Security-Policy frame-ancestors ${location.origin}.`,
      );
    }, env.waitForAppBridge ?? 4000);
  }
  frame?.addEventListener('load', frameLoaded);

  // ---- banners ---------------------------------------------------------------------------------

  function banner(key, tone, message) {
    const banners = query('.lsf-banners');
    let existing = banners.querySelector(`[data-banner="${key}"]`);
    if (!existing) {
      existing = el('div', { class: `lsf-banner lsf-banner-${tone}`, 'data-banner': key, role: tone === 'critical' ? 'alert' : 'status' });
      banners.append(existing);
    }
    const close = el('button', { class: 'lsf-banner-close', type: 'button', 'aria-label': 'Dismiss' }, ['×']);
    close.addEventListener('click', () => existing.remove());
    existing.replaceChildren(el('p', {}, [message]), close);
  }
  function removeBanner(key) {
    query('.lsf-banners').querySelector(`[data-banner="${key}"]`)?.remove();
  }

  // ---- the title bar ---------------------------------------------------------------------------

  function actionButton(action, primary) {
    const button = el(
      'button',
      {
        type: 'button',
        class: ['lsf-button', primary ? 'lsf-button-primary' : '', action.tone === 'critical' ? 'lsf-button-critical' : ''].filter(Boolean).join(' '),
        disabled: action.disabled || action.loading,
        'aria-busy': action.loading ? 'true' : null,
      },
      [action.label || '…'],
    );
    button.addEventListener('click', () => emit('title.action', { index: action.index }));
    return button;
  }

  function renderTitle(title) {
    state.title = title;
    const heading = title.title || appName;
    query('.lsf-titlebar-title').textContent = heading;
    document.title = `${heading} · ${config.shopName}`;
    const breadcrumb = query('.lsf-titlebar-breadcrumb');
    breadcrumb.replaceChildren();
    if (title.breadcrumb) {
      const back = el('button', { type: 'button', class: 'lsf-titlebar-back', title: title.breadcrumb.label }, ['‹ ', title.breadcrumb.label]);
      back.addEventListener('click', () => emit('title.action', { index: title.breadcrumb.index }));
      breadcrumb.append(back);
    }
    const accessory = query('.lsf-titlebar-accessory');
    accessory.replaceChildren();
    if (title.accessory?.label) {
      accessory.append(el('span', { class: `lsf-badge lsf-badge-${title.accessory.tone ?? 'neutral'}` }, [title.accessory.label]));
    }
    const actions = query('.lsf-titlebar-actions');
    actions.replaceChildren();
    for (const entry of title.secondary ?? []) {
      if (entry.group) {
        const menu = el('div', { class: 'lsf-menu' });
        for (const action of entry.actions) menu.append(actionButton(action, false));
        const details = el('details', { class: 'lsf-menu-group' }, [el('summary', { class: 'lsf-button' }, [entry.label || 'More actions']), menu]);
        actions.append(details);
      } else {
        actions.append(actionButton(entry, false));
      }
    }
    if (title.primary) actions.append(actionButton(title.primary, true));
  }

  // ---- the app's navigation --------------------------------------------------------------------

  function renderNav(nav) {
    state.nav = nav;
    const list = query('.lsf-nav-items');
    list.replaceChildren();
    for (const item of nav.items) {
      const link = el('a', { class: 'lsf-nav-item', href: adminHref(config.app.base, appPathOf(item.path, config.frame?.base), config.query), 'data-path': item.path }, [item.label]);
      link.addEventListener('click', (event) => {
        event.preventDefault();
        goTo({ path: item.path });
      });
      list.append(el('li', {}, [link]));
    }
    markCurrent();
  }

  function goTo({ path, home }) {
    if (state.connected) {
      emit('navigate', home ? { home: true } : { path });
      return;
    }
    // No App Bridge to follow it: the admin opens the app there again.
    location.assign(adminHref(config.app.base, home ? '/' : appPathOf(path, config.frame?.base), config.query));
  }

  const appLink = query('.lsf-nav-app');
  appLink?.addEventListener('click', (event) => {
    if (!frame) return;
    event.preventDefault();
    if (state.nav.home) goTo({ home: true });
    else location.assign(adminHref(config.app.base, '/', config.query));
  });

  function markCurrent() {
    const current = state.path.split(/[?#]/)[0];
    let best = null;
    for (const link of document.querySelectorAll('.lsf-nav-item')) {
      link.removeAttribute('aria-current');
      const path = appPathOf(link.getAttribute('data-path'), config.frame?.base).split(/[?#]/)[0];
      const matches = current === path || (path !== '/' && current.startsWith(`${path.replace(/\/$/, '')}/`));
      if (matches && (!best || path.length > best.path.length)) best = { link, path };
    }
    if (best) best.link.setAttribute('aria-current', 'page');
    appLink?.toggleAttribute('aria-current', Boolean(frame) && !best);
  }

  // The app went somewhere: the admin's URL follows it, as Shopify's admin does.
  function followApp(path) {
    state.path = appPathOf(path, config.frame?.base);
    history.replaceState(history.state, '', adminHref(config.app.base, state.path, config.query));
    markCurrent();
  }

  // A URL of Shopify's admin (`https://admin.shopify.com/store/<store>/…`, or the older
  // `https://<store>.myshopify.com/admin/…`) as lsf's page of it, for that shop: where the app's
  // `redirect(…, { target: '_top' })` and `open(…, '_top')` send the merchant, such as the plan
  // selection page of managed pricing. `null` for another URL.
  function shopifyAdmin(url) {
    let parsed;
    try {
      parsed = new URL(url);
    } catch (_) {
      return null;
    }
    if (parsed.protocol !== 'https:') return null;
    let shop;
    let path;
    const store = parsed.hostname === 'admin.shopify.com' && parsed.pathname.match(/^\/store\/([^/]+)(\/.*)?$/);
    if (store) {
      shop = `${decodeURIComponent(store[1]).toLowerCase()}.myshopify.com`;
      path = store[2] ?? '';
    } else if (/\.myshopify\.com$/i.test(parsed.hostname) && /^\/admin(\/|$)/.test(parsed.pathname)) {
      shop = parsed.hostname.toLowerCase();
      path = parsed.pathname.slice('/admin'.length);
    } else {
      return null;
    }
    const admin = new URL(`/admin${path}${parsed.search}`, 'http://admin.invalid');
    for (const [name, value] of Object.entries(config.query ?? {})) admin.searchParams.set(name, value);
    admin.searchParams.set('shop', shop);
    return admin.pathname + admin.search + parsed.hash;
  }

  // A link of the app that leaves it: `shopify:admin/…` (a page of the admin, lsf's page of it),
  // a URL of Shopify's admin (lsf's page of it too), or a URL for the whole window.
  function navigate(url, target) {
    let href = url;
    if (url.startsWith('shopify:')) {
      const admin = new URL(url.slice('shopify:'.length), 'http://admin.invalid');
      for (const [name, value] of Object.entries(config.query ?? {})) admin.searchParams.set(name, value);
      href = admin.pathname + admin.search;
    } else if (!/^https?:/i.test(url)) {
      return;
    } else {
      href = shopifyAdmin(url) ?? url;
    }
    if (target === '_blank') window.open(href, '_blank', 'noopener');
    else location.assign(href);
  }

  // ---- the save bar ----------------------------------------------------------------------------

  function renderSaveBar(bar) {
    state.saveBar = bar;
    const element = query('.lsf-savebar');
    element.hidden = !bar.visible;
    root.classList.toggle('lsf-saving', Boolean(bar.visible));
    if (!bar.visible) return;
    const save = query('.lsf-savebar-save');
    const discard = query('.lsf-savebar-discard');
    save.toggleAttribute('disabled', Boolean(bar.save?.disabled || bar.save?.loading));
    discard.toggleAttribute('disabled', Boolean(bar.discard?.disabled || bar.discard?.loading));
    save.toggleAttribute('aria-busy', Boolean(bar.save?.loading));
  }

  function shake() {
    const element = query('.lsf-savebar');
    element.classList.remove('lsf-shake');
    // Read the layout so that the animation starts again.
    void element.getBoundingClientRect?.();
    element.classList.add('lsf-shake');
  }

  query('.lsf-savebar-save')?.addEventListener('click', () => emit('saveBar.save'));
  query('.lsf-savebar-discard')?.addEventListener('click', async () => {
    if (state.saveBar.discardConfirmation) {
      const discard = await confirmDialog({
        title: 'Discard your changes?',
        message: 'What you changed since the last save will be lost.',
        cancel: 'Keep editing',
        confirm: 'Discard changes',
        destructive: true,
      });
      if (!discard) return;
    }
    emit('saveBar.discard');
  });

  // Leaving the app (the brand of the top bar) with unsaved changes asks first. The links of
  // "Preview" open a new tab: they leave nothing.
  for (const link of document.querySelectorAll('.lsf-brand')) {
    link.addEventListener('click', async (event) => {
      if (!state.saveBar.visible) return;
      event.preventDefault();
      if (await requests.leaveConfirmation()) location.assign(link.getAttribute('href'));
    });
  }

  // ---- toasts ----------------------------------------------------------------------------------

  function showToast({ id, message, duration, isError, action }) {
    hideToast(id, false);
    const toast = el('div', { class: `lsf-toast${isError ? ' lsf-toast-error' : ''}`, 'data-toast': id }, [el('span', { class: 'lsf-toast-message' }, [message])]);
    if (action) {
      const button = el('button', { type: 'button', class: 'lsf-toast-action' }, [action]);
      button.addEventListener('click', () => emit('toast.action', { id }));
      toast.append(button);
    }
    const close = el('button', { type: 'button', class: 'lsf-toast-close', 'aria-label': 'Dismiss' }, ['×']);
    close.addEventListener('click', () => hideToast(id));
    toast.append(close);
    query('.lsf-toasts').append(toast);
    const timer = setTimeout(() => hideToast(id), Number.isFinite(duration) && duration > 0 ? duration : 5000);
    state.toasts.set(id, { element: toast, timer });
  }

  function hideToast(id, dismissed = true) {
    const toast = state.toasts.get(id);
    if (!toast) return;
    clearTimeout(toast.timer);
    toast.element.remove();
    state.toasts.delete(id);
    if (dismissed) emit('toast.dismiss', { id });
  }

  // ---- dialogs ---------------------------------------------------------------------------------

  function dialog(className, children) {
    const element = el('dialog', { class: `lsf-dialog ${className}` }, children);
    document.body.append(element);
    element.showModal();
    return element;
  }

  function confirmDialog({ title, message, cancel, confirm, destructive = false }) {
    return new Promise((resolve) => {
      const no = el('button', { type: 'button', class: 'lsf-button' }, [cancel]);
      const yes = el('button', { type: 'button', class: `lsf-button ${destructive ? 'lsf-button-critical' : 'lsf-button-primary'}` }, [confirm]);
      const element = dialog('lsf-confirm', [
        el('h2', {}, [title]),
        el('p', {}, [message]),
        el('div', { class: 'lsf-dialog-actions' }, [no, yes]),
      ]);
      let answered = false;
      const answer = (value) => {
        if (answered) return;
        answered = true;
        element.close();
        element.remove();
        resolve(value);
      };
      no.addEventListener('click', () => answer(false));
      yes.addEventListener('click', () => answer(true));
      element.addEventListener('cancel', () => answer(false));
      element.addEventListener('close', () => answer(false));
    });
  }

  // ---- the resource picker ---------------------------------------------------------------------

  async function resourcesOf(type, search, after) {
    const params = new URLSearchParams({ shop: config.shop, type, first: '50' });
    if (search) params.set('query', search);
    if (after) params.set('after', after);
    const response = await fetch(`${config.endpoints.resources}?${params}`);
    const body = await response.json();
    if (!response.ok) throw new Error(body.error ?? `lsf answered ${response.status}`);
    return body;
  }

  function openResourcePicker(options) {
    const type = options.type;
    const [singular, plural] = NOUNS[type];
    const limit = typeof options.multiple === 'number' ? Math.max(1, options.multiple) : options.multiple ? Infinity : 1;
    const verb = options.action === 'select' ? 'Select' : 'Add';
    const showVariants = type === 'variant' || (type === 'product' && options.filter?.variants !== false);
    const known = new Map();
    // What is chosen: the id of a product (or a collection), and its variants chosen.
    const chosen = new Map();
    let order = [];
    let page = null;

    const counted = () => (type === 'variant' ? [...chosen.values()].reduce((sum, set) => sum + set.size, 0) : chosen.size);
    const status = el('span', { class: 'lsf-picker-count' });
    const list = el('div', { class: 'lsf-picker-list', role: 'list' });
    const more = el('button', { type: 'button', class: 'lsf-button lsf-picker-more', hidden: true }, ['Load more']);
    const search = el('input', { type: 'search', class: 'lsf-picker-search', placeholder: `Search ${plural}`, value: options.query ?? '' });
    search.value = options.query ?? '';
    const cancel = el('button', { type: 'button', class: 'lsf-button' }, ['Cancel']);
    const confirm = el('button', { type: 'button', class: 'lsf-button lsf-button-primary' }, [verb]);
    const title = `${verb} ${limit === 1 ? singular : plural}`;
    const element = dialog('lsf-picker', [
      el('header', { class: 'lsf-dialog-header' }, [el('h2', {}, [title])]),
      search,
      list,
      more,
      el('footer', { class: 'lsf-dialog-actions' }, [status, cancel, confirm]),
    ]);

    const variantsOf = (resource) => resource.variants ?? [];
    const full = () => counted() >= limit;

    function choose(resource, variant, on) {
      const id = resource.id;
      if (on && limit === 1 && !(type === 'product' && chosen.has(id))) chosen.clear();
      if (type === 'collection') {
        if (on) chosen.set(id, new Set());
        else chosen.delete(id);
      } else if (variant) {
        const set = chosen.get(id) ?? new Set();
        if (on) set.add(variant.id);
        else set.delete(variant.id);
        if (set.size) chosen.set(id, set);
        else chosen.delete(id);
      } else if (on) {
        chosen.set(id, new Set(type === 'variant' && limit === 1 ? variantsOf(resource).slice(0, 1).map((v) => v.id) : variantsOf(resource).map((v) => v.id)));
      } else {
        chosen.delete(id);
      }
      draw();
    }

    function badge(resource) {
      const filter = options.filter ?? {};
      if (resource.status === 'DRAFT' && filter.draft === undefined) return el('span', { class: 'lsf-badge lsf-badge-info' }, ['Draft']);
      if (resource.status === 'ARCHIVED' && filter.archived === undefined) return el('span', { class: 'lsf-badge lsf-badge-neutral' }, ['Archived']);
      return null;
    }

    function row(resource, variant) {
      const on = variant ? Boolean(chosen.get(resource.id)?.has(variant.id)) : chosen.has(resource.id);
      const partial = !variant && on && type !== 'collection' && chosen.get(resource.id).size < variantsOf(resource).length;
      const box = el('input', { type: 'checkbox', 'aria-label': variant ? variant.title : resource.title });
      box.checked = on;
      box.indeterminate = partial;
      if (!on && full() && !(limit === 1)) box.setAttribute('disabled', '');
      box.addEventListener('change', () => choose(resource, variant, box.checked));
      const image = (variant?.image ?? resource.images?.[0] ?? resource.image)?.originalSrc;
      const thumbnail = image ? el('img', { class: 'lsf-thumb', src: image, alt: '' }) : el('span', { class: 'lsf-thumb lsf-thumb-empty' });
      const details = variant
        ? [variant.price, variant.sku ? `SKU ${variant.sku}` : null, variant.inventoryQuantity != null ? `${variant.inventoryQuantity} available` : null]
        : type === 'collection'
          ? [`${resource.productsCount} products`]
          : [resource.vendor, `${variantsOf(resource).length} variants`, `${resource.totalInventory} in stock`];
      const label = el('label', { class: `lsf-picker-row${variant ? ' lsf-picker-variant' : ''}`, role: 'listitem' }, [
        box,
        variant ? el('span', { class: 'lsf-thumb-space' }) : thumbnail,
        el('span', { class: 'lsf-picker-text' }, [
          el('span', { class: 'lsf-picker-title' }, [variant ? variant.title : resource.title]),
          el('span', { class: 'lsf-picker-details' }, [details.filter(Boolean).join(' · ')]),
        ]),
      ]);
      const status = !variant && badge(resource);
      if (status) label.append(status);
      return label;
    }

    function draw() {
      list.replaceChildren();
      for (const id of order) {
        const resource = known.get(id);
        list.append(row(resource));
        const variants = variantsOf(resource);
        if (showVariants && !(resource.hasOnlyDefaultVariant && type === 'product')) {
          for (const variant of variants) list.append(row(resource, variant));
        }
      }
      if (!order.length) list.append(el('p', { class: 'lsf-picker-empty' }, [page ? `No ${plural} found` : 'Loading…']));
      more.hidden = !page?.pageInfo?.hasNextPage;
      const count = counted();
      status.textContent = count ? `${count} ${count === 1 ? singular : plural} selected` : '';
      confirm.toggleAttribute('disabled', count === 0);
    }

    async function load(reset) {
      try {
        page = await resourcesOf(type, searchQuery(options, search.value), reset ? null : page?.pageInfo?.endCursor);
      } catch (error) {
        list.replaceChildren(el('p', { class: 'lsf-picker-empty' }, [error.message]));
        return;
      }
      if (reset) order = [];
      for (const resource of page.resources) {
        known.set(resource.id, resource);
        if (!order.includes(resource.id)) order.push(resource.id);
      }
      draw();
    }

    // What `selectionIds` preselects, fetched by id so that it is answered whole.
    async function preselect() {
      const ids = (options.selectionIds ?? []).filter((entry) => entry && entry.id);
      if (!ids.length) return;
      const filter = type === 'variant' ? 'variant_id' : 'id';
      const found = await resourcesOf(type, ids.map((entry) => `${filter}:${numeric(entry.id)}`).join(' OR ')).catch(() => ({ resources: [] }));
      for (const resource of found.resources) known.set(resource.id, resource);
      for (const entry of ids) {
        if (type === 'variant') {
          const product = found.resources.find((resource) => variantsOf(resource).some((variant) => variant.id === entry.id));
          if (product) chosen.set(product.id, new Set([...(chosen.get(product.id) ?? []), entry.id]));
        } else if (known.has(entry.id)) {
          const resource = known.get(entry.id);
          const variants = entry.variants?.length ? entry.variants.map((variant) => variant.id) : variantsOf(resource).map((variant) => variant.id);
          chosen.set(entry.id, new Set(type === 'collection' ? [] : variants));
        }
      }
    }

    let typing;
    search.addEventListener('input', () => {
      clearTimeout(typing);
      typing = setTimeout(() => load(true), 250);
    });
    more.addEventListener('click', () => load(false));

    return new Promise((resolve) => {
      let answered = false;
      const answer = (value) => {
        if (answered) return;
        answered = true;
        element.close();
        element.remove();
        resolve(value);
      };
      cancel.addEventListener('click', () => answer(null));
      element.addEventListener('cancel', () => answer(null));
      element.addEventListener('close', () => answer(null));
      confirm.addEventListener('click', () => {
        const picked = [];
        for (const [id, variants] of chosen) {
          const resource = known.get(id);
          if (!resource) continue;
          if (type === 'collection') picked.push(resource);
          else if (type === 'variant') picked.push(...variantsOf(resource).filter((variant) => variants.has(variant.id)));
          else picked.push({ ...resource, variants: showVariants ? variantsOf(resource).filter((variant) => variants.has(variant.id)) : variantsOf(resource) });
        }
        answer(picked);
      });
      draw();
      preselect().then(() => load(true));
    });
  }

  // ---- the picker of the app's own items -------------------------------------------------------

  function openPicker(options) {
    const limit = typeof options.multiple === 'number' ? Math.max(1, options.multiple) : options.multiple ? Infinity : 1;
    const chosen = new Set(options.items.filter((item) => item.selected && !item.disabled).map((item) => item.id));
    const cancel = el('button', { type: 'button', class: 'lsf-button' }, ['Cancel']);
    const confirm = el('button', { type: 'button', class: 'lsf-button lsf-button-primary' }, ['Select']);
    const table = el('table', { class: 'lsf-picker-table' });
    const status = el('span', { class: 'lsf-picker-count' });
    const element = dialog('lsf-picker', [
      el('header', { class: 'lsf-dialog-header' }, [el('h2', {}, [options.heading ?? 'Select'])]),
      table,
      el('footer', { class: 'lsf-dialog-actions' }, [status, cancel, confirm]),
    ]);
    function draw() {
      const head = el('tr', {}, [el('th', {}, ['']), el('th', {}, ['']), ...(options.headers ?? []).map((header) => el('th', { class: header.type === 'number' ? 'lsf-number' : null }, [header.content ?? '']))]);
      const rows = options.items.map((item) => {
        const box = el('input', { type: 'checkbox', 'aria-label': item.heading });
        box.checked = chosen.has(item.id);
        if (item.disabled || (!box.checked && chosen.size >= limit && limit !== 1)) box.setAttribute('disabled', '');
        box.addEventListener('change', () => {
          if (box.checked) {
            if (limit === 1) chosen.clear();
            chosen.add(item.id);
          } else {
            chosen.delete(item.id);
          }
          draw();
        });
        const badges = (item.badges ?? []).map((entry) => el('span', { class: `lsf-badge lsf-badge-${entry.tone ?? 'neutral'}` }, [entry.content]));
        const thumbnail = item.thumbnail?.url ? el('img', { class: 'lsf-thumb', src: item.thumbnail.url, alt: '' }) : el('span', {});
        return el('tr', {}, [
          el('td', {}, [box]),
          el('td', {}, [thumbnail]),
          el('td', {}, [el('span', { class: 'lsf-picker-title' }, [item.heading]), ...badges]),
          ...(item.data ?? []).map((value, index) => el('td', { class: options.headers?.[index]?.type === 'number' ? 'lsf-number' : null }, [value === undefined ? '' : String(value)])),
        ]);
      });
      table.replaceChildren(el('thead', {}, [head]), el('tbody', {}, rows));
      status.textContent = chosen.size ? `${chosen.size} selected` : '';
      confirm.toggleAttribute('disabled', chosen.size === 0);
    }
    draw();
    return new Promise((resolve) => {
      let answered = false;
      const answer = (value) => {
        if (answered) return;
        answered = true;
        element.close();
        element.remove();
        resolve(value);
      };
      cancel.addEventListener('click', () => answer(null));
      element.addEventListener('cancel', () => answer(null));
      element.addEventListener('close', () => answer(null));
      confirm.addEventListener('click', () => answer([...chosen]));
    });
  }

  markCurrent();
  // What shopify.app.toml gets wrong for the embedded app (no client_id, …).
  (config.warnings ?? []).forEach((message, index) => banner(`config-${index}`, 'warning', message));
  // The frame may have loaded before this module ran: wait for its App Bridge from now on.
  if (frame) frameLoaded();
  return { state, receive, frameLoaded, frameUnloaded, renderTitle, renderNav, openResourcePicker, openPicker };
}

// In the browser: the page's own config, its window and its document.
if (typeof document !== 'undefined' && document.getElementById?.('lsf-admin-config')) {
  const config = JSON.parse(document.getElementById('lsf-admin-config').textContent);
  startAdmin({ window, document, config, location: window.location, history: window.history, fetch: window.fetch.bind(window) });
}

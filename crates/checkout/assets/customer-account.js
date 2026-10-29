// The local customer account. The server renders a shell with the state of the account as JSON
// (`customer-account.rs`); this script draws the page from it (`account-views.js`) and hosts the
// customer account UI extensions (`extension-host.js`). Nothing on these pages changes the
// state: a navigation, a login or a switch of customer loads another page.

import { createExtensionHost } from './extension-host.js';
import { languageOf, withCurrency } from './format.js';
import { renderAccount } from './account-views.js';
import { h, render } from './vdom.js';

const TOAST_MS = 4000;

/**
 * Where `url` of `shopify.navigation.navigate` leads: a path of the local customer account, or
 * null when it is not one (nothing happens then, as on Shopify).
 *
 * - `shopify:customer-account/orders`, `…/orders/<id>`, `…/profile`: the pages of the account;
 * - `extension://<handle>` or `extension:<handle>/<path>`: the full page of that extension;
 * - `extension://<route>`, `extension:/<route>` or `/<route>`: a route of the full page the
 *   extension is on.
 *
 * `from` is what the frame is for: `{handle, path, order}` for a full page.
 */
export function resolveNavigation(url, { root, from = {}, handles = [] }) {
  const text = String(url ?? '');
  const own = (handle, path) => {
    const clean = String(path ?? '').replace(/^\/+/, '');
    const base = from.order ? `${root}/orders/${from.order}/pages/` : `${root}/pages/`;
    return `${base}${encodeURIComponent(handle)}${clean ? `/${clean}` : ''}`;
  };
  let match = /^shopify:\/{0,2}customer-account(\/[^?#]*)?$/.exec(text);
  if (match) {
    const rest = (match[1] ?? '').replace(/\/+$/, '');
    if (rest === '' || rest === '/orders') return root;
    if (rest === '/profile') return `${root}/profile`;
    const order = /^\/orders\/(?:gid:\/\/shopify\/Order\/)?(\d+)$/.exec(rest);
    return order ? `${root}/orders/${order[1]}` : null;
  }
  match = /^extension:(?:\/\/)?([^/?#]*)(\/[^?#]*)?$/.exec(text);
  if (match) {
    const first = decodeURIComponent(match[1]);
    if (first && handles.includes(first)) return own(first, match[2]);
    if (!from.handle) return null;
    return own(from.handle, `${first ? `/${first}` : ''}${match[2] ?? ''}`);
  }
  if (text.startsWith('/') && !text.startsWith('//') && from.handle) return own(from.handle, text);
  return null;
}

/** Starts the page in `root` from `initial` (the state JSON). */
export function startAccount({ root, initial, document, window, location, storage, sandboxUrl, console: log = console }) {
  const state = withCurrency(initial);
  const ui = { menuOpen: false, action: null, toasts: [] };
  const here = (() => {
    try {
      const url = new URL(location.href);
      return `${url.pathname}${url.search}`;
    } catch {
      return state.root;
    }
  })();
  const showAll = new URL(location.href ?? 'http://localhost/', 'http://localhost/').searchParams.get('targets') === 'all';
  const preview = showAll ? { all: true } : null;
  /** The extensions that have a full page, which `extension://<handle>` names. */
  const handles = (state.extensions ?? [])
    .filter((ext) => ext.targets?.includes('customer-account.page.render'))
    .map((ext) => ext.handle);
  let toastCounter = 0;

  const host = createExtensionHost({
    document,
    window,
    sandboxUrl,
    storage,
    getState: () => state,
    // Nothing an extension of the customer account asks changes the account.
    dispatch: () => Promise.reject(new Error('The customer account has nothing to change.')),
    onRequest(request, instance) {
      switch (request.type) {
        case 'navigate': {
          const href = resolveNavigation(request.url, { root: state.root, from: instance.ctx ?? {}, handles });
          if (href) {
            location.assign(href);
            return true;
          }
          if (/^https?:\/\//i.test(request.url ?? '')) return false;
          log.warn?.(`[${instance.ext.handle}] navigate: ${request.url} is not a page of the customer account`);
          return true;
        }
        case 'toast':
          if (request.hide) ui.toasts = ui.toasts.filter((toast) => toast.handle !== instance.ext.handle);
          else showToast(instance.ext.handle, request.content);
          paint();
          return true;
        case 'action':
          ui.action = { handle: instance.ext.handle, orderId: instance.ctx?.orderId ?? null };
          paint();
          return true;
        case 'close':
          ui.action = null;
          paint();
          return true;
        default:
          return false;
      }
    },
  });

  function showToast(handle, content) {
    const toast = { id: ++toastCounter, handle, content };
    ui.toasts = [...ui.toasts, toast];
    setTimeout(() => {
      ui.toasts = ui.toasts.filter((entry) => entry !== toast);
      paint();
    }, TOAST_MS);
  }

  function slot(target, instance = {}) {
    if (!host.hasTargetFor(target, instance.handle)) return null;
    return h('div', {
      class: 'lsf-slot',
      key: `slot:${target}:${instance.key ?? ''}`,
      opaque: true,
      'data-target': target,
      hooks: {
        insert: (el) => host.attach(el, target, instance),
        destroy: (el) => host.detach(el),
      },
    });
  }

  const actions = {
    toggleMenu() {
      ui.menuOpen = !ui.menuOpen;
      paint();
    },
    closeAction() {
      ui.action = null;
      paint();
    },
  };

  function paint() {
    if (document.documentElement) document.documentElement.lang = languageOf(state);
    render(root, renderAccount({ state, ui, slot, actions, here, preview, refusals: host.refusals(), extensionErrors: host.errors() }));
    host.update(state);
  }

  paint();
  return { actions, ui, host, paint, getState: () => state };
}

if (typeof document !== 'undefined' && document.getElementById?.('lsf-account-state')) {
  const initial = JSON.parse(document.getElementById('lsf-account-state').textContent);
  document.title = `Account · ${initial.shop?.name ?? ''}`.trim();
  startAccount({
    root: document.getElementById('lsf-account'),
    initial,
    document,
    window,
    location,
    storage: (() => {
      try {
        return window.localStorage;
      } catch {
        return undefined;
      }
    })(),
    sandboxUrl: new URL('sandbox.html', import.meta.url).href,
  });
}

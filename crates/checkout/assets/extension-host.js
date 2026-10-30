// The page side of checkout and customer account UI extensions: one sandboxed iframe per
// (extension, target) placed where the target sits in the page, and the messages between page
// and iframe. A target that renders nothing (the address autocomplete) gets a hidden frame the
// page asks (`run`).
//
// The iframe (`sandbox.html`) runs the built bundle of the extension against a `shopify`
// global it builds from the state we send. Everything an extension changes comes back as a
// message, becomes operations (`operations.js`) and ends in a new state, sent to every iframe.

import api from './api.json' with { type: 'json' };
import { refusalOf } from './api-versions.js';
import { operationsForChange } from './operations.js';

/** The targets the checkout page renders: those of the checkout and the thank-you page. */
export const RENDERED_TARGETS = api.placements
  .filter((placement) => placement.surface === 'checkout' && !placement.runs)
  .map((placement) => placement.target);

/** The targets the checkout page runs, which render nothing: the address autocomplete. */
export const RUN_TARGETS = api.placements.filter((placement) => placement.runs).map((placement) => placement.target);

/** The targets the customer account renders. */
export const ACCOUNT_TARGETS = api.placements
  .filter((placement) => placement.surface === 'customer-account')
  .map((placement) => placement.target);

/** The part of the state extensions see: the runs of Functions are the page's business. */
export function stateForExtensions(state) {
  // `account` is the page's: the customers of the store data to log in as.
  // `builds` too: the builds of `--watch` that failed.
  const { runs, diagnostics, extensions, account, builds, profiles, ...visible } = state;
  return visible;
}

const base64url = (text) =>
  btoa(text).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

/** A session token like the one Shopify signs, with a signature nobody can verify. */
export function sessionToken(state, ext, now = Date.now()) {
  const shop = (state.shop?.storefrontUrl ?? 'http://localhost').replace(/\/$/, '');
  const issued = Math.floor(now / 1000);
  const claims = {
    iss: `${shop}/admin`,
    dest: shop,
    aud: state.app?.clientId ?? ext.handle,
    sub: state.buyer?.customer?.id ?? state.customer?.id,
    exp: issued + 60,
    nbf: issued - 5,
    iat: issued,
    jti: `${ext.handle}-${issued}`,
    sid: state.token,
  };
  return [
    base64url(JSON.stringify({ alg: 'HS256', typ: 'JWT' })),
    base64url(JSON.stringify(claims)),
    base64url('lsf-local-checkout'),
  ].join('.');
}

const INTERCEPT_TIMEOUT_MS = 5000;
/** How long a frame may take to render before the page is ready without it. */
const RENDER_TIMEOUT_MS = 5000;

/**
 * The text of the bundle at `url`, or null when it does not load. The page fetches it, with the
 * origin of the server: the frame has an opaque origin, and would need CORS to load it with its
 * errors shown, which would let any site read the bundles.
 */
export async function bundleSource(window, url) {
  try {
    const response = await window.fetch(url);
    return response.ok ? await response.text() : null;
  } catch {
    return null;
  }
}

/**
 * @param {object} options
 * @param {Document} options.document
 * @param {Window} options.window       where the iframes post their messages
 * @param {string} options.sandboxUrl   `sandbox.html`
 * @param {() => object} options.getState
 * @param {(operations: object[] | (() => object[])) => Promise<object>} options.dispatch  sends
 *   operations. A function is called when its turn in the queue comes, so that what it builds
 *   follows the changes that were made before it.
 * @param {Storage} [options.storage]
 * @param {() => void} [options.onError]  called when an extension reports an error the page has
 *   not seen yet (see `errors`)
 * @param {() => void} [options.onRendered]  called when a frame is done: its target rendered,
 *   threw, or did not answer in time (see `pending`)
 * @param {(request: object, instance: object) => boolean} [options.onRequest]  what the page does
 *   for a message of the customer account (`navigate`, `toast`, `action`, `close`): true when it
 *   handled it. Without it, `navigate` opens a web page in a new tab, and the others do nothing.
 */
export function createExtensionHost({ document, window, sandboxUrl, getState, dispatch, storage, onError, onRequest, onRendered, console: log = console }) {
  /** Every live iframe. */
  const instances = new Set();
  /** What the extensions threw, by handle, target and message: `{handle, target, message, stack}`. */
  const reported = new Map();
  const slots = new Map();
  /** The hidden frames of the targets that render nothing, by `handle target`. */
  const runnables = new Map();
  let counter = 0;

  // An extension on an API version we do not run, or declared against its rules, is not loaded.
  const extensions = () => (getState().extensions ?? []).filter((ext) => ext.script && !refusalOf(ext));

  /** `[{handle, reason}]`: the extensions that are not loaded because of their API version. */
  const refusals = () =>
    (getState().extensions ?? []).flatMap((ext) => {
      const reason = refusalOf(ext);
      return reason ? [{ handle: ext.handle, apiVersion: ext.apiVersion, reason }] : [];
    });

  function hasTarget(target) {
    return extensions().some((ext) => ext.targets?.includes(target));
  }

  function post(instance, message) {
    instance.iframe.contentWindow?.postMessage({ lsf: true, ...message }, '*');
  }

  /** Sends the frame what it runs, once the page has the text of the bundle. */
  async function init(instance) {
    const { ext } = instance;
    const script = new URL(ext.script, document.baseURI ?? sandboxUrl).href;
    const source = await bundleSource(window, script);
    // The frame was reloaded or dropped while the bundle was fetched.
    if (!instance.ready || !instances.has(instance)) return;
    const state = getState();
    post(instance, {
      type: 'init',
      handle: ext.handle,
      target: instance.target,
      script,
      source,
      apiVersion: ext.apiVersion,
      ctx: instance.ctx,
      variant: state.variant,
      runnable: Boolean(instance.runnable),
      ext: {
        settings: ext.settings ?? {},
        capabilities: ext.capabilities ?? {},
        translations: ext.translations ?? {},
        defaultLocale: ext.defaultLocale ?? null,
      },
      state: stateForExtensions(state),
    });
  }

  function create(el, target, ext, ctx, before = null) {
    const frame = document.createElement('div');
    frame.className = 'lsf-ext';
    frame.setAttribute('data-extension', ext.handle);
    const iframe = document.createElement('iframe');
    iframe.setAttribute('sandbox', 'allow-scripts');
    iframe.setAttribute('title', `${ext.handle} (${target})`);
    iframe.setAttribute('scrolling', 'no');
    iframe.setAttribute('src', sandboxUrl);
    iframe.className = 'lsf-ext-frame';
    frame.appendChild(iframe);
    el.insertBefore(frame, before);
    const instance = {
      id: ++counter, ext, target, ctx, frame, iframe, ready: false, interceptors: 0, pending: new Map(),
      error: null, initialized: null, rendered: false,
    };
    instances.add(instance);
    // A frame that says nothing in time counts as done: the page is ready without it.
    setTimeout(() => settle(instance), RENDER_TIMEOUT_MS);
    return instance;
  }

  /** A frame is done: its target rendered, threw, or did not answer in time. */
  function settle(instance) {
    if (instance.rendered) return;
    instance.rendered = true;
    onRendered?.();
  }

  /** Stops an instance: its frame leaves the page, and the buyer journey no longer asks it. */
  function drop(instance) {
    instances.delete(instance);
    instance.frame.remove?.();
    instance.iframe.remove?.();
    for (const resolve of instance.pending.values()) resolve([]);
    instance.pending.clear();
  }

  /** What makes an extension another one for a frame: its handle, its bundle and how it is set up. */
  const identityOf = (ext) => JSON.stringify(ext);

  /** The extensions that belong in a slot of `target`, in order: all of them, or the one a full
   * page or a dialog is for (`ctx.handle`). */
  const wanted = (target, ctx = {}) =>
    extensions().filter((ext) => ext.targets?.includes(target) && (!ctx.handle || ext.handle === ctx.handle));

  function hasTargetFor(target, handle) {
    return wanted(target, { handle }).length > 0;
  }

  function attach(el, target, ctx = {}) {
    const created = wanted(target, ctx).map((ext) => create(el, target, ext, ctx));
    slots.set(el, { el, target, ctx, created });
  }

  function detach(el) {
    for (const instance of slots.get(el)?.created ?? []) drop(instance);
    slots.delete(el);
  }

  /**
   * Makes every slot hold the extensions the state selects now. A slot outlives a change of the
   * selection when another extension takes the target of the one that left, so the frames of an
   * extension that is no longer selected are removed (with their buyer journey interceptors),
   * the frames of one that is still selected stay as they are, and the new ones are mounted.
   */
  function reconcile() {
    for (const slot of slots.values()) {
      const { target, ctx } = slot;
      const current = new Map(slot.created.map((instance) => [identityOf(instance.ext), instance]));
      const next = [];
      for (const ext of wanted(target, ctx)) {
        const key = identityOf(ext);
        const kept = current.get(key);
        if (kept) {
          current.delete(key);
          next.push(kept);
        } else {
          next.push({ ext });
        }
      }
      for (const gone of current.values()) drop(gone);
      // New frames go before the next frame that stays, so that the order is the selection's.
      let before = null;
      for (let i = next.length - 1; i >= 0; i--) {
        if (next[i].frame) {
          before = next[i].frame;
        } else {
          next[i] = create(slot.el, target, next[i].ext, ctx, before);
          before = next[i].frame;
        }
      }
      slot.created = next;
    }
  }

  function update(state) {
    reconcile();
    const visible = stateForExtensions(state);
    for (const instance of instances) {
      if (instance.ready) post(instance, { type: 'state', state: visible, ctx: instance.ctx });
    }
  }

  /** The banner above the frame: the last error, and its stack under it when there is one. */
  function showError(instance, message, stack) {
    instance.error ??= (() => {
      const banner = document.createElement('div');
      banner.className = 'lsf-ext-error';
      instance.frame.insertBefore(banner, instance.iframe);
      return banner;
    })();
    const line = document.createElement('p');
    line.textContent = `${instance.ext.handle} (${instance.target}): ${message}`;
    const parts = [line];
    if (stack) {
      const details = document.createElement('details');
      const summary = document.createElement('summary');
      summary.textContent = 'Stack';
      const pre = document.createElement('pre');
      pre.textContent = stack;
      details.append(summary, pre);
      parts.push(details);
    }
    instance.error.replaceChildren(...parts);
  }

  /** Keeps an error for the dialog of the Functions; the same one again changes nothing. */
  const MAX_ERRORS = 50;
  function remember(instance, message, stack) {
    const key = `${instance.ext.handle}\n${instance.target}\n${message}`;
    if (reported.has(key) || reported.size >= MAX_ERRORS) return;
    reported.set(key, { handle: instance.ext.handle, target: instance.target, message, stack: stack ?? null });
    onError?.();
  }

  async function change(instance, data) {
    const { method, change: requested, id } = data;
    const planned = operationsForChange(method, requested, getState());
    let result;
    if (planned.error) {
      result = { type: 'error', message: planned.error };
    } else {
      try {
        // A partial address is merged with the address the checkout has when its turn comes, not
        // the one it had when the extension asked: two changes in a row (`{city}` then `{zip}`)
        // do not undo each other, nor a change of the buyer that came in between.
        await dispatch(
          method === 'applyShippingAddressChange'
            ? () => operationsForChange(method, requested, getState()).operations
            : planned.operations,
        );
        result = { type: 'success' };
      } catch (error) {
        result = { type: 'error', message: error.message };
      }
    }
    if (method === 'applyShippingAddressChange') {
      result = result.type === 'success'
        ? { type: 'success', errors: null }
        : { type: 'error', errors: [{ message: result.message }] };
    }
    post(instance, { type: 'result', id, result });
  }

  /**
   * Where an extension's value lives, as Shopify scopes the Storage API: to the checkout (its
   * token and the number of orders the session placed before it, as a new checkout after an
   * order starts empty, and so does the thank-you page of the order), then to the extension,
   * whatever the target, never to the other extensions.
   */
  function storageKey(instance, key) {
    const state = getState();
    const checkout = `${encodeURIComponent(state.token ?? '')}.${state.orderGeneration ?? 0}`;
    const stage = state.surface === 'customer-account' ? 'customer-account' : state.status === 'completed' ? 'thank-you' : 'checkout';
    return `lsf-extension:${checkout}:${stage}:${instance.ext.handle}:${key}`;
  }

  function onStorage(instance, data) {
    let value = null;
    try {
      if (data.op === 'write') storage?.setItem(storageKey(instance, data.key), JSON.stringify(data.value));
      else if (data.op === 'delete') storage?.removeItem(storageKey(instance, data.key));
      else value = JSON.parse(storage?.getItem(storageKey(instance, data.key)) ?? 'null');
    } catch {
      // Storage may be disabled or full: the extension sees a store that forgets.
    }
    post(instance, { type: 'result', id: data.id, result: value });
  }

  function onMessage(event) {
    const data = event.data;
    if (!data || data.lsf !== true) return;
    const instance = [...instances].find((candidate) => candidate.iframe.contentWindow === event.source);
    if (!instance) return;
    switch (data.type) {
      case 'ready':
        instance.ready = true;
        instance.initialized = init(instance);
        break;
      case 'rendered':
        settle(instance);
        break;
      case 'ran':
        instance.pending.get(data.id)?.(data.error ? null : data.result);
        instance.pending.delete(data.id);
        break;
      case 'toast':
      case 'action':
      case 'close':
        onRequest?.(data, instance);
        break;
      case 'resize':
        instance.iframe.style.height = `${Math.max(0, Math.ceil(data.height))}px`;
        break;
      case 'overlay':
        instance.iframe.className = data.open ? 'lsf-ext-frame lsf-ext-overlay' : 'lsf-ext-frame';
        break;
      case 'change':
        change(instance, data);
        break;
      case 'storage':
        onStorage(instance, data);
        break;
      case 'sessionToken':
        post(instance, { type: 'result', id: data.id, result: sessionToken(getState(), instance.ext) });
        break;
      case 'analytics':
        log.debug?.(`[${instance.ext.handle}] analytics`, data.name, data.data);
        break;
      case 'log':
        log[data.level]?.(`[${instance.ext.handle}]`, ...(data.args ?? []));
        break;
      case 'error': {
        const message = String(data.message);
        const stack = typeof data.stack === 'string' && data.stack ? data.stack : null;
        log.error?.(`[${instance.ext.handle}] ${message}`, ...(stack ? [`\n${stack}`] : []));
        showError(instance, message, stack);
        remember(instance, message, stack);
        break;
      }
      case 'interceptors':
        instance.interceptors = data.count;
        break;
      case 'intercepted':
        instance.pending.get(data.id)?.(data.results ?? []);
        instance.pending.delete(data.id);
        break;
      case 'navigate':
        if (onRequest?.(data, instance)) break;
        // Always a new tab, whatever the extension asks for: `_self` would take the checkout
        // itself away from the buyer. Only web pages: no `javascript:`, `data:` or relative URL
        // (it would mean the sandbox's, not the shop's).
        if (typeof data.url === 'string' && /^https?:\/\//i.test(data.url)) {
          window.open(data.url, '_blank', 'noopener');
        }
        break;
      case 'dismiss':
        instance.frame.style.display = 'none';
        break;
      case 'reload':
        instance.ready = false;
        instance.rendered = false;
        setTimeout(() => settle(instance), RENDER_TIMEOUT_MS);
        instance.iframe.setAttribute('src', sandboxUrl);
        break;
      default:
        break;
    }
  }

  function ask(instance, canBlockProgress) {
    const id = `${instance.id}:${++counter}`;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        instance.pending.delete(id);
        log.warn?.(`[${instance.ext.handle}] did not answer its buyer journey interceptors in time`);
        resolve([]);
      }, INTERCEPT_TIMEOUT_MS);
      instance.pending.set(id, (results) => {
        clearTimeout(timer);
        resolve(results);
      });
      post(instance, { type: 'intercept', id, canBlockProgress });
    });
  }

  /**
   * Asks the interceptors of every extension whether the buyer may go on. Resolves to
   * `{blocks: [{handle, reason, errors}]}`; an empty list lets the buyer pay.
   */
  async function intercept() {
    const asked = [...instances].filter((instance) => instance.ready && instance.interceptors > 0);
    const answers = await Promise.all(asked.map((instance) => ask(instance, canBlock(instance))));
    const blocks = [];
    answers.forEach((results, index) => {
      for (const result of results) {
        if (result.behavior === 'block') {
          blocks.push({ handle: asked[index].ext.handle, reason: result.reason, errors: result.errors ?? [] });
        }
      }
    });
    const behavior = blocks.length ? 'block' : 'allow';
    for (const instance of asked) post(instance, { type: 'perform', behavior });
    return { blocks };
  }

  const canBlock = (instance) => Boolean(instance.ext.capabilities?.block_progress);

  /** The hidden frame of `ext` at `target`, a target that renders nothing, made the first time. */
  function runnable(ext, target) {
    const key = `${ext.handle} ${target}`;
    const kept = runnables.get(key);
    if (kept && instances.has(kept) && identityOf(kept.ext) === identityOf(ext)) return kept;
    if (kept) drop(kept);
    let box = document.querySelector?.('.lsf-runnables');
    if (!box) {
      box = document.createElement('div');
      box.className = 'lsf-runnables';
      box.setAttribute('hidden', '');
      document.body.appendChild(box);
    }
    const instance = create(box, target, ext, {});
    instance.runnable = true;
    runnables.set(key, instance);
    return instance;
  }

  /**
   * Runs `target` (one that renders nothing) of the first extension that declares it, with
   * `payload` as its `shopify.target`. Resolves to what it returned, or null when no extension
   * declares it, it threw, or it did not answer in time.
   */
  async function run(target, payload, timeout = INTERCEPT_TIMEOUT_MS) {
    const ext = extensions().find((candidate) => candidate.targets?.includes(target));
    if (!ext) return null;
    const instance = runnable(ext, target);
    const started = Date.now();
    while (!instance.initialized) {
      if (Date.now() - started > timeout || !instances.has(instance)) return null;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    await instance.initialized;
    const id = `${instance.id}:${++counter}`;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        instance.pending.delete(id);
        log.warn?.(`[${ext.handle}] did not answer ${target} in time`);
        resolve(null);
      }, timeout);
      instance.pending.set(id, (result) => {
        clearTimeout(timer);
        resolve(result);
      });
      post(instance, { type: 'run', id, payload });
    });
  }

  window.addEventListener('message', onMessage);

  return {
    hasTarget,
    hasTargetFor,
    run,
    /** How many frames on the page have not rendered yet. */
    pending: () => [...instances].filter((instance) => !instance.runnable && !instance.rendered).length,
    refusals,
    /** `[{handle, target, message, stack}]`: what the extensions threw since the page loaded. */
    errors: () => [...reported.values()],
    attach,
    detach,
    update,
    intercept,
    /** For tests and the page's own use. */
    onMessage,
    instances: () => [...instances],
    destroy() {
      window.removeEventListener('message', onMessage);
      for (const instance of instances) instance.iframe.remove?.();
      instances.clear();
    },
  };
}

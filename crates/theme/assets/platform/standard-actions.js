// Shopify's standard storefront actions (`Shopify.actions`), for the local server.
//
// Same contract as https://shopify.dev/docs/api/storefront-events-and-actions: `getCart`,
// `updateCart` and `openCart`, the `configure` and `isDefault` methods, the events
// `updateCart` emits and the way the default behaviour refreshes a theme it recognises.
// Shopify's runtime writes to the Storefront API; this one writes to the cart endpoints of
// the local server (`/cart/add.js`, `/cart/change.js`, `/cart/update.js`).
(() => {
  const GID = 'gid://shopify/';

  const toGid = (type, id) => {
    const text = String(id);
    return text.startsWith(GID) ? text : `${GID}${type}/${text}`;
  };

  /** `gid://shopify/ProductVariant/123` → `123`. Cart line GIDs may end with `?cart=…`. */
  const fromGid = (id) =>
    String(id)
      .replace(/^gid:\/\/shopify\/[A-Za-z]+\//, '')
      .replace(/\?cart=.*$/, '');

  class ValidationError extends Error {
    constructor(message) {
      super(message);
      this.name = 'ValidationError';
    }
  }

  class CartActionError extends Error {
    constructor(message, cause) {
      super(message, { cause });
      this.name = 'CartActionError';
    }
  }

  // --- Requests -----------------------------------------------------------------------

  const abortError = () => new DOMException('The operation was aborted.', 'AbortError');

  const throwIfAborted = (signal) => {
    if (signal?.aborted) throw abortError();
  };

  /** Settles like `promise`, or rejects as soon as `signal` aborts. */
  const abortable = (promise, signal) => {
    if (!signal) return promise;
    throwIfAborted(signal);
    return new Promise((resolve, reject) => {
      const onAbort = () => reject(abortError());
      signal.addEventListener('abort', onAbort, { once: true });
      promise.finally(() => signal.removeEventListener('abort', onAbort)).then(resolve, reject);
    });
  };

  const endpoint = (path) => {
    const root = window.Shopify?.routes?.root ?? '/';
    return `${root.replace(/\/$/, '')}${path}`;
  };

  const send = async (path, body, signal) => {
    const response = await fetch(endpoint(path), {
      method: body === undefined ? 'GET' : 'POST',
      headers: {
        Accept: 'application/json',
        ...(body !== undefined && { 'Content-Type': 'application/json' }),
      },
      ...(body !== undefined && { body: JSON.stringify(body) }),
      signal,
    });
    let data = null;
    try {
      data = await response.json();
    } catch {
      // Not JSON: the status says enough.
    }
    return { ok: response.ok, status: response.status, data };
  };

  /** A cart exists once something was put in it: the server then sets the `cart` cookie. */
  const hasCart = () => /(?:^|;\s*)cart=[^;]+/.test(document.cookie);

  // --- The cart, as actions report it -------------------------------------------------

  const fractionDigits = (currency) => {
    try {
      return new Intl.NumberFormat('en', { style: 'currency', currency }).resolvedOptions()
        .maximumFractionDigits;
    } catch {
      return 2;
    }
  };

  const price = (cents, currency) => ({
    amount: (Number(cents) / 100).toFixed(fractionDigits(currency)),
    currencyCode: currency,
  });

  /**
   * A `/cart.js` response in the shape actions resolve with. Like Shopify's runtime, which
   * passes on what the Storefront API returns, the lines sit under `nodes`; the events get
   * them as a plain array (see `forEvents`).
   */
  const summarize = (cart) => ({
    id: toGid('Cart', cart.token),
    totalQuantity: cart.item_count,
    cost: { totalAmount: price(cart.total_price, cart.currency) },
    lines: {
      nodes: cart.items.map((item) => ({
        id: `${toGid('CartLine', item.key ?? item.id)}?cart=${cart.token}`,
        quantity: item.quantity,
        cost: { totalAmount: price(item.final_line_price, cart.currency) },
      })),
    },
    discountCodes: (cart.discount_codes ?? []).map(({ applicable, code }) => ({
      applicable,
      code,
    })),
  });

  /** The result of a call as the promise of its events reports it: with the lines as an array. */
  const forEvents = (result) => ({
    ...result,
    cart: result.cart && { ...result.cart, lines: result.cart.lines?.nodes ?? result.cart.lines },
  });

  const readCart = async (signal) => {
    const { ok, status, data } = await send('/cart.js', undefined, signal);
    if (!ok || !data) throw new Error(`The cart could not be read (${status})`);
    return data;
  };

  // --- getCart ------------------------------------------------------------------------

  const getCart = (() => {
    // Concurrent reads share one request.
    let pending = null;
    const shared = () => {
      pending ??= readCart()
        .then(summarize)
        .finally(() => {
          pending = null;
        });
      return pending;
    };
    return async (payload, options) => {
      const signal = options?.signal;
      throwIfAborted(signal);
      if (!payload?.cartId && !hasCart()) return { cart: null };
      return { cart: await abortable(shared(), signal) };
    };
  })();

  // --- updateCart ---------------------------------------------------------------------

  /** Splits lines into the three kinds of change, remembering their position in the payload. */
  const classify = (lines) => {
    const groups = { add: [], update: [], remove: [] };
    lines.forEach((line, index) => {
      const entry = { ...line, index };
      if (!line.id) groups.add.push(entry);
      else if (line.quantity === 0) groups.remove.push(entry);
      else if (line.quantity > 0) groups.update.push(entry);
    });
    return groups;
  };

  const validate = (payload) => {
    if (!payload || typeof payload !== 'object') {
      throw new ValidationError('updateCart requires a payload.');
    }
    const hasLines = Array.isArray(payload.lines) && payload.lines.length > 0;
    const hasOther =
      typeof payload.note === 'string' ||
      payload.discountCodes !== undefined ||
      payload.attributes !== undefined;
    if (!hasLines && !hasOther) {
      throw new ValidationError(
        'updateCart requires at least one of: lines, note, discountCodes, or attributes.',
      );
    }
    const usable = (line) =>
      Boolean(
        (typeof line?.id === 'string' && line.id.trim() && line.quantity >= 0) ||
          (line?.merchandiseId && line.quantity > 0),
      );
    if (hasLines && payload.lines.some((line) => !usable(line))) {
      throw new ValidationError(
        'updateCart lines must add, update, or remove at least one item.',
      );
    }
  };

  /** The payload with every id in its canonical form. */
  const normalize = (payload) => ({
    ...payload,
    ...(payload.lines && {
      lines: payload.lines.map((line) => ({
        ...line,
        ...(line.merchandiseId && { merchandiseId: toGid('ProductVariant', line.merchandiseId) }),
        ...(line.sellingPlanId && { sellingPlanId: toGid('SellingPlan', line.sellingPlanId) }),
      })),
    }),
  });

  const pairs = (attributes) =>
    Object.fromEntries((attributes ?? []).map(({ key, value }) => [key, value]));

  /** What the server says about a change it refused. */
  const refusal = (data, status) =>
    data?.description || data?.message || `The cart refused the change (${status})`;

  /** Writes the change to the cart of the local server and reads the cart back. */
  const write = async (payload, signal) => {
    const userErrors = [];
    const refuse = (code, field, message) => userErrors.push({ code, field, message });
    const { add, update, remove } = classify(payload.lines ?? []);

    // One request per line, so that a refusal names the line it is about.
    for (const line of add) {
      const { ok, status, data } = await send(
        '/cart/add.js',
        {
          id: Number(fromGid(line.merchandiseId)),
          quantity: line.quantity,
          ...(line.attributes?.length && { properties: pairs(line.attributes) }),
          ...(line.sellingPlanId && { selling_plan: fromGid(line.sellingPlanId) }),
        },
        signal,
      );
      if (ok) continue;
      if (status === 404) {
        refuse('INVALID', ['lines', String(line.index), 'merchandiseId'], refusal(data, status));
      } else if (status === 422 || status === 400) {
        refuse('INVALID', ['lines', String(line.index), 'quantity'], refusal(data, status));
      } else {
        throw new CartActionError(refusal(data, status), { status });
      }
    }

    for (const line of [...update, ...remove]) {
      const { ok, status, data } = await send(
        '/cart/change.js',
        {
          id: fromGid(line.id),
          quantity: line.quantity,
          ...(line.attributes && { properties: pairs(line.attributes) }),
        },
        signal,
      );
      if (ok) continue;
      if (status === 400 || status === 404 || status === 422) {
        refuse('INVALID', ['lines', String(line.index), 'id'], refusal(data, status));
      } else {
        throw new CartActionError(refusal(data, status), { status });
      }
    }

    if (payload.note !== undefined || payload.attributes !== undefined) {
      const body = {};
      if (payload.note !== undefined) body.note = payload.note;
      if (payload.attributes !== undefined) {
        // The payload is the complete set: what the cart has and the payload lacks goes away.
        const current = (await readCart(signal)).attributes ?? {};
        const removed = Object.fromEntries(Object.keys(current).map((key) => [key, null]));
        body.attributes = { ...removed, ...pairs(payload.attributes) };
      }
      const { ok, status, data } = await send('/cart/update.js', body, signal);
      if (!ok) throw new CartActionError(refusal(data, status), { status });
    }

    const cart = summarize(await readCart(signal));
    if (payload.discountCodes !== undefined) {
      // Discounts are not simulated: every code is reported as not applicable.
      cart.discountCodes = payload.discountCodes.map((code) => ({ applicable: false, code }));
    }
    return { cart, ...(userErrors.length && { userErrors }) };
  };

  // --- How the default refreshes a theme it recognises --------------------------------

  const DAWN_CART_ELEMENTS = ['cart-drawer', 'cart-items', 'cart-drawer-items', 'cart-notification'];

  /** Horizon and themes based on it re-render from a `cart:update` event. */
  const refreshHorizonStyle = (itemCount) => {
    const icon = document.querySelector('cart-icon');
    const items = document.querySelector('cart-items-component');
    if (typeof icon?.renderCartBubble !== 'function') return false;
    if (typeof items?.updateQuantity !== 'function') return false;
    document.dispatchEvent(
      new CustomEvent('cart:update', {
        bubbles: true,
        detail: { resource: null, sourceId: 'external', data: { source: 'external', itemCount } },
      }),
    );
    return true;
  };

  /** Dawn and themes based on it say which sections show the cart: fetch and swap them. */
  const refreshDawnStyle = async () => {
    const recognised = DAWN_CART_ELEMENTS.some(
      (name) =>
        typeof window.customElements?.get(name)?.prototype?.getSectionsToRender === 'function',
    );
    if (!recognised) return false;

    // Dawn's own subscribers already re-render these when `publish` exists.
    const selfRendering =
      typeof window.publish === 'function'
        ? ['cart-drawer:cart-drawer', 'cart-drawer-items:CartDrawer', 'cart-items:main-cart-items']
        : [];
    const targets = new Map();
    for (const element of document.querySelectorAll(DAWN_CART_ELEMENTS.join(','))) {
      let sections;
      try {
        sections = element.getSectionsToRender?.();
      } catch {
        continue;
      }
      const tag = element.tagName.toLowerCase();
      for (const section of sections ?? []) {
        if (selfRendering.includes(`${tag}:${section.id}`)) continue;
        const key = section.section ?? section.id;
        if (!key || targets.has(key)) continue;
        const container = section.section ? document.getElementById(section.id) : document;
        if (!container) continue;
        const mount = section.selector
          ? (container.querySelector(section.selector) ?? (section.section ? container : null))
          : document.getElementById(section.id);
        if (mount) targets.set(key, { mount, selector: section.selector || '.shopify-section' });
      }
    }

    const cartUrl = window.routes?.cart_url || '/cart';
    const query = targets.size ? `?sections=${[...targets.keys()].join(',')}` : '';
    const cart = await fetch(`${cartUrl}.js${query}`, { headers: { Accept: 'application/json' } })
      .then((response) => (response.ok ? response.json() : null))
      .catch(() => null);
    for (const [key, { mount, selector }] of targets) {
      const html = cart?.sections?.[key];
      if (!html) continue;
      const fresh = new DOMParser().parseFromString(html, 'text/html').querySelector(selector);
      if (fresh) mount.replaceChildren(...fresh.childNodes);
    }
    try {
      await window.publish?.(window.PUB_SUB_EVENTS?.cartUpdate ?? 'cart-update', {
        source: 'external-refresh',
        cartData: cart ?? undefined,
      });
    } catch {
      // The theme's subscribers are its own business.
    }
    return true;
  };

  const refreshTheme = async (itemCount) => {
    if (refreshHorizonStyle(itemCount)) return;
    if (await refreshDawnStyle()) return;
    window.location.reload();
  };

  // --- Events -------------------------------------------------------------------------

  const EVENTS = {
    lines: 'shopify:cart:lines-update',
    note: 'shopify:cart:note-update',
    attributes: 'shopify:cart:attributes-update',
    discount: 'shopify:cart:discount-update',
    error: 'shopify:cart:error',
  };

  /** A standard event: a DOM event that carries its payload as own properties. */
  class StandardEvent extends Event {
    constructor(type, payload) {
      super(type, { bubbles: true, cancelable: true });
      Object.assign(this, payload);
    }
  }

  const deferred = () => {
    let resolve;
    let reject;
    const promise = new Promise((onResolve, onReject) => {
      resolve = onResolve;
      reject = onReject;
    });
    // A listener may never look at the promise: its rejection is then nobody's error.
    promise.catch(() => {});
    return { promise, resolve, reject };
  };

  const dispatch = (configuration, meta, payload) => {
    const target = configuration?.eventTarget?.(meta) ?? document;
    target.dispatchEvent(new StandardEvent(meta.type, payload));
  };

  /** Emits the events of a call as it starts. Returns what settles their promises. */
  const announce = (configuration, payload, options) => {
    const context = options?.event?.context ?? 'standard-action';
    const detail = options?.event?.detail;
    const extra = detail ? { detail } : {};
    const outcomes = [];
    const emit = (meta, fields) => {
      const outcome = deferred();
      outcomes.push(outcome);
      dispatch(configuration, meta, { ...fields, promise: outcome.promise, ...extra });
    };

    const { add, update, remove } = classify(payload.lines ?? []);
    if (add.length) {
      emit(
        { type: EVENTS.lines, action: 'add' },
        {
          action: 'add',
          context,
          lines: add.map(({ merchandiseId, quantity }) => ({ merchandiseId, quantity })),
        },
      );
    }
    for (const [action, lines] of [
      ['update', update],
      ['remove', remove],
    ]) {
      if (!lines.length) continue;
      emit(
        { type: EVENTS.lines, action },
        { action, context, lines: lines.map(({ id, quantity }) => ({ id, quantity })) },
      );
    }
    if (payload.note !== undefined) {
      emit({ type: EVENTS.note }, { context, note: payload.note });
    }
    if (payload.attributes !== undefined) {
      emit({ type: EVENTS.attributes }, { context, attributes: payload.attributes });
    }
    if (payload.discountCodes !== undefined) {
      emit(
        { type: EVENTS.discount },
        { discountCodes: payload.discountCodes.map((code) => ({ code })) },
      );
    }
    return outcomes;
  };

  /** What `shopify:cart:error` says about a failure, or nothing when it is not one to report. */
  const describeFailure = (error) => {
    if (!(error instanceof Error)) return null;
    if (error.name === 'AbortError' || error.name === 'ValidationError') return null;
    const first = error.cause?.userErrors?.[0];
    if (first?.message) return { code: first.code ?? 'SERVICE_UNAVAILABLE', message: first.message };
    return { code: 'SERVICE_UNAVAILABLE', message: error.message };
  };

  // --- Actions ------------------------------------------------------------------------

  /** An action a storefront can configure once. */
  const configurable = (run) => {
    let configuration = null;
    return Object.assign((...args) => run(configuration, ...args), {
      isDefault: () => configuration === null,
      configure(options) {
        if (configuration || !options || typeof options !== 'object') return false;
        configuration = options;
        return true;
      },
    });
  };

  const updateCart = (() => {
    // The default refresh waits for every call in flight, then runs once.
    let running = 0;
    let refreshDue = false;
    let itemCount;
    const refreshWhenIdle = () => {
      if (!refreshDue || running > 0) return;
      setTimeout(() => {
        if (!refreshDue || running > 0) return;
        refreshDue = false;
        refreshTheme(itemCount);
      }, 0);
    };

    return configurable(async (configuration, rawPayload, options, ...rest) => {
      validate(rawPayload);
      const payload = normalize(rawPayload);
      const unconfigured = !configuration;
      const signal = options?.signal;
      const defaultHandler = () => abortable(write(payload, signal), signal);

      if (unconfigured) running += 1;
      const outcomes = announce(configuration, payload, options);
      try {
        throwIfAborted(signal);
        const { cart, userErrors, warnings, detail } = await (configuration?.handler
          ? configuration.handler(defaultHandler, payload, options, ...rest)
          : defaultHandler());
        const result = {
          cart,
          ...(userErrors !== undefined && { userErrors }),
          ...(warnings !== undefined && { warnings }),
          ...(detail !== undefined && { detail }),
        };
        const reported = forEvents(result);
        for (const outcome of outcomes) outcome.resolve(reported);
        if (unconfigured) {
          refreshDue = true;
          itemCount = cart?.totalQuantity;
        }
        return result;
      } catch (error) {
        for (const outcome of outcomes) outcome.reject(error);
        const failure = describeFailure(error);
        if (failure) {
          const detail = options?.event?.detail;
          dispatch(
            configuration,
            { type: EVENTS.error },
            { error: failure.message, code: failure.code, ...(detail && { detail }) },
          );
        }
        throw error;
      } finally {
        if (unconfigured) {
          running -= 1;
          refreshWhenIdle();
        }
      }
    });
  })();

  const openDrawer = (selector) => {
    const drawer = document.querySelector(selector);
    if (typeof drawer?.open !== 'function') return false;
    try {
      drawer.open();
      return true;
    } catch {
      return false;
    }
  };

  const showCart = async () => {
    if (openDrawer('cart-drawer-component') || openDrawer('cart-drawer')) return;
    window.location.href = '/cart';
  };

  const openCart = configurable(async (configuration, ...args) =>
    configuration?.handler ? configuration.handler(showCart, ...args) : showCart(),
  );

  window.Shopify ??= {};
  Object.defineProperty(window.Shopify, 'actions', {
    value: Object.freeze({ getCart, updateCart, openCart }),
    writable: false,
    configurable: false,
    enumerable: true,
  });
})();

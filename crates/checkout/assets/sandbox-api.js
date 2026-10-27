// The `shopify` global of checkout UI extensions, built from the state of the checkout.
//
// A classic script (not a module): the sandboxed iframe has an opaque origin, where the
// browser refuses module scripts that the server did not mark with CORS headers. It adds
// `createRuntime` to `LsfSandbox`; `sandbox.js` wires it to the page.
//
// Values are signals, `shopify.lines.value`, and an extension built with Preact passes its own
// `Signal` class to `shopify.setSignals`, so it re-renders by itself. The API releases the
// page runs (2026-10, 2027-01) look the same from here: `extension-host.js` decides whether an
// extension runs at all (`api-versions.js`).
(function (global) {
  'use strict';
  const Sandbox = (global.LsfSandbox = global.LsfSandbox || {});

  // --- Signals ---------------------------------------------------------------------------

  /** `subscribe` calls back at once with the current value, as signals do. */
  function createSignalClass() {
    return class LsfSignal {
      constructor(value) {
        this._value = value;
        this._subscribers = new Set();
      }
      get value() {
        return this._value;
      }
      set value(next) {
        if (next === this._value) return;
        this._value = next;
        for (const subscriber of [...this._subscribers]) subscriber(next);
      }
      peek() {
        return this._value;
      }
      subscribe(subscriber) {
        this._subscribers.add(subscriber);
        subscriber(this._value);
        return () => {
          this._subscribers.delete(subscriber);
        };
      }
      destroy() {
        this._subscribers.clear();
        return Promise.resolve();
      }
      valueOf() {
        return this._value;
      }
      toString() {
        return String(this._value);
      }
      toJSON() {
        return this._value;
      }
    };
  }

  // --- Mapping the state to the API ------------------------------------------------------

  const clean = (object) => {
    for (const key of Object.keys(object)) if (object[key] === null || object[key] === undefined) delete object[key];
    return object;
  };

  function allocation(entry) {
    return entry.code
      ? { type: 'code', code: entry.code, discountedAmount: entry.amount }
      : { type: 'automatic', title: entry.title, discountedAmount: entry.amount };
  }

  // The title Shopify gives the only variant of a product without options. The state leaves it
  // out of `variantTitle` (the checkout page does not show it), the API does not.
  const DEFAULT_VARIANT_TITLE = 'Default Title';

  // The Cart Lines API of 2026-10 and 2027-01: `title` is the title of the variant ("Small",
  // "Red / Large"), "not the parent product title", which `product` does not expose (`id`,
  // `vendor` and `productType`). `subtitle` is a secondary description the store data does not
  // have, so it is undefined.
  function merchandise(line) {
    return clean({
      type: 'variant',
      id: 'gid://shopify/ProductVariant/' + line.variantId,
      title: line.variantTitle || DEFAULT_VARIANT_TITLE,
      image: line.image ? clean({ url: line.image, altText: line.imageAlt }) : undefined,
      selectedOptions: (line.selectedOptions || []).map((option) => ({ name: option.name, value: option.value })),
      sellingPlan: line.sellingPlan
        ? { id: line.sellingPlan.id, recurringDeliveries: line.sellingPlan.recurringDeliveries === true }
        : undefined,
      product: {
        id: 'gid://shopify/Product/' + line.productId,
        vendor: line.vendor || '',
        productType: line.productType || '',
      },
      requiresShipping: line.requiresShipping !== false,
      sku: line.sku,
    });
  }

  /**
   * The cost of a line as Shopify defines it: before order discounts, only the product
   * discounts are taken off. The state's total is after every discount; the allocations tell
   * the order ones apart by their class (an allocation without one counts as a product one).
   */
  function lineCost(line) {
    const ordered = (line.discountAllocations || []).filter((entry) => entry.class === 'ORDER');
    if (!ordered.length || !line.total) return line.total;
    const cents = ordered.reduce((sum, entry) => sum + Math.round(entry.amount.amount * 100), 0);
    return { ...line.total, amount: Math.round(line.total.amount * 100 + cents) / 100 };
  }

  function mapLine(line) {
    return {
      id: line.id,
      merchandise: merchandise(line),
      quantity: line.quantity,
      cost: { totalAmount: lineCost(line) },
      attributes: line.attributes || [],
      discountAllocations: (line.discountAllocations || []).map(allocation),
      lineComponents: (line.components || []).map((component) => ({
        type: 'bundle',
        id: component.id,
        merchandise: merchandise(component),
        quantity: component.quantity,
        cost: { totalAmount: lineCost(component) },
        attributes: component.attributes || [],
      })),
      parentRelationship: null,
    };
  }

  function mapAddress(address) {
    if (!address) return undefined;
    return clean({
      name: [address.firstName, address.lastName].filter(Boolean).join(' ') || undefined,
      firstName: address.firstName,
      lastName: address.lastName,
      company: address.company,
      address1: address.address1,
      address2: address.address2,
      city: address.city,
      zip: address.zip,
      countryCode: address.countryCode,
      provinceCode: address.provinceCode,
      phone: address.phone,
    });
  }

  function mapOption(option) {
    const base = {
      handle: option.handle,
      title: option.title,
      description: option.description || undefined,
      code: option.handle,
      metafields: [],
    };
    if (option.kind === 'PICK_UP') {
      return clean({ ...base, type: 'pickup', location: { name: option.title, address: {} } });
    }
    return clean({
      ...base,
      type: option.kind === 'LOCAL' ? 'local' : 'shipping',
      carrier: {},
      cost: option.originalCost,
      costAfterDiscounts: option.cost,
      deliveryEstimate: {},
    });
  }

  function mapGroup(group) {
    return clean({
      id: group.handle,
      targetedCartLines: group.lineIds.map((id) => ({ id })),
      deliveryOptions: group.options.map(mapOption),
      selectedDeliveryOption: group.selected ? { handle: group.selected } : undefined,
      groupType: 'oneTimePurchase',
      isDeliveryRequired: true,
    });
  }

  function paymentOption(method) {
    const handle = method.handle.toLowerCase();
    let type = 'other';
    if (handle === 'card' || handle.includes('credit')) type = 'creditCard';
    else if (handle.includes('cod') || handle.includes('delivery')) type = 'paymentOnDelivery';
    else if (handle.includes('bank') || handle.includes('manual') || handle.includes('deposit')) type = 'manualPayment';
    return { type, handle: method.handle };
  }

  /** The customer of the Buyer Identity API. The store knows no avatar, no consent for SMS and
   * no store credit: those stay empty. */
  function mapCustomer(customer) {
    if (!customer) return undefined;
    const accepts = Boolean(customer.acceptsMarketing);
    return clean({
      id: customer.id,
      email: customer.email,
      phone: customer.phone || undefined,
      fullName: [customer.firstName, customer.lastName].filter(Boolean).join(' ') || undefined,
      firstName: customer.firstName,
      lastName: customer.lastName,
      image: { url: '' },
      acceptsMarketing: accepts,
      acceptsEmailMarketing: accepts,
      acceptsSmsMarketing: false,
      storeCreditAccounts: [],
      ordersCount: customer.ordersCount || 0,
    });
  }

  /** `buyerIdentity.purchasingCompany`: the company and the location a B2B customer buys for. */
  function mapPurchasingCompany(purchasing) {
    if (!purchasing || !purchasing.company) return undefined;
    const entry = (value) => value && clean({ id: value.id, name: value.name, externalId: value.externalId || undefined });
    return clean({ company: entry(purchasing.company), location: entry(purchasing.location) });
  }

  const METAFIELD_TYPES = { number_integer: 'integer', number_decimal: 'float', boolean: 'boolean', json: 'json_string' };

  function mapMetafield(metafield, token) {
    return {
      target: { type: 'cart', id: 'gid://shopify/Cart/' + token },
      metafield: {
        namespace: metafield.namespace,
        key: metafield.key,
        value: metafield.value,
        type: metafield.type,
        valueType: METAFIELD_TYPES[metafield.type] || 'string',
      },
    };
  }

  /** The names of the capabilities an extension was granted: `[extensions.capabilities]`. */
  function capabilityNames(capabilities) {
    const names = [];
    for (const key of Object.keys(capabilities || {})) {
      const value = capabilities[key];
      if (value && typeof value === 'object') {
        for (const inner of Object.keys(value)) if (value[inner]) names.push(key + '.' + inner);
      } else if (value) {
        names.push(key);
      }
    }
    return names;
  }

  // --- i18n ------------------------------------------------------------------------------

  const LOCALE_KEY = /^[a-z]{2,3}([-_][A-Za-z0-9]+)?$/;

  /** The dictionaries to read a string from, best first, each with the language it is in:
   * `translations` is `{en: {...}, fr: {...}}` or the strings of a single locale. The exact
   * locale, then its language, then the default one of the extension (`defaultLocale`, the
   * `.default.json` file), then, for the extensions that declare none, English or the first one.
   * The first one is the language the extension renders in. */
  function pickStrings(translations, locale, defaultLocale) {
    const keys = Object.keys(translations || {});
    if (keys.length === 0) return [];
    const byLocale = keys.every((key) => LOCALE_KEY.test(key) || key === 'default');
    if (!byLocale) return [{ strings: translations, language: undefined }];
    const tag = String(locale || 'en').replace('_', '-');
    const find = (wanted) => {
      const lower = String(wanted).replace('_', '-').toLowerCase();
      return keys.find((key) => key.replace('_', '-').toLowerCase() === lower);
    };
    // A declared default ends the chain: a key it lacks shows as the key, as on Shopify, so a
    // missing translation is seen. Without one, English or the first file stands in for it.
    const declared = defaultLocale ? find(defaultLocale) : undefined;
    const candidates = [find(tag), find(tag.split('-')[0])];
    if (declared !== undefined) candidates.push(declared);
    else candidates.push(find('en'), find('default'), keys[0]);
    const chain = [];
    for (const key of candidates) {
      if (typeof key === 'string' && !chain.includes(key)) chain.push(key);
    }
    return chain.map((key) => ({
      strings: translations[key],
      language: key === 'default' ? undefined : key.replace('_', '-'),
    }));
  }

  function lookup(strings, key) {
    if (key in strings) return strings[key];
    let node = strings;
    for (const part of key.split('.')) {
      if (node === null || typeof node !== 'object' || !(part in node)) return undefined;
      node = node[part];
    }
    return node;
  }

  function createI18n(locale, currency, translations, defaultLocale) {
    const dictionaries = pickStrings(translations, locale, defaultLocale);
    const tag = String(locale || 'en').replace('_', '-');
    // Plural rules are the ones of the language a string is in, not the buyer's: a French string
    // shown to a German buyer is counted the French way.
    const rules = new Map();
    const pluralCategory = (language, count) => {
      const wanted = language || tag;
      if (!rules.has(wanted)) {
        try {
          rules.set(wanted, new Intl.PluralRules(wanted));
        } catch {
          rules.set(wanted, new Intl.PluralRules(tag));
        }
      }
      return rules.get(wanted).select(count);
    };
    return {
      formatNumber: (number, options) => new Intl.NumberFormat(tag, options).format(number),
      formatCurrency: (number, options) =>
        new Intl.NumberFormat(tag, { style: 'currency', currency, ...options }).format(number),
      formatDate: (date, options) => new Intl.DateTimeFormat(tag, options).format(date),
      translate(key, replacements) {
        // Each key is resolved on its own, so a partial dictionary falls back key by key.
        let value;
        let language;
        for (const dictionary of dictionaries) {
          value = lookup(dictionary.strings, key);
          if (value !== undefined) {
            language = dictionary.language;
            break;
          }
        }
        if (value && typeof value === 'object') {
          const count = replacements && replacements.count;
          const category = typeof count === 'number' ? pluralCategory(language, count) : 'other';
          value = value[category] !== undefined ? value[category] : value.other;
        }
        if (typeof value !== 'string') return key;
        return value.replace(/\{\{?\s*(\w+)\s*\}?\}/g, (whole, name) =>
          replacements && name in replacements ? String(replacements[name]) : whole,
        );
      },
    };
  }

  // --- The runtime -----------------------------------------------------------------------

  /**
   * @param {object} options
   * @param {(message: object) => void} options.post   sends a message to the page
   * @param {object} options.init                      the `init` message of the page
   */
  function createRuntime({ post, init }) {
    const { target, ext, script } = init;
    let state = init.state;
    let ctx = init.ctx || {};
    let SignalClass = createSignalClass();

    // Signals are made when an extension first reads them, with the class it handed to
    // `setSignals` by then; every update recomputes the ones that exist.
    const signals = [];
    function signalProperty(object, name, compute) {
      const entry = { signal: null, compute, last: undefined };
      signals.push(entry);
      Object.defineProperty(object, name, {
        enumerable: true,
        configurable: true,
        get() {
          if (!entry.signal) {
            const value = entry.compute(state, ctx);
            entry.last = JSON.stringify(value);
            entry.signal = new SignalClass(value);
          }
          return entry.signal;
        },
      });
    }

    function refresh(nextState, nextCtx) {
      state = nextState;
      ctx = nextCtx || ctx;
      for (const entry of signals) {
        if (!entry.signal) continue;
        const value = entry.compute(state, ctx);
        const text = JSON.stringify(value);
        if (text === entry.last) continue;
        entry.last = text;
        entry.signal.value = value;
      }
    }

    // Requests to the page that wait for an answer.
    let counter = 0;
    const pending = new Map();
    function call(type, payload) {
      const id = ++counter;
      return new Promise((resolve) => {
        pending.set(id, resolve);
        post({ ...payload, type, id });
      });
    }

    // The language of the buyer, which is the one of the storefront they came from: not
    // necessarily the primary language of the shop.
    const languageOf = (s) => (s.localization && s.localization.language && s.localization.language.isoCode)
      || (s.shop && s.shop.locale) || 'en';
    const locale = languageOf(state);
    const currency = (state.cost && state.cost.currencyCode) || (state.shop && state.shop.currencyCode) || 'USD';
    const capabilities = capabilityNames(ext.capabilities);
    const api = {};
    const registered = new Map();

    // --- Platform APIs, on every target
    api.extensionPoint = target;
    api.version = 'lsf';
    api.extension = { apiVersion: init.apiVersion, scriptUrl: script, target, rendered: null, capabilities: null };
    signalProperty(api.extension, 'capabilities', () => capabilities);
    signalProperty(api.extension, 'rendered', () => true);
    signalProperty(api, 'settings', () => ext.settings || {});
    api.i18n = createI18n(locale, currency, ext.translations, ext.defaultLocale);
    const extensionLanguage = (pickStrings(ext.translations, locale, ext.defaultLocale)[0] || {}).language;
    api.shop = {
      id: 'gid://shopify/Shop/1',
      name: (state.shop && state.shop.name) || '',
      storefrontUrl: state.shop && state.shop.storefrontUrl,
      myshopifyDomain: String((state.shop && state.shop.name) || 'shop').toLowerCase().replace(/[^a-z0-9]+/g, '-') + '.myshopify.com',
    };
    api.storage = {
      read: (key) => call('storage', { op: 'read', key }),
      write: (key, value) => call('storage', { op: 'write', key, value }).then(() => undefined),
      delete: (key) => call('storage', { op: 'delete', key }).then(() => undefined),
    };
    api.sessionToken = { get: () => call('sessionToken', {}) };
    api.query = () =>
      Promise.reject(new Error('The Storefront API is not available in the local checkout: query() has no store to ask.'));
    api.analytics = {
      publish: (name, data) => {
        post({ type: 'analytics', name, data });
        return Promise.resolve(true);
      },
      visitor: () => Promise.resolve({ type: 'success' }),
    };
    api.localization = {
      acceptedAddressInputFieldsForCountry: () =>
        Promise.resolve(['firstName', 'lastName', 'company', 'address1', 'address2', 'city', 'zip', 'provinceCode', 'phone']),
    };
    signalProperty(api.localization, 'currency', (s) => ({ isoCode: (s.cost && s.cost.currencyCode) || currency }));
    signalProperty(api.localization, 'language', (s) => ({ isoCode: languageOf(s) }));
    // The language the extension is rendered in: the one of the strings it got, not the buyer's.
    signalProperty(api.localization, 'extensionLanguage', (s) => ({ isoCode: extensionLanguage || languageOf(s) }));
    signalProperty(api.localization, 'timezone', () => Intl.DateTimeFormat().resolvedOptions().timeZone);
    // The country of the shipping address, else the one the visitor selected on the storefront.
    signalProperty(api.localization, 'country', (s) => {
      const country = s.localization && s.localization.country;
      if (country && country.isoCode) return { isoCode: country.isoCode };
      return s.shippingAddress && s.shippingAddress.countryCode ? { isoCode: s.shippingAddress.countryCode } : undefined;
    });
    signalProperty(api.localization, 'market', (s) => {
      const market = s.localization && s.localization.market;
      return market ? { id: market.id, handle: market.handle } : { id: 'gid://shopify/Market/1', handle: 'primary' };
    });

    // --- Checkout state, on every target
    signalProperty(api, 'lines', (s) => s.lines.map(mapLine));
    signalProperty(api, 'attributes', (s) => s.attributes || []);
    signalProperty(api, 'note', (s) => s.note || undefined);
    signalProperty(api, 'discountCodes', (s) => (s.discountCodes || []).map((entry) => ({ code: entry.code })));
    signalProperty(api, 'discountAllocations', (s) =>
      (s.discounts || []).filter((entry) => entry.class === 'ORDER').map(allocation));
    signalProperty(api, 'appliedGiftCards', () => []);
    signalProperty(api, 'deliveryGroups', (s) => (s.deliveryGroups || []).map(mapGroup));
    signalProperty(api, 'availablePaymentOptions', (s) => (s.paymentMethods || []).map(paymentOption));
    signalProperty(api, 'selectedPaymentOptions', (s) =>
      (s.paymentMethods || []).filter((m) => m.handle === s.selectedPayment).map(paymentOption));
    signalProperty(api, 'shippingAddress', (s) => mapAddress(s.shippingAddress));
    signalProperty(api, 'billingAddress', (s) => mapAddress(s.shippingAddress));
    signalProperty(api, 'checkoutToken', (s) => s.token);
    signalProperty(api, 'metafields', (s) =>
      (s.metafields || []).map((m) => ({ namespace: m.namespace, key: m.key, value: m.value, valueType: m.type })));
    signalProperty(api, 'appMetafields', (s) => (s.metafields || []).map((m) => mapMetafield(m, s.token)));
    signalProperty(api, 'localizedFields', () => []);
    signalProperty(api, 'checkoutSettings', () => ({ orderSubmission: 'ORDER', shippingAddress: { isEditable: true } }));
    signalProperty(api, 'customerPrivacy', () => ({
      allowedProcessing: { analytics: true, marketing: true, preferences: true, saleOfData: true },
      metafields: [],
      visitorConsent: {},
      shouldShowBanner: false,
      saleOfDataRegion: false,
    }));
    signalProperty(api, 'instructions', () => ({
      attributes: { canUpdateAttributes: true },
      delivery: { canSelectCustomAddress: true },
      discounts: { canUpdateDiscountCodes: true },
      lines: { canAddCartLine: true, canRemoveCartLine: true, canUpdateCartLine: true },
      metafields: { canSetCartMetafields: true, canDeleteCartMetafield: true },
      notes: { canUpdateNote: true },
    }));
    api.cost = {};
    signalProperty(api.cost, 'subtotalAmount', (s) => s.cost.subtotal);
    signalProperty(api.cost, 'totalShippingAmount', (s) => s.cost.shipping);
    signalProperty(api.cost, 'totalTaxAmount', (s) => s.cost.tax);
    signalProperty(api.cost, 'totalAmount', (s) => s.cost.total);
    api.buyerIdentity = {};
    signalProperty(api.buyerIdentity, 'email', (s) => (s.buyer && s.buyer.email) || undefined);
    signalProperty(api.buyerIdentity, 'phone', (s) => (s.buyer && s.buyer.phone) || undefined);
    signalProperty(api.buyerIdentity, 'customer', (s) => mapCustomer(s.buyer && s.buyer.customer));
    signalProperty(api.buyerIdentity, 'purchasingCompany', (s) => mapPurchasingCompany(s.buyer && s.buyer.purchasingCompany));
    api.applyTrackingConsentChange = () => Promise.resolve({ type: 'success' });

    // --- The buyer journey
    const interceptors = [];
    let requests = [];
    api.buyerJourney = {};
    signalProperty(api.buyerJourney, 'completed', (s) => s.status === 'completed');
    signalProperty(api.buyerJourney, 'activeStep', (s) => ({ handle: s.status === 'completed' ? 'thank-you' : 'checkout' }));
    signalProperty(api.buyerJourney, 'steps', (s) =>
      s.status === 'completed'
        ? [{ handle: 'thank-you', label: 'Thank you', to: '', disabled: false }]
        : [{ handle: 'checkout', label: 'Checkout', to: '', disabled: false }]);
    api.buyerJourney.intercept = (interceptor) => {
      interceptors.push(interceptor);
      post({ type: 'interceptors', count: interceptors.length });
      return Promise.resolve(() => {
        const index = interceptors.indexOf(interceptor);
        if (index >= 0) interceptors.splice(index, 1);
        post({ type: 'interceptors', count: interceptors.length });
      });
    };

    // --- Changes, on checkout targets
    if (target.indexOf('purchase.checkout.') === 0 || target === 'purchase.cart-line-item.line-components.render') {
      for (const method of [
        'applyAttributeChange', 'applyCartLinesChange', 'applyDiscountCodeChange', 'applyGiftCardChange',
        'applyMetafieldChange', 'applyNoteChange', 'applyShippingAddressChange', 'applyRedeemableChange',
        'applyPaymentMethodAttributesChange',
      ]) {
        api[method] = (change) => call('change', { method, change });
      }
      api.experimentalIsShopAppStyle = init.variant === 'shop';
    }

    // --- What a target adds
    const line = (s, c) => s.lines.map(mapLine).find((candidate) => candidate.id === c.lineId);
    const groupOf = (s, c) => (s.deliveryGroups || []).find((group) => group.handle === c.group);
    if (/cart-line-item|line-components/.test(target)) signalProperty(api, 'target', line);
    if (/shipping-option-item/.test(target)) {
      signalProperty(api, 'target', (s, c) => {
        const group = groupOf(s, c);
        const option = group && group.options.find((candidate) => candidate.handle === c.option);
        return option && mapOption(option);
      });
      signalProperty(api, 'isTargetSelected', (s, c) => {
        const group = groupOf(s, c);
        return Boolean(group && group.selected === c.option);
      });
      api.renderMode = { overlay: false };
    }
    if (/shipping-option-list/.test(target)) {
      signalProperty(api, 'target', (s) =>
        s.deliveryGroups && s.deliveryGroups.length
          ? { groupType: 'oneTimePurchase', deliveryGroups: s.deliveryGroups.map(mapGroup) }
          : undefined);
      signalProperty(api, 'deliverySelectionGroups', (s) =>
        (s.deliveryGroups || []).flatMap((group) => group.options.map((option) => ({
          handle: option.handle,
          selected: group.selected === option.handle,
          title: option.title,
          associatedDeliveryOptions: [{ handle: option.handle }],
          cost: option.originalCost,
          costAfterDiscounts: option.cost,
        }))));
    }
    if (target.indexOf('purchase.thank-you.') === 0) {
      signalProperty(api, 'orderConfirmation', (s) => ({
        order: { id: (s.order && s.order.id) || 'gid://shopify/Order/' + s.token },
        number: (s.order && s.order.number) || undefined,
        isFirstOrder: true,
      }));
    }
    if (target === 'purchase.thank-you.announcement.render') {
      const listeners = [];
      api.announcement = {
        close: () => {
          post({ type: 'dismiss' });
          listeners.forEach((listener) => listener());
        },
        addEventListener: (type, listener) => listeners.push(listener),
        removeEventListener: (type, listener) => {
          const index = listeners.indexOf(listener);
          if (index >= 0) listeners.splice(index, 1);
        },
      };
    }

    // --- The global
    const shopify = Object.create(api, {
      extend: {
        value(name, callback) {
          registered.set(name, callback);
        },
      },
      reload: {
        value() {
          post({ type: 'reload' });
        },
      },
      setSignals: {
        value(Signal) {
          if (typeof Signal !== 'function') return;
          SignalClass = Signal;
          // Signals read before the extension chose its class are dropped: it gets new ones.
          for (const entry of signals) entry.signal = null;
        },
      },
    });

    async function runInterceptors(canBlockProgress) {
      requests = [];
      const results = [];
      for (const interceptor of interceptors.slice()) {
        let request;
        try {
          request = await interceptor({ canBlockProgress });
        } catch (error) {
          post({ type: 'log', level: 'error', args: ['A buyer journey interceptor threw: ' + (error && error.message)] });
          continue;
        }
        if (!request) continue;
        requests.push(request);
        if (request.behavior === 'block' && !canBlockProgress) {
          post({
            type: 'log',
            level: 'warn',
            args: ['The extension tried to block the buyer but has no block_progress capability: the buyer goes on.'],
          });
          results.push({ behavior: 'allow' });
        } else {
          results.push({ behavior: request.behavior, reason: request.reason, errors: request.errors || [] });
        }
      }
      return results;
    }

    /** Handles what the page sends: `state`, `result`, `intercept`, `perform`. */
    function receive(message) {
      switch (message.type) {
        case 'state':
          refresh(message.state, message.ctx);
          break;
        case 'result': {
          const resolve = pending.get(message.id);
          pending.delete(message.id);
          if (resolve) resolve(message.result);
          break;
        }
        case 'intercept':
          runInterceptors(message.canBlockProgress).then((results) => {
            post({ type: 'intercepted', id: message.id, results });
          });
          break;
        case 'perform':
          for (const request of requests) {
            if (typeof request.perform === 'function') {
              Promise.resolve(request.perform({ behavior: message.behavior })).catch(() => {});
            }
          }
          requests = [];
          break;
        default:
          break;
      }
    }

    return { shopify, api, registered, receive, refresh };
  }

  Sandbox.createRuntime = createRuntime;
  Sandbox.createSignalClass = createSignalClass;
  Sandbox.capabilityNames = capabilityNames;
})(typeof globalThis !== 'undefined' ? globalThis : this);

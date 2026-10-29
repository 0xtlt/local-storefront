// What the `shopify` global of a customer account UI extension has besides what every target
// has (`sandbox-api.js`): the account logged in, navigation, toasts and intents, and, on the
// order status page and the full page of an order, the order (the Order Status API). A classic
// script, like the other files of the sandbox.
//
// The page sends the state of the account (`customer-account.js`): `buyer`, `order` (the order
// of the page, written like the state of a checkout), and in `ctx` what the slot is for:
// `lineId`, `orderId`, `fulfillmentId`, `returnId`, `locationId`, and `handle` and `path` for a
// full page.
(function (global) {
  'use strict';
  const Sandbox = (global.LsfSandbox = global.LsfSandbox || {});

  const ORDER_STATUS = /^customer-account\.(order-status\.|order\.page\.render$)/;

  /** The Order of the Order Status API. */
  function mapOrder(order) {
    if (!order) return undefined;
    const out = { id: order.id, name: order.name, confirmationNumber: order.confirmationNumber, processedAt: order.processedAt };
    if (order.cancelledAt) out.cancelledAt = order.cancelledAt;
    return out;
  }


  /**
   * Adds to `api` what a customer account target has.
   * @param {object} options  what `createRuntime` shares: `api`, `init`, `signalProperty`, `post`,
   *   `getCtx` (what the slot is for)
   */
  function customerAccountApi({ api, init, signalProperty, post, getCtx }) {
    const { target } = init;
    const map = Sandbox.mappers;
    const ctx = getCtx() || {};

    api.query = () =>
      Promise.reject(new Error('The Storefront API is not available in the local customer account: query() has no store to ask.'));

    // --- Who is logged in
    api.authenticatedAccount = {};
    signalProperty(api.authenticatedAccount, 'customer', (s) =>
      s.buyer && s.buyer.customer ? { id: s.buyer.customer.id } : undefined);
    signalProperty(api.authenticatedAccount, 'purchasingCompany', (s) => {
      const purchasing = s.buyer && s.buyer.purchasingCompany;
      if (!purchasing || !purchasing.company) return undefined;
      return { company: { id: purchasing.company.id }, location: purchasing.location ? { id: purchasing.location.id } : undefined };
    });

    // --- Navigation, toasts, intents
    const fullPage = target === 'customer-account.page.render' || target === 'customer-account.order.page.render';
    const entry = {
      key: 'lsf-' + Math.random().toString(36).slice(2),
      url: fullPage && ctx.handle ? 'extension:' + ctx.handle + (ctx.path || '/') : null,
      state: undefined,
      getState() {
        return this.state;
      },
    };
    api.navigation = {
      // The page says where it leads (`customer-account.js`).
      navigate(url) {
        post({ type: 'navigate', url: String(url) });
      },
      currentEntry: entry,
      updateCurrentEntry(options) {
        entry.state = options && options.state;
      },
      // The page loads again on a navigation: the entry never changes under the extension.
      addEventListener() {},
      removeEventListener() {},
    };
    api.toast = {
      show(content) {
        post({ type: 'toast', content: String(content) });
        return Promise.resolve({ hide: () => post({ type: 'toast', hide: true }) });
      },
    };
    api.intents = {
      invoke(query) {
        post({ type: 'log', level: 'warn', args: ['Intents are not available in the local customer account: ' + JSON.stringify(query) + ' was closed.'] });
        return Promise.resolve({ complete: Promise.resolve({ code: 'closed' }) });
      },
    };

    // --- What the slot is for
    if (ctx.orderId) api.orderId = ctx.orderId;
    if (ctx.fulfillmentId) api.fulfillmentId = ctx.fulfillmentId;
    if (ctx.returnId) api.returnId = ctx.returnId;
    if (ctx.locationId) api.locationId = ctx.locationId;
    if (target === 'customer-account.order.action.render') api.close = () => post({ type: 'close' });
    if (target === 'customer-account.order.action.menu-item.render') {
      // Pressing the button of the menu item opens the dialog of the extension
      // (`customer-account.order.action.render`), unless it leads somewhere.
      global.document.addEventListener('click', (event) => {
        const closest = (tag) => event.target && event.target.closest && event.target.closest(tag);
        const button = closest('s-button') || closest('s-link');
        if (button && !button.hasAttribute('href') && !button.hasAttribute('disabled')) post({ type: 'action' });
      });
    }

    // --- The order (the Order Status API)
    if (!ORDER_STATUS.test(target)) return;
    const order = (s) => s.order || {};
    signalProperty(api, 'order', (s) => mapOrder(s.order));
    signalProperty(api, 'lines', (s) => (order(s).lines || []).map(map.mapLine));
    signalProperty(api, 'attributes', (s) => order(s).attributes || []);
    signalProperty(api, 'note', (s) => order(s).note || undefined);
    signalProperty(api, 'discountCodes', (s) => (order(s).discountCodes || []).map((entry) => ({ code: entry.code })));
    signalProperty(api, 'discountAllocations', (s) =>
      (order(s).discounts || []).filter((entry) => entry.class === 'ORDER').map(map.allocation));
    signalProperty(api, 'appliedGiftCards', () => []);
    signalProperty(api, 'appMetafields', () => []);
    signalProperty(api, 'checkoutToken', () => undefined);
    signalProperty(api, 'checkoutSettings', () => ({ orderSubmission: 'ORDER', shippingAddress: { isEditable: false } }));
    signalProperty(api, 'shippingAddress', (s) => map.mapAddress(order(s).shippingAddress));
    signalProperty(api, 'billingAddress', (s) => map.mapAddress(order(s).billingAddress || order(s).shippingAddress));
    signalProperty(api, 'authenticationState', () => 'fully_authenticated');
    api.requireLogin = () => Promise.resolve();
    api.cost = {};
    const cost = (s) => order(s).cost || {};
    signalProperty(api.cost, 'subtotalAmount', (s) => cost(s).subtotal);
    signalProperty(api.cost, 'totalShippingAmount', (s) => cost(s).shipping);
    signalProperty(api.cost, 'totalTaxAmount', (s) => cost(s).tax);
    signalProperty(api.cost, 'totalAmount', (s) => cost(s).total);
    api.buyerIdentity = {};
    signalProperty(api.buyerIdentity, 'customer', (s) => {
      const customer = map.mapCustomer(s.buyer && s.buyer.customer);
      if (customer) {
        delete customer.acceptsEmailMarketing;
        delete customer.acceptsSmsMarketing;
        delete customer.ordersCount;
      }
      return customer;
    });
    signalProperty(api.buyerIdentity, 'email', (s) => order(s).email || (s.buyer && s.buyer.customer && s.buyer.customer.email) || undefined);
    signalProperty(api.buyerIdentity, 'phone', (s) => order(s).phone || (s.buyer && s.buyer.customer && s.buyer.customer.phone) || undefined);
    signalProperty(api.buyerIdentity, 'purchasingCompany', (s) => map.mapPurchasingCompany(s.buyer && s.buyer.purchasingCompany));
    if (target === 'customer-account.order-status.cart-line-item.render-after') {
      signalProperty(api, 'target', (s, c) => (order(s).lines || []).map(map.mapLine).find((line) => line.id === c.lineId));
    }
  }

  Sandbox.customerAccountApi = customerAccountApi;
})(typeof globalThis !== 'undefined' ? globalThis : this);

// ca-every-target · API 2026-10 · the 24 targets of customer accounts, each marked with a badge
// that names it and says what it is for (the order, the shipment, the return, the location):
// a map of where each target sits in the orders, the order status and the profile.
//
// - its full page (`customer-account.page.render`, /account/pages/ca-every-target) has a
//   route of its own (`/rewards`) and buttons that navigate;
// - the menu item of an order (`customer-account.order.action.menu-item.render`) opens its
//   dialog (`customer-account.order.action.render`), which shows a toast and closes.
(() => {
  // h(tag, attributes, ...children) makes a Polaris web component, as in the other bundles
  // written by hand.
  const h = (tag, attributes, ...children) => {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes || {})) {
      if (value === undefined || value === null || value === false) continue;
      if (typeof value === 'function') node.addEventListener(name.slice(2).toLowerCase(), value);
      else node.setAttribute(name, value === true ? '' : String(value));
    }
    for (const child of children.flat(Infinity)) {
      if (child === undefined || child === null || child === false) continue;
      node.append(typeof child === 'object' ? child : String(child));
    }
    return node;
  };
  const mount = (...nodes) => document.body.replaceChildren(...nodes);
  const id = (gid) => String(gid || '').split('/').pop();

  const TARGETS = [
    'customer-account.order-index.announcement.render',
    'customer-account.order-index.block.render',
    'customer-account.order.action.menu-item.render',
    'customer-account.order.action.render',
    'customer-account.order-status.announcement.render',
    'customer-account.order-status.block.render',
    'customer-account.order-status.fulfillment-details.render-after',
    'customer-account.order-status.unfulfilled-items.render-after',
    'customer-account.order-status.return-details.render-after',
    'customer-account.order-status.customer-information.render-after',
    'customer-account.order-status.payment-details.render-after',
    'customer-account.order-status.cart-line-item.render-after',
    'customer-account.order-status.cart-line-list.render-after',
    'customer-account.order.page.render',
    'customer-account.profile.announcement.render',
    'customer-account.profile.block.render',
    'customer-account.profile.addresses.render-after',
    'customer-account.profile.payment.render-after',
    'customer-account.profile.company-details.render-after',
    'customer-account.profile.company-location-addresses.render-after',
    'customer-account.profile.company-location-payment.render-after',
    'customer-account.profile.company-location-staff.render-after',
    'customer-account.page.render',
    'customer-account.footer.render-after',
  ];

  /** What a target is for, besides the page: the ids the slot gives it. */
  function context() {
    const parts = [];
    if (shopify.order && shopify.order.value) parts.push('order ' + shopify.order.value.name);
    else if (shopify.orderId) parts.push('order ' + id(shopify.orderId));
    if (shopify.fulfillmentId) parts.push('shipment ' + id(shopify.fulfillmentId));
    if (shopify.returnId) parts.push('return ' + id(shopify.returnId));
    if (shopify.locationId) parts.push('location ' + id(shopify.locationId));
    if (shopify.target && shopify.target.value) parts.push(shopify.target.value.merchandise.title);
    return parts.join(' · ');
  }

  const badge = (target) =>
    h('s-box', { padding: 'small-400', border: 'base dashed', borderRadius: 'base' },
      h('s-stack', { direction: 'inline', gap: 'small-200', alignItems: 'center' },
        h('s-badge', { tone: 'info' }, target.replace(/^customer-account\./, '')),
        h('s-text', { color: 'subdued' }, context())));

  const RENDER = {
    'customer-account.page.render': () => {
      const route = (shopify.navigation.currentEntry.url || '').replace(/^extension:[^/]*/, '') || '/';
      return h('s-page', { heading: route === '/rewards' ? 'Rewards' : 'Every target' , subheading: 'customer-account.page.render, route ' + route },
        h('s-section', { heading: 'Navigate' },
          h('s-button-group', {},
            h('s-button', { onClick: () => shopify.navigation.navigate('extension://rewards') }, 'Rewards (a route of this page)'),
            h('s-button', { onClick: () => shopify.navigation.navigate('shopify:customer-account/orders') }, 'Orders'),
            h('s-button', { onClick: () => shopify.navigation.navigate('shopify:customer-account/profile') }, 'Profile'))),
        h('s-section', { heading: 'Who is logged in' },
          h('s-text', {}, 'Customer ' + id(shopify.authenticatedAccount.customer.value && shopify.authenticatedAccount.customer.value.id))));
    },
    'customer-account.order.page.render': () =>
      h('s-page', { heading: 'Order ' + (shopify.order.value ? shopify.order.value.name : ''), subheading: 'customer-account.order.page.render' },
        h('s-text', {}, shopify.lines.value.length + ' lines, ' + shopify.i18n.formatCurrency(shopify.cost.totalAmount.value.amount))),
    'customer-account.order.action.menu-item.render': () => h('s-button', {}, 'Ask about this order'),
    'customer-account.order.action.render': () =>
      h('s-customer-account-action', { heading: 'Ask about order ' + id(shopify.orderId) },
        h('s-text', {}, 'customer-account.order.action.render: a dialog the menu item opened.'),
        h('s-button', {
          slot: 'primary-action',
          variant: 'primary',
          onClick: async () => {
            await shopify.toast.show('Question sent about order ' + id(shopify.orderId));
            shopify.close();
          },
        }, 'Send'),
        h('s-button', { slot: 'secondary-actions', onClick: () => shopify.close() }, 'Cancel')),
  };

  for (const target of TARGETS) {
    shopify.extend(target, () => mount((RENDER[target] || badge)(target)));
  }
})();

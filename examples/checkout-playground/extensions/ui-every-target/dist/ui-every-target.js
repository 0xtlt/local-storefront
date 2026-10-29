// ui-every-target · API 2026-10 · the 36 targets of the checkout and the thank-you page that
// render (28 of the checkout, 8 of the thank-you page), each marked with a badge that
// names it: a map of where each target sits. Best seen with `?targets=all`.
(() => {
  // h(tag, attributes, ...children) makes a Polaris web component: attributes are set as
  // attributes, functions become listeners (`onClick` listens to `click`), children are
  // appended. Each bundle carries its own copy, as a real build carries its runtime.
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

  const TARGETS = [
    'purchase.checkout.header.render-after',
    'purchase.checkout.contact.render-after',
    'purchase.checkout.delivery-address.render-before',
    'purchase.checkout.delivery-address.render-after',
    'purchase.checkout.shipping-option-list.render-before',
    'purchase.checkout.shipping-option-item.render-after',
    'purchase.checkout.shipping-option-item.details.render',
    'purchase.checkout.shipping-option-list.render-after',
    'purchase.checkout.pickup-location-list.render-before',
    'purchase.checkout.pickup-location-option-item.render-after',
    'purchase.checkout.pickup-location-list.render-after',
    'purchase.checkout.pickup-point-list.render-before',
    'purchase.checkout.pickup-point-list.render-after',
    'purchase.checkout.block.render',
    'purchase.checkout.payment-method-list.render-before',
    'purchase.checkout.payment-option-item.details.render',
    'purchase.checkout.payment-option-item.hosted-fields.render-after',
    'purchase.checkout.payment-option-item.action-required.render',
    'purchase.checkout.payment-method-list.render-after',
    'purchase.checkout.gift-card.render',
    'purchase.checkout.actions.render-before',
    'purchase.checkout.footer.render-after',
    'purchase.checkout.cart-line-item.render-after',
    'purchase.cart-line-item.line-components.render',
    'purchase.checkout.cart-line-list.render-after',
    'purchase.checkout.reductions.render-before',
    'purchase.checkout.reductions.render-after',
    'purchase.checkout.chat.render',
    'purchase.thank-you.announcement.render',
    'purchase.thank-you.header.render-after',
    'purchase.thank-you.block.render',
    'purchase.thank-you.customer-information.render-after',
    'purchase.thank-you.footer.render-after',
    'purchase.thank-you.cart-line-item.render-after',
    'purchase.thank-you.cart-line-list.render-after',
    'purchase.thank-you.chat.render',
  ];

  for (const target of TARGETS) {
    shopify.extend(target, () => {
      const extra = h('s-text', { color: 'subdued' });
      // Line and option targets say which line or option they are under.
      if (shopify.target) {
        shopify.target.subscribe((value) => {
          extra.textContent = value ? (value.merchandise ? value.merchandise.title : value.title || value.handle || '') : '';
        });
      }
      mount(h('s-box', { padding: 'small-400', border: 'base dashed', borderRadius: 'base' },
        h('s-stack', { direction: 'inline', gap: 'small-200', alignItems: 'center' },
          h('s-badge', { tone: target.includes('thank-you') ? 'success' : 'info' }, target.replace(/^purchase\./, '')),
          extra)));
    });
  }
})();

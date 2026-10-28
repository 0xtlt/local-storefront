// ui-every-target · API 2026-10 · the 24 targets the local checkout renders (17 of the
// checkout, 7 of the thank-you page), each marked with a badge that names it: a map of where
// each target sits. Best seen with `?targets=all`.
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
  const money = (value) => (value ? shopify.i18n.formatCurrency(value.amount) : '—');

  const TARGETS = [
    'purchase.checkout.header.render-after',
    'purchase.checkout.block.render',
    'purchase.checkout.contact.render-after',
    'purchase.checkout.delivery-address.render-before',
    'purchase.checkout.delivery-address.render-after',
    'purchase.checkout.shipping-option-list.render-before',
    'purchase.checkout.shipping-option-list.render-after',
    'purchase.checkout.shipping-option-item.render-after',
    'purchase.checkout.shipping-option-item.details.render',
    'purchase.checkout.payment-method-list.render-before',
    'purchase.checkout.payment-method-list.render-after',
    'purchase.checkout.cart-line-list.render-after',
    'purchase.checkout.cart-line-item.render-after',
    'purchase.checkout.reductions.render-before',
    'purchase.checkout.reductions.render-after',
    'purchase.checkout.actions.render-before',
    'purchase.checkout.footer.render-after',
    'purchase.thank-you.announcement.render',
    'purchase.thank-you.header.render-after',
    'purchase.thank-you.block.render',
    'purchase.thank-you.customer-information.render-after',
    'purchase.thank-you.cart-line-list.render-after',
    'purchase.thank-you.cart-line-item.render-after',
    'purchase.thank-you.footer.render-after',
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

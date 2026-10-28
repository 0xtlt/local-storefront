// ui-header-notice · API 2026-10 · purchase.checkout.header.render-after
// Shows a banner under the header with the name of the shop (`shopify.shop`) and how far the
// subtotal is from free shipping at 75.00 (`shopify.cost.subtotalAmount`, followed live).
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

  const FREE_FROM = 75;

  shopify.extend('purchase.checkout.header.render-after', () => {
    const text = h('s-text');
    const progress = h('s-progress', { max: FREE_FROM, value: 0 });
    shopify.cost.subtotalAmount.subscribe((subtotal) => {
      const amount = subtotal ? subtotal.amount : 0;
      const left = FREE_FROM - amount;
      text.textContent = left > 0
        ? `Add ${shopify.i18n.formatCurrency(left)} more and shipping is free.`
        : 'Your order ships free.';
      progress.setAttribute('value', String(Math.min(amount, FREE_FROM)));
    });
    mount(
      h('s-banner', { tone: 'info', heading: `Welcome to ${shopify.shop.name}` },
        h('s-stack', { gap: 'small-200' }, text, progress)),
    );
  });
})();

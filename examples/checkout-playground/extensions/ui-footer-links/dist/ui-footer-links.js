// ui-footer-links · API 2026-10 · purchase.checkout.footer.render-after
// In the footer: two links (`s-link`, which the page opens in a new tab) and what
// `shopify.extension` says (`apiVersion`, `target`, `capabilities`) with the shop's domain.
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

  shopify.extend('purchase.checkout.footer.render-after', () => {
    const { extension, shop } = shopify;
    mount(h('s-stack', { gap: 'small-200' },
      h('s-stack', { direction: 'inline', gap: 'base' },
        h('s-link', { href: 'https://example.com/help', target: '_blank' }, 'Help center'),
        h('s-link', { href: 'https://example.com/returns' }, 'Returns')),
      h('s-text', { color: 'subdued' },
        `${extension.target} · API ${extension.apiVersion} · capabilities: ${extension.capabilities.value.join(', ') || 'none'} · ${shop.myshopifyDomain}`)));
  });
})();

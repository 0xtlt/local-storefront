// ui-broken-long-text · API 2026-10 · EDGE CASE
//   purchase.checkout.block.render: a long unbroken word, URL and code in a banner heading,
//     text, a badge, a button and a field.
//   purchase.checkout.cart-line-item.render-after: a long unbroken string under each line.
// Shows whether the components wrap or overflow the column.
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

  const WORD = 'Pneumonoultramicroscopicsilicovolcanoconiosis'.repeat(4);
  const LINK = `https://example.com/${'a-very-long-path-segment/'.repeat(8)}?ref=${'x'.repeat(60)}`;
  const CODE = 'CODE-' + '0123456789ABCDEF'.repeat(6);

  shopify.extend('purchase.checkout.block.render', () => {
    mount(h('s-stack', { gap: 'small-200' },
      h('s-banner', { tone: 'info', heading: WORD }, h('s-text', {}, `${WORD} ${LINK}`)),
      h('s-badge', {}, CODE),
      h('s-button', { variant: 'secondary' }, `Apply ${CODE}`),
      h('s-text-field', { label: WORD, value: LINK }),
      h('s-link', { href: LINK }, LINK)));
  });

  shopify.extend('purchase.checkout.cart-line-item.render-after', () => {
    mount(h('s-text', { color: 'subdued' }, `${WORD}${CODE}`));
  });
})();

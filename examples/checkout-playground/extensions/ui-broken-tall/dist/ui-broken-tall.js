// ui-broken-tall · API 2026-10 · purchase.checkout.block.render · EDGE CASE
// Draws 150 rows: the frame grows to fit them (no inner scroll bar), far past the window.
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

  shopify.extend('purchase.checkout.block.render', () => {
    const rows = [];
    for (let i = 1; i <= 150; i++) {
      rows.push(h('s-box', { padding: 'small-300', border: 'base', borderRadius: 'base', background: i % 2 ? 'subdued' : 'base' },
        h('s-text', {}, `Row ${i} of 150`)));
    }
    mount(h('s-stack', { gap: 'small-300' }, h('s-heading', {}, 'A very tall extension'), rows, h('s-text', { type: 'strong' }, 'The end.')));
  });
})();

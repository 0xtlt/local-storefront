// ui-order-summary · API 2026-10 · purchase.checkout.cart-line-list.render-after
// Under the lines: the number of lines and units (`lines`) and the cost the extension reads
// (`cost.subtotalAmount`, `totalShippingAmount`, `totalTaxAmount`, `totalAmount`), to compare
// with the page's own summary.
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

  shopify.extend('purchase.checkout.cart-line-list.render-after', () => {
    const grid = h('s-grid', { gridTemplateColumns: '1fr auto', gap: 'small-300' });
    const show = () => {
      const lines = shopify.lines.value;
      const units = lines.reduce((sum, line) => sum + line.quantity, 0);
      const rows = [
        [`${lines.length} lines, ${units} units`, ''],
        ['Subtotal (cost.subtotalAmount)', money(shopify.cost.subtotalAmount.value)],
        ['Shipping (cost.totalShippingAmount)', money(shopify.cost.totalShippingAmount.value)],
        ['Tax (cost.totalTaxAmount)', money(shopify.cost.totalTaxAmount.value)],
        ['Total (cost.totalAmount)', money(shopify.cost.totalAmount.value)],
      ];
      grid.replaceChildren(...rows.flatMap(([label, value]) => [
        h('s-text', { color: 'subdued' }, label),
        h('s-text', {}, value),
      ]));
    };
    shopify.lines.subscribe(show);
    for (const name of ['subtotalAmount', 'totalShippingAmount', 'totalTaxAmount', 'totalAmount']) shopify.cost[name].subscribe(show);
    mount(h('s-box', { padding: 'small-200', border: 'base', borderRadius: 'base' }, grid));
  });
})();

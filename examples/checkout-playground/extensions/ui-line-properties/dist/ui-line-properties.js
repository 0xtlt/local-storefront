// ui-line-properties · API 2026-10 · purchase.checkout.cart-line-item.render-after
// Shows, under each line, what `shopify.target` gives of it: the variant title, SKU and
// options, the line item properties (`attributes`), the selling plan, the product discounts
// of the line (`discountAllocations`) and its cost.
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

  shopify.extend('purchase.checkout.cart-line-item.render-after', () => {
    const stack = h('s-stack', { gap: 'small-500' });
    shopify.target.subscribe((line) => {
      if (!line) return stack.replaceChildren();
      const { merchandise } = line;
      const rows = [
        `${merchandise.title}${merchandise.sku ? ` · SKU ${merchandise.sku}` : ''} · ${money(line.cost.totalAmount)}`,
        merchandise.selectedOptions.length
          ? merchandise.selectedOptions.map((option) => `${option.name}: ${option.value}`).join(', ')
          : null,
        ...line.attributes.map((attribute) => `Property ${attribute.key}: ${attribute.value}`),
        merchandise.sellingPlan ? `Selling plan ${merchandise.sellingPlan.id}${merchandise.sellingPlan.recurringDeliveries ? ' (recurring)' : ''}` : null,
        ...line.discountAllocations.map((entry) => `Discount ${entry.code || entry.title}: −${money(entry.discountedAmount)}`),
      ].filter(Boolean);
      stack.replaceChildren(...rows.map((row) => h('s-text', { color: 'subdued' }, row)));
    });
    mount(stack);
  });
})();

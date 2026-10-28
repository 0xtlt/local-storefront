// ui-reductions-codes · API 2027-01
//   purchase.checkout.reductions.render-before: the codes entered (`discountCodes`), each with a
//     Remove button (`applyDiscountCodeChange` removeDiscountCode), and the order discounts
//     (`discountAllocations`).
//   purchase.checkout.reductions.render-after: buttons that enter a code
//     (`applyDiscountCodeChange` addDiscountCode) and what the call answered. A code that no
//     discount of the checkout has is entered and refused, as on Shopify.
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

  shopify.extend('purchase.checkout.reductions.render-before', () => {
    const codes = h('s-stack', { direction: 'inline', gap: 'small-200' });
    const allocations = h('s-text', { color: 'subdued' });
    shopify.discountCodes.subscribe((list) => {
      codes.replaceChildren(...(list.length ? list.map(({ code }) =>
        h('s-clickable-chip', {
          removable: true,
          accessibilityLabel: `Remove ${code}`,
          onRemove: () => shopify.applyDiscountCodeChange({ type: 'removeDiscountCode', code }),
          onClick: () => shopify.applyDiscountCodeChange({ type: 'removeDiscountCode', code }),
        }, code)) : [h('s-text', { color: 'subdued' }, 'No code entered.')]));
    });
    shopify.discountAllocations.subscribe((list) => {
      allocations.textContent = list.length
        ? `Order discounts: ${list.map((entry) => `${entry.code || entry.title} −${money(entry.discountedAmount)}`).join(', ')}`
        : 'No order discount.';
    });
    mount(h('s-stack', { gap: 'small-200' }, codes, allocations));
  });

  shopify.extend('purchase.checkout.reductions.render-after', () => {
    const result = h('s-text', { color: 'subdued' });
    const field = h('s-text-field', { label: 'A code to enter from the extension' });
    const add = async (code) => {
      const outcome = await shopify.applyDiscountCodeChange({ type: 'addDiscountCode', code });
      result.textContent = `addDiscountCode ${code} → ${outcome.type}${outcome.message ? `: ${outcome.message}` : ''}`;
    };
    mount(h('s-stack', { gap: 'small-200' },
      h('s-stack', { direction: 'inline', gap: 'small-200' },
        h('s-button', { variant: 'secondary', onClick: () => add('WELCOME10') }, 'Enter WELCOME10'),
        h('s-button', { variant: 'secondary', onClick: () => add('NOT-A-CODE') }, 'Enter NOT-A-CODE')),
      h('s-stack', { direction: 'inline', gap: 'small-200', alignItems: 'end' },
        field,
        h('s-button', { onClick: () => add(field.value || '') }, 'Enter')),
      result));
  });
})();

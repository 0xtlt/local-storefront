// ui-upsell · API 2026-10 · purchase.checkout.block.render
// Three offers, each an `applyCartLinesChange` addCartLine:
//   - Sample Pack (variant 9100000021, `products/ui-sample-pack.json`): added, then shows
//     "In your cart" with a button that removes it;
//   - Enamel Pin (variant 9100000031, `products/ui-sold-out-pin.json`): sold out, refused;
//   - a variant id that the store does not have: refused.
// The message of each refusal is shown under its offer.
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

  const OFFERS = [
    { title: 'Sample Pack', price: 4, id: 'gid://shopify/ProductVariant/9100000021' },
    { title: 'Enamel Pin (sold out)', price: 9, id: 'gid://shopify/ProductVariant/9100000031' },
    { title: 'A product that does not exist', price: 1, id: 'gid://shopify/ProductVariant/1' },
  ];

  shopify.extend('purchase.checkout.block.render', () => {
    const rows = OFFERS.map((offer) => {
      const status = h('s-text', { color: 'subdued' });
      const add = h('s-button', {
        variant: 'secondary',
        onClick: async () => {
          add.setAttribute('loading', '');
          const outcome = await shopify.applyCartLinesChange({ type: 'addCartLine', merchandiseId: offer.id, quantity: 1 });
          add.removeAttribute('loading');
          status.textContent = outcome.type === 'success' ? 'Added.' : `Refused: ${outcome.message}`;
        },
      }, `Add · ${shopify.i18n.formatCurrency(offer.price)}`);
      const remove = h('s-button', {
        variant: 'secondary',
        tone: 'critical',
        onClick: () => {
          const line = shopify.lines.value.find((candidate) => candidate.merchandise.id === offer.id);
          if (line) shopify.applyCartLinesChange({ type: 'removeCartLine', id: line.id, quantity: line.quantity });
        },
      }, 'Remove');
      return { offer, status, add, remove };
    });
    shopify.lines.subscribe((lines) => {
      for (const row of rows) {
        const inCart = lines.find((line) => line.merchandise.id === row.offer.id);
        row.add.hidden = Boolean(inCart);
        row.remove.hidden = !inCart;
        if (inCart) row.status.textContent = `In your cart: ${inCart.quantity}.`;
      }
    });
    mount(h('s-stack', { gap: 'base' },
      h('s-heading', {}, 'You may also like'),
      rows.map((row) => h('s-stack', { direction: 'inline', gap: 'small-200', alignItems: 'center' },
        h('s-text', { type: 'strong' }, row.offer.title), row.add, row.remove, row.status))));
  });
})();

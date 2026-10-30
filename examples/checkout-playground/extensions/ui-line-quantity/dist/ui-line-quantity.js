// ui-line-quantity · API 2027-01 · purchase.checkout.cart-line-item.render-after
// Under each line: −, + and Remove buttons (`applyCartLinesChange` updateCartLine and
// removeCartLine). The change goes to the cart of the session; the Functions run again.
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

  shopify.extend('purchase.checkout.cart-line-item.render-after', () => {
    const count = h('s-text', {});
    const result = h('s-text', { color: 'subdued' });
    const change = async (build) => {
      const line = shopify.target.value;
      if (!line) return;
      const outcome = await shopify.applyCartLinesChange(build(line));
      result.textContent = outcome.type === 'success' ? '' : `Refused: ${outcome.message}`;
    };
    shopify.target.subscribe((line) => {
      count.textContent = line ? `Quantity ${line.quantity}` : '';
    });
    mount(h('s-stack', { direction: 'inline', gap: 'small-200', alignItems: 'center' },
      h('s-button', { variant: 'secondary', accessibilityLabel: 'One less', onClick: () => change((line) => ({ type: 'updateCartLine', id: line.id, quantity: line.quantity - 1 })) }, '−'),
      count,
      h('s-button', { variant: 'secondary', accessibilityLabel: 'One more', onClick: () => change((line) => ({ type: 'updateCartLine', id: line.id, quantity: line.quantity + 1 })) }, '+'),
      h('s-button', { variant: 'secondary', tone: 'critical', onClick: () => change((line) => ({ type: 'removeCartLine', id: line.id, quantity: line.quantity })) }, 'Remove'),
      result));
  });
})();

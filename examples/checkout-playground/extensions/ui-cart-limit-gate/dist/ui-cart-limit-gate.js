// ui-cart-limit-gate · API 2026-10 · purchase.checkout.block.render · block_progress
// Blocks the buyer journey on a condition of the cart: more than five units in all. A banner
// says how many units are too many, live (`shopify.lines`); the interceptor blocks Pay with
// the same message until the cart is small enough.
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

  const LIMIT = 5;
  const units = () => shopify.lines.value.reduce((sum, line) => sum + line.quantity, 0);

  shopify.extend('purchase.checkout.block.render', () => {
    const banner = h('s-banner', { heading: `At most ${LIMIT} units per order` });
    const text = h('s-text');
    banner.append(text);
    shopify.lines.subscribe(() => {
      const count = units();
      banner.setAttribute('tone', count > LIMIT ? 'critical' : 'success');
      text.textContent = count > LIMIT
        ? `The cart has ${count} units: remove ${count - LIMIT} to pay.`
        : `The cart has ${count} units: fine.`;
    });
    shopify.buyerJourney.intercept(({ canBlockProgress }) =>
      canBlockProgress && units() > LIMIT
        ? { behavior: 'block', reason: `More than ${LIMIT} units`, errors: [{ message: `Orders are limited to ${LIMIT} units: this one has ${units()}.` }] }
        : { behavior: 'allow' });
    mount(banner);
  });
})();

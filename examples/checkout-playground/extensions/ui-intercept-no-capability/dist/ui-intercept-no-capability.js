// ui-intercept-no-capability · API 2026-10 · purchase.checkout.block.render
// Registers a `buyerJourney.intercept` that always answers "block", without the
// `block_progress` capability. The checkout asks it with `canBlockProgress: false` and lets the
// buyer go on (the frame's console logs a warning). The text shows what the last ask said.
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
    const text = h('s-text', { color: 'subdued' }, 'Not asked yet: press Pay.');
    shopify.buyerJourney.intercept(({ canBlockProgress }) => {
      text.textContent = `Asked with canBlockProgress: ${canBlockProgress}. Answered "block"; ${canBlockProgress ? 'the order is blocked' : 'ignored, the buyer goes on'}.`;
      return { behavior: 'block', reason: 'Always blocks', errors: [{ message: 'ui-intercept-no-capability blocks every order (it should not be able to).' }] };
    });
    mount(h('s-banner', { tone: 'warning', heading: 'An interceptor without block_progress' }, text));
  });
})();

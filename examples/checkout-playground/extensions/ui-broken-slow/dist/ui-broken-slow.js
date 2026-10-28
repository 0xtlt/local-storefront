// ui-broken-slow · API 2026-10 · purchase.checkout.block.render · block_progress · EDGE CASE
// Draws nothing for 4 seconds, then a banner. Its buyer journey interceptor takes 7 seconds to
// answer "block": the checkout waits 5, then lets the buyer go on without it.
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

  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

  shopify.extend('purchase.checkout.block.render', async () => {
    const started = Date.now();
    shopify.buyerJourney.intercept(async () => {
      await wait(7000);
      return { behavior: 'block', reason: 'Too late', errors: [{ message: 'ui-broken-slow answered after 7 seconds.' }] };
    });
    await wait(4000);
    mount(h('s-banner', { tone: 'warning', heading: `Rendered after ${((Date.now() - started) / 1000).toFixed(1)} s` },
      h('s-text', {}, 'Press Pay: this interceptor answers after 7 s, the checkout stops waiting at 5 s.')));
  });
})();

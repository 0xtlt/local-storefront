// ui-broken-bad-owner-type · BROKEN ON PURPOSE in its toml, not here: the bundle is fine, and would draw a
// banner if the checkout loaded it. The page says why it does not.
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

  shopify.extend('purchase.checkout.block.render', () => {
    mount(h('s-banner', { tone: 'critical', heading: 'You should not see this' },
      h('s-text', {}, `${shopify.extension.scriptUrl} was loaded although its toml is wrong.`)));
  });
})();

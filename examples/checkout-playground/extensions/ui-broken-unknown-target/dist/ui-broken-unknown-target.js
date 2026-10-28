// ui-broken-unknown-target · API 2026-10 · EDGE CASE
// Declares purchase.checkout.nowhere.render (no such target), purchase.checkout.chat.render
// (a real target the local checkout has no place for) and purchase.checkout.block.render. Only
// the block renders; the server warns about the first one at start.
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

  for (const target of ['purchase.checkout.nowhere.render', 'purchase.checkout.chat.render', 'purchase.checkout.block.render']) {
    shopify.extend(target, () => {
      mount(h('s-banner', { tone: 'warning', heading: 'ui-broken-unknown-target' },
        h('s-text', {}, `Rendered at ${target}. Its two other targets have no place on the page.`)));
    });
  }
})();

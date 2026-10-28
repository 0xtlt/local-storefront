// ui-metafield-legacy · API 2026-10 · purchase.checkout.block.render
// Counts clicks in the metafield `$app:playground.clicks` with the older change of the
// Metafields API (`updateMetafield` with `valueType`, `removeMetafield`), and reads it from the
// older `shopify.metafields` list. The toml declares it without `owner_type`, which only
// 2026-10 accepts.
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

  const NAMESPACE = '$app:playground';
  const KEY = 'clicks';
  const current = () => Number((shopify.metafields.value.find((entry) => entry.namespace === NAMESPACE && entry.key === KEY) || {}).value || 0);

  shopify.extend('purchase.checkout.block.render', () => {
    const count = h('s-text', { type: 'strong' });
    const all = h('s-text', { color: 'subdued' });
    shopify.metafields.subscribe((entries) => {
      count.textContent = `Clicks stored in the cart: ${current()}`;
      all.textContent = `shopify.metafields: ${entries.map((entry) => `${entry.namespace}.${entry.key}=${entry.value}`).join(', ') || 'none'}`;
    });
    mount(h('s-stack', { gap: 'small-200' },
      h('s-heading', {}, 'Cart metafield (2026-10, older change)'),
      count,
      h('s-stack', { direction: 'inline', gap: 'small-200' },
        h('s-button', {
          variant: 'secondary',
          onClick: () => shopify.applyMetafieldChange({ type: 'updateMetafield', namespace: NAMESPACE, key: KEY, valueType: 'integer', value: current() + 1 }),
        }, 'Count a click'),
        h('s-button', {
          variant: 'secondary',
          onClick: () => shopify.applyMetafieldChange({ type: 'removeMetafield', namespace: NAMESPACE, key: KEY }),
        }, 'Reset')),
      all));
  });
})();

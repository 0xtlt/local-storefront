// ui-settings-empty · API 2026-10 · purchase.checkout.block.render
// The toml declares three settings (`title`, `tone`, `threshold`). The local checkout does not
// fill settings in, so `shopify.settings.value` is `{}`: the banner says so and uses its
// defaults. On Shopify, the merchant sets them in the checkout editor.
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

  const DECLARED = ['title', 'tone', 'threshold'];
  const DEFAULTS = { title: 'Settings (defaults in use)', tone: 'info', threshold: 50 };

  shopify.extend('purchase.checkout.block.render', () => {
    const banner = h('s-banner', {});
    const list = h('s-unordered-list');
    const raw = h('s-text', { color: 'subdued' });
    banner.append(h('s-stack', { gap: 'small-200' }, raw, list));
    shopify.settings.subscribe((settings) => {
      const values = settings || {};
      banner.setAttribute('heading', values.title || DEFAULTS.title);
      banner.setAttribute('tone', values.tone || DEFAULTS.tone);
      raw.textContent = `shopify.settings.value = ${JSON.stringify(values)}`;
      list.replaceChildren(...DECLARED.map((key) => h('s-list-item', {},
        key in values ? `${key}: ${values[key]}` : `${key}: not set, default ${JSON.stringify(DEFAULTS[key])}`)));
    });
    mount(banner);
  });
})();

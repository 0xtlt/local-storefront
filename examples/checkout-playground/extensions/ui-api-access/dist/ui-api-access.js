// ui-api-access · API 2026-10 · purchase.checkout.block.render · api_access
// Calls the APIs that answer asynchronously and shows each answer:
//   - `shopify.query` (Storefront API): rejects in the local checkout, the message is shown;
//   - `shopify.sessionToken.get()`: the claims of the token (unsigned locally);
//   - `shopify.storage`: a counter of the times this block rendered in this checkout;
//   - `shopify.analytics.publish`: resolves true.
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

  const claims = (token) => {
    const part = token.split('.')[1] || '';
    return JSON.parse(atob(part.replace(/-/g, '+').replace(/_/g, '/')));
  };

  shopify.extend('purchase.checkout.block.render', async () => {
    const rows = {
      query: h('s-text', {}, '…'),
      sessionToken: h('s-text', {}, '…'),
      storage: h('s-text', {}, '…'),
      analytics: h('s-text', {}, '…'),
    };
    mount(h('s-stack', { gap: 'small-200' },
      h('s-heading', {}, 'Asynchronous APIs'),
      Object.entries(rows).map(([name, node]) =>
        h('s-stack', { gap: 'none' }, h('s-text', { type: 'strong' }, name), node))));

    shopify.query('query { shop { name } }')
      .then((result) => { rows.query.textContent = `resolved: ${JSON.stringify(result)}`; })
      .catch((error) => { rows.query.textContent = `rejected: ${error.message}`; });

    shopify.sessionToken.get()
      .then((token) => {
        const { sub, dest, aud, exp, sid } = claims(token);
        rows.sessionToken.textContent = `sub ${sub || '(guest)'} · dest ${dest} · aud ${aud} · sid ${sid} · expires ${new Date(exp * 1000).toLocaleTimeString()}`;
      })
      .catch((error) => { rows.sessionToken.textContent = `failed: ${error.message}`; });

    const seen = (await shopify.storage.read('renders')) || 0;
    await shopify.storage.write('renders', seen + 1);
    rows.storage.textContent = `This block rendered ${seen + 1} time(s) in this checkout (reload the page to count).`;

    const published = await shopify.analytics.publish('ui-api-access-rendered', { renders: seen + 1 });
    rows.analytics.textContent = `publish → ${published}`;
  });
})();

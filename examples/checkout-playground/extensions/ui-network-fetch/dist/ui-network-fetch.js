// ui-network-fetch · API 2027-01 · purchase.checkout.block.render · network_access
// Calls the playground's fake API (`node tools/fake-api.mjs`, http://localhost:8787) with
// `fetch`, with a 3 second timeout, and shows the status and the start of the body. When the
// server is down (or answers without CORS headers: the frame has an opaque origin), it shows a
// warning with the error instead of breaking. A button calls again.
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

  const ENDPOINT = 'http://localhost:8787/';
  const TIMEOUT_MS = 3000;

  shopify.extend('purchase.checkout.block.render', () => {
    const banner = h('s-banner', { tone: 'info', heading: `GET ${ENDPOINT}` });
    const text = h('s-text', {}, 'Calling…');
    const again = h('s-button', { variant: 'secondary', onClick: () => call() }, 'Call again');
    banner.append(h('s-stack', { gap: 'small-200' }, text, again));

    async function call() {
      text.textContent = 'Calling…';
      again.setAttribute('loading', '');
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
      const started = Date.now();
      try {
        const response = await fetch(ENDPOINT, { signal: controller.signal, headers: { accept: 'application/json' } });
        const body = await response.text();
        banner.setAttribute('tone', response.ok ? 'success' : 'warning');
        banner.setAttribute('heading', `GET ${ENDPOINT} → ${response.status} in ${Date.now() - started} ms`);
        text.textContent = body.length > 300 ? `${body.slice(0, 300)}…` : body || '(empty body)';
      } catch (error) {
        banner.setAttribute('tone', 'warning');
        banner.setAttribute('heading', 'The fake API does not answer');
        text.textContent = `${error.name === 'AbortError' ? `No answer within ${TIMEOUT_MS} ms` : `${error.name}: ${error.message}`}. Start it with \`node tools/fake-api.mjs\`, then press Call again.`;
      } finally {
        clearTimeout(timer);
        again.removeAttribute('loading');
      }
    }

    mount(banner);
    call();
  });
})();

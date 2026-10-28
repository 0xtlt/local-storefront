// ui-broken-old-api · API 2025-07 · purchase.checkout.block.render · REFUSED ON PURPOSE
// What a React extension on 2025-07 boiled down to: `shopify.extend` handing a remote-ui
// `root` and the API to the callback, which builds `Banner` components on the root. 2025-07 was
// the last release on React and remote-ui; the local checkout runs 2026-10 and 2027-01 only, so
// it does not load this bundle and the page says why.
(() => {
  shopify.extend('purchase.checkout.block.render', (root, api) => {
    const banner = root.createComponent('Banner', { title: 'Hello from 2025-07', status: 'info' });
    banner.appendChild(root.createText(`Rendered for ${api.shop.name}.`));
    root.appendChild(banner);
    root.mount();
  });
})();

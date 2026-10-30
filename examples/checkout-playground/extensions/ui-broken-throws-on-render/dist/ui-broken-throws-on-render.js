// ui-broken-throws-on-render · API 2026-10 · purchase.checkout.block.render · BROKEN ON PURPOSE
// Registers its target, then throws as soon as the target runs: the page shows the error
// where the extension would be.
(() => {
  shopify.extend('purchase.checkout.block.render', () => {
    const lines = shopify.lines.value;
    // A typical mistake: reading a field of something that is not there.
    return lines[99].merchandise.title;
  });
})();

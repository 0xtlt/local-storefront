// ui-broken-renders-nothing · API 2027-01 · purchase.checkout.block.render · EDGE CASE
// Registers its target and draws nothing: a frame zero pixels tall, no error.
(() => {
  shopify.extend('purchase.checkout.block.render', () => {
    // Nothing to show for this cart.
  });
})();

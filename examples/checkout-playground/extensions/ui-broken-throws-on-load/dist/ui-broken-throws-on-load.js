// ui-broken-throws-on-load · API 2026-10 · purchase.checkout.block.render · BROKEN ON PURPOSE
// Throws while the bundle is evaluated, before `shopify.extend` is called: the frame reports
// the error, then that the bundle registered nothing.
(() => {
  throw new Error('ui-broken-throws-on-load: the bundle throws before it registers its target.');
})();

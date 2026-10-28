# Checkout playground

A Shopify app made to try `lsf checkout` on: many Functions and checkout UI extensions, and a
scenario for each case. Nothing here is deployed.

```bash
cd examples/checkout-playground
lsf checkout --app .
```

The store data (`shopify-local/`) is the demo store that `lsf checkout init` writes.

## Checkout UI extensions

The `extensions/ui-*` folders are checkout UI extensions. Each bundle, `dist/<handle>.js`, is
written by hand: a plain script that registers its targets with `shopify.extend` and draws
Polaris web components (`s-banner`, `s-stack`, ...) into `document.body`. There is no build
step and no source to rebuild from: edit the bundle and reload the page. Each bundle starts with
a comment that says what it shows. Every one has a scenario, `shopify-local/checkouts/ui-*.json`
(open `/checkout?scenario=<name>`; add `&targets=all` to see every target, the thank-you page
included).

Only Polaris web components are used: the local checkout runs API 2026-10 and 2027-01, and
React and remote-ui extensions stopped at 2025-07 (`ui-broken-old-api` shows the refusal).

| Extension | API | Targets | What it shows | Scenario |
|---|---|---|---|---|
| `ui-header-notice` | 2026-10 | `checkout.header.render-after` | A banner with the shop's name and the distance to free shipping, live | `ui-static-targets` |
| `ui-contact-newsletter` | 2027-01 | `checkout.contact.render-after` | Who gets the confirmation (`buyerIdentity`); a newsletter checkbox that writes a cart attribute | `ui-static-targets`, `ui-address` |
| `ui-address-autofill` | 2026-10 | `checkout.delivery-address.render-before`, `-after` | The country of the checkout; buttons that fill the address, change only the ZIP, or move it to France (`applyShippingAddressChange`) | `ui-address` |
| `ui-shipping-options` | 2027-01 | `checkout.shipping-option-list.render-before`, `-after`, `shipping-option-item.render-after`, `shipping-option-item.details.render` | The options, their costs and the selected one | `ui-static-targets` |
| `ui-payment-notes` | 2026-10 | `checkout.payment-method-list.render-before`, `-after` | The payment options and their types, the selected one | `ui-static-targets` |
| `ui-reductions-codes` | 2027-01 | `checkout.reductions.render-before`, `-after` | The codes entered, each removable; buttons that enter codes (`applyDiscountCodeChange`) | `ui-discount-codes` |
| `ui-line-properties` | 2026-10 | `checkout.cart-line-item.render-after` | Under each line: SKU, options, line item properties, selling plan, discounts | `ui-static-targets` |
| `ui-line-quantity` | 2027-01 | `checkout.cart-line-item.render-after` | −, + and Remove under each line (`applyCartLinesChange`) | `ui-cart-edits`, `ui-buyer-journey` |
| `ui-order-summary` | 2026-10 | `checkout.cart-line-list.render-after` | Lines, units and the four amounts of `shopify.cost` | `ui-static-targets` |
| `ui-footer-links` | 2026-10 | `checkout.footer.render-after` | Links (opened in a new tab) and `shopify.extension` | `ui-static-targets` |
| `ui-thank-you` | 2027-01 | the 7 `thank-you.*` targets | The order confirmation, the customer, the lines; a closable announcement | `ui-thank-you` |
| `ui-every-target` | 2026-10 | the 24 targets the page renders | A badge naming each target: a map of the page | `ui-every-target` |
| `ui-gift-wrap` | 2027-01 | `checkout.block.render` | A checkbox that adds the Gift Wrap line and a `gift_wrap` attribute, and takes them back | `ui-cart-edits` |
| `ui-upsell` | 2026-10 | `checkout.block.render` | Three offers: one added, one sold out, one unknown variant (both refused, with the message) | `ui-cart-edits` |
| `ui-note-attributes` | 2026-10 | `checkout.block.render` | Edits the cart note and two attributes; lists the attributes, each removable | `ui-note-attributes` |
| `ui-metafield-writer` | 2027-01 | `checkout.block.render` | Writes and removes a cart metafield declared with `owner_type = "CART"` | `ui-metafields` |
| `ui-metafield-legacy` | 2026-10 | `checkout.block.render` | Counts clicks with the older `updateMetafield`, declared without `owner_type` | `ui-metafields` |
| `ui-terms-gate` | 2027-01 | `checkout.actions.render-before` | A terms checkbox; `buyerJourney.intercept` blocks Pay until it is ticked (`block_progress`) | `ui-buyer-journey` |
| `ui-cart-limit-gate` | 2026-10 | `checkout.block.render` | Blocks Pay while the cart has more than five units (`block_progress`) | `ui-buyer-journey` |
| `ui-intercept-no-capability` | 2026-10 | `checkout.block.render` | An interceptor that answers "block" without `block_progress`: ignored | `ui-buyer-journey` |
| `ui-checkout-inspector` | 2026-10 | `checkout.block.render` | Everything the `shopify` global exposes, live: buyer, localization, cost, discounts, delivery, lines, attributes, metafields... | `ui-inspector`, `ui-inspector-b2b` |
| `ui-i18n-greeting` | 2027-01 | `checkout.block.render` | `i18n.translate` from `locales/en.default.json` and `locales/fr.json`, plurals, a fallback, a missing key; money, number and date formats | `ui-i18n`, `ui-i18n-fr` |
| `ui-settings-empty` | 2026-10 | `checkout.block.render` | Three declared settings; `shopify.settings.value` is `{}` locally, so the defaults show | `ui-settings` |
| `ui-network-fetch` | 2027-01 | `checkout.block.render` | `fetch` to `http://localhost:8787/` (`network_access`), or a warning when the fake API is down | `ui-capabilities` |
| `ui-api-access` | 2026-10 | `checkout.block.render` | `query` rejected (no Storefront API locally), the session token's claims, `storage`, `analytics` | `ui-capabilities` |
| `ui-change-refusals` | 2026-10 | `checkout.block.render` | One button per change the checkout refuses, and its answer | `ui-change-refusals` |

`ui-everything` runs all of the above at once, and `ui-shop-look` a set of them in the Shop
look. The upsell and gift wrap add the products `shopify-local/products/ui-*.json`, whose
variant ids are fixed (`9100000011`, `9100000021`, `9100000031`).

The `ui-broken-*` extensions fail on purpose. They are in no scenario but their own:

| Extension | API | What goes wrong | Scenario |
|---|---|---|---|
| `ui-broken-throws-on-load` | 2026-10 | The bundle throws before `shopify.extend`: nothing is registered | `ui-broken-runtime` |
| `ui-broken-throws-on-render` | 2026-10 | Throws as soon as its target runs | `ui-broken-runtime` |
| `ui-broken-throws-on-event` | 2027-01 | Throws in a click handler, rejects a promise, throws in a signal subscriber | `ui-broken-runtime` |
| `ui-broken-renders-nothing` | 2027-01 | Draws nothing: a frame 0px tall | `ui-broken-runtime` |
| `ui-broken-unregistered-target` | 2026-10 | Registers the footer target while its toml declares the block one | `ui-broken-runtime` |
| `ui-broken-slow` | 2026-10 | Renders after 4 s; its interceptor answers after 7 s, past the 5 s the checkout waits | `ui-broken-runtime` |
| `ui-broken-unknown-target` | 2026-10 | Declares `purchase.checkout.nowhere.render` and `chat.render` (no place) next to the block | `ui-broken-loader` |
| `ui-broken-old-api` | 2025-07 | A remote-ui bundle on an API version that is not supported | `ui-broken-loader` |
| `ui-broken-no-api-version` | none | No `api_version` | `ui-broken-loader` |
| `ui-broken-not-built` | 2026-10 | No `dist/` bundle | `ui-broken-loader` |
| `ui-broken-missing-owner-type` | 2027-01 | A metafield without the `owner_type` 2027-01 requires | `ui-broken-loader` |
| `ui-broken-bad-owner-type` | 2027-01 | `owner_type = "ORDER"`, not one of the seven | `ui-broken-loader` |
| `ui-broken-tall` | 2026-10 | 150 rows, far taller than the window | `ui-broken-layout` |
| `ui-broken-long-text` | 2026-10 | Long unbroken words, URLs and codes in a banner, badge, button, field, link and under each line | `ui-broken-layout` |

A scenario without an `extensions` list runs every extension of the app, the broken ones
included. `crates/checkout/assets/tests/playground.test.mjs` (`mise run test:js`) checks that
every bundle registers the targets of its toml and that the working ones draw without an error.

# Checkout UI extension API versions

The local checkout runs checkout UI extensions built for **2026-10** (stable) and **2027-01**
(release candidate, stable on 2027-01-01). Nothing else: an extension on another `apiVersion` is
not loaded and the page says why. The list is `API_VERSIONS` in
`crates/checkout/assets/api-versions.js`; adding a release is one entry there.

## What differs between 2026-10 and 2027-01

Researched on 2026-10-26 with the `shopify-dev` MCP (`learn_shopify_api`, `search_docs_chunks` and
`validate` for `polaris-checkout-extensions`), the developer changelog and the npm packages.

| | 2026-10 | 2027-01 (RC) |
| --- | --- | --- |
| `owner_type` of `[[extensions.metafields]]` and `[[extensions.targeting.metafields]]` | optional | **required**: a deployment without it is blocked |
| Targets | 31, as listed in the 2026-10 reference | same |
| `shopify` API members (Addresses, Analytics, Attributes, Buyer Identity, Buyer Journey, Cart Instructions, Cart Lines, Checkout Token, Cost, Customer Privacy, Delivery, Discounts, Extension, Gift Cards, Localization, Localized Fields, Metafields, Note, Order, Payments, Storefront API, Session Token, Settings, Shop, Storage) | | same, nothing added or removed |
| Polaris `s-*` components and their props | `@shopify/ui-extensions` 2026.10.x | same, nothing announced |
| `Merchandise` of a cart line (and of a `lineComponents` entry) | `title` is the title of the variant, `subtitle` optional, `product` has `id`, `vendor`, `productType` | same types in `@shopify/ui-extensions` 2027.0.0-rc.5 |
| Deprecations | none | none |

`owner_type` names the resource the metafield is read from: `CART`, `COMPANY`, `COMPANY_LOCATION`,
`CUSTOMER`, `PRODUCT`, `PRODUCTVARIANT` or `SHOP`. If the same `namespace` and `key` are needed on
several resources, declare it once for each. Extensions without a metafield declaration are not
affected.

`Merchandise` follows the 2026-10 reference of the Cart Lines API, which is the same in both
releases: `title` is "the display title of the product variant, such as "Small" or "Red / Large".
This is the variant-specific label, not the parent product title"; `subtitle` is "a secondary
description for the variant […] undefined if no subtitle is available"; `product` exposes the
`id`, `vendor` and `productType` of the product, no title. So `shopify.lines` gives the variant
title as `title` (`Default Title` for the only variant of a product without options, which is
the title Shopify gives it) and no `subtitle`: the store data has no secondary description of a
variant. Bundle components (`lineComponents[].merchandise`) and the lines of the thank-you page
are built the same way. Earlier releases worded the field as "the product variant's title"; the
reference does not say what a checkout returns at runtime, which this page could not check
against a real store.

That is the only change Shopify documents for checkout UI extensions between the two releases.
The rest of the 2027-01 release notes (collections, discounts, markets, metaobjects, inventory,
tax apps, access scopes) concerns the Admin API. Other entries that touch the surface but belong
to 2026-10 or earlier, so they do not tell the two versions apart:

- 2026-10 marks `territoryCode` on customer addresses deprecated in favour of `countryCode`
  (Customer Account API, listed under Checkout UI): not a change of the `shopify` global.
- 2026-10 POS UI changes (`staffMemberId` removed, cart mutations reject) are not checkout.
- Since 2026-01 an extension that can block the buyer is non-blocking until the merchant
  allows it (`block_progress` capability, which the page already honours).
- 2025-07 is the last version on React and remote-ui; Shopify blocks updating such an extension
  after 2026-10-01. 2025-10 moved to Preact and Polaris web components.

### Where the sources stop

- The MCP knows 2026-10 and older: `learn_shopify_api` and `validate` answer
  "Version '2027-01' is not available" and `search_docs_chunks` has no 2027-01 pages. The
  `…/checkout-ui-extensions/2027-01` reference URL serves the 2026-10 reference.
- `@shopify/ui-extensions` has no 2027-01 package yet. Its newest tags are `2026.10.2` and
  `rc` (`2026.10.0-rc.14`). A `2027.0.0-rc.5` is published, dated 2026-08-17, before
  `2026.7.4`: an early build, not the 2027-01 candidate, so it is not used.
- A candidate can still change until 2027-01-01. Re-check the changelog
  (<https://shopify.dev/changelog>) before that date.

Sources: [owner_type changelog (2026-10-08)](https://shopify.dev/changelog/posts/metafield-owner-types-required-for-checkout-and-customer-account-ui-extensions),
[2026-10 release notes](https://shopify.dev/changelog/release-notes/2026-10),
[2027-01 release notes](https://shopify.dev/changelog/release-notes/2027-01),
[metafields in checkout](https://shopify.dev/docs/apps/build/checkout/metafields),
[checkout UI extensions 2026-10](https://shopify.dev/docs/api/checkout-ui-extensions/2026-10).

## What the host does with the version

- `refusalOf(ext)` (`api-versions.js`) gives the reason an extension is not loaded, or `null`:
  1. `apiVersion` is not `2026-10` or `2027-01`;
  2. a declared metafield has an `owner_type` that is not one of the seven;
  3. the version requires `owner_type` (2027-01) and a declared metafield has none.
- `createExtensionHost` leaves those extensions out (no slot, no iframe) and lists them in
  `host.refusals()`. The page shows each as a warning banner, "`handle`: not loaded. `reason`",
  and marks it "not loaded" in the Functions dialog.
- The `init` message carries `apiVersion`, which the sandbox exposes as
  `shopify.extension.apiVersion`. The two versions have the same `shopify` surface, so the
  sandbox has no version switch: it is one code path.
- State JSON: `extensions[].metafields` is the list of declarations,
  `[{"namespace", "key", "ownerType"?}]` (`ownerType` is the toml's `owner_type`). Absent means
  none declared. `crates/checkout/src/app.rs` reads them from `[[extensions.metafields]]` and
  reports a missing `owner_type` on 2027-01 as an error diagnostic; an extension whose
  `api_version` is not supported is an error too and is left out of `extensions`.

## Fixtures (`crates/checkout/tests/fixtures/app/extensions/`)

| Extension | API | Specific to its version |
| --- | --- | --- |
| `gift-message` | 2026-10 | declares `$app:gift.wrapped` **without** `owner_type`, which only 2026-10 accepts; DOM drawn by hand on the signals of the `shopify` global |
| `order-notice` | 2027-01 | a real Preact + `@preact/signals` bundle (`src/`, built with esbuild, see the header of `dist/order-notice.js`); declares `$app:notice.dismissed` with `owner_type = "CART"` and reads it from `shopify.appMetafields` |

`validate` (2026-10, with each target) caught two real mistakes in `order-notice`'s first draft,
`variant="tertiary"` on `s-button` and `size` on `s-text`, both fixed. It still reports
`event.currentTarget.value` on `s-text-area` as an error ("does not exist on type
`EventTarget`"); that is a false positive: Shopify's own `s-number-field` migration example
fails the same way.

Preview: `node crates/checkout/assets/preview/serve.mjs`, then `/` and `/?variant=shop`. Add
`?apiVersion=order-notice:2025-07` to see the page refuse an extension.

## Removed

React and remote-ui bundles, the versions that predate 2026-10: `sandbox-legacy.js`
(remote-ui host, legacy component map), the `Checkout::…` target aliases
(`Checkout::Dynamic::Render`, …) with `canonicalTarget` and `namesOfTarget`, `modeOf` and the
`mode` and `names` fields of `init`, the legacy subscribable flavour of the signals (`.current`,
no call back at once).

## Gaps

- `[[extensions.targeting.metafields]]` (per target) is not modelled: only the extension-wide
  declarations are checked.
- `shopify.appMetafields` returns every cart metafield, not only the declared ones, and no
  customer, product or other owner data (the local checkout has none).

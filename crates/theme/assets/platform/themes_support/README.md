# Shopify's global assets

What `{{ 'name' | shopify_asset_url }}` points at, served by the local CDN under
`/cdn/shopifycloud/storefront/assets/themes_support/`. Shopify lists them at
https://shopify.dev/docs/api/liquid/filters/shopify_asset_url.

| File | What is served |
| --- | --- |
| `vendor/qrcode.js` | [qrcode.js](https://github.com/davidshimjs/qrcodejs) by davidshimjs, commit `04f46c6a0708418cb7b96fc563eacae0fbf77674` (master): the library is verbatim, under a header comment that holds davidshimjs' copyright and the MIT permission notice. The notice travels with the file, so it is in every `lsf` binary and in what the CDN serves; the file also keeps the notice of Kazuhiko Arase's library it embeds. Shopify's own `vendor/qrcode.js` is this library. |
| `shopify_common.js` | Our own small implementation of `Shopify.formatMoney` and `Shopify.resizeImage`. |
| `option_selection.js`, `api.jquery.js`, `customer_area.js`, `currencies.js` | A `console.warn`: Shopify's files are proprietary and are not emulated. |
| `customer.css` | An empty stylesheet. |

Shopify's proprietary files are not copied here.

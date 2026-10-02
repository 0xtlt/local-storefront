# Compatibility with Shopify

The goal is HTML identical to what Shopify renders for the same theme and the same data. This
page says how that is checked, what is covered, and where the local server knowingly differs.

## How fidelity is checked

- **Liquid itself** is a port of Shopify's reference implementation (the `liquid` Ruby gem,
  version 5.14), including its lax parsing mode, whitespace control, number and string
  coercions, and error messages. 548 templates covering every tag and filter are rendered by
  both implementations and must produce the same output (`mise run oracle:golden` regenerates
  the expectations from the gem).
- **Shopify's objects, filters and tags** are checked against the examples of
  [shopify.dev](https://shopify.dev/docs/api/liquid): each example's code and data are replayed
  and compared with the documented output. 195 of 313 examples match. The others depend on the
  content of Shopify's own demo store (products, images and ids that are not in the examples'
  data), not on behaviour known to differ.
- **Real themes.** Shopify's Horizon and Dawn themes render every page type without a Liquid
  error, and their JavaScript (variant pickers, cart drawer, predictive search, filters, section
  rendering) runs against the local endpoints unchanged.

`lsf check` reports what a theme uses that is not implemented: Liquid that does not parse, and
filters that do not exist here.

## Liquid tags

`assign`, `break`, `capture`, `case`, `comment`, `content_for`, `continue`, `cycle`,
`decrement`, `doc`, `echo`, `for`, `form`, `if`, `ifchanged`, `include`, `increment`, `#`,
`javascript`, `layout`, `liquid`, `paginate`, `raw`, `render`, `schema`, `section`,
`sections`, `style`, `stylesheet`, `tablerow`, `unless`.

## Liquid filters

Every filter of Shopify's reference: array, string, math, date, money, color, font, media,
HTML, URL, cart, customer, metafield and translation filters, 159 in total. Among them
`image_url` and `image_tag` (with `srcset`, `sizes`, `preload`, focal points), the legacy
`img_url` family, `money` and its variants with every `money_format` placeholder, `t` with
pluralization and interpolation, `date` with locale formats, `json`, `structured_data`,
`payment_type_svg_tag`, `placeholder_svg_tag`, `font_face`, `color_*`, `metafield_tag`,
`default_pagination`, `link_to_*`, `highlight`, `time_tag`.

## Objects

Global objects: `shop`, `cart`, `customer`, `request`, `routes`, `localization`, `template`,
`theme`, `settings`, `section`, `block`, `closest`, `linklists`, `collections`, `all_products`,
`pages`, `blogs`, `articles`, `images`, `metaobjects`, `product`, `collection`, `page`, `blog`,
`article`, `policy`, `order`, `gift_card`, `search`, `predictive_search`, `recommendations`,
`paginate`, `form`, `forloop`, `tablerowloop`, `canonical_url`, `page_title`,
`page_description`, `page_image`, `handle`, `current_page`, `current_tags`, `shop_locale`,
`content_for_header`, `content_for_layout`, `country_option_tags`, `all_country_option_tags`,
`powered_by_link`, `additional_checkout_buttons`.

With their related types: variants, options and option values (with swatches), media (images,
videos, external videos, 3D models), image presentation and focal points, collection filters
and sort options, links, addresses, orders and line items, comments, metafields of every type,
metaobjects, countries, languages, currencies, markets, fonts, colors, color schemes, quantity
rules and quantity price breaks, unit prices.

## Theme architecture

- JSON templates and Liquid templates, alternate templates (`?view=`, `template_suffix`),
  layouts (`{% layout %}`, `"layout"` in JSON templates, `layout none`).
- Sections, static sections (with the settings stored in `settings_data.json` or the `default`
  of their schema), section groups (`{% sections %}`), section and block settings with
  defaults from the schema, `disabled` sections and blocks.
- Theme blocks (`blocks/*.liquid`), nested blocks, static blocks, `{% content_for 'blocks' %}`
  and `{% content_for 'block' %}`, `closest.<resource>`.
- Settings resolved to the objects Liquid expects: `image_picker`, `video`, `video_url`,
  `product`, `collection`, `page`, `blog`, `article`, `product_list`, `collection_list`,
  `link_list`, `color`, `color_palette`, `color_scheme`, `color_scheme_group`, `font_picker`,
  `url`, `liquid`, `checkbox`, `number` and `range`; the other types are passed as written.
  Dynamic sources are evaluated (`{{ product.title }}`,
  `{{ closest.product.metafields.custom.x }}`).
- Locales: storefront translations and schema translations, pluralization rules by language,
  the fallback to the default locale, Shopify's built-in translations.
- Assets: `asset_url` and friends, `.liquid` assets, the `{% stylesheet %}` and
  `{% javascript %}` bundles, `{% style %}`, preload headers.

## What Shopify injects into pages

A storefront page is more than the theme's markup: Shopify adds scripts through
`{{ content_for_header }}`, before `</head>` and before `</body>`. `lsf` adds the same ones,
modelled on what a Shopify storefront serves, with every URL pointing at the local server:

| What | Where | Local behaviour |
|---|---|---|
| `Shopify.shop`, `locale`, `currency`, `country`, `theme`, `cdnHost`, `routes.root`, `modules` | `content_for_header` | Same values and shape as on Shopify. `Shopify.designMode` is not set, as on a live storefront. |
| `shopify-features` JSON, `__st`, `shopify-digital-wallet` meta, `hreflang` alternates | `content_for_header` | Same shape. The Storefront API token is a placeholder. |
| `Shopify.loadFeatures`, `Shopify.autoloadFeatures` | `content_for_header` | `consent-tracking-api` installs a local `Shopify.customerPrivacy`. Features hosted by Shopify (`model-viewer-ui`, `shopify-xr`, ...) report an error to `onLoad`, which themes handle. |
| `Shopify.customerPrivacy` | after `loadFeatures` | Consent is kept in the browser. It has to be collected in the EEA, the United Kingdom and Switzerland (by `Shopify.country`): there, nothing is allowed and `shouldShowBanner()` is true until `setTrackingConsent` is called. A real store decides this in its privacy settings. |
| `Shopify.PaymentButton` | `content_for_header` | Present; `init()` does nothing, since dynamic checkout buttons are drawn by Shopify. |
| `ShopifyAnalytics.meta` (`currency`, `page`, `product`), `trekkie` | before `</head>` | Same shape. Nothing is sent anywhere. |
| `Shopify.analytics.publish` | before `</head>` | Calls are kept in `Shopify.analytics.replayQueue`, which a test can read. |
| `Shopify.actions` (`getCart`, `updateCart`, `openCart`) | before `</body>` | The [standard storefront actions](https://shopify.dev/docs/api/storefront-events-and-actions), with `configure`, `isDefault`, the `shopify:cart:*` events and the default refresh of Horizon- and Dawn-style carts. They write to the local cart instead of the Storefront API. |
| `cart` cookie | cart endpoints | Set when something first goes into the cart, as on Shopify: `getCart` resolves with `cart: null` until then. |

Left out, because they only talk to Shopify's own services: analytics collection (web pixels,
performance monitoring), Shop Pay and sign-in with Shop, bot protection, and the MCP and
agent endpoints.

A theme that imports Shopify's standard events library from
`https://cdn.shopify.com/storefront/standard-events.js`, as Horizon does, loads it from
Shopify's CDN: it is a static file, but it does need network access.

## Known differences

These are deliberate or not done yet. None of them raises a Liquid error.

| Area | Difference |
|---|---|
| Checkout | Not part of a theme. `/checkout` shows a summary of the cart. |
| Apps | App blocks and app embeds render nothing. `content_for_header` contains the scripts a theme relies on (`Shopify.*` globals, routes, the compiled asset tags), not Shopify's analytics and app scripts. |
| Customer accounts | Login and logout work against the customers of the data. Registration, password reset, activation and address editing are not simulated: their forms answer with an error. New customer accounts (hosted by Shopify) are not available. |
| Discounts | Carts and orders have no discounts: discount arrays are empty and totals are undiscounted. `Shopify.actions.updateCart` reports every discount code as not applicable. |
| Standard actions | A change the cart refuses (unknown variant, not enough stock) resolves with `userErrors` whose code is `INVALID`, and leaves that line untouched. The Storefront API would add what is in stock and return a warning. |
| Selling plans | Products have no selling plans (`selling_plan_groups` is empty). |
| B2B | No companies, company locations or catalogs. |
| Markets | One price list: every country sees the shop currency's prices. The selected country changes `localization.country`, not prices. |
| Taxes and shipping | Not computed. |
| Search | A simple search over titles, descriptions, vendors, types, tags and SKUs. Ranking is not Shopify's. |
| Recommendations | Products of the same collections, or the product's explicit `recommendations`. |
| Collection filters | Availability, price, vendor, product type, tag and variant options. Metafield filters are not generated. |
| Placeholder artwork | `placeholder_svg_tag` draws generic artwork of the right proportions, not Shopify's illustrations. |
| Fonts | Shopify's font library is not bundled. `font_face` and `font_url` generate the same markup and URLs; the files are blank unless provided in `files/fonts/`. |
| Shopify's shared assets | Requests under `/cdn/shopifycloud/` (payment buttons, model viewer UI, ...) get an empty file of the right type. |
| Theme editor | `request.design_mode` is always `false`, `Shopify.designMode` is not set, and `block.shopify_attributes` is empty. |
| Integers | 64-bit. Liquid on Shopify uses arbitrary-precision integers. |
| `robots.txt.liquid`, `sitemap` | Not rendered: `/robots.txt` disallows everything. |

Found a difference that is not in this table? It is a bug: a Liquid snippet and the HTML
Shopify renders for it are enough to reproduce it.

# Compatibility with Shopify

The goal is HTML identical to what Shopify renders for the same theme and the same data. This
page says how that is checked, what is covered, and where the local server knowingly differs.

## How fidelity is checked

- **Liquid itself** is a port of Shopify's reference implementation (the `liquid` Ruby gem,
  version 5.14), including its lax parsing mode, whitespace control, number and string
  coercions, and error messages. 616 templates covering every tag and filter are rendered by
  both implementations and must produce the same output (`mise run oracle:golden` regenerates
  the expectations from the gem).
- **Whitespace is trimmed the way storefronts trim it**, which is not quite what the language
  says. `{{-` and `{%-` remove the whitespace before them. When the text before one is nothing
  but whitespace, a storefront keeps its first character:

  ```liquid
  {{ 'a' }}
    {{- 'b' }}
  ```

  prints `a` and `b` on two lines on Shopify, and `ab` in the language. The gem has this
  behaviour as a parse option (`bug_compatible_whitespace_trimming`), and themes are rendered
  with it. It is what gives a `robots.txt.liquid` its line breaks: the `robots.txt` of a
  store, rendered from its own template, is reproduced byte for byte.
- **Shopify's test suite of the language**, [liquid-spec](https://github.com/Shopify/liquid-spec),
  is replayed against the engine (`mise run liquid-spec:fetch` downloads it). 4,630 of its
  7,369 language specs apply here, and 4,608 of them pass. The others expect a strict parser
  to reject a template that Shopify accepts in a theme (1,923), or need what the engine has
  no equivalent for: Ruby objects, byte strings, render limits (816). The 22 that differ are
  [listed below](#differences-with-liquid-spec).
- **Shopify's objects, filters and tags** are checked against the examples of
  [shopify.dev](https://shopify.dev/docs/api/liquid): each example's code and data are replayed
  and compared with the documented output. 196 of 313 examples match. The others depend on the
  content of Shopify's own demo store (products, images and ids that are not in the examples'
  data), not on behaviour known to differ.
- **Real themes.** Shopify's Horizon and Dawn themes render every page type without a Liquid
  error, and their JavaScript (variant pickers, cart drawer, predictive search, filters, section
  rendering, pickup availability) runs against the local endpoints unchanged.

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
`powered_by_link`, `additional_checkout_buttons`, `robots`, `self`.

With their related types: variants, options and option values (with swatches), media (images,
videos, external videos, 3D models), image presentation and focal points, collection filters
(with swatches) and sort options, links, addresses, orders and line items, comments,
metafields of every type, metaobjects, countries, languages, currencies, markets, fonts,
colors, color schemes, quantity rules and quantity price breaks, unit prices, selling plan
groups, selling plans and their allocations, store availabilities and locations, companies,
company locations and company addresses.

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
| Customer accounts | New customer accounts are hosted by Shopify, outside the theme. `/account` is a page of `lsf` that stands in for them: it shows who is logged in and lets you become anyone in the data. Every account URL (`/account/login`, `/account/register`, `/customer_authentication/login`, ...) leads to it. With legacy accounts (a theme with `templates/customers`), the theme's pages are rendered, and login and logout work against the customers of the data. Registration, password reset, activation and address editing are not simulated: their forms answer with an error. |
| Password page | `/password` always shows the theme's password page, and accepts `shop.password` (`password` by default). The storefront is never locked: nothing redirects to that page. On Shopify, a protected store redirects every page to it, and an open store redirects it to the home page. |
| Discounts | Carts and orders have no discounts: discount arrays are empty and totals are undiscounted. `Shopify.actions.updateCart` reports every discount code as not applicable. |
| Standard actions | A change the cart refuses (unknown variant, not enough stock) resolves with `userErrors` whose code is `INVALID`, and leaves that line untouched. The Storefront API would add what is in stock and return a warning. |
| Selling plans | Selling plan groups, plans and allocations are there, on products, variants, the cart and `/products/<handle>.js`. A group applies to every variant of a product, not to some of them. Prepaid plans are not modelled: `per_delivery_price` is the price. The line items of orders have no selling plan. The widget of a subscription app is an app block, which renders nothing: the theme's own purchase options do. |
| Store pickup | `variant.store_availabilities` and `/variants/<id>?section_id=` are there. Locations come in the order of the data: Shopify sorts them by distance from the visitor. Choosing pickup happens at checkout, which is not part of a theme, so `order.pickup_in_store?` is always `false`. |
| Swatches | A swatch is given to option values by name, or to one value of a product. On Shopify it comes from the category metafield the option is linked to. `filter_value.image` (the `image` presentation of a filter) is not generated. |
| B2B | Companies, company locations, their addresses and metafields are there: `customer.b2b?`, `current_company`, `current_location`, `company_available_locations` and the link that changes location. Catalogs are not: a B2B customer sees the same products, prices and quantity rules as everyone. No payment terms, no store credit. |
| Markets | One price list: every country sees the shop currency's prices. The selected country changes `localization.country`, not prices. |
| Taxes and shipping | Not computed. |
| Search | A simple search over titles, descriptions, vendors, types, tags and SKUs. Ranking is not Shopify's. |
| Recommendations | Products of the same collections, or the product's explicit `recommendations`. |
| Collection filters | Availability, price, vendor, product type, tag and variant options, with the swatches of option values. Metafield filters are not generated. |
| Placeholder artwork | `image`, `collection-2` and `lifestyle-2` are Shopify's illustrations: the three that are published under a free license. For the other names, `placeholder_svg_tag` draws generic artwork of the right proportions. |
| Fonts | Shopify's font library is not bundled. `font_face` and `font_url` generate the same markup and URLs; the files are blank unless provided in `files/fonts/`. |
| Shopify's shared assets | Requests under `/cdn/shopifycloud/` (payment buttons, model viewer UI, ...) get an empty file of the right type. |
| Theme editor | `request.design_mode` is always `false`, `Shopify.designMode` is not set, and `block.shopify_attributes` is empty. |
| Integers | 64-bit. Liquid on Shopify uses arbitrary-precision integers. |
| Byte strings | Strings are text (UTF-8). `base64_decode` of bytes that are not text replaces them with `�`, where Shopify carries the bytes on to the next filter. |
| Sitemaps | `/robots.txt` is rendered from `templates/robots.txt.liquid`, or with Shopify's default rules. `/sitemap.xml` links one sitemap per kind (products, pages, collections, blogs), in the primary language only and without paging. |
| Compression | Pages, styles, scripts and JSON are compressed with Brotli or gzip for the clients that accept it, at fast levels: the sizes are close to a storefront's, not equal. `--no-compression` turns it off. |

Found a difference that is not in this table? It is a bug: a Liquid snippet and the HTML
Shopify renders for it are enough to reproduce it.

### Differences with liquid-spec

The specs of liquid-spec that apply to the engine and do not pass, at the revision recorded in
`tools/liquid-spec/REVISION`:

| Specs | Reason |
|---|---|
| 15 | Integers beyond 64 bits: literals such as `18446744073709551615`, and the way Shopify's renderer reads such numbers from strings that end with a NUL character. |
| 3 | A date written with a UTC offset (`'2020-06-15 14:30:00 -0400' \| date: '%z'`) is shown in the shop's time zone, like every date of a storefront. The reference gem keeps the offset it was written with. |
| 2 | How the host stores files: a recording expects `{% include ".liquid" %}` not to find a file named `""`, another expects a renderer without a file system. |
| 1 | The spec expects `{{ #{1+1} }}` to print nothing. The gem (5.14) reports a variable that is not terminated, and so does the engine. |
| 1 | The recording injects a failure into Shopify's renderer and expects `Liquid error: internal`. |

The suite also has 26 sections of the Dawn theme with the HTML Shopify rendered. They are not
replayed: the specs hold neither the snippets, the translations nor the section settings the
HTML was rendered with. Those are in Dawn 5.0.0, so replaying them takes that theme and store
data written from the specs' environments.

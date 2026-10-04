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

## Caching

Responses say how long they can be kept with the `Cache-Control` Shopify sends for the same
kind of response. The values were read from Shopify's own demo stores of Dawn and Horizon in
October 2026.

| Response | `Cache-Control` |
|---|---|
| Pages, sections, product, search and recommendation JSON, `robots.txt`, sitemaps | `private, max-age=0, must-revalidate` |
| What is not found, redirects | `private, no-store` |
| `/cart.js` | none |
| What changes the cart (`/cart/add.js`, `/cart/change.js`, ...) | `no-cache, no-store` |
| Theme assets, the compiled scripts, images, files | `public, max-age=31557600` |
| The compiled stylesheet, fonts | `public, max-age=31536000, immutable` |
| Shopify's feature loader, payment icons | `public, max-age=31536000` |
| `standard-actions.js` | `public, max-age=600, must-revalidate` |
| A theme asset that does not exist | `public, max-age=60` |

What the storefront answers itself also says `Vary: Accept`, and images too. A file that is
kept for a year has a URL that changes with its content: the `?v=` of a theme asset, which
for a `.liquid` asset follows the theme settings too, and the hash in the name of the
feature loader.

## Minification

Shopify minifies the stylesheets and the scripts of a theme before it serves them, and so
does `lsf`. The rules are the ones read from Shopify's answers in October 2026, where the 97
stylesheets and scripts of Dawn come out of esbuild to the byte.

- Whitespace and syntax are minified. Names are kept: a function or a variable is called the
  same in the minified file.
- Stylesheets are also rewritten for older browsers, of the level of Safari 14: nesting is
  flattened, the range syntax of media queries becomes `min-width` and `max-width`, recent
  color functions get a fallback, and vendor prefixes are added.
- This applies to the assets, `.liquid` ones included once they are rendered, and to the
  bundles built from the `{% stylesheet %}` and `{% javascript %}` tags.
- A minified file ends with a link to its source map, which is served at the path of the
  file followed by `.map`: the tools of a browser show the file as it is written.
- A file is served as written when its name ends with `.min.js` or `.min.css`, when the
  minified file would not be lighter, or when it cannot be read as CSS or JavaScript. In a
  stylesheet, a rule that cannot be read is kept as written and the rest is minified.

`--no-minify` serves every file as it is written.

## Images

The images of the store (`/cdn/shop/files/...`) and the ones among the theme's assets go
through a local stand-in for Shopify's image CDN. It resizes, crops and pads as the URL asks,
and chooses the format the way Shopify's CDN was seen to in October 2026:

- A client that names `image/webp` in its `Accept` header can get WebP, and one that names
  `image/avif` as well can get AVIF. The weights (`;q=`) do not count. Browsers ask for
  images that way; `fetch()`, `curl` and most HTTP clients do not, and get the format of the
  file.
- The image is encoded in each format the client reads, and the lightest one is sent. This
  is why a browser gets AVIF for one image and WebP for the next. The response says
  `Vary: Accept`.
- An image of which nothing is asked (no size, no crop) is the file itself, unless a lighter
  format can be sent.
- `format: 'pjpg'` gives a JPEG to every client. `format: 'jpg'` and `format: 'png'` name the
  format for the clients that read neither WebP nor AVIF. An image with transparency does
  not become a JPEG.
- `quality` is the quality of every format. Without it, the qualities are the ones with
  which the files weigh about what Shopify's do: 85 for JPEG and AVIF, 90 for WebP.
- A WebP file is a JPEG for the clients that do not read WebP, or a PNG when it has
  transparency.

Encoding an image takes a moment the first time it is asked for in a format: about a tenth
of a second for a product image as AVIF. It is then kept in memory until the file changes.

## Preloading

`stylesheet_tag: preload: true`, `image_tag: preload: true` and `preload_tag` ask the
browser to load a file before it finds it in the page. As on Shopify, the tag itself does
not change: the page is answered with one `Link` header, written as Shopify writes it.

```
Link: <//shop/cdn/shop/t/1/assets/base.css?v=1>; as="style"; rel="preload", <//shop/cdn/shop/files/hero.jpg?v=1&width=1600>; as="image"; rel="preload"; imagesrcset="//shop/cdn/shop/files/hero.jpg?v=1&width=800 800w, ..."; imagesizes="100vw"
```

An image names the `srcset` and the `sizes` of its tag, so that the browser preloads the
size it is going to show. `preload_tag` repeats the attributes of its tag.

A font is asked for without credentials, whether the theme says so or not: a browser does
not use a preloaded font otherwise. `preload_tag: as: 'font'` writes `crossorigin="anonymous"`
in its tag and `crossorigin` in the header, as Shopify was seen to on stores that run
Horizon 4.

Shopify also reads the `<head>` it rendered and adds hints of its own, and so does `lsf`:
the stylesheets and the scripts that block rendering are preloaded, and the other origins
they come from are connected to. A stylesheet for print, a script with `async`, `defer` or
`type="module"`, and what is inside `<noscript>` do not block rendering.

The header names, in this order, as on the stores it was read from in October 2026:

1. the other origins to connect to;
2. what blocks rendering in the `<head>`, in the order of the document;
3. the stylesheets the theme asks to preload;
4. the stylesheet built from the `{% stylesheet %}` tags;
5. the fonts, the images and the other files the theme asks to preload.

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
| Caching | Shopify keeps rendered pages in a cache of its own. An answer from that cache has an `ETag` and no `Cache-Control`; `lsf` always answers like a page that was just rendered. Files have no `Last-Modified`, so nothing is answered with `304 Not Modified`. |
| Image formats | The format of an image is chosen by Shopify's rule, with other encoders: the files weigh about what Shopify's do, not the same, so an image can come as AVIF where Shopify sends WebP, or the reverse. A GIF is never converted. The color profile and the metadata of a file are not kept when it is encoded. |
| Preloading | Shopify sends the `Link` header ahead of the page, as `103 Early Hints`, and with the pages it answers from its own cache. `lsf` sends it with every page. Shopify's header also asks to connect to its CDN (`cdn.shopify.com`), which serves nothing locally, and has a limit on its number of entries, which `lsf` does not have. |
| Minification | Shopify minifies with esbuild, `lsf` with other libraries (oxc for scripts, Lightning CSS for stylesheets): a file weighs about what it does on Shopify, without being the same to the byte. In a stylesheet, declarations can come in another order within a rule. `inset` is not replaced with `top`, `right`, `bottom` and `left`. |
| Compiled scripts | Shopify runs the `{% javascript %}` of a section only on the pages that have that section. `lsf` runs them all on every page. |

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

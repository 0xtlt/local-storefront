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
- **Shopify's test suite of the language**,
  [liquid-spec](https://github.com/Shopify/liquid-spec), is replayed against the engine
  (`mise run liquid-spec:fetch` downloads it). 4,630 of its 7,369 language specs apply here, and
  4,608 of them pass. The others expect a strict parser to reject a template that Shopify
  accepts in a theme (1,923), or need what the engine has no equivalent for: Ruby objects, byte
  strings, render limits (816). The 22 that differ are [listed
  below](#differences-with-liquid-spec). What the strict parser rejects is checked by
  `npx local-storefront check`: see [Shopify's strict Liquid
  parser](#shopifys-strict-liquid-parser).
- **Shopify's objects, filters and tags** are checked against the examples of
  [shopify.dev](https://shopify.dev/docs/api/liquid): each example's code and data are replayed
  and compared with the documented output. 196 of 313 examples match. The others depend on the
  content of Shopify's own demo store (products, images and ids that are not in the examples'
  data), not on behaviour known to differ.
- **Real themes.** Shopify's Horizon and Dawn themes render every page type without a Liquid
  error, and their JavaScript (variant pickers, cart drawer, predictive search, filters, section
  rendering, pickup availability) runs against the local endpoints unchanged.
- **Functions and checkout extensions.** A JavaScript Function is built in the steps of Shopify's
  CLI (Javy writes the same bytes for the same JavaScript), and its module costs the instructions
  the CLI's would, within a few hundred: `crates/app/src/checkout/javascript/tests.rs` compares them
  with those of Shopify's CLI 5.0.0. A Rust module is linked by Shopify's own trampoline, to the
  byte. A Function's input is built from its input query, and its
  output checked, against the schema of its API version. The checkout UI extensions lsf bundles
  are drawn next to the CLI's bundles of the same sources, acted on and compared
  (`crates/app/assets/tests/built-extensions.test.mjs`). What the local checkout leaves out
  or decides on its own: [what to test on a real store](checkout.md#what-to-test-on-a-real-store).

`npx local-storefront check` reports what a theme uses that is not implemented: Liquid that does
not parse, and filters that do not exist here.

## Liquid tags

`assign`, `break`, `capture`, `case`, `comment`, `content_for`, `continue`, `cycle`,
`decrement`, `doc`, `echo`, `for`, `form`, `if`, `ifchanged`, `include`, `increment`, `#`,
`javascript`, `layout`, `liquid`, `paginate`, `raw`, `render`, `schema`, `section`,
`sections`, `style`, `stylesheet`, `tablerow`, `unless`.

With `--preview liquid-july-26`: `block` and `partial`. Without it they are unknown tags, as
on a live store.

### The render budget

A render that does not end (nested `{% for i in (1..1000000000) %}`, in a file of the theme or
one the Admin API wrote) would hold a thread of the server, and its page, for ever. A render
stops once it spends its budget, as Ruby Liquid's resource limits stop it:

- a **render score** of 10,000,000: each body rendered counts its nodes (Ruby Liquid's
  `increment_render_score`), and a loop over a range counts its items before it takes them;
- **10 seconds** of wall time.

The page then answers `500` with Liquid's error, `Liquid error: Memory limits exceeded`, and what
lsf counted; what was rendered is not shown, and nothing more renders. The pages of lsf's
fixture themes score 11 to 310 (a collection page the most): the limit is tens of thousands
of times what they need. Counting costs what it does not show: release builds before and after
it, the fixture theme and the demo store, 1000 renders a page interleaved by batches of 25,
medians of `Server-Timing: render` (2026-11-01, alternated twice): `/` 173 → 176 and
174 → 175 µs, a collection 291 → 295 and 291 → 292 µs, a product 232 → 231 and 230 → 231 µs, a
search 139 → 141 and 139 → 139 µs.

Shopify documents neither number. What it documents: "You can do a maximum of 50 iterations
with a `for` loop" over a paginated array (`collection.products` and the others of `paginate`),
pages of 1 to 250 items, and no further than the 25,000th item. Its renderer stops a page that
takes too long, without publishing the time. lsf's limits are set far above what a real page
needs, so that only a render that would never end is stopped.

## Liquid filters

Every one of the 154 filters of Shopify's reference: array, string, math, date, money, color,
font, media, HTML, URL, cart, customer, metafield and translation filters.
`applicable_disclosures` keeps every disclosure, as Shopify does when it cannot tell the
buyer's country. Among them
`image_url` and `image_tag` (with `srcset`, `sizes`, `preload`, focal points), the legacy
`img_url` family, `money` and its variants with every `money_format` placeholder (in a market
with another currency, Shopify's format of that currency, which `shop.money_format` and
`shop.money_with_currency_format` give too), `t` with
pluralization and interpolation, `date` with locale formats, `json`, `structured_data`,
`payment_type_svg_tag`, `placeholder_svg_tag`, `font_face`, `color_*`, `metafield_tag`,
`default_pagination`, `link_to_*`, `highlight`, `time_tag`.
`handleize` (and its alias `handle`) gives what a store gives: Latin accents folded, a straight
apostrophe removed (`Women's Tops` → `womens-tops`), a curly one a hyphen, and underscores, the
letters of other scripts, emoji and ligatures kept (`A_B` → `a_b`, `日本語` → `日本語`). The
handles lsf derives from titles and file names follow the same rules.

## Objects

Global objects: `shop`, `cart`, `customer`, `request`, `routes`, `localization`, `template`,
`theme`, `settings`, `section`, `block`, `closest`, `linklists`, `collections`, `all_products`,
`pages`, `blogs`, `articles`, `images`, `metaobjects`, `metaobject` (on the pages of a
metaobject type), `product`, `collection`, `page`, `blog`, `article`, `policy`, `order`,
`gift_card`, `search`, `predictive_search`, `recommendations`, `paginate`, `form`, `forloop`, `tablerowloop`, `canonical_url`, `page_title`,
`page_description`, `page_image`, `handle`, `current_page`, `current_tags`, `shop_locale`,
`content_for_header`, `content_for_layout`, `country_option_tags`, `all_country_option_tags`,
`powered_by_link`, `additional_checkout_buttons`, `robots`, `self`.

A handle that names nothing (`collections['x']`, `all_products['x']`, `pages['x']`, `blogs['x']`,
`linklists['x']`, `linklists['']`) and a metafield namespace a resource does not have
(`product.metafields.x`) give Shopify's [`empty` object](https://shopify.dev/docs/api/liquid/basics#empty):
truthy, `== empty` and `== blank`, printing nothing, with empty `products`, `articles` and
`links` (their `size` is 0) and nil for its other properties; `json` gives
`{"error":"json not allowed for this object"}`. `articles['blog/x']` is still nil.

With their related types: variants (`product.variants` gives the first 250, as on Shopify, and
`product.variants_count` all of them; so does the product JSON of the Ajax API), options and
option values (with swatches), media (images,
videos, external videos, 3D models), image presentation and focal points, collection filters
(with swatches) and sort options, links, addresses, orders and line items (a cart line has the
`index` shopify.dev leaves out: its 0-based position, which Horizon posts plus 1 as the `line` of
`/cart/change.js`; `/cart.js` has no such field), comments, metafields of every type (a
value written as the Admin API returns it is read as its type (`"true"` is `true`), the
deprecated `string`, `integer` and `json_string` give their value directly, a
`date` or a `date_time` is the string it is stored as, a
`color` is a color, a measurement has `type`, a decimal `value` and the `unit` Liquid gives
(`kg`, `cm`, `mL`) and prints both (`1.5 cm`), a `rating` prints its rating, a `money` prints as cents,
has the `currency` it is written in, and the money filters and `metafield_text` format it in
that currency; in a market with another currency, a `money` in the shop's currency is
converted at the market's rate, as prices are, a reference is a GID or the key the data gives the entity
([reference values](data-format.md#reference-values)), a `file_reference` is an image, a video or a file by its extension;
a metafield of a text, number, `boolean`, `date` or `date_time` type used without `.value` acts
as its value in `if`, `==`, `>`, the math and date filters, `size`, and `sort`, `where` and
`sum`, as on Shopify; `metafield_tag` and `metafield_text` print what Shopify prints, nothing
for a list other than `list.single_line_text_field` and `list.metaobject_reference`, and a
Liquid error for `metafield_tag` of a metaobject without `field:`, and `metafield_text`
HTML-escapes text, as `metafield_tag` does; `json` refuses a metafield, as on Shopify
(`.value | json` gives its value), and gives a metaobject entry and a namespace as the values
of their fields by key, a `number_decimal` as a string;
the value of a `list.product_reference` or `list.metaobject_reference` is a list drop, as on
Shopify: `.count` and `.size` give its length, `.first`, `.last` and `[n]` give nothing, and the
`first` and `last` filters, `for` and `paginate` reach its items; the other lists are arrays;
as on Shopify, a loop over it or over `metaobjects.<type>.values` outside `paginate` gets its
first 50 items, and `paginate` gives up to 250 a page, with a page parameter of the list's own,
`page_` and 8 hexadecimal digits (`paginate.page_param`), not `page`),
metaobjects, countries, languages (with `direction`, from the script of the language),
currencies, markets (`localization.market` and `country.market` are the market of the country,
with its handle, its id and its metafields), fonts, colors, color schemes, quantity rules and quantity price breaks,
unit prices, selling plan groups, selling plans and their allocations, store availabilities and
locations, companies, company locations and company addresses.

## Theme architecture

- JSON templates and Liquid templates, alternate templates (`?view=`, `template_suffix`),
  layouts (`{% layout %}`, `"layout"` in JSON templates, `layout none`).
- Theme JSON read as Shopify reads it: JSON templates, section groups, locales,
  `config/settings_schema.json`, `config/settings_data.json` and `{% schema %}` may have
  comments, trailing commas and unknown escapes (`\“` is `“`). A file that starts with a UTF-8
  byte order mark is refused, as Shopify refuses it on upload: a render goes on without it and
  says so in a warning.
- Sections, static sections (with the settings stored in `settings_data.json` or the `default`
  of their schema), section groups (`{% sections %}`), section and block settings with
  defaults from the schema, `disabled` sections and blocks.
- Theme blocks (`blocks/*.liquid`), nested blocks, static blocks, `{% content_for 'blocks' %}`
  and `{% content_for 'block' %}`, `closest.<resource>`.
- `closest.product`, `closest.collection`, `closest.article`, `closest.blog` and `closest.page`
  are, as on Shopify, the setting of that type of the block being rendered, else of its parent
  blocks, else of the section, else the resource of the template. The nearest setting counts
  even when it is empty: a block with an unset `product` setting has no `closest.product`. A
  `closest.product:` parameter of `{% content_for %}` stands for the setting of the block that
  passes it, so the rendered block's own setting comes first. `closest.metaobject` is not
  supported.
- The id of a theme block (`block.id`, and its wrapper's `id="shopify-block-<id>"`) is
  `<prefix>__<key>`, the key being its key in the JSON template or the `id` of a static block.
  A block rendered again on the same page is numbered as on Shopify: the first render is
  `<prefix>__<key>`, the second `<prefix>__<key>-1`, the third `<prefix>__<key>-2`. This
  happens with a static block in a `for` loop (Horizon's product card), the blocks it holds,
  or a second `{% content_for 'blocks' %}`. `section.blocks` gives the ids without a number.
  A section rendered alone (`?section_id=`, `?sections=`) starts again from the first, and a
  section asked for twice in `?sections=` is rendered once. The prefix is not Shopify's: do
  not rely on its value. Shopify gives each block a prefix of its own, local-storefront one
  per section, so two blocks that give a static block the same id (two collection links with
  their `title`) give it the same id, and neither is numbered. A block defined in a section's
  schema keeps its key as its id, and app blocks and blocks called with `{% block %}` are not
  numbered.
- With `--preview liquid-july-26`, blocks called from a template or a layout, as in Shopify's
  Skeleton theme: `{% block 'name', parameter: value %}content{% endblock %}`. The parameters
  are read when the tag opens. The block sees them and its body, as `content` (the Liquid
  reference) or `block.content` (the developer preview's guide), not the caller's variables;
  a parameter named after a setting of its schema also sets `block.settings`, the body does
  not. Arrays can be written in place and nested (`items: ['a', ['b', c]]`), not followed by a
  lookup (`['a'].first` is a syntax error). The output is not wrapped in a `shopify-block`
  element. A call from a section, a snippet, a block or the Liquid of a setting is an error,
  as the reference says, and `npx local-storefront check` reports it (in `{% liquid %}` too).
- With `--preview liquid-july-26`, partials, `{% partial 'name' %}...{% endpartial %}`, and
  `@shopify/partial-rendering` (`fetch`, `apply`, `refresh`, `get`, `getAll`). Shopify has not
  published the markup and the requests of its own runtime yet, so local-storefront uses its
  own: the region sits between `<!--shopify-partial name="..."-->` and
  `<!--/shopify-partial-->`, and the runtime fetches the whole page and takes the regions out of
  it. A theme that only uses the documented JavaScript API works the same; one that reads the
  markup does not. A partial rendered several times (in a loop) is updated everywhere, the n-th
  of the response replacing the n-th of the page (counted over the whole page, also when
  `refresh(element)` updates only those inside an element); regions the response has fewer of
  are emptied; one around table rows works too. `apply()` keeps focus, selection, the values of
  form controls (hidden ones included, as Shopify documents: a theme updates what the server
  changed) and scroll; scripts inside a region do not run again.
- Both tags are a developer preview on Shopify ("Liquid July '26"), on development stores that
  turn it on, and may still change.
- The URLs Liquid builds from the request (`paginate.previous.url`, `paginate.next.url`,
  `paginate.parts[].url`, `default_pagination`, a filter's and a filter value's `url_to_add` and
  `url_to_remove`) lead to the page, also in a section rendered alone: `section_id` and
  `sections` are left out, and the parameters are written as Shopify writes them, `page` first,
  the others sorted by name (`/collections/all?page=2&sort_by=price-ascending`). A list of
  references and `metaobjects.<type>.values` are paged with a parameter of their own
  (`/products/widget?page_1a2b3c4d=2`), as on Shopify; its digits are lsf's own, a hash of
  where the list is, not those of a store: build page links from `paginate.page_param`.
- Settings resolved to the objects Liquid expects: `image_picker`, `video`, `video_url`,
  `product`, `collection`, `page`, `blog`, `article`, `metaobject`, `product_list`,
  `collection_list`, `metaobject_list`, `link_list`, `color`, `color_palette`, `color_scheme`, `color_scheme_group`, `font_picker`,
  `url`, `liquid`, `checkbox`, `number` and `range`; the other types are passed as written.
  A `metaobject` setting holds the entry's handle, `<type>.<handle>`, `<type>/<handle>` or
  `gid://shopify/Metaobject/<id>`. A `product`, `collection`, `page`, `blog`, `article` or
  `link_list` setting whose handle names nothing is truthy, `== blank`, `== empty` and `== nil`,
  and prints its handle, as on Shopify. Dynamic sources are evaluated (`{{ product.title }}`,
  `{{ closest.product.metafields.custom.x }}`). A `product_list` setting is a list drop, as
  `metaobjects.<type>.values` is: `.count` gives its length (0 when nothing is selected), and
  `.first`, `.last` and `[n]` give nothing. Fed by a `list.product_reference` metafield
  (`"{{ product.metafields.custom.related.value }}"`), it holds the metafield's products in
  their order.
  A `color_scheme` setting, of a section, a block or the theme, gives the scheme's colors under
  `settings` (`section.settings.scheme.settings.background`), and `settings.color_schemes` gives
  its schemes in a `for` loop and one of them by id (`settings.color_schemes['scheme-1']`).
- Locales: storefront translations and schema translations, pluralization rules by language,
  the fallback to the default locale, Shopify's built-in translations. The translated content
  of metaobjects, from the store data, on the routes of their language
  ([data format](data-format.md#translations-of-metaobjects)); not yet that of products,
  collections, pages or articles.
- Assets: `asset_url` and friends, `.liquid` assets, the `{% stylesheet %}` and
  `{% javascript %}` bundles, `{% style %}`, preload headers.
- With `--app`, theme app extensions: app blocks (`{% render block %}`, `{% content_for 'blocks'
  %}`, the `apps` section), app embed blocks (`head`, `compliance_head`, `body`), and in their
  Liquid the extension's snippets, assets and translations, `app.metafields`, `javascript`,
  `stylesheet`, `class`, `tag`, `enabled_on`, `disabled_on` and `available_if`
  ([storefront](storefront.md#theme-app-extensions)).

## What Shopify injects into pages

A storefront page is more than the theme's markup: Shopify adds scripts through
`{{ content_for_header }}`, before `</head>` and before `</body>`. local-storefront adds the
same ones, modelled on what a Shopify storefront serves, with every URL pointing at the local
server:

| What | Where | Local behaviour |
|---|---|---|
| `Shopify.shop`, `locale`, `currency`, `country`, `theme`, `cdnHost`, `routes.root`, `modules` | `content_for_header` | Same values and shape as on Shopify. `Shopify.designMode` is not set, as on a live storefront. |
| `shopify-features` JSON, `__st`, `shopify-digital-wallet` meta, `hreflang` alternates | `content_for_header` | Same shape. The Storefront API token is a placeholder. |
| `Shopify.loadFeatures`, `Shopify.autoloadFeatures` | `content_for_header` | `consent-tracking-api` installs a local `Shopify.customerPrivacy`. Features hosted by Shopify (`model-viewer-ui`, `shopify-xr`, ...) report an error to `onLoad`, which themes handle. |
| `Shopify.customerPrivacy` | after `loadFeatures` | Consent is kept in the browser. It has to be collected in the EEA, the United Kingdom and Switzerland (by `Shopify.country`): there, nothing is allowed and `shouldShowBanner()` is true until `setTrackingConsent` is called. A real store decides this in its privacy settings. |
| Import map for `@shopify/partial-rendering` | first in `content_for_header` | Only with `--preview liquid-july-26`, when a Liquid file of the theme has a `{% partial %}`. The module is served by local-storefront, see [Theme architecture](#theme-architecture). |
| `Shopify.PaymentButton` | `content_for_header` | Present; `init()` does nothing, since dynamic checkout buttons are drawn by Shopify. |
| The app's script tags (`asyncLoad`) | `content_for_header` | With the Admin API: each script tag of `scriptTagCreate` shown on the online store, loaded once the page has loaded, `shop=<domain>` added to its URL, as on Shopify. |
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

Shopify minifies the stylesheets and the scripts of a theme before it serves them, and so does
local-storefront. The rules are the ones read from Shopify's answers in October 2026, where the
97 stylesheets and scripts of Dawn come out of esbuild to the byte.

- Whitespace and syntax are minified. Names are kept: a function or a variable is called the
  same in the minified file.
- Stylesheets are also rewritten for older browsers, of the level of Safari 14: nesting is
  flattened, the range syntax of media queries becomes `min-width` and `max-width`, recent
  color functions get a fallback, and vendor prefixes are added.
- This applies to the assets, `.liquid` ones included once they are rendered, and to the
  bundles built from the `{% stylesheet %}` and `{% javascript %}` tags.
- A declaration is merged into another only when the result sets the same properties: four
  sides into `margin`, for instance. `translate`, `rotate` and `scale` stay apart from
  `transform`, since they are applied before it and cascade on their own, and `font` and
  `transition` stay where they are written, before or after `font-variant-numeric` or
  `transition-behavior`, which they reset.
- A minified file ends with a link to its source map, which is served at the path of the
  file followed by `.map`: the tools of a browser show the file as it is written. The link
  carries the `?v=` the file was asked at (`base.css?v=123` links `base.css.map?v=123`), as
  Shopify's does, so that the map's URL changes with the file.
- A file is served as written when its name ends with `.min.js` or `.min.css`, when the
  minified file, link included, would not be lighter, or when it cannot be read as CSS or
  JavaScript. There is no size limit below which a file is left alone: a file of a few hundred
  bytes is minified when that saves bytes. In a stylesheet, a rule that cannot be read is kept
  as written and the rest is minified.

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
browser to load a file before it finds it in the page. As on Shopify, the page is answered
with one `Link` header, written as Shopify writes it, and the tag itself does not change, except
that `image_tag` writes `loading="eager"`, whatever its `loading` argument.

```
Link: <//shop/cdn/shop/t/1/assets/base.css?v=1>; as="style"; rel="preload", <//shop/cdn/shop/files/hero.jpg?v=1&width=1600>; as="image"; rel="preload"; imagesrcset="//shop/cdn/shop/files/hero.jpg?v=1&width=800 800w, ..."; imagesizes="100vw"
```

An image names the `srcset` and the `sizes` of its tag, so that the browser preloads the
size it is going to show. `preload_tag` repeats the attributes of its tag.

A font is asked for without credentials, whether the theme says so or not: a browser does
not use a preloaded font otherwise. `preload_tag: as: 'font'` writes `crossorigin="anonymous"`
in its tag and `crossorigin` in the header, as Shopify was seen to on stores that run
Horizon 4.

Shopify also reads the `<head>` it rendered and adds hints of its own, and so does
local-storefront: the stylesheets and the scripts that block rendering are preloaded, and the
other origins they come from are connected to. A stylesheet for print, a script with `async`,
`defer` or `type="module"`, and what is inside `<noscript>` do not block rendering.

The header names, in this order, as on the stores it was read from in October 2026:

1. the other origins to connect to;
2. what blocks rendering in the `<head>`, in the order of the document;
3. the stylesheets the theme asks to preload;
4. the stylesheet built from the `{% stylesheet %}` tags;
5. the fonts, the images and the other files the theme asks to preload.

<a id="formatting-lsf-fmt"></a>

## Formatting

`npx local-storefront fmt` is a port of Prettier with Shopify's Liquid plugin, and the goal is
the output they give, byte for byte. JavaScript is formatted by [Oxc](https://oxc.rs)'s
formatter, itself a port of Prettier's. The reference is Prettier 3.9.10 with
`@shopify/prettier-plugin-liquid` 1.11.2 (`tools/prettier/package.json`;
`mise run prettier:install` installs them).

How it is checked:

- **Liquid and HTML.** The 371 Liquid files of Shopify's Dawn and Horizon themes are formatted
  by both, with what `<script>`, `<style>` and `{% schema %}` hold left alone
  (`embeddedLanguageFormatting: off`), under four sets of options: widths of 120, 80, 40 and
  20 columns, tabs, both kinds of quotes, `bracketSameLine`, `singleAttributePerLine`, the
  three values of `htmlWhitespaceSensitivity`. The 1,484 outputs are identical.
- **Everything at once.** The same 371 files, with CSS, JSON and JavaScript formatted too,
  under six sets of options that include those of JavaScript (`semi`, `trailingComma`,
  `arrowParens`, `quoteProps`, `objectWrap`, `bracketSpacing`, tabs, widths of 120, 80 and
  60): the 2,226 outputs are identical. Formatted again, they stay identical to what Prettier
  gives.
- **The tests of the plugin.** Its 444 cases are replayed
  (`mise run prettier:fixtures`): 443 give what Prettier gives. The other one is Markdown.
- **CSS.** The 68 style sheets of the two themes are formatted by both at 120, 80 and 40
  columns: identical. Of the 157 files Prettier tests its own CSS printer with, which are edge
  cases, 145 are identical through `{% stylesheet %}`.
- **66 cases** written for this project, one per feature and per option, are formatted by
  Prettier into `crates/fmt/tests/golden` and compared on every test run
  (`mise run prettier:golden` regenerates them).

To compare on your own theme, without writing to it:

```bash
mise run prettier:compare -- path/to/theme
```

What differs:

- **CSS inside JavaScript** (a template literal tagged `css`) is left as written. Prettier
  formats it.
- **`experimentalTernaries`** is not supported.
- **JSON with comments** is left as written. Prettier formats it.
- **CSS that is more than CSS.** The selectors of `@nest`, `@custom-selector` and `@extend`,
  the control rules of SCSS (`@if`, `@each`), the names CSS Modules export, a comment inside a
  selector or inside the block of a custom property, a `//` comment in a value: these are
  printed as written, or not as Prettier prints them.
- **Markdown** in `<script type="text/markdown">` is left as written.
- **Configuration written in JavaScript** (`prettier.config.js`, `.prettierrc.js`) is not
  read, nor are TOML and JSON5 files. Other Prettier plugins do not apply.
- `requirePragma`, `insertPragma` and formatting a range of a file are not supported.

## Shopify's strict Liquid parser

Shopify is moving every store from its lenient Liquid parser to a strict one.
`npx local-storefront check` reports the markup the strict parser rejects, and
`npx local-storefront check --fix` rewrites it into markup the strict parser accepts. Where what
the code meant is clear (a filter in a `render` argument, an `and` in an argument,
`('X' | downcase)`, `reversed` after `limit`, `=` in a condition, `and` after the value of a
`when`), the fix does it and says how the output changes. A `case` with such a `when` and an
`else` becomes `if`/`elsif`/`else`, so that the `else` runs when the condition is false; only
when its `when`s compare with distinct literals, a single `else` comes last and nothing but
whitespace precedes the first `when`, so that it renders the same otherwise. Elsewhere it
renders what the file renders today.

The rules are those of Shopify's reference implementation, the `liquid` gem 5.14, in
`error_mode: :strict2`: the `strict2_parse` of each tag and of outputs, ported tag by tag
(`crates/liquid/src/strict2`). Today, Shopify parses a tag with that parser first and falls
back on the lenient one when it fails; outputs and `{% when %}` go to the lenient parser
directly when they are not strictly valid. A fix that keeps the output is printed from what the
engine parses today, and is kept only when the strict parser accepts it and both parsers read
it the same as the original.

How it is checked:

- **liquid-spec runs most of its specs in `strict2` mode.** For the 6,314 that apply here,
  `npx local-storefront check` finds a rejection exactly when the spec expects one, with the
  gem's message, and every fix leaves nothing to reject (`crates/liquid/tests/strict2_spec.rs`).
  Two recordings of Shopify's production expect a template that the gem rejects in `strict2`
  mode (`{% if !name.first %}`): they are left out.
- **Fixes render the same with the gem.** 1,548 of those templates get a fix. The gem renders
  each original in `:warn` mode, the mode Shopify renders themes in today, and each fix in
  `:strict2` mode, with the data of the spec and again with data where every variable prints
  its own path. The outputs are identical (`mise run oracle:strict2-fixes`).
- **Real themes.** Horizon and Dawn have nothing the strict parser rejects.

What to know:

- **Shopify's own tags** (`section`, `sections`, `layout`, `form`, `paginate`, `content_for`,
  and `render` of an object, as in `{% render block %}`) are not in the gem. Shopify has not
  published how its strict parser reads them: local-storefront reads them with the rules of the
  standard tags, an expression without bare brackets, and a quoted name for `section`.
  `{% block %}` and `{% partial %}` (behind `--preview liquid-july-26`) are not checked.
- **A fix writes the markup it replaces in the strict parser's own form:** a fixed tag gets
  single spaces between its parts, `a . b` becomes `a.b`, and strings keep the quote the tag
  used. Arguments written one per line stay one per line, and the rest of the file is left as
  it is.
- **What the lenient parser passes to a snippet by mistake is left out when the snippet does
  not read it.** In `{% render 'image', alt: image.alt | default: shop.name %}`, it reads
  `default: shop.name` as an argument of `render`. Where the fix cannot apply the filter
  instead, it leaves `default` out when `snippets/image.liquid` never mentions it (nor `self`):
  a rendered snippet sees only its arguments.
- **`[name]` becomes `self[name]`.** The strict parser refuses a lookup that starts with a
  bracket, and `self` is the variable it suggests instead. This reads the same unless the
  theme assigns a variable named `self`.
- **Some markup has no fix:** what renders an error today (`{% if a = b %}`, where `=` is an
  unknown operator), or what the strict parser has no way to write (an assigned variable named
  `a.b`, a string that holds both kinds of quotes).
- **A fix that does what the code meant has no reference to be checked against**, since it
  changes what renders on purpose. Each is checked to be accepted by the strict parser, and the
  gem renders them as meant (a filtered `render` argument, an `and`, `('X' | downcase)`,
  `reversed`, `==`). A variable such a fix adds gets a name the file does not use, but a
  snippet included with `{% include %}` could still use that name.

## Known differences

These are deliberate or not done yet. None of them raises a Liquid error.

| Area | Difference |
|---|---|
| Checkout | Not part of a theme. `/checkout` shows a summary of the cart. With `--app <dir>`, it is a local checkout that runs the Functions and shows the checkout UI extensions of a Shopify app: [checkout](checkout.md#what-it-does-not-do) lists what it leaves out (payments, pickup points, discount conditions and combinations, tax lines, the Storefront API). It supports API versions 2026-10 and 2027-01. |
| Admin API | With `--app`, a local GraphQL Admin API (2026-10, and 2027-01 with `--admin-versions`), OAuth and webhooks, for the backend of the app: every mutation is simulated ([what that means](admin-reference.md#what-lsf-simulates)). The REST Admin API and the Storefront API are not served: `404`. Work Shopify does in the background (a job, a bulk operation) is done within the request. There are no exchange rates (a market's manual rate, else 1) and orders have no duties. Its state is in memory, never written to disk: a start seeds it again from the store data. Each shop of `POST /__lsf/admin/shops` holds about 2 MB, and each order created some tens of KB (about 65 KB in a release build), until a reset or the shop is deleted. |
| Apps | Without `--app`, app blocks and app embed blocks render nothing; the page says so in `X-LSF-Reason`. `content_for_header` contains the scripts a theme relies on (`Shopify.*` globals, routes, the compiled asset tags) and the script tags apps created through the Admin API (an uninstall deletes them, as on Shopify), not Shopify's analytics scripts nor web pixels. |
| Theme app extensions | With `--app`, the app's blocks render ([storefront](storefront.md#theme-app-extensions)); the blocks of other apps render nothing. Their assets are served at `/cdn/extensions/<uid>/assets/<file>?v=<hash>`, where Shopify serves `cdn.shopify.com/extensions/<uid>/<version>/assets/<file>`. The `javascript` and `stylesheet` of a schema are added before `</head>` (`<script async>`, `<link rel="stylesheet">`); a section rendered alone (`?sections=`) does not add them. `compliance_head` embeds come before the `head` ones, before `</head>`, not first in the head. `app.metafields` reads `app.metafields` of the store data: what the backend writes to the app installation through the Admin API is not shown there. An embed turned on by `app.embeds` has the id of its name (`shopify-block-chat-bubble`). A resource setting with `autofill` is not filled in, and the objects Shopify withholds from extension Liquid are not withheld. `app dev`, which serves no theme, renders none. |
| Customer accounts | New customer accounts are hosted by Shopify, outside the theme. `/account` is a page of local-storefront that stands in for them: it shows who is logged in and lets you become anyone in the data. With `--app`, it is the customer account of the app instead: orders, profile and the pages of its customer account UI extensions ([checkout](checkout.md#customer-account)). Every account URL (`/account/login`, `/account/register`, `/customer_authentication/login`, ...) leads to it. Classic customer accounts are not simulated: the theme's `templates/customers` are never rendered, and its account forms (login, register, password reset, addresses) lead to `/account`. |
| Password page | `/password` always shows the theme's password page, and accepts `shop.password` (`password` by default). The storefront is never locked: nothing redirects to that page. On Shopify, a protected store redirects every page to it, and an open store redirects it to the home page. |
| Discounts | Carts and orders have no discounts: discount arrays are empty and totals are undiscounted (the checkout of an app applies the discounts its Functions give, nowhere else). `Shopify.actions.updateCart` reports every discount code as not applicable. |
| Standard actions | A change the cart refuses (unknown variant) resolves with `userErrors` whose code is `INVALID`, and leaves that line untouched. More than the stock resolves with the same `userErrors`, the line holding what is in stock: the Storefront API would return a warning instead. |
| Selling plans | Selling plan groups, plans and allocations are there, on products, variants, the cart and `/products/<handle>.js`. A group applies to every variant of a product, not to some of them. Prepaid plans are not modelled: `per_delivery_price` is the price. The line items of orders have no selling plan. The widget of a subscription app is an app block: it renders with `--app` when it is that app's, else the theme's own purchase options do. |
| Store pickup | `variant.store_availabilities` and `/variants/<id>?section_id=` are there. Locations come in the order of the data: Shopify sorts them by distance from the visitor. Choosing pickup happens at checkout, which is not part of a theme, so `order.pickup_in_store?` is always `false`. |
| Swatches | A swatch is given to option values by name, or to one value of a product. On Shopify it comes from the category metafield the option is linked to. `filter_value.image` (the `image` presentation of a filter) is not generated. |
| B2B | Companies, company locations, their addresses and metafields are there: `customer.b2b?`, `current_company`, `current_location`, `company_available_locations` and the link that changes location. Catalogs are not: a B2B customer sees the same products, prices and quantity rules as everyone. No payment terms, no store credit. |
| Markets | A country pays in its currency (its own, its market's, else the shop's), at its market's `rate` (1 when there is none: lsf has no exchange rates) or at its fixed `prices`, on the storefront and at the checkout. Shopify's rounding rules of converted prices (`.99`, `.95`) and price adjustments in percent are not applied. A market is served at `/` or under subfolders of its own (`/en-ca`); a market with a domain of its own is not served. Orders the checkout records in the Admin API keep the amounts of the checkout, without converting them back to the shop's currency. |
| Taxes and shipping | Not computed. |
| Search | A simple search over titles, descriptions, vendors, types, tags and SKUs. Ranking is not Shopify's: `relevance` keeps the order of the data. `sort_by` takes `price-ascending` and `price-descending` (any other value is `relevance`); pages and articles come after the products. |
| Recommendations | Products of the same collections, or the product's explicit `recommendations`. |
| Collection filters | Availability, price, vendor, product type, tag and variant options, with the swatches of option values. Metafield filters are not generated. |
| Missing files | A file of `files/` that the data names (`images['hero.jpg']`, an `image_picker` setting) is a placeholder image of that name when the file is missing, so that test data needs no image files: `image_url` gives its URL. On Shopify a file the shop does not have is nothing, and `image_url` raises `invalid url input`. `image_url` also reads any relative path (`'hero.jpg'`) as a file of `files/`, where Shopify takes the `src` of an image. |
| Placeholder artwork | `image`, `collection-2` and `lifestyle-2` are Shopify's illustrations: the three that are published under a free license. For the other names, `placeholder_svg_tag` draws generic artwork of the right proportions. |
| Fonts | Shopify's font library is not bundled. `font_face` and `font_url` generate the same markup and URLs; the files are blank unless provided in `files/fonts/`. |
| Shopify's shared assets | Requests under `/cdn/shopifycloud/` (payment buttons, model viewer UI, ...) get an empty file of the right type. The files of `shopify_asset_url` are served from the binary: `vendor/qrcode.js` is the qrcode.js library (MIT), as on Shopify; `shopify_common.js` has our own `Shopify.formatMoney`, `Shopify.resizeImage` and `Shopify.CountryProvinceSelector` (the province selector of an address form, filled from `data-provinces`); `option_selection.js`, `api.jquery.js`, `customer_area.js` and `currencies.js` only log a warning to the console, and `customer.css` is empty, because Shopify's code is proprietary; `gift-card/card.svg` is a drawing of our own, of the size of Shopify's, and `gift-card/add-to-apple-wallet.svg` a placeholder drawing. Any other name is a 404, logged by the server. |
| Theme editor | `request.design_mode` is always `false`, `Shopify.designMode` is not set, and `block.shopify_attributes` is empty. |
| Measurement units | `.unit` of a `weight` or a `dimension` is its short unit (`kg`, `cm`), and of a `volume` in milliliters, centiliters or US gallons `mL`, `cL` or `gal`, as a store gives them. The other volume units, and the units of the other measurement types, are given as the data writes them: how a store writes them is not confirmed. |
| Integers | 64-bit. Liquid on Shopify uses arbitrary-precision integers. |
| Byte strings | Strings are text (UTF-8). `base64_decode` of bytes that are not text replaces them with `�`, where Shopify carries the bytes on to the next filter. |
| Sitemaps | `/robots.txt` is rendered from `templates/robots.txt.liquid`, or with Shopify's default rules. `/sitemap.xml` links one sitemap per kind (products, pages, collections, blogs), in the primary language only and without paging. |
| Compression | Pages, styles, scripts and JSON are compressed with Brotli or gzip for the clients that accept it, at fast levels: the sizes are close to a storefront's, not equal. `--no-compression` turns it off. |
| Caching | Shopify keeps rendered pages in a cache of its own. An answer from that cache has an `ETag` and no `Cache-Control`; local-storefront always answers like a page that was just rendered. Files have no `Last-Modified`, so nothing is answered with `304 Not Modified`. |
| Image formats | The format of an image is chosen by Shopify's rule, with other encoders: the files weigh about what Shopify's do, not the same, so an image can come as AVIF where Shopify sends WebP, or the reverse. A GIF is never converted. The color profile and the metadata of a file are not kept when it is encoded. |
| Preloading | Shopify sends the `Link` header ahead of the page, as `103 Early Hints`, and with the pages it answers from its own cache. local-storefront sends it with every page. Shopify's header also asks to connect to its CDN (`cdn.shopify.com`), which serves nothing locally, and has a limit on its number of entries, which local-storefront does not have. |
| Minification | Shopify minifies with esbuild, local-storefront with other libraries (oxc for scripts, Lightning CSS for stylesheets): a file weighs about what it does on Shopify, without being the same to the byte. In a stylesheet, declarations can come in another order within a rule. `inset` is not replaced with `top`, `right`, `bottom` and `left`, and the width, style and color of the four sides of a border, all set in one rule, become `border`, which also resets `border-image`. Shopify's link to the source map carries the `?v=` of the request, local-storefront's does not: a file that minifying saves only a few dozen bytes of can be minified locally and served as written on Shopify. |
| Compiled scripts | Shopify runs the `{% javascript %}` of a section only on the pages that have that section. local-storefront runs them all on every page. |

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

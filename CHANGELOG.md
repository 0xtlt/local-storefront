# Changelog

What changed in each version of `lsf`, for someone who uses it. The section of a version is
what its [GitHub release](https://github.com/0xtlt/local-storefront/releases) says.

## 0.1.14 - 2026-10-06

### Fixed

- `lsf serve` gives back the memory it used to encode images. A server left running for
  three hours on a large theme held 1 GB. After every image of that theme is asked for,
  277 MB are left where 664 MB were, and pages render as fast as before.

## 0.1.13 - 2026-10-05

### Added

- **`Server-Timing` on every response**, as on a Shopify storefront: `processing` for the
  whole, then `render`, `template`, `layout`, `compress` and `throttle`.
  `lsf serve --timings` also names every section and every theme block.
- **`lsf profile <path>`** renders one page and prints what rendered what: the template, the
  layout, each section, its blocks and their snippets, with the time of each. `--lines` goes
  down to the tag.
- **`lsf profile --points`** counts what the page asks of a store instead of the time it
  takes here: a product, a metafield, a search. A local server reads a metafield as fast as
  a title, Shopify does not. `--cost` changes what each kind of thing costs.
- **Flame graphs.** `/__lsf/profile?path=<path>&html=1` opens the profile in speedscope, the
  viewer that `shopify theme profile` opens. It is embedded: nothing is fetched.
- `/__lsf/timings` adds up the renders so far by template, section and block.

## 0.1.12 - 2026-10-04

The responses of `lsf serve` now weigh what a Shopify storefront sends, so that the sizes a
browser or an audit reports are those of the real store.

### Added

- **Minified stylesheets and scripts**, by the rules Shopify follows, with source maps: the
  tools of a browser still show the files as written. `--no-minify` serves them as they are.
- **Images in the lightest format the browser reads.** AVIF, WebP or the format of the file,
  chosen from the `Accept` header as Shopify's image CDN does.
- **`Cache-Control`** as Shopify sends it for each kind of response: pages, assets, images,
  fonts, the cart.
- **The resource hints Shopify adds.** The `Link` header preloads the stylesheets and the
  scripts that block rendering, and connects to the origins they come from.

### Fixed

- The `Link` header of `preload: true` and `preload_tag` is written as Shopify writes it,
  one header with quoted values. A browser could not read the previous form for an image
  with a `srcset`.
- A font preloaded with `preload_tag` gets `crossorigin`, without which a browser does not
  use it.
- The `?v=` of a `.liquid` asset changes with the theme settings it is rendered with, not
  only with its source.
- A placeholder image served from the cache says that it is one (`x-lsf-placeholder`).

## 0.1.11 - 2026-10-03

### Added

- **Responses are compressed** with Brotli or gzip for the clients that accept it, as on a
  storefront. A 420 KB collection page of Horizon becomes 53 KB. `--no-compression` sends
  responses as they are.

### Fixed

- `{{-` and `{%-` trim whitespace the way storefronts do, which is not quite what the
  language says: when the text before one is nothing but whitespace, its first character
  stays. Line breaks in a `<title>` or between two outputs now match Shopify's.
- The rules of `robots.txt` are on their own lines, those a theme adds with
  `{{ 'Disallow: ...' }}` included.

## 0.1.10 - 2026-10-03

### Changed

- **`--live-reload` goes through a WebSocket.** A page reloads about 100 ms after a save,
  where it asked the server every 700 ms. It still does when the socket cannot open, behind
  a proxy for example.
- A page survives a restart of the server, and reloads if the files changed meanwhile.
- A save or a build that writes several files reloads the pages once.
- Only the directories a theme is made of and the data directory are watched, which is much
  faster for a theme that keeps `node_modules` next to its sources. A data directory outside
  the theme (`--data`) now reloads the pages. A file outside those directories no longer
  does.

## 0.1.9 - 2026-10-03

### Added

- **Selling plans.** `selling_plan_groups` in the data describes subscriptions. Products and
  variants expose their groups and allocations, `?selling_plan=<id>` selects a plan, and
  `/cart/add.js` and `/cart/change.js` take `selling_plan`.
- **Store pickup.** `locations` in the data are the places that stock the products, and
  variants have `store_availabilities`. `/variants/<id>?section_id=` renders a section for a
  variant, which is how Dawn loads pickup availability.
- **Native swatches.** `swatches` in the data gives a color or an image to option values:
  `product_option_value.swatch` and `filter_value.swatch` are filled.
- The demo store has a subscription, two locations and swatches for its colors.

## 0.1.8 - 2026-10-02

### Changed

- Without `--port`, `lsf serve` listens on the next free port when 9292 is taken, up to
  9391, and says which. A port given with `--port` is still that port or an error.

## 0.1.7 - 2026-10-02

The Liquid engine is now checked against
[liquid-spec](https://github.com/Shopify/liquid-spec), Shopify's test suite of the language:
4,608 of the 4,630 specs that apply pass. The others are listed in
[compatibility](https://github.com/0xtlt/local-storefront/blob/main/docs/compatibility.md).

### Added

- `self`, the object that reads the variables of the current scope (`self[name]`).

### Fixed

- `truncate` crashed when given the smallest 64-bit integer.
- Comparing with a float that is not a number raised an error instead of being false.
- `replace`, `replace_first` and `remove` ignored the back-references of the replacement
  (`\'`, `` \` ``, `\+`, `\k<name>`).
- Floats from 1e15 on were printed in fixed notation, and some JSON numbers were read one
  bit off.
- `{% assign x ) = 1 %}` was accepted, and `{% docEXTRA %}` was taken for a nested `doc`.
- An error at the end of a `{% liquid %}` tag was reported one line too far.
- A contact or newsletter form given an `id` posts back to that id
  (`action="/contact#ContactForm"`), as on Shopify.

## 0.1.6 - 2026-10-02

### Added

- **Customer accounts.** `/account` shows who is logged in and lets the visitor become any
  customer of the data. A theme with `templates/customers` keeps its own pages. A customer
  is named by email, `default` or `none`: in the data, in a session, with `/__lsf/login` or
  with `lsf serve --customer`.
- **B2B.** Companies with their locations, the customers that buy for them, and the objects
  that go with them: `customer.b2b?`, `current_company`, `current_location`. Catalogs are
  not simulated.
- **`/robots.txt` and `/sitemap.xml`.** `robots.txt` renders `templates/robots.txt.liquid`,
  or Shopify's default rules.
- **`/password`** shows the password page of the theme and accepts `shop.password`.
- `placeholder_svg_tag` draws Shopify's own illustrations for the three names that are
  published under a free license.

### Changed

- A password in the data no longer locks the storefront: nothing redirects to `/password`.

## 0.1.5 - 2026-10-02

### Added

- **`lsf serve --throttle`** answers requests late, so that loading states can be tested:
  every request (`--throttle 300ms`) or one kind of request
  (`--throttle cart=500ms,cart-add=1s`).
- **Presets.** `simulated` answers as late as a Shopify storefront on a good connection,
  `slow` as on a slow mobile connection. A preset can be adjusted:
  `--throttle simulated,cart-add=2s`.
- A session can have a throttle of its own through `PUT /__lsf/session`: one test can be
  slow without slowing the suite.

### Changed

- The README is written around what a user does, and documents every option of `lsf serve`.

## 0.1.4 - 2026-10-02

### Added

- **The scripts Shopify adds to a page.** `content_for_header` follows Shopify's, with the
  `Shopify` object and `Shopify.loadFeatures`, and pages get `ShopifyAnalytics.meta` and
  `Shopify.analytics`.
- **`Shopify.actions`** (`getCart`, `updateCart`, `openCart`) and the `shopify:cart:*`
  events, written against the local cart.
- **`Shopify.customerPrivacy`**, loaded with `consent-tracking-api`: consent is kept in the
  browser.
- The `standard_event_data` filter, for products, collections and the cart.
- A page can render a section that belongs to another template, which a cart drawer does to
  load recommendations.

### Changed

- The `cart` cookie is set when a cart is created, `/cart.js` bundles sections, and
  `/cart/update.js` removes the attributes set to null.

## 0.1.3 - 2026-10-02

### Fixed

- `image_url` accepts decimal sizes, which themes compute with `width | times: ratio`. They
  raised "invalid integer", and the image was left out.

## 0.1.2 - 2026-10-02

### Added

- **`lsf` is on npm.** `npm install --save-dev local-storefront` gives a project the
  command, with the binary of its platform only. Run it with `npx local-storefront`:
  `npx lsf` can download an unrelated package of that name.

## 0.1.1 - 2026-10-02

### Changed

- **Rendering scales with the number of clients.** On a 14-core machine, the collection page
  of Horizon goes from 70 pages per second to 430 with one client, and from 107 to 3435
  with fourteen. The HTML is the same to the byte.
- With `--static`, an asset edited while the server runs keeps its `?v=`: nothing is read
  from disk twice.

## 0.1.0 - 2026-10-02

First release. `lsf` renders a Shopify theme without calling Shopify, so that end-to-end
tests run against a local server with no rate limit.

- **A port of Shopify's Liquid**, checked against the reference gem by 548 differential
  tests, with Shopify's objects, filters and tags, sections, blocks, JSON templates and
  locales.
- **Store data as JSON files**, with a JSON Schema and a validation that explains mistakes.
- **`lsf serve`**: storefront pages, the Cart Ajax API, the Section Rendering API, forms,
  and images transformed and served locally.
- **A control API** that gives each test session its own cart, customer and data.
- The other commands: `render`, `validate`, `check`, `init`, `routes`, `schema`, `docs`.
- One binary for macOS, Linux (glibc 2.17 or later, and static musl) and Windows, on x86-64
  and ARM64.

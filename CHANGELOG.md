# Changelog

What changed in each version of `lsf`, for someone who uses it. The section of a version is
what its [GitHub release](https://github.com/0xtlt/local-storefront/releases) says.

## 0.1.17 - 2026-10-20

### Added

- **The pages of metaobjects are served.** With `templates/metaobject/<type>.json` (or
  `.liquid`) in the theme, `/pages/<type>/<handle>` renders the entry, with the global
  `metaobject`, `?view=` and a per-entry `template_suffix` in the data. `shopify://metaobjects/`
  links lead there, and `metaobject.system.url` is nil for a type without a template, as on a
  store whose definition has no web pages.
- **A 404 page says why.** The server answers it with an `x-lsf-not-found` header (no route,
  unknown resource, no template) and logs the reason, so a test can fail on it.
- **`shopify_asset_url` files are served.** `vendor/qrcode.js` is the MIT qrcode.js library that
  Shopify serves, so a gift card draws its QR code. The other documented files are small local
  stand-ins: `shopify_common.js` formats money, the others only warn in the console.

### Fixed

- **`metaobject` and `metaobject_list` settings are metaobjects**, not the string of the
  template: a handle, `type.handle`, `type/handle` or a `gid://shopify/Metaobject/` id.
- **The search follows `sort_by`**: `relevance`, `price-ascending` and `price-descending`, with
  the other results after the products under a price order, as Shopify does. `search.sort_by`
  says which order applies and `search.sort_options` lists those three.
- **`'now' | date: '%N'` differs between calls.** Each `'now'` reads the clock again, as on
  Shopify, so ids built from it no longer repeat. With a fixed `now` in the data, each read is one
  nanosecond after the previous one: renders stay reproducible.

## 0.1.16 - 2026-10-14

### Added

- **`npx local-storefront check` finds what Shopify's strict Liquid parser rejects, and
  `npx local-storefront check --fix` rewrites it.** Shopify is moving every store to a strict
  parser, and a file it rejects can no longer be uploaded
  (`Expected end_of_string but found pipe`). `npx local-storefront check` reports each tag and
  output it rejects, with Shopify's message and the fix, in `.liquid` files and in the `liquid`
  settings of JSON templates, section groups and `config/settings_data.json`.
  - Where what the code meant is clear, `--fix` does it, and the report says how the output
    changes: a filter in a `render` argument is computed into a variable on the line above, an
    `and` in an argument is evaluated, `('X' | downcase)` is the string and not the variable
    `X`, `reversed` after `limit` reverses, `=` in a condition compares, and the body of
    `{% when 'x' and y %}` goes in `{% if y %}` (a `case` with an `else` becomes
    `if`/`elsif`/`else`).
  - Elsewhere it keeps today's output, as Shopify's own migration does, without the comment it
    leaves in the file.
  - A fix keeps the layout of the markup and is checked before it is written. Markup without a
    fix is reported with the reason.
  - `--format json` prints one JSON document, for tools and AI agents: each problem with its
    code, file, line, markup and fix.

  See [strict
  Liquid](https://github.com/0xtlt/local-storefront/blob/main/docs/strict-liquid.md).

### Changed

- **The documentation is organised by task.** The README is short: what local-storefront does,
  how to start, and a link per task. Each task has its guide in `docs/` (data, testing, strict
  Liquid, formatting, performance, storefront), next to two references: the
  [CLI](https://github.com/0xtlt/local-storefront/blob/main/docs/cli.md), with every option,
  exit code and output format, and the [control
  API](https://github.com/0xtlt/local-storefront/blob/main/docs/control-api.md).
  [docs/agents.md](https://github.com/0xtlt/local-storefront/blob/main/docs/agents.md) tells an
  AI agent how to drive local-storefront, and `llms.txt` lists the documentation for language
  models.
- Messages name the subcommand to run (`the init command`, `run again with --fix`) rather
  than `lsf`, which is only the command of the installs that are not from npm.

## 0.1.15 - 2026-10-14

### Added

- **`lsf fmt`** formats the Liquid files of a theme the way Prettier does with Shopify's
  Liquid plugin, without Node: Liquid, HTML, the JSON of `{% schema %}`, the CSS of
  `<style>`, `{% style %}` and `{% stylesheet %}`, and the JavaScript of `<script>` and
  `{% javascript %}`. `lsf fmt --check` lists the files that are not formatted and fails if
  there are any, for CI. The 283 files of Shopify's Horizon theme are checked in 0.08 s, where
  Prettier takes 1.6 s.
  - It reads the configuration Prettier reads: `.prettierrc`, `.editorconfig` and
    `.prettierignore`. `--option printWidth=100` sets an option from the command line, and
    `--stdin` (with `--stdin-filepath`) formats what an editor sends. Prettier's options for
    JavaScript (`semi`, `trailingComma`, `arrowParens`, ...) apply too.
  - Files named one by one, as a commit hook names them, are formatted when they are `.liquid`
    files that `.prettierignore` does not name. A file is replaced in one step, with its
    permissions. A file that cannot be formatted is reported, and the others are formatted
    all the same.

  See [what differs from Prettier](https://github.com/0xtlt/local-storefront/blob/main/docs/compatibility.md#formatting-lsf-fmt).
- **Feature previews.** `--preview <name>` (or `LSF_PREVIEW`) turns on a feature preview of
  Shopify, as a development store can. Without it, a page renders as on a live store: a theme
  that uses a preview gets the error a live store gives, and `lsf` names the preview to turn
  on. The first one is `liquid-july-26`, Shopify's "Liquid July '26 changes":
  - **`{% block 'name', parameter: value %}content{% endblock %}`** renders
    `blocks/name.liquid` from a template or a layout. The block sees its parameters and its
    body, as `content` or `block.content`; a parameter named after a setting of its schema
    sets `block.settings`; arrays can be written in place. With it, Shopify's Skeleton theme
    renders.
  - **`{% partial 'name' %}`** and **`@shopify/partial-rendering`**: a theme that uses
    partials gets an import map for the module, and `partials.refresh('name')` swaps in the
    region from the server, keeping focus, form values and scroll, in loops and table rows
    too. Shopify has not published its own markup yet: lsf marks a partial with HTML comments.

### Fixed

- A `file_reference` metafield or metaobject field that points to a video (`.mp4`, `.mov`,
  `.webm`, `.m4v`) is a video, as on Shopify: `sources`, `preview_image` and `video_tag`
  work, and `metafield_tag` prints a `<video>` as Shopify does. It was a plain file with only
  a `url`, so themes rendered it empty. The poster is the `.jpg` of the same name. A `.mov` is
  announced as `video/mp4`, as Shopify announces the MP4 it makes of it, so that Chrome and
  Firefox play it; the `video` setting does the same.
- Files of `files/` are served in parts when the browser asks with a `Range` header. Safari
  played no video without it, and no browser could seek in one. Only the part asked for is
  read, and a part is never compressed. `.m4v` files are served as `video/mp4`.
- Metafield values are the objects Shopify gives for four more types. A `color` is a color
  (`.red`, `.rgb`...), not text. An `article_reference` is the article, named `blog/article`
  or by the handle of an article that only one blog has. Every measurement type (`distance`,
  `temperature`, `duration`...) has a `type`, a `value` and a `unit`, and `metafield_tag`
  prints it with two decimals at most, where only `weight`, `volume` and `dimension` were
  printed. A `money` has the `currency` it is written in, and is formatted in it: `€5,00 EUR`
  in a store that sells in dollars.
- `request.locale.direction` (and that of every locale) is `rtl` for Arabic, Hebrew, Persian
  and the other languages written right to left, and follows the script when the locale names
  one (`pa-Arab`). Horizon sets `dir` on `<html>` from it.
- The `applicable_disclosures` filter keeps every disclosure, as Shopify does when it cannot
  tell the buyer's country. It was an unknown filter that printed a warning.

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

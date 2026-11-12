# Storefront

What you see when you open the local storefront in a browser, and the options that change it.

```bash
npx local-storefront serve --live-reload
```

Open <http://127.0.0.1:9292>. If that port is taken, local-storefront uses the next free one and
prints the address.

## Reload as you edit

`--live-reload` reloads open pages as soon as you edit the theme or the data. The page listens
on a WebSocket, and asks the server every 700 ms where a socket cannot be opened. It adds a
script to the pages: leave it off for tests. Only HTML pages get it: a `{% layout none %}`
template that writes JSON is left as it is.

An edit of `assets/` needs no restart either, with or without `--live-reload`: a stylesheet, a
script, an image, a `.css.liquid` or `.js.liquid` file, the bundle of the `{% stylesheet %}` and
`{% javascript %}` tags, and the assets of a [theme app extension](#theme-app-extensions). The
page links each file at a new `?v=` once it changes, as Shopify does, and the browser fetches
it there. A file the theme loads without its `?v=` (a `url()` in a stylesheet, an `import` in a
script) is kept a year by the browser, as from Shopify's CDN: link it with `asset_url`, or
reload without the cache. `--live-reload` reloads the whole page when an asset changes; it does
not swap a stylesheet in place.

With `--live-reload`, a page with problems also shows a small panel: the errors of the store
data, the Liquid errors of the page, and why a page is the 404 page. Its button closes it. The
terminal has the same problems.

The panel also has notes, which are not problems. A page that renders one of
[lsf's default menus](data-format.md#lsfs-default-menus) gets a note: its links are lsf's,
since the data has no menu with that handle. Without problems, the panel has a grey border
instead of a red one.

## Log in as a customer

Open <http://127.0.0.1:9292/account>. The page lists the customers of your data: click one to
be logged in as them, or "Nobody" to be logged out. Every account link of your theme leads to
this page, because Shopify hosts customer accounts outside the theme.

To start logged in:

```bash
npx local-storefront serve --customer default
```

`default` is the first customer of your data. You can also give an email, or `none`.

A link can log a browser in too: `/__lsf/login?customer=jane.doe@example.com&return_to=/cart`.

With `--app`, `/account` is the customer account of the app instead: the orders of the customer
logged in, their profile, and the pages where the app's customer account UI extensions render.
Logged out, it lists the customers of your data too. See
[checkout](checkout.md#customer-account).

Classic customer accounts are not simulated: the theme's `templates/customers` are never
rendered, and its login and register forms lead to `/account` too. See the
[data guide](data-format.md#logging-in).

### B2B

A customer with a company is a B2B customer: `customer.b2b?` is true, and
`customer.current_company` and `customer.current_location` are filled. The account page lets
you change location. Company prices (catalogs) are not simulated. How to write companies:
[data guide](data-format.md#b2b-customers).

## The password page

Open <http://127.0.0.1:9292/password>. The password is `password`, or the `password` of the
`shop` in your data. A wrong password stays on the page with the error. The storefront is
never locked: nothing redirects to that page.

## Images and videos

`image_url` and `image_tag` point to your machine, which resizes and crops like Shopify's CDN,
and sends each browser the lightest format it reads: AVIF, WebP, or the one of the file. An
image with `preload: true` is preloaded through the `Link` header, as on Shopify.

Put the files in `shopify-local/files/`. An image that is not there becomes a placeholder of
the right size, marked with the header `x-lsf-placeholder: 1`.

A video of `files/` (a `video` setting, or a metafield or a metaobject field that points to
an `.mp4`) is a video object, with its sources and its poster, the `.jpg` of the same name.
Videos are served in the parts a browser asks for, so they play in Safari and can be sought
in.

## What else is served as on Shopify

- **Styles and scripts are minified**, with a source map, as Shopify minifies them: when that
  makes the file lighter, and without merging declarations that cascade apart, such as
  `translate` into `transform` ([the rules](compatibility.md#minification)). `--no-minify`
  serves them as written.
- **Responses are compressed** with Brotli or gzip, with Shopify's `Cache-Control`, so that an
  audit of your theme sees what it would see on the store. `--no-compression` turns it off.
- **Pages have Shopify's scripts:** the `Shopify` object, `Shopify.actions`,
  `Shopify.loadFeatures`, the Customer Privacy API and `ShopifyAnalytics.meta`.
- **The Ajax APIs a theme calls** answer as Shopify's do: cart, Section Rendering, predictive
  search, recommendations, forms. The list: [testing](testing.md#what-the-server-answers).
- **The app proxy of an app:** with `--app` and an `[app_proxy]` in `shopify.app.toml`,
  `/apps/<subpath>` (or the `prefix` it names) is forwarded to the backend of the app, signed as
  Shopify signs it, and a Liquid answer is rendered in the theme's layout. See the
  [Admin API guide](admin.md#app-proxy).

## Theme app extensions

With `--app`, the theme renders the app blocks and app embed blocks of the app's theme app
extensions, as Shopify does once the app is installed. An extension is a folder of the app
whose `shopify.extension.toml` says `type = "theme"`, with `blocks/`, `snippets/`, `assets/`
and `locales/`.

```bash
npx local-storefront serve --app ../my-app --live-reload
```

The `app:` line counts them: `(0 Functions, 0 UI extensions, 1 theme extension (2 app blocks,
1 app embed block))`.

### App blocks

A JSON template or a section group names an app block by its type, as the theme editor writes
it:

```json
"reviews": {
  "type": "shopify://apps/product-reviews/blocks/reviews/5b1c7a52-2d0e-4c8a-9a3e-6f1d2b7c9e01",
  "settings": { "heading": "Loved by customers" }
}
```

`product-reviews` is the `handle` of `shopify.app.toml`, or its `name` as a handle. `reviews` is
the file `blocks/reviews.liquid` of one of its extensions. The last part is the `uid` of the
extension. A type copied from another store has another id: the block is found by its name all
the same.

The block renders where the theme renders blocks:

- `{% render block %}`, for a block of `section.blocks` whose `block.type` is `@app`, as in Dawn;
- `{% content_for 'blocks' %}`, in a section or a theme block that accepts `@app` blocks, as in
  Horizon;
- a section of type `apps`, which the theme editor adds around an app block: the theme's
  `sections/apps.liquid`, or Shopify's own wrapper when the theme has none.

Its output is wrapped as on Shopify, in `<div id="shopify-block-<id>" class="shopify-block
shopify-app-block">`, with the `class` and the `tag` of its schema. Its Liquid reads:

- `block.settings`, with the defaults of its schema; a `t:` default is read from the
  extension's `locales/*.schema.json`;
- `block.id`, `block.type` (`@app`), `section`, `closest`, and the objects of the page
  (`product` on a product page);
- `app.metafields`: the app-data metafields of the store data (`app.metafields`, below).

In the Liquid of an extension, `{% render 'stars' %}` renders its `snippets/stars.liquid`,
`asset_url` points to its `assets/` (served at `/cdn/extensions/<uid>/assets/`), and `t` reads
its `locales/`, never the theme's. The `javascript` and the `stylesheet` of a schema load once
per page, before `</head>`, when a block that names them renders. `enabled_on` and
`disabled_on` keep a block out of templates and section groups; `available_if` keeps it out
until its metafield is `true`.

### App embed blocks

An app embed block (`"target": "head"`, `"compliance_head"` or `"body"`) renders on every page
where it is turned on: `compliance_head` then `head` before `</head>`, `body` before `</body>`.
The theme editor turns one on in `config/settings_data.json`, under `current.blocks`, with
`"disabled": false`. To turn one on without changing the theme, or to set the app-data
metafields, write `app` in the store data:

```json
{
  "app": {
    "embeds": {
      "chat-bubble": true,
      "reviews-loader": { "settings": { "api_key": "test-key" } }
    },
    "metafields": { "reviews": { "badge_enabled": true } }
  }
}
```

An embed is named by its file in `blocks/`. `true` turns it on, with the settings of
`config/settings_data.json` or its defaults; `false` turns it off; `{"settings": {...}}` turns
it on with these. A test does it for its session alone with `PUT /__lsf/session` and
`{"data": {"app": ...}}` ([testing](testing.md#data-for-one-test-only)).

### When a block renders nothing

lsf says why, once in the terminal and in the `X-LSF-Reason` header of the page: the app has no
block of that name, the block is another app's, no app is served (`--app`), `enabled_on` leaves
the page out, `available_if` is not `true`. An app embed block the theme turns on says so too
when no app is served. When several blocks of a page render nothing, `X-LSF-Reason` gives them
all: `3 reasons: … | … | …`. A Liquid error in a file of an extension is reported
as the theme's are, with its path: `Liquid syntax error (extensions/reviews/snippets/stars line
1): Unknown tag 'unknowntag'`.

At start, `serve` prints the blocks the theme names that the app does not have
(`app: warning: ...`, with the name it probably meant), and the blocks no template uses or the
app embed blocks that are off (`app: note: ...`). The home page of the app
(`/__lsf/checkouts`) lists its extensions and their blocks. `check --app ../my-app` checks the
app blocks of the theme, and exits with `1` for one that renders nothing (a block the app does
not have, one its `enabled_on` keeps out of the template). `app build` checks the extensions as
Shopify does when it deploys them ([CLI reference](cli.md#app-build)).

With `--live-reload`, open pages reload when a file of an extension changes. The Section
Rendering API (`?sections=`) renders app blocks as the page does. What differs from Shopify is
in [compatibility](compatibility.md#known-differences).

## Feature previews

Shopify tries changes on development stores before it ships them. A store turns them on in its
list of feature previews. local-storefront renders what a live store renders, so a theme that
uses a preview fails as it would there, and local-storefront says which preview to turn on:

```bash
npx local-storefront serve --preview liquid-july-26
```

`--preview` can be repeated, takes several names separated by commas, and is also read from
`LSF_PREVIEW`. Every command accepts it.

| Preview | Shopify's name | What it adds |
|---|---|---|
| `liquid-july-26` | Liquid July '26 changes | `{% block 'name' %}...{% endblock %}`, to call a theme block from a template or a layout; `{% partial 'name' %}...{% endpartial %}`, and `@shopify/partial-rendering` to refresh it. Shopify's [Skeleton theme](https://github.com/Shopify/skeleton-theme) needs it. |

A preview can still change on Shopify's side. [Compatibility](compatibility.md) says what
local-storefront does where Shopify has not said yet.

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
script to the pages: leave it off for tests.

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

- **Styles and scripts are minified**, with a source map, as Shopify minifies them.
  `--no-minify` serves them as written.
- **Responses are compressed** with Brotli or gzip, with Shopify's `Cache-Control`, so that an
  audit of your theme sees what it would see on the store. `--no-compression` turns it off.
- **Pages have Shopify's scripts:** the `Shopify` object, `Shopify.actions`,
  `Shopify.loadFeatures`, the Customer Privacy API and `ShopifyAnalytics.meta`.
- **The Ajax APIs a theme calls** answer as Shopify's do: cart, Section Rendering, predictive
  search, recommendations, forms. The list: [testing](testing.md#what-the-server-answers).

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

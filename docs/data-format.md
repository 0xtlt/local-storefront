# Store data format

`lsf` renders a Shopify theme from JSON files instead of the Shopify API. This document
describes those files. It is printed by `lsf docs`, and `lsf init` writes it next to the data
as `README.md`.

The format is strict on purpose: unknown fields, wrong types and broken references are errors.
Run `lsf validate` after every change. Each problem is reported with a code, the file, a JSON
pointer to the value, and usually a hint:

```text
error[unknown_field]: unknown field "titel"
  --> products/shirt.json at /titel
  hint: did you mean "title"?
```

`lsf validate --format json` prints the same report as JSON, for tools and LLMs:

```json
{
  "ok": false,
  "errors": 1,
  "warnings": 0,
  "diagnostics": [
    {
      "severity": "error",
      "code": "unknown_field",
      "file": "products/shirt.json",
      "path": "/titel",
      "message": "unknown field \"titel\"",
      "hint": "did you mean \"title\"?"
    }
  ]
}
```

The exit status is `0` when the data is valid and `1` when it has errors. Warnings do not fail.

## The data directory

By default the data is read from `shopify-local/` inside the theme. Pass `--data <dir>` (or
set `LSF_DATA`) to use another directory. Without any data directory, a built-in demo store is
used; `lsf init` writes that demo store to disk as a starting point.

```text
shopify-local/
  store.json            data files: any name, any number, merged together
  menus.json
  products/             one product per file; the file name is the default handle
    blue-shirt.json
  collections/          same for collections,
  pages/                pages,
  blogs/                blogs (with their articles),
  customers/            customers,
  menus/                and menus (the file name is the menu handle)
  files/                images, videos, fonts: served under /cdn/shop/files/
  schema/               JSON Schemas for editors, written by `lsf init` (not read back)
  README.md             this document
```

Every `*.json` file at the root is a **data file**: an object with any of the keys `shop`,
`products`, `collections`, `pages`, `blogs`, `menus`, `customers`, `companies`, `gift_cards`,
`metaobjects`, `localization`, `files`, `session`, `now` and `theme_settings`. Root files are
merged, so the data can be split however is convenient. Lists add up; `shop`, `localization`,
`session` and `now` may be defined in one file only.

A file in `products/`, `collections/`, `pages/`, `blogs/` or `customers/` holds one entity, or
an array of them. Files are read in alphabetical order, which is the order products matched by
the rules of an automated collection are listed in.

JSON files may contain `//` and `/* */` comments.

### Editor support

Point `$schema` at the schema of the file's kind to get completion and inline errors:

```json
{ "$schema": "../schema/product.schema.json", "title": "Blue shirt", "price": "19.99" }
```

`lsf schema <kind>` prints a schema (`store`, `product`, `collection`, `page`, `blog`,
`customer`, `menu`, `session`), `lsf schema --out <dir>` writes them all, and a running server
serves them at `/__lsf/schema/<kind>`.

## Conventions

- **Unknown fields are errors.** A typo never silently does nothing.
- **Ids and handles are optional.** A handle is derived from the title (`"Blue Shirt"` →
  `blue-shirt`), or from the file name for one-entity files. Ids are derived from handles. Both
  are stable across runs and machines, so URLs and ids can be hard-coded in tests. Set `id`
  explicitly only to reproduce a specific Shopify id.
- **Money** is an integer number of cents (`1999`) or a decimal string (`"19.99"`). A JSON
  float (`19.99`) is rejected, because it is ambiguous. Liquid sees cents, as on Shopify.
- **Dates** are ISO 8601 strings: `"2024-05-01"` or `"2024-05-01T10:00:00Z"`. Entities without
  dates get fixed ones, so renders are reproducible.
- **References** between entities use handles (products, collections, pages, blogs), emails
  (customers) and SKUs or ids (variants). A reference to something that does not exist is an
  error, with a suggestion when a close match exists.
- **HTML** fields (`description`, `content`, `body`) are inserted as written.

## Products

The smallest product:

```json
{ "title": "Blue shirt", "price": "19.99" }
```

It gets a single default variant, like a product without options in the Shopify admin. A
product with options lists its variants; each gives one value per option:

```json
{
  "title": "Organic Cotton T-Shirt",
  "vendor": "Local Supply Co.",
  "type": "Shirts",
  "tags": ["cotton", "new"],
  "options": ["Color", "Size"],
  "price": "28.00",
  "compare_at_price": "35.00",
  "images": [
    { "src": "products/tee-white.jpg", "alt": "White t-shirt", "width": 1600, "height": 2000 },
    "products/tee-black.jpg"
  ],
  "variants": [
    { "options": ["White", "S"], "sku": "TEE-WHT-S", "inventory_quantity": 12, "image": "products/tee-white.jpg" },
    { "options": ["White", "M"], "sku": "TEE-WHT-M", "inventory_quantity": 0 },
    { "options": ["Black", "S"], "sku": "TEE-BLK-S", "price": "30.00", "available": false }
  ],
  "collections": ["apparel"],
  "metafields": { "custom": { "care": "Machine wash cold" } }
}
```

- `price` and `compare_at_price` on the product apply to the variants that do not set their own.
- **Availability** follows Shopify's rules. A variant with `inventory_quantity` is tracked: it is
  available while the quantity is above zero, or always with `"inventory_policy": "continue"`.
  A variant without `inventory_quantity` is not tracked and always available. `available` forces
  the answer either way. Adding more than the stock to the cart fails as it does on Shopify.
- `options` can spell out the order of the values: `{ "name": "Size", "values": ["S", "M", "L"] }`.
- Use `media` instead of `images` to mix images with videos, external videos and 3D models.
- `recommendations` lists the handles returned by product recommendations; by default they are
  other products of the same collections.
- `template_suffix` selects an alternate template (`templates/product.<suffix>.json`).

## Collections

```json
{
  "collections": [
    { "title": "Apparel", "products": ["organic-cotton-t-shirt", "linen-overshirt"] },
    { "title": "Sale", "rules": [{ "column": "tag", "condition": "sale" }] },
    {
      "title": "Under $30",
      "sort_order": "price-ascending",
      "rules": [{ "column": "price", "relation": "less_than", "condition": "3000" }]
    }
  ]
}
```

A product belongs to a collection when the collection lists it in `products`, when the product
lists the collection in its own `collections`, or when it matches the collection's `rules`
(all of them, or any of them with `"disjunctive": true`).

`collections.all` always exists and contains every product, sorted by title as on Shopify.
Define a collection with the handle `all` only to change its title or order.

Storefront filtering (`collection.filters`, `?filter.v.availability=1`,
`?filter.v.price.gte=10`, `?filter.p.vendor=...`, `?filter.v.option.color=...`), sorting
(`?sort_by=`) and tag URLs (`/collections/<handle>/<tag>`) work from this data without any
extra configuration.

## Pages, blogs and articles

```json
{
  "pages": [
    { "title": "About", "content": "<p>Since 2012.</p>" },
    { "title": "Contact", "template_suffix": "contact" }
  ],
  "blogs": [
    {
      "title": "Journal",
      "comments_enabled": true,
      "articles": [
        {
          "title": "How to care for linen",
          "author": "Sam",
          "tags": ["care"],
          "published_at": "2024-05-01",
          "image": { "src": "blog/linen.jpg", "width": 1600, "height": 900 },
          "excerpt": "<p>Linen gets better with every wash.</p>",
          "content": "<p>Wash cold, hang dry.</p>",
          "comments": [{ "author": "Alex", "content": "Great tips." }]
        }
      ]
    }
  ]
}
```

## Menus

`menus` maps a menu handle (`main-menu`, `footer`, ...) to its links. A link points at a
resource with one of `url`, `collection`, `product`, `page`, `blog`, `article`
(`"<blog>/<article>"`) or `policy`; its URL, `link.type` and `link.object` follow from it.
`link.active` and `link.child_active` follow from the page being rendered.

```json
{
  "menus": {
    "main-menu": {
      "links": [
        { "title": "Home", "url": "/" },
        {
          "title": "Shop",
          "collection": "all",
          "links": [{ "title": "Apparel", "collection": "apparel" }]
        },
        { "title": "About", "page": "about" }
      ]
    }
  }
}
```

## The shop

Every field of `shop` is optional.

```json
{
  "shop": {
    "name": "Local Supply Co.",
    "currency": "EUR",
    "money_format": "{{amount_with_comma_separator}} €",
    "timezone": "Europe/Paris",
    "policies": { "refund_policy": { "body": "<p>30 days.</p>" } },
    "enabled_payment_types": ["visa", "master", "paypal"]
  }
}
```

- `money_format` and `money_with_currency_format` default to the usual format of the currency.
- `timezone` is the zone dates are displayed in by the `date` filter.
- `password` is the password the `/password` page accepts. It is `password` when you do not
  set it. The storefront is never locked: the page is always there, and nothing leads to it.
- `customer_accounts` says which accounts the store uses: `new` or `legacy` (see below).
- Each policy that is set gets a page under `/policies/`.

## Customers and sessions

A **customer** exists in the store. A **session** is one browser's state: who is logged in,
what is in the cart, which country is selected.

```json
{
  "customers": [
    {
      "email": "jane.doe@example.com",
      "first_name": "Jane",
      "last_name": "Doe",
      "tags": ["vip"],
      "password": "password",
      "addresses": [{ "address1": "12 Maple Street", "city": "Portland", "country_code": "US" }],
      "orders": [{ "line_items": [{ "variant": "TEE-WHT-S", "quantity": 2 }] }]
    }
  ],
  "session": {
    "customer": "jane.doe@example.com",
    "cart": { "items": [{ "variant": "TEE-WHT-S", "quantity": 1 }], "note": "Gift wrap" },
    "country": "FR"
  }
}
```

`session` is what every new browser session starts with. Leave it out for an anonymous visitor
with an empty cart. A variant is referenced by its SKU, by its id, or by a product handle
(meaning the product's first variant).

`session.customer` is the email of a customer, `"default"` for the first customer of the data,
or `"none"`. When the data has no customer at all, there is still one to log in as:
`customer@example.com`.

### Logging in

How a visitor logs in depends on the accounts the store uses:

- **New customer accounts** are hosted by Shopify and are not part of a theme. Here, `/account`
  is a page of `lsf` that stands in for them. It lists the customers of the data: click one to
  be logged in as them, or "Nobody" to be logged out. Every account link of a theme
  (`routes.account_login_url`, `routes.storefront_login_url`, ...) leads to that page.
- **Legacy accounts** are rendered by the theme's `templates/customers`. The login form accepts
  a customer's `password`, or any password when none is set.

`lsf` picks legacy accounts when the theme has `templates/customers/login`, and new accounts
otherwise. Set `shop.customer_accounts` to `"new"` or `"legacy"` to decide yourself.

`lsf serve --customer <email>` starts every visitor logged in as that customer. It also accepts
`default` and `none`.

### B2B customers

A B2B customer buys for a **company**, at one of its **locations**. Describe the company, then
name it in the customer:

```json
{
  "companies": [
    {
      "name": "Northwind Hotels",
      "external_id": "NW-001",
      "metafields": { "custom": { "payment_terms": "Net 30" } },
      "locations": [
        {
          "name": "Northwind Portland",
          "tax_registration_id": "93-1234567",
          "shipping_address": { "address1": "815 Harbor Way", "city": "Portland", "country_code": "US" },
          "metafields": { "custom": { "delivery_notes": "Loading dock B" } }
        },
        { "name": "Northwind Seattle" }
      ]
    }
  ],
  "customers": [
    {
      "email": "alex.morgan@example.com",
      "first_name": "Alex",
      "company": "Northwind Hotels",
      "metafields": { "custom": { "job_title": "Purchasing manager" } }
    }
  ]
}
```

- The customer can buy for every location of the company. List some in `company_locations` to
  restrict them: `"company_locations": ["Northwind Seattle"]`.
- They start at the first location. `session.company_location` names another one.
- In Liquid, `customer.b2b?` is true, and `customer.current_company`,
  `customer.current_location` and `customer.company_available_locations` are filled.
- `location.url_to_set_as_current` changes the location, as on Shopify.

Catalogs are not simulated: a B2B customer sees the same products and prices as everyone.

Tests usually set the session per test rather than in the files, by sending it to the running
server. The body is a session, optionally with a `data` object: a data file applied on top of
the files on disk for that session only.

```bash
curl -X PUT http://127.0.0.1:9292/__lsf/session \
  -H 'x-lsf-session: my-test' -H 'content-type: application/json' \
  -d '{"customer": "jane.doe@example.com", "data": {"shop": {"name": "Another name"}}}'
```

A person can do the same from a browser: `/__lsf` lists the customers, and
`/__lsf/login?customer=<email>` logs the browser in as one of them (`default` and `none` work
too).

A wrong body is answered with `422` and the same diagnostics as `lsf validate --format json`.

## Gift cards

Each gift card has a page rendered by `templates/gift_card.liquid`. Its URL is listed by
`lsf routes`.

```json
{
  "gift_cards": [
    { "code": "WCGX7X97G74JGDGC", "initial_value": "50.00", "balance": "32.50", "expires_on": "2030-12-31" }
  ]
}
```

## Localization

```json
{
  "localization": {
    "countries": [{ "iso_code": "US", "popular": true }, { "iso_code": "FR" }],
    "languages": [{ "iso_code": "en" }, { "iso_code": "fr" }]
  }
}
```

The first language is the primary one, served at `/`. The others are served under
`/<iso_code>/` (`/fr/products/blue-shirt`) and use the theme's `locales/<iso_code>.json`.
Names and currencies of countries default to built-in values. The localization form
(`{% form 'localization' %}`) switches the session's country and language.

## Images and files

An image is a path inside `files/`, optionally with metadata:

```json
"products/tee-white.jpg"
{ "src": "products/tee-white.jpg", "alt": "White t-shirt", "width": 1600, "height": 2000 }
```

URLs keep the shape Shopify gives them but point to the local server, never to Shopify's CDN:

```text
{{ product.featured_image | image_url: width: 400, height: 400, crop: 'center' }}
→ //localhost:9292/cdn/shop/files/products/tee-white.jpg?crop=center&height=400&v=1651229318&width=400
```

The server resizes and crops on the fly with Shopify's parameters: `width`, `height`, `crop`
(`top`, `center`, `bottom`, `left`, `right`, or a region with `crop_left`, `crop_top`,
`crop_width`, `crop_height`), `pad_color`, `format` and `quality`. Images are never scaled up
beyond their own size, as on Shopify. The legacy `img_url` sizes (`shirt_400x400_crop_center.jpg`)
work too.

**The file does not have to exist.** When it is missing, the server draws a placeholder of the
declared `width` × `height` (1200 × 1200 when not declared) showing the file name, and answers
with the header `x-lsf-placeholder: 1`. Layout, aspect ratios and `srcset` are then the same as
with real images, so fixtures do not need to ship binaries. When the file exists, its real
size is read from it.

`files` gives metadata to files that are referenced by name from theme settings or metafields:

```json
{ "files": { "hero.jpg": { "alt": "Storefront", "width": 2400, "height": 1200 } } }
```

A setting of type `image_picker` holding `shopify://shop_images/hero.jpg` resolves to
`files/hero.jpg`.

Fonts picked with `font_picker` come from Shopify's font library, which is not available
offline. Put a font at `files/fonts/<file name>` to serve the real file; otherwise a blank font
is served so that browsers fall back to the theme's fallback stack without network errors.

## Metafields and metaobjects

Metafields are grouped by namespace, then key. A plain value gets its type inferred; spell out
`{ "type", "value" }` for anything else. References are written as handles, files as paths
inside `files/`.

```json
{
  "metafields": {
    "custom": {
      "care": "Machine wash cold",
      "rating": 5,
      "organic": true,
      "size_guide": { "type": "page_reference", "value": "size-guide" },
      "related": { "type": "list.product_reference", "value": ["linen-overshirt"] },
      "swatch": { "type": "color", "value": "#f4f1ea" },
      "lookbook": { "type": "file_reference", "value": "lookbook.jpg" }
    }
  }
}
```

`{{ product.metafields.custom.care }}` and `{{ product.metafields.custom.size_guide.value.title }}`
then behave as on Shopify, including `metafield_tag` and `metafield_text`.

Metaobjects are listed by type:

```json
{
  "metaobjects": {
    "designer": [{ "handle": "sam", "fields": { "name": "Sam", "portrait": { "type": "file_reference", "value": "sam.jpg" } } }]
  }
}
```

and read with `{{ metaobjects.designer.sam.name }}` or `{% for d in metaobjects.designer.values %}`.

## Time and theme settings

- `now` freezes the clock: `"now": "2025-01-15T10:00:00Z"`. `{{ 'now' | date: ... }}`, sale
  countdowns and "new" badges then render the same on every run.
- `theme_settings` overrides values of `config/settings_data.json` by setting id, without
  touching the theme: `"theme_settings": { "cart_type": "drawer" }`.

## Error codes

| Code | Meaning |
|---|---|
| `invalid_json` | The file is not valid JSON. The hint gives the line and column. |
| `unreadable_file` | The file could not be read. |
| `unknown_field` | A field that does not exist. The hint suggests the closest one. |
| `missing_field` | A required field is absent. |
| `wrong_type` | A value of the wrong JSON type. |
| `invalid_value`, `invalid_format` | A value outside what the field accepts. |
| `invalid_money`, `invalid_date`, `invalid_email`, `invalid_handle`, `invalid_timezone`, `invalid_gift_card_code` | A value that does not parse as what it should be. |
| `duplicate_handle`, `duplicate_id`, `duplicate_email`, `duplicate_menu`, `duplicate_variant`, `duplicate_gift_card` | Two entities with the same identity. |
| `duplicate_section` | `shop`, `localization`, `session` or `now` defined in two files. |
| `unknown_product`, `unknown_collection`, `unknown_variant`, `unknown_customer`, `unknown_company`, `unknown_location`, `unknown_country`, `unknown_image`, `unknown_reference` | A reference to something that is not in the data. The hint suggests the closest match. |
| `duplicate_company`, `duplicate_location`, `missing_locations`, `missing_company` | A company without a location, two companies or two locations with the same name, or a customer that lists locations without a company. |
| `missing_price` | A product without a price on itself or on its variants. |
| `missing_variants`, `missing_options`, `option_mismatch`, `too_many_options` | The options of a product and the option values of its variants do not line up. |
| `media_conflict`, `missing_sources`, `missing_external_video` | Incomplete or contradictory product media. |
| `ambiguous_link` | A menu link that points at more than one thing. |
| `missing_locale_file` (warning) | A language the theme has no `locales/<code>.json` for. |
| `gift_card_balance` (warning) | A gift card whose balance is above its initial value. |

`path` is always a JSON pointer into `file`, so the value to fix can be located mechanically.

## Field reference

Every field of every type is listed in [data-reference.md](data-reference.md), which is
generated from the JSON Schema the validator uses.

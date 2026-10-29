# Store data format

local-storefront renders a Shopify theme from JSON files instead of the Shopify API. This
document describes those files. It is printed by `npx local-storefront docs`, and
`npx local-storefront init` writes it next to the data as `README.md`.

The format is strict on purpose: unknown fields, wrong types and broken references are errors.
Run `npx local-storefront validate` after every change. Each problem is reported with a code,
the file, a JSON pointer to the value, and usually a hint:

```text
error[unknown_field]: unknown field "titel"
  --> products/shirt.json at /titel
  hint: did you mean "title"?
```

`npx local-storefront validate --format json` prints the same report as JSON, for tools and
LLMs:

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
used; `npx local-storefront init` writes that demo store to disk as a starting point.

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
  checkouts/            checkout scenarios, one named setup of the local checkout per file
  files/                images, videos, fonts: served under /cdn/shop/files/
  schema/               JSON Schemas for editors, written by `init` (not read back)
  README.md             this document
```

Every `*.json` file at the root is a **data file**: an object with any of the keys `shop`,
`products`, `collections`, `pages`, `blogs`, `menus`, `customers`, `companies`, `gift_cards`,
`selling_plan_groups`, `locations`, `swatches`, `metaobjects`, `localization`, `files`,
`session`, `checkout`, `now` and `theme_settings`. Root files are merged, so the data can be
split however is convenient. Lists add up; `shop`, `localization`, `session`, `checkout` and
`now` may be defined in one file only.

A file in `products/`, `collections/`, `pages/`, `blogs/` or `customers/` holds one entity, or
an array of them. Files are read in alphabetical order, which is the order products matched by
the rules of an automated collection are listed in.

JSON files may contain `//` and `/* */` comments.

### Editor support

Point `$schema` at the schema of the file's kind to get completion and inline errors:

```json
{ "$schema": "../schema/product.schema.json", "title": "Blue shirt", "price": "19.99" }
```

`npx local-storefront schema <kind>` prints a schema (`store`, `product`, `collection`, `page`,
`blog`, `customer`, `menu`, `session`, `checkout-scenario`), `npx local-storefront schema --out <dir>` writes them
all, and a running server serves them at `/__lsf/schema/<kind>`.

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
- **Availability** follows Shopify's rules. A variant with `inventory_quantity` is tracked: it
  is available while the quantity is above zero, or always with
  `"inventory_policy": "continue"`. A variant without `inventory_quantity` is not tracked and
  always available. `available` forces the answer either way. Adding more than the stock to the
  cart fails as it does on Shopify.
- `options` can spell out the order of the values:
  `{ "name": "Size", "values": ["S", "M", "L"] }`. A value can carry a [swatch](#swatches).
- `selling_plan_groups` and `requires_selling_plan` sell the product by subscription: see
  [selling plans](#selling-plans).
- A variant's `store_availabilities` says where it can be picked up: see
  [store pickup](#store-pickup).
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

## Selling plans

A selling plan is a way of buying a product other than once: a subscription, a pre-order.
Plans come in groups, which is what a subscription app creates.

```json
{
  "selling_plan_groups": [
    {
      "name": "Subscribe and save",
      "options": ["Delivery frequency"],
      "selling_plans": [
        {
          "name": "Deliver every month, 10% off",
          "options": ["Every month"],
          "price_adjustments": [{ "value_type": "percentage", "value": 10 }]
        },
        {
          "name": "Deliver every 3 months",
          "options": ["Every 3 months"]
        }
      ],
      "products": ["organic-cotton-t-shirt"]
    }
  ]
}
```

- A product is sold with a group when the group lists it in `products`, or when the product
  names the group: `"selling_plan_groups": ["Subscribe and save"]`. The plans apply to every
  variant of the product.
- `"requires_selling_plan": true` on a product means it cannot be bought once.
- A **price adjustment** has a `value_type`: `percentage` (`value` percent off), `fixed_amount`
  (`value` off, an amount of money) or `price` (`value` is the price). A plan has at most two:
  the first one gives the price, the second one is what later orders cost. Without any, the
  plan costs what the variant costs.
- `options` of the group are the names of what the plans differ by, and each plan gives one
  value per option. They default to one option, `Delivery frequency`, whose value is the
  plan's name.
- `checkout_charge` is what is paid at checkout, for a deposit:
  `{ "value_type": "percentage", "value": 50 }`. It defaults to the whole price. The checkout
  of an [app](checkout.md#deferred-payments) charges only that today, and shows the rest as due
  later.
- `recurring_deliveries` defaults to `true`. Ids are generated and stable; set `id` on a plan
  to reproduce a Shopify id.

In Liquid, `product.selling_plan_groups`, `variant.selling_plan_allocations` and
`product.selected_or_first_available_selling_plan_allocation` are filled. `?selling_plan=<id>`
selects a plan (`product.selected_selling_plan`, `selling_plan.selected`), and with `?variant=`
an allocation (`product.selected_selling_plan_allocation`). `/products/<handle>.js` and the
`json` filter list the groups and the allocations as Shopify does.

In the cart, `selling_plan` is accepted by `/cart/add.js` and `/cart/change.js`. The line then
has the plan's price and its `selling_plan_allocation`, and stays separate from the same
variant bought once. A product is refused with a plan it is not sold with, and without one
when it requires one.

## Store pickup

`locations` are the places the store stocks its products at.

```json
{
  "locations": [
    {
      "name": "Portland flagship",
      "address": {
        "address1": "12 Maple Street",
        "city": "Portland",
        "province_code": "OR",
        "zip": "97204",
        "country_code": "US",
        "phone": "+1 503 555 0100"
      },
      "pick_up_time": "Usually ready in 2 hours",
      "latitude": 45.5202,
      "longitude": -122.6742
    },
    { "name": "Seattle warehouse", "pick_up_enabled": false }
  ]
}
```

- `pick_up_enabled` and `physical_storefront` default to `true`, `pick_up_time` to
  `Usually ready in 24 hours`.
- Every location stocks every variant, and a variant is in stock there when it is available. A
  variant that lists `store_availabilities` is stocked by those locations only, each with its
  own stock:
  `"store_availabilities": { "Portland flagship": true, "Seattle warehouse": false }`.

In Liquid, `variant.store_availabilities` lists a `store_availability` per location, with its
`location` (`name`, `address`, `latitude`, `longitude`, `metafields`). As on Shopify, only the
selected variant and the first available one have them: the other variants of a product have
an empty list.

`/variants/<id>?section_id=<section>` renders a section for a variant, which the section reads
as `product_variant`. Themes fetch that URL to show pickup availability. Without `section_id`,
the URL leads to the product with the variant selected.

## Swatches

A swatch shows an option value as a color or an image instead of its name. `swatches` gives
them by the name of the value, for the whole store:

```json
{
  "swatches": {
    "White": "#f4f1ea",
    "Sage": "rgb(156, 175, 136)",
    "Floral": { "image": "swatches/floral.jpg" },
    "Sand": { "color": "#d8c7a8", "image": "swatches/sand.jpg" }
  }
}
```

Every option value with one of these names gets the swatch, whatever the product and the
option. A product can give its own to a value, in the long form of `options`:

```json
{
  "options": [
    { "name": "Color", "values": ["White", { "name": "Sunset", "swatch": "#ff7a00" }] },
    "Size"
  ]
}
```

In Liquid, `product_option_value.swatch` has a `color` and an `image`, and is `nil` for a
value without a swatch. In collection filters, a filter whose values have swatches has the
`swatch` presentation, and each `filter_value.swatch` is filled.

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

A cart item bought with a [selling plan](#selling-plans) names the plan, or gives its id:
`{ "variant": "TEE-WHT-S", "selling_plan": "Deliver every month, 10% off" }`.

`session.customer` is the email of a customer, `"default"` for the first customer of the data,
or `"none"`. When the data has no customer at all, there is still one to log in as:
`customer@example.com`.

### Logging in

Customer accounts are Shopify's new customer accounts: they are hosted by Shopify and are not
part of a theme. Here, `/account` is a page of local-storefront that stands in for them. It
lists the customers of the data: click one to be logged in as them, or "Nobody" to be logged
out. Every account link of a theme (`routes.account_login_url`, `routes.storefront_login_url`,
...) leads to that page, and so do its login and register forms.

Classic customer accounts are not simulated: the theme's `templates/customers` are never
rendered, and customers have no password. Data that still has `shop.customer_accounts` or a
customer's `password` is refused, with a hint to remove the field.

`npx local-storefront serve --customer <email>` starts every visitor logged in as that customer.
It also accepts `default` and `none`.

At the [checkout of an app](checkout.md#log-in-as-a-customer), the customers are the login
profiles: "Log in" in the Contact section lists them, and picking one logs the checkout (and the
session) in as them, with their saved addresses (the first one is the default) and, for a B2B
customer, the locations they buy for. Give the data a customer of each kind you test: a new one
(no orders, no addresses), a returning one (`addresses`, `orders`, `tags`, `metafields`), a B2B
buyer with several `company_locations`, a customer in another country. The demo store has one of
each.

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
- The [local checkout](checkout.md) shows what Shopify's Functions read of a B2B buyer
  (`purchasingCompany`), and you can set it. A company has `created_at` and `updated_at`, a
  location the same plus `locale` (`"fr"`), `orders_count` and `total_spent` (`"1250.00"`). The
  customer is the contact of their company: `company_contact` holds their `title`, `locale`,
  `created_at` and `updated_at`.

```json
{
  "companies": [
    {
      "name": "Northwind Hotels",
      "created_at": "2023-06-01",
      "locations": [
        { "name": "Northwind Portland", "locale": "en", "orders_count": 12, "total_spent": "8450.00" }
      ]
    }
  ],
  "customers": [
    {
      "email": "alex.morgan@example.com",
      "company": "Northwind Hotels",
      "company_contact": { "title": "Purchasing manager", "locale": "en" }
    }
  ]
}
```

  What is not set has a value: the dates are 2024-01-01 12:00 UTC plus one day per company,
  the dates of a location and of a contact are those of their company (`updated_at` those of
  `created_at`), the language is empty, and a location has no order and has spent nothing. The
  orders of the customers are not counted in `orders_count`: it is what you write.

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

A wrong body is answered with `422` and the same diagnostics as
`npx local-storefront validate --format json`.

## Gift cards

Each gift card has a page rendered by `templates/gift_card.liquid`. Its URL is listed by
`npx local-storefront routes`.

```json
{
  "gift_cards": [
    { "code": "WCGX7X97G74JGDGC", "initial_value": "50.00", "balance": "32.50", "expires_on": "2030-12-31" }
  ]
}
```

## Checkout

The `checkout` section sets up the local checkout of a Shopify app (`npx local-storefront
checkout --app <dir>`, see [checkout](checkout.md)). Every field has a default, and the section
changes nothing for a theme.

```json
{
  "checkout": {
    "shipping_rates": [{ "handle": "standard", "title": "Standard", "price": "4.90" }],
    "payment_methods": [{ "handle": "card", "name": "Credit card" }],
    "discounts": [
      {
        "function": "volume-discount",
        "title": "Volume",
        "classes": ["PRODUCT"],
        "metafields": { "$app:config.settings": { "type": "json", "value": { "percentage": 10 } } }
      }
    ],
    "functions": { "delivery-rules": { "metafields": { "$app:config.settings": { "type": "json", "value": {} } } } },
    "tax_rate": 20
  }
}
```

- `shipping_rates`: the delivery options offered to every address. Without it, one: "Standard"
  at 5.00.
- `payment_methods`: without it, "Credit card", "Cash on Delivery" and "Bank Deposit".
  `placements` says where a method is offered, which payment customization Functions read:
  `["PAYMENT_METHOD"]` (the list of the checkout, the default) and/or `["ACCELERATED_CHECKOUT"]`
  (the express buttons). A method that is only an express button is not in the list the page
  shows.
- `discounts`: each one uses a discount Function of the app by its handle. A `code` makes it a
  code discount; without one it is automatic. `classes` (`PRODUCT`, `ORDER`, `SHIPPING`) says
  what it may reduce, all three by default. `metafields` are those of the discount, by
  `"namespace.key"`, in the same shorthand as other metafields.
- `functions`: the metafields of the other Functions (cart transform, delivery or payment
  customization, validation), by the handle of the Function.
- `tax_rate`: a percentage, added to the discounted subtotal.

### Checkout scenarios

A scenario is a named setup of the local checkout: one file in `checkouts/`, named after the
scenario (`checkouts/b2b-seattle.json` is `b2b-seattle`). The home page of the checkout lists
them and opens a fresh checkout for each; `function run --scenario <name>` runs the Functions
for one ([checkout](checkout.md#scenarios)). A scenario is a set of overrides on top of the
store data: what it does not set stays as the data says.

```json
{
  "$schema": "../schema/checkout-scenario.schema.json",
  "title": "B2B buyer at the Seattle location",
  "description": "Four tees, a company customer, and a shipping rate for pallets.",
  "session": {
    "customer": "alex.morgan@example.com",
    "company_location": "Northwind Seattle",
    "country": "US",
    "cart": {
      "items": [{ "variant": "TEE-BLK-M", "quantity": 4 }],
      "note": "Deliver to dock B",
      "attributes": { "po_number": "PO-1042" }
    }
  },
  "entered": {
    "email": "alex.morgan@example.com",
    "shipping_address": { "last_name": "Morgan", "address1": "2200 Alder Street", "city": "Seattle", "country_code": "US" },
    "discount_codes": ["SAVE10"],
    "delivery": "pallet",
    "payment": "bank"
  },
  "language": "en",
  "checkout": {
    "shipping_rates": [{ "handle": "pallet", "title": "Pallet", "price": "45.00" }],
    "discounts": [{ "function": "volume-discount", "code": "SAVE10", "classes": ["ORDER"] }]
  },
  "variant": "standard",
  "extensions": ["volume-discount", "gift-message"]
}
```

Every field is optional.

- `title` and `description` are for the home page. The title defaults to the file name.
- `session` is a [session](#customers-and-sessions), applied on top of the `session` of the
  store data: `customer`, `cart` (`items` with `variant`, `quantity`, `properties` and
  `selling_plan`, `note`, `attributes`), `country` and `company_location`.
- `entered` is what the buyer already typed and chose: `email`, `phone`, `shipping_address`
  (`first_name`, `last_name`, `company`, `address1`, `address2`, `city`, `country_code`,
  `province_code`, `zip`, `phone`), `delivery` (the handle of a shipping rate), `payment` (the
  handle of a payment method) and `discount_codes`. A code that no discount has is entered all
  the same, and refused.
- `language` is a language of the store (`localization.languages`). The checkout opens under
  its locale root (`/fr/checkouts/cn/...`).
- `checkout` overrides the [`checkout` section](#checkout) of the store data. Every field that is
  present replaces the store's: `shipping_rates`, `payment_methods`, `discounts` (`[]` runs no
  discount), `tax_rate`. `functions` replaces the settings of the Functions it names and keeps
  those of the others.
- `variant` is the look: `standard` or `shop`. `?variant=` in the URL still wins.
- `extensions` lists the handles of the extensions (Functions and checkout UI extensions) that
  run. The others of the app are left out. It narrows what `--extension` allows.

A scenario that does not validate is still listed, with its errors, and cannot be opened.
`validate` reports it with the file `checkouts/<name>.json` and a pointer into it
(`/session/cart/items/0/variant`). Files that are not `.json`, and folders, are ignored.
Scenarios are reloaded when the files change, like the rest of the data. The built-in demo
store has none: `npx local-storefront checkout init` writes a set.

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

`markets` groups countries the way Shopify's markets do. The checkout Functions read the
market of the buyer (`localization.market`) and of an address (`MailingAddress.market`):

```json
{
  "localization": {
    "countries": [{ "iso_code": "FR" }, { "iso_code": "DE" }, { "iso_code": "US" }],
    "markets": [
      { "handle": "europe", "countries": ["FR", "DE"], "metafields": { "custom": { "tier": "gold" } } }
    ]
  }
}
```

A country in no market is a market of its own, named after the country (`france`, `united-states`).
An address in a country the store does not sell to has no market.

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
beyond their own size, as on Shopify. The legacy `img_url` sizes
(`shirt_400x400_crop_center.jpg`) work too.

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

`{{ product.metafields.custom.care }}` and
`{{ product.metafields.custom.size_guide.value.title }}` then behave as on Shopify, including
`metafield_tag` and `metafield_text`.

A `file_reference` is an image when its extension is one (`.jpg`, `.png`, `.webp`...) and a
video for `.mp4`, `.mov`, `.webm` and `.m4v`: `sources`, `preview_image`, `video_tag` and
`metafield_tag` work.
A video's poster is the `.jpg` of the same name (`looks/summer.jpg` for `looks/summer.mp4`),
and its size the one declared in `files` (1920×1080 otherwise). Any other file has a `url`.

Metaobjects are listed by type:

```json
{
  "metaobjects": {
    "designer": [{ "handle": "sam", "fields": { "name": "Sam", "portrait": { "type": "file_reference", "value": "sam.jpg" } } }]
  }
}
```

and read with `{{ metaobjects.designer.sam.name }}` or
`{% for d in metaobjects.designer.values %}`.

A type whose theme has `templates/metaobject/<type>.json` (or `.liquid`) is a type with web
pages on Shopify: each entry is served at `/pages/<type>/<handle>`, the template sees it as
`metaobject`, and `metaobject.system.url` is that URL. Without the template the entry has no
page and `system.url` is blank. `"template_suffix": "alt"` on an entry renders it with
`templates/metaobject/<type>.alt.json`, and `?view=alt` does the same for one request. The page
title is the entry's `title` or `name` field, or its handle. `shopify://metaobjects/<type>/<handle>`
in a `url` setting leads to the page.

The Functions of the [local checkout](checkout.md) read the same metaobjects as
`shop.metaobject(handle: {type: "designer", handle: "sam"})`, with their `field(key:)`.


## Time and theme settings

- `now` freezes the clock: `"now": "2025-01-15T10:00:00Z"`. `{{ 'now' | date: ... }}`, sale
  countdowns and "new" badges then render the same on every run. As on Shopify, where each
  `'now'` is a new reading of the clock, each `'now'` of a render is one nanosecond after the
  previous one: `{{ 'now' | date: '%N' }}` gives `000000000`, then `000000001`, and so on, so
  ids built from it differ. Without `now`, `'now'` reads the real clock each time.
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
| `duplicate_section` | `shop`, `localization`, `session`, `checkout` or `now` defined in two files. |
| `unknown_language` | A checkout scenario names a language that is not in `localization.languages`. |
| `unknown_product`, `unknown_collection`, `unknown_variant`, `unknown_customer`, `unknown_company`, `unknown_location`, `unknown_country`, `unknown_image`, `unknown_reference` | A reference to something that is not in the data, in a file of `checkouts/` as anywhere else. The hint suggests the closest match. |
| `duplicate_company`, `duplicate_location`, `missing_locations`, `missing_company` | A company without a location, two companies or two locations with the same name, or a customer that lists locations or a `company_contact` without a company. |
| `duplicate_market_country` | A country in two `localization.markets`. |
| `unknown_selling_plan`, `missing_selling_plan`, `duplicate_selling_plan` | A selling plan or a group that is not in the data, or that the product is not sold with; a group without a plan, or a product that requires a plan and has none; two groups or two plans with the same name. |
| `invalid_color`, `empty_swatch` | A swatch whose color is not a CSS color, or that has neither a color nor an image. |
| `missing_price` | A product without a price on itself or on its variants. |
| `missing_variants`, `missing_options`, `option_mismatch`, `too_many_options` | The options of a product and the option values of its variants do not line up. |
| `media_conflict`, `missing_sources`, `missing_external_video` | Incomplete or contradictory product media. |
| `ambiguous_link` | A menu link that points at more than one thing. |
| `missing_locale_file` (warning) | A language the theme has no `locales/<code>.json` for. |
| `gift_card_balance` (warning) | A gift card whose balance is above its initial value. |

`path` is always a JSON pointer into `file`, so the value to fix can be located mechanically.

## Field reference

Every type of the format, generated from the JSON Schema the validator uses.

### Data file

A data file: any combination of the sections below. Every `*.json` file at the root of the
data directory is one of these, and they are merged, so you can split the data as you like
(`shop.json`, `catalog.json`, ...).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"./schema/store.schema.json"`. |
| `shop` | [Shop](#shop) | General store information. At most one data file may define it. |
| `products` | array of [Product](#product) | Products. They can also live one per file in `products/`. |
| `collections` | array of [Collection](#collection) | Collections. They can also live one per file in `collections/`. |
| `pages` | array of [Page](#page) | Pages. They can also live one per file in `pages/`. |
| `blogs` | array of [Blog](#blog) | Blogs with their articles. They can also live one per file in `blogs/`. |
| `menus` | map of [Menu](#menu) | Navigation menus by handle, e.g. `main-menu` and `footer`. |
| `customers` | array of [Customer](#customer) | Customers. They can also live one per file in `customers/`. |
| `companies` | array of [Company](#company) | Companies that buy from the store (B2B). Customers join one with their `company`. |
| `gift_cards` | array of [GiftCard](#giftcard) | Issued gift cards, each with its own page. |
| `metaobjects` | map of array of [Metaobject](#metaobject) | Metaobject entries by type. |
| `localization` | [Localization](#localization) | The countries and languages of the store. At most one data file may define it. |
| `swatches` | map of [Swatch](#swatch) | The swatches of product option values, by the name of the value: `{"White": "#ffffff", "Floral": {"image": "swatches/floral.jpg"}}`. Every option value with one of these names gets the swatch, on products and in collection filters. |
| `locations` | array of [Location](#location) | The places the store stocks products at, for store pickup. |
| `selling_plan_groups` | array of [SellingPlanGroup](#sellingplangroup) | Groups of selling plans: subscriptions, pre-orders and other purchase options. |
| `files` | map of [File](#file) | Metadata for files in `files/`, by path. |
| `session` | [Session](#session) | What every new browser session starts with: a logged-in customer, a filled cart... |
| `checkout` | [Checkout](#checkout) | The fake checkout of `lsf checkout --app`: shipping rates, payment methods, discounts and Function settings. At most one data file may define it. |
| `now` | string | Freezes the clock: the instant `'now'` resolves to, e.g. `"2025-01-15T10:00:00Z"`. Set it to make renders reproducible; each `'now'` of a render is then one nanosecond after the previous one, so `%N` still differs between calls. Defaults to the real time, read at each `'now'`. |
| `theme_settings` | map of any JSON value | Overrides for theme settings (`config/settings_data.json`), by setting id. |

### Shop

General store information (the `shop` object). Every field has a sensible default.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `name` | string | Defaults to `"Local Store"`. |
| `description` | string |  |
| `email` | string |  |
| `phone` | string |  |
| `domain` | string | The primary domain, without scheme. Defaults to the host the server is reached on. |
| `permanent_domain` | string | The `*.myshopify.com` domain. Defaults to a value derived from the name. |
| `currency` | string | ISO 4217 code of the store currency. Defaults to `USD`. |
| `money_format` | string | How `money` formats amounts, e.g. `"${{amount}}"` or `"{{amount_with_comma_separator}} €"`. Defaults to the usual format of the currency. |
| `money_with_currency_format` | string | How `money_with_currency` formats amounts, e.g. `"${{amount}} USD"`. |
| `timezone` | string | IANA time zone used to display dates, e.g. `"Europe/Paris"`. Defaults to `UTC`. |
| `address` | [Address](#address) |  |
| `policies` | [Policies](#policies) |  |
| `enabled_payment_types` | array of string | Payment icons to show, e.g. `["visa", "master", "american_express", "paypal"]`. |
| `customer_accounts_enabled` | boolean | Whether customer accounts exist at all. Defaults to `true`. |
| `customer_accounts_optional` | boolean | Whether checking out as a guest is possible. Defaults to `true`. |
| `taxes_included` | boolean | Whether prices include taxes. Defaults to `false`. |
| `password_message` | string | The message shown on the password page. |
| `password` | string | The password the `/password` page accepts. Defaults to `"password"`. The storefront is never locked: the page is there to be worked on, and nothing leads to it. |
| `brand` | [Brand](#brand) |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

### Address

A postal address. Every field is optional.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `first_name` | string |  |
| `last_name` | string |  |
| `company` | string |  |
| `address1` | string |  |
| `address2` | string |  |
| `city` | string |  |
| `province` | string | Province or state name, e.g. `"Ontario"`. |
| `province_code` | string | E.g. `"ON"`. |
| `country` | string | Country name, e.g. `"Canada"`. |
| `country_code` | string | ISO 3166-1 alpha-2 code, e.g. `"CA"`. |
| `zip` | string |  |
| `phone` | string |  |

### Policies

The store policies. Each one that is set gets a page under `/policies/`.

| Field | Type | Description |
|---|---|---|
| `privacy_policy` | [Policy](#policy) |  |
| `refund_policy` | [Policy](#policy) |  |
| `shipping_policy` | [Policy](#policy) |  |
| `terms_of_service` | [Policy](#policy) |  |
| `subscription_policy` | [Policy](#policy) |  |

### Policy

A store policy (`shop.refund_policy`, `/policies/refund-policy`).

| Field | Type | Description |
|---|---|---|
| `title` | string | Defaults to the standard title of the policy. |
| `body` | string | **Required.** HTML body. |

### Brand

The brand assets of the store (`shop.brand`).

| Field | Type | Description |
|---|---|---|
| `slogan` | string |  |
| `short_description` | string |  |
| `logo` | [Image](#image) |  |
| `square_logo` | [Image](#image) |  |
| `cover_image` | [Image](#image) |  |
| `favicon` | [Image](#image) |  |
| `colors` | any JSON value | Brand colors as CSS hex strings, e.g. `{"primary": ["#112233"], "secondary": []}`. |

### Image

An image: either just its source, or an object with more detail.

The source is a path inside the `files/` directory of the data directory
(`"shirt.jpg"` → `files/shirt.jpg`). When no such file exists a placeholder of the declared
size is generated, so fixtures work without shipping real images.

One of:

- string
- [ImageDetail](#imagedetail)

### ImageDetail

An image with its metadata.

| Field | Type | Description |
|---|---|---|
| `src` | string | **Required.** Path of the image inside `files/`, e.g. `"products/shirt-front.jpg"`. |
| `alt` | string | Alternative text. Defaults to an empty string. |
| `width` | integer | Width in pixels. Read from the file when it exists, otherwise defaults to 1200. |
| `height` | integer | Height in pixels. Read from the file when it exists, otherwise defaults to 1200. |
| `id` | integer |  |
| `focal_point` | [FocalPoint](#focalpoint) | The focal point as percentages from the top-left corner, e.g. `{"x": 50, "y": 20}`. |

### FocalPoint

The point of an image that should stay visible when it is cropped.

| Field | Type | Description |
|---|---|---|
| `x` | number | **Required.** Horizontal position, 0 (left) to 100 (right). |
| `y` | number | **Required.** Vertical position, 0 (top) to 100 (bottom). |

### Metafield

A metafield value. Write the value directly and its type is inferred (string →
`single_line_text_field`, integer → `number_integer`, decimal → `number_decimal`, boolean →
`boolean`, anything else → `json`), or spell out `{"type": ..., "value": ...}`.

One of:

- [TypedMetafield](#typedmetafield)
- any JSON value

### TypedMetafield

A metafield with an explicit type.

| Field | Type | Description |
|---|---|---|
| `type` | string | **Required.** A Shopify metafield type, e.g. `single_line_text_field`, `multi_line_text_field`, `rich_text_field`, `number_integer`, `number_decimal`, `boolean`, `color`, `date`, `date_time`, `url`, `json`, `money`, `rating`, `weight`, `volume`, `dimension` and the other measurements (`distance`, `temperature`, `duration`...), `product_reference`, `collection_reference`, `page_reference`, `article_reference` (`blog/article`, or the article's handle alone), `file_reference`, `metaobject_reference`, or a `list.` variant of those. |
| `value` | any JSON value | **Required.** The value. References are written as handles (`"blue-shirt"`), files as paths inside `files/`, lists as JSON arrays. |

### Product

A product.

The smallest valid product is `{"title": "Blue shirt", "price": 1999}`: it gets a single
default variant, like a product without options in the Shopify admin.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/product.schema.json"`. |
| `id` | integer | Generated from the handle when omitted. |
| `title` | string | **Required.** |
| `handle` | string | The URL slug (`/products/<handle>`). Derived from the title when omitted. In a `products/<name>.json` file it defaults to the file name. |
| `description` | string | HTML description. Also exposed as `product.content`. |
| `vendor` | string |  |
| `type` | string | The product type, e.g. `"Shirts"`. |
| `tags` | array of string |  |
| `options` | array of [Option](#option) | Option names, e.g. `["Size", "Color"]`. At most three. Omit for a product with a single default variant. |
| `variants` | array of [Variant](#variant) | The variants. Each must give one value per option. |
| `price` | [Money](#money) | Price of the variants that do not set their own, and of the default variant. |
| `compare_at_price` | [Money](#money) | Compare-at price of the variants that do not set their own. |
| `images` | array of [Image](#image) | Shorthand for image-only media. Use `media` for videos and 3D models. |
| `media` | array of [Media](#media) | Images, videos and models in display order. When set, `images` must be empty. |
| `available` | boolean | Shorthand to set availability on every variant that does not say otherwise. |
| `collections` | array of string | Handles of the collections this product belongs to, in addition to the collections that list it in their own `products`. |
| `template_suffix` | string | Selects `templates/product.<suffix>.json` instead of `templates/product.json`. |
| `gift_card` | boolean | Whether the product is a gift card (`product.gift_card?`). |
| `created_at` | string |  |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |
| `recommendations` | array of string | Handles of the products returned by product recommendations for this product. Defaults to other products from the same collections. |
| `selling_plan_groups` | array of string | Names of the selling plan groups (from `selling_plan_groups`) this product is sold with, in addition to the groups that list it in their own `products`. |
| `requires_selling_plan` | boolean | Whether the product can only be bought with a selling plan, like a product sold by subscription only. Defaults to `false`. |

### Option

A product option. Write just the name (`"Size"`); the values are collected from the variants.

One of:

- string
- [OptionDetail](#optiondetail)

### OptionDetail

A product option with its values in a chosen order.

| Field | Type | Description |
|---|---|---|
| `name` | string | **Required.** |
| `values` | array of [OptionValue](#optionvalue) | The values in display order. Defaults to the order they appear in the variants. A value is its name (`"White"`), or its name with a swatch (`{"name": "White", "swatch": "#ffffff"}`). |

### OptionValue

A value of a product option: its name, or its name with a swatch.

One of:

- string
- [OptionValueDetail](#optionvaluedetail)

### OptionValueDetail

A value of a product option with its swatch (`product_option_value.swatch`).

| Field | Type | Description |
|---|---|---|
| `name` | string | **Required.** |
| `swatch` | [Swatch](#swatch) | The swatch of the value on this product. Defaults to the swatch the store's `swatches` give to this name. |

### Swatch

How an option value is shown instead of its name: a CSS color (`"#f4f1ea"`), or an object
with a color, an image, or both.

One of:

- string
- [SwatchDetail](#swatchdetail)

### SwatchDetail

A swatch with a color, an image, or both (`swatch.color`, `swatch.image`).

| Field | Type | Description |
|---|---|---|
| `color` | string | A CSS color: `"#f4f1ea"`, `"rgb(244, 241, 234)"`, ... |
| `image` | [Image](#image) | An image, for a pattern that a single color cannot show. |

### Variant

A product variant.

| Field | Type | Description |
|---|---|---|
| `id` | integer | Generated from the product handle and the variant's position when omitted. |
| `title` | string | Defaults to the option values joined with ` / ` (`"Small / Blue"`). |
| `options` | array of string | One value per product option, in the order of the product's `options`. |
| `price` | [Money](#money) | Defaults to the product's `price`. |
| `compare_at_price` | [Money](#money) | The price before a sale. Must be higher than `price` to show as a discount. |
| `sku` | string | Also a convenient way to reference the variant from carts and orders. |
| `barcode` | string |  |
| `available` | boolean | Whether the variant can be bought. Derived from the inventory fields when omitted. |
| `inventory_quantity` | integer | Units in stock. Setting it turns inventory tracking on. |
| `inventory_tracked` | boolean | Set to `false` to disable inventory tracking (the variant is then always available). Defaults to `true` when `inventory_quantity` is set, `false` otherwise. |
| `inventory_policy` | [InventoryPolicy](#inventorypolicy) | Defaults to `"deny"`. |
| `weight` | integer | Weight in grams. |
| `weight_unit` | string | The unit the weight is displayed in: `g`, `kg`, `oz` or `lb`. Defaults to `kg`. |
| `requires_shipping` | boolean | Defaults to `true`. |
| `taxable` | boolean | Defaults to `true`. |
| `image` | string | The `src` of the product image that shows this variant. |
| `unit_price` | [Money](#money) |  |
| `unit_price_measurement` | [UnitPriceMeasurement](#unitpricemeasurement) |  |
| `quantity_rule` | [QuantityRule](#quantityrule) |  |
| `quantity_price_breaks` | array of [QuantityPriceBreak](#quantitypricebreak) |  |
| `store_availabilities` | map of boolean | The locations (from `locations`, by name) that stock this variant, and whether it is in stock there: `{"Portland": true, "Seattle": false}`. When omitted, every location stocks the variant, and it is in stock there when the variant is available. |
| `metafields` | map of map of [Metafield](#metafield) |  |

### Money

An amount of money: an integer number of cents (`1999`, the unit Liquid uses) or a decimal string in the shop currency (`"19.99"`). Floats such as `19.99` are rejected because they are ambiguous.

One of:

- integer: Cents, e.g. 1999 for 19.99.
- string: Decimal amount, e.g. "19.99" or "20".

### InventoryPolicy

What happens when a tracked variant is out of stock.

One of:

- `"deny"`: Stop selling when the stock reaches zero.
- `"continue"`: Keep selling when out of stock.

### UnitPriceMeasurement

What a unit price refers to, e.g. 250 g priced per 100 g.

| Field | Type | Description |
|---|---|---|
| `measured_type` | string | `weight`, `volume`, `length` or `area`. |
| `quantity_value` | number | **Required.** |
| `quantity_unit` | string | **Required.** E.g. `g`, `kg`, `ml`, `l`, `m`. |
| `reference_value` | number |  |
| `reference_unit` | string |  |

### QuantityRule

The quantities a variant can be bought in (`variant.quantity_rule`).

| Field | Type | Description |
|---|---|---|
| `min` | integer | Minimum quantity. Defaults to 1. |
| `max` | integer | Maximum quantity. No maximum when omitted. |
| `increment` | integer | Quantities must be a multiple of this. Defaults to 1. |

### QuantityPriceBreak

A volume price: the unit price from a given quantity.

| Field | Type | Description |
|---|---|---|
| `minimum_quantity` | integer | **Required.** |
| `price` | [Money](#money) | **Required.** |

### Media

A piece of product media. Images can be written as a plain string.

One of:

- [Image](#image)
- [MediaDetail](#mediadetail)

### MediaDetail

A video, an external (YouTube/Vimeo) video or a 3D model.

| Field | Type | Description |
|---|---|---|
| `media_type` | [MediaType](#mediatype) | **Required.** |
| `id` | integer |  |
| `alt` | string |  |
| `preview_image` | [Image](#image) | The image shown before the media plays. |
| `sources` | array of [MediaSource](#mediasource) | `video` and `model`: the files, by decreasing preference. |
| `duration` | integer | `video`: length in milliseconds. |
| `host` | [VideoHost](#videohost) | `external_video`: `youtube` or `vimeo`. |
| `external_id` | string | `external_video`: the id of the video on the host, e.g. `dQw4w9WgXcQ`. |
| `aspect_ratio` | number | Width divided by height. Defaults to the preview image's ratio, or 16:9. |

### MediaType

The kind of a piece of media that is not an image.

One of: `"video"`, `"external_video"`, `"model"`.

### MediaSource

One file of a video or 3D model.

| Field | Type | Description |
|---|---|---|
| `url` | string | **Required.** Path inside `files/` or an absolute URL. |
| `mime_type` | string | E.g. `video/mp4`, `model/gltf-binary`. |
| `format` | string | E.g. `mp4`, `m3u8`, `glb`, `usdz`. |
| `width` | integer |  |
| `height` | integer |  |

### VideoHost

Where an external video is hosted.

One of: `"youtube"`, `"vimeo"`.

### Collection

A collection of products.

A collection named `all` containing every product always exists; define it yourself only
to change its title or order.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/collection.schema.json"`. |
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `description` | string | HTML description. |
| `image` | [Image](#image) |  |
| `products` | array of string | Handles of the products in the collection, in manual order. |
| `rules` | array of [CollectionRule](#collectionrule) | Makes the collection automated: products matching the rules are added after the ones listed in `products`. |
| `disjunctive` | boolean | With several rules: `true` to match any of them, `false` (default) to match all. |
| `sort_order` | [SortOrder](#sortorder) | The default order of the products. Defaults to `manual`. |
| `template_suffix` | string | Selects `templates/collection.<suffix>.json`. |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

### CollectionRule

A rule of an automated collection: products matching it are included.

| Field | Type | Description |
|---|---|---|
| `column` | [RuleColumn](#rulecolumn) | **Required.** What to look at: `tag`, `type`, `vendor`, `title`, `price` (cents) or `compare_at_price` (cents). |
| `relation` | [RuleRelation](#rulerelation) | Defaults to `equals`. |
| `condition` | string | **Required.** The value compared with, e.g. `"sale"` for a tag or `"5000"` for a price in cents. |

### RuleColumn

The product property an automated collection rule looks at.

One of: `"tag"`, `"type"`, `"vendor"`, `"title"`, `"price"`, `"compare_at_price"`.

### RuleRelation

How an automated collection rule compares the property to its condition.

One of: `"equals"`, `"not_equals"`, `"contains"`, `"not_contains"`, `"starts_with"`, `"ends_with"`, `"greater_than"`, `"less_than"`.

### SortOrder

How a collection orders its products by default.

One of: `"manual"`, `"best-selling"`, `"title-ascending"`, `"title-descending"`, `"price-ascending"`, `"price-descending"`, `"created-ascending"`, `"created-descending"`.

### Page

A page (`/pages/<handle>`).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/page.schema.json"`. |
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `content` | string | HTML content. |
| `author` | string |  |
| `template_suffix` | string | Selects `templates/page.<suffix>.json`, e.g. `"contact"`. |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

### Blog

A blog (`/blogs/<handle>`) and its articles.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/blog.schema.json"`. |
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `articles` | array of [Article](#article) | Articles, newest first unless they carry `published_at` dates. |
| `comments_enabled` | boolean | Whether readers can comment. Defaults to `false`. |
| `moderated` | boolean | Whether comments wait for approval. Defaults to `false`. |
| `template_suffix` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

### Article

A blog post.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `author` | string |  |
| `content` | string | HTML content. |
| `excerpt` | string | HTML excerpt. `article.excerpt_or_content` falls back to the content. |
| `image` | [Image](#image) |  |
| `tags` | array of string |  |
| `comments` | array of [Comment](#comment) |  |
| `template_suffix` | string |  |
| `created_at` | string |  |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

### Comment

A comment on an article.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `author` | string | **Required.** |
| `email` | string |  |
| `content` | string | **Required.** HTML content. |
| `created_at` | string |  |

### Menu

A navigation menu (`linklists.<handle>`).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/menu.schema.json"`. |
| `title` | string | Defaults to the handle, title-cased. |
| `links` | array of [Link](#link) |  |

### Link

A link in a navigation menu.

Point it at a resource with exactly one of `url`, `collection`, `product`, `page`, `blog`,
`article` or `policy`; the URL and the link type follow from it. A link with none of them
points to `#`.

| Field | Type | Description |
|---|---|---|
| `title` | string | **Required.** |
| `url` | string | A literal URL: `/`, `/collections/all`, `/search`, `https://example.com`. |
| `collection` | string | Handle of a collection. |
| `product` | string | Handle of a product. |
| `page` | string | Handle of a page. |
| `blog` | string | Handle of a blog. |
| `article` | string | An article as `<blog-handle>/<article-handle>`. |
| `policy` | string | A policy: `privacy-policy`, `refund-policy`, `shipping-policy`, `terms-of-service`. |
| `links` | array of [Link](#link) | Nested links (up to three levels, like the Shopify admin). |

### Customer

A customer. Sessions log in as a customer by email (see `session.customer`), and the checkout
of an app lists the customers as login profiles: "Log in" logs the checkout in as one of them,
with their addresses (the first is the default) and, for a B2B customer, their locations.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/customer.schema.json"`. |
| `id` | integer |  |
| `email` | string | **Required.** |
| `first_name` | string |  |
| `last_name` | string |  |
| `phone` | string |  |
| `tags` | array of string |  |
| `accepts_marketing` | boolean |  |
| `has_account` | boolean | Defaults to `true`. |
| `tax_exempt` | boolean |  |
| `addresses` | array of [Address](#address) | The first address is the default one (`customer.default_address`). |
| `orders` | array of [Order](#order) |  |
| `company` | string | The name of a company in `companies`: the customer buys for it (B2B). |
| `company_locations` | array of string | The names of the locations of the company the customer can buy for. Defaults to all of them. |
| `company_contact` | [CompanyContact](#companycontact) | What the company knows of the customer as a buyer: their job title, language, dates. Checkout Functions read it as `purchasingCompany.contact`. |
| `metafields` | map of map of [Metafield](#metafield) |  |

### Order

An order of a customer, shown on account pages.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `name` | string | The order name, e.g. `"#1001"`. Generated when omitted. |
| `created_at` | string |  |
| `financial_status` | string | `paid`, `pending`, `refunded`, ... Defaults to `paid`. |
| `fulfillment_status` | string | `fulfilled`, `unfulfilled`, `partial`. Defaults to `unfulfilled`. |
| `line_items` | array of [OrderLine](#orderline) |  |
| `shipping_address` | [Address](#address) |  |
| `billing_address` | [Address](#address) |  |
| `shipping_price` | [Money](#money) |  |
| `tax_price` | [Money](#money) |  |
| `cancelled` | boolean |  |
| `note` | string |  |
| `returns` | array of [OrderReturn](#orderreturn) | What the customer sent back, as the customer account shows it. |

### OrderLine

A line of an order.

| Field | Type | Description |
|---|---|---|
| `variant` | [VariantRef](#variantref) | **Required.** A variant, by id or SKU, or a product handle (its first variant is used). |
| `quantity` | integer | Defaults to 1. |

### VariantRef

A reference to a variant: its numeric id, its SKU, or a product handle (meaning that
product's first variant).

One of:

- integer
- string

### OrderReturn

A return of an order.

| Field | Type | Description |
|---|---|---|
| `status` | string | `requested`, `open` or `closed`. Defaults to `open`. |
| `line_items` | array of [OrderLine](#orderline) | The lines sent back, by their variant, and how many of each. Defaults to every line of the order. |

### CompanyContact

A customer as a contact of their company (B2B): `purchasingCompany.contact` of a checkout
Function.

| Field | Type | Description |
|---|---|---|
| `title` | string | The job title of the contact, e.g. `"Purchasing manager"`. |
| `locale` | string | The language of the contact, e.g. `"fr"`. None when absent. |
| `created_at` | string | Defaults to the creation date of the company. |
| `updated_at` | string | Defaults to `created_at`. |

### Company

A company that buys from the store (B2B). A customer whose `company` names it is a B2B
customer: `customer.b2b?` is true, and `customer.current_company` is this company.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `name` | string | **Required.** The name customers refer to the company by. |
| `external_id` | string | The id the merchant gives the company in its own systems. |
| `locations` | array of [CompanyLocation](#companylocation) | The places the company buys for. The first one is selected when a customer logs in. |
| `created_at` | string | Defaults to a fixed date, 2024-01-01 plus one day per company. |
| `updated_at` | string | Defaults to `created_at`. |
| `metafields` | map of map of [Metafield](#metafield) |  |

### CompanyLocation

A place a company buys for (`customer.current_location`): a branch, a shop, a warehouse.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `name` | string | **Required.** |
| `external_id` | string | The id the merchant gives the location in its own systems. |
| `shipping_address` | [Address](#address) | Where orders of this location ship to. |
| `tax_registration_id` | string | The tax number of the location, e.g. a VAT number. |
| `locale` | string | The preferred language of the location, e.g. `"fr"`. None when absent. |
| `created_at` | string | Defaults to the creation date of the company. |
| `updated_at` | string | Defaults to `created_at`. |
| `orders_count` | integer | The number of orders placed at the location, which checkout Functions read as `ordersCount`. The orders of the customers are not counted: defaults to 0. |
| `total_spent` | [Money](#money) | The amount spent at the location (`totalSpent`), e.g. `"1250.00"`. Defaults to 0. |
| `metafields` | map of map of [Metafield](#metafield) |  |

### GiftCard

An issued gift card, shown by `templates/gift_card.liquid` at the URL the `routes` command lists
(`/gift_cards/<shop id>/<token>`).

| Field | Type | Description |
|---|---|---|
| `code` | string | **Required.** The code customers redeem at checkout, e.g. `"WCGX7X97G74JGDGC"`. |
| `initial_value` | [Money](#money) | **Required.** The value the card was issued with. |
| `balance` | [Money](#money) | What is left on the card. Defaults to `initial_value`. |
| `currency` | string | ISO 4217 code. Defaults to the shop's currency. |
| `expires_on` | string | The day the card stops working, e.g. `"2026-12-31"`. A date before `now` makes the card `expired`. |
| `enabled` | boolean | `false` for a card the merchant disabled. |
| `customer` | string | The email of the customer (from `customers`) the card belongs to. |
| `message` | string |  |
| `recipient` | [GiftCardRecipient](#giftcardrecipient) |  |
| `send_on` | string | When the card is scheduled to be sent to its recipient. |
| `product` | string | The handle of the gift card product that was bought. |
| `properties` | map of string | Line item properties of the purchase. |
| `template_suffix` | string | Selects `templates/gift_card.<suffix>.liquid`. |

### GiftCardRecipient

Who a gift card was sent to.

| Field | Type | Description |
|---|---|---|
| `name` | string |  |
| `email` | string |  |
| `nickname` | string |  |

### Metaobject

A metaobject entry (`metaobjects.<type>.<handle>`).

| Field | Type | Description |
|---|---|---|
| `handle` | string | **Required.** |
| `fields` | map of [Metafield](#metafield) | Field values, with the same shorthand as metafields. |
| `template_suffix` | string | Selects `templates/metaobject/<type>.<suffix>.json` instead of `templates/metaobject/<type>.json` for the page of the entry, which is `/pages/<type>/<handle>`. |

### Localization

The countries and languages the store sells in (the `localization` object).

The first language is the primary one and is served at `/`; the others are served under
`/<iso_code>/`. Defaults to one country (`US`) and the theme's default language.

| Field | Type | Description |
|---|---|---|
| `countries` | array of [Country](#country) |  |
| `languages` | array of [Language](#language) |  |
| `markets` | array of [Market](#market) | Groups of countries sold as one market. A country in none of them is a market of its own, named after the country. |

### Country

A country the store sells to.

| Field | Type | Description |
|---|---|---|
| `iso_code` | string | **Required.** ISO 3166-1 alpha-2 code, e.g. `"FR"`. |
| `name` | string | Defaults to the English name of the country. |
| `currency` | string | ISO 4217 currency code. Defaults to the store currency. |
| `unit_system` | string | `metric` or `imperial`. Defaults to `metric`. |
| `popular` | boolean | Whether to list the country among the popular ones. |

### Language

A language the store is published in.

| Field | Type | Description |
|---|---|---|
| `iso_code` | string | **Required.** Locale code, e.g. `"en"`, `"fr"`, `"pt-BR"`. |
| `name` | string | The language's name in English. Defaults to a built-in name. |
| `endonym_name` | string | The language's name in that language. Defaults to a built-in name. |

### Market

A market: the countries the store sells to as one, as checkout Functions see it in
`localization.market` and `MailingAddress.market`.

| Field | Type | Description |
|---|---|---|
| `handle` | string | **Required.** What Functions call the market: `"europe"`. |
| `countries` | array of string | **Required.** The ISO 3166-1 alpha-2 codes of its countries, which are in `localization.countries`. |
| `metafields` | map of map of [Metafield](#metafield) |  |

### Location

A place the store stocks products at, where customers can pick up their orders
(`variant.store_availabilities`, `store_availability.location`).

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `name` | string | **Required.** The name of the location. Variants refer to it by this name. |
| `address` | [Address](#address) |  |
| `pick_up_enabled` | boolean | Whether orders can be picked up at the location. Defaults to `true`. |
| `pick_up_time` | string | How long a pickup order takes to be ready. Defaults to `"Usually ready in 24 hours"`. |
| `physical_storefront` | boolean | Whether the location is a shop that sells in person. Defaults to `true`. |
| `latitude` | number |  |
| `longitude` | number |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

### SellingPlanGroup

A group of selling plans (`product.selling_plan_groups`): what a subscription app creates
for "Subscribe and save". Products are sold with a group when the group lists them in
`products`, or when they name the group in their own `selling_plan_groups`.

| Field | Type | Description |
|---|---|---|
| `name` | string | **Required.** The name shown to customers, e.g. `"Subscribe and save"`. Products refer to the group by this name. |
| `app_id` | string | The id of the app that created the group (`selling_plan_group.app_id`). |
| `options` | array of string | The names of the options the plans differ by. Defaults to one option, `"Delivery frequency"`. |
| `selling_plans` | array of [SellingPlan](#sellingplan) | **Required.** The plans of the group. At least one. |
| `products` | array of string | Handles of the products sold with these plans. |

### SellingPlan

A selling plan: one way of buying a product, such as "Deliver every month, 10% off".

| Field | Type | Description |
|---|---|---|
| `id` | integer | Generated from the names of the group and of the plan when omitted. |
| `name` | string | **Required.** The name shown to customers, e.g. `"Deliver every month, 10% off"`. |
| `description` | string |  |
| `options` | array of string | One value per option of the group, e.g. `["Every month"]`. Defaults to the plan's name. |
| `price_adjustments` | array of [PriceAdjustment](#priceadjustment) | How the plan changes the price: at most two adjustments, the first one applies to the first orders. No adjustment means the price of the variant. |
| `recurring_deliveries` | boolean | Whether the plan delivers more than once. Defaults to `true`. |
| `checkout_charge` | [CheckoutCharge](#checkoutcharge) | What is paid at checkout. Defaults to the whole price (`percentage`, `100`). |

### PriceAdjustment

A price adjustment of a selling plan (`selling_plan.price_adjustments`).

| Field | Type | Description |
|---|---|---|
| `value_type` | [AdjustmentType](#adjustmenttype) | **Required.** |
| `value` | [Money](#money) | **Required.** With `percentage`: the percentage, e.g. `10`. With `fixed_amount` and `price`: an amount of money, e.g. `"2.00"` or `200`. |
| `order_count` | integer | The number of orders the adjustment applies to. Every order when omitted. |

### AdjustmentType

How a price adjustment of a selling plan changes the price.

One of:

- `"percentage"`: `value` percent off the price of the variant.
- `"fixed_amount"`: `value` (an amount of money) off the price of the variant.
- `"price"`: `value` (an amount of money) is the price.

### CheckoutCharge

What is paid at checkout, for plans that take a deposit (`selling_plan.checkout_charge`).

| Field | Type | Description |
|---|---|---|
| `value_type` | [CheckoutChargeType](#checkoutchargetype) | **Required.** |
| `value` | [Money](#money) | **Required.** With `percentage`: the percentage, e.g. `50`. With `price`: an amount of money. |

### CheckoutChargeType

How the amount due at checkout is expressed.

One of:

- `"percentage"`: `value` percent of the price.
- `"price"`: `value` (an amount of money).

### File

A file in `files/` that needs metadata. Files without an entry work too.

| Field | Type | Description |
|---|---|---|
| `alt` | string |  |
| `width` | integer | Size to assume when the file itself is absent (a placeholder is generated). |
| `height` | integer |  |
| `focal_point` | [FocalPoint](#focalpoint) |  |

### Session

The state a browser session starts with. Tests can replace it per session through the
control API (`PUT /__lsf/session`).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/session.schema.json"`. |
| `customer` | string | Who is logged in: the email of a customer, `"default"` for the first customer of the data, or `"none"`. Nobody is logged in when omitted. |
| `cart` | [Cart](#cart) | The content of the cart. Empty when omitted. |
| `country` | string | ISO code of the selected country. Defaults to the first country. |
| `company_location` | string | For a B2B customer: the name of the company location they buy for. Defaults to the first one they have access to. |

### Cart

The cart a session starts with.

| Field | Type | Description |
|---|---|---|
| `items` | array of [CartLine](#cartline) |  |
| `note` | string |  |
| `attributes` | map of string | Cart attributes. |

### CartLine

A line of the cart a session starts with.

| Field | Type | Description |
|---|---|---|
| `variant` | [VariantRef](#variantref) | **Required.** A variant, by id or SKU, or a product handle (its first variant is used). |
| `quantity` | integer | Defaults to 1. |
| `properties` | map of string | Line item properties, e.g. `{"Engraving": "Hello"}`. |
| `selling_plan` | [SellingPlanRef](#sellingplanref) | The selling plan the line is bought with: its name or its id. The product of the variant must be sold with it. |

### SellingPlanRef

A reference to a selling plan: its numeric id, or its name.

One of:

- integer
- string

### Checkout

How the local checkout (`lsf checkout --app`) is set up. Every field has a default.

| Field | Type | Description |
|---|---|---|
| `shipping_rates` | array of [CheckoutShippingRate](#checkoutshippingrate) | The delivery options offered to every address. Defaults to one, "Standard" at 5.00. |
| `payment_methods` | array of [CheckoutPaymentMethod](#checkoutpaymentmethod) | The payment methods. Defaults to "Credit card", "Cash on Delivery" and "Bank Deposit". |
| `discounts` | array of [CheckoutDiscount](#checkoutdiscount) | Discounts backed by discount Functions of the app. |
| `functions` | map of [CheckoutFunction](#checkoutfunction) | The settings of the other Functions, by the handle of the Function. |
| `tax_rate` | number | Tax in percent, added on top of the discounted subtotal: `20` for 20 %. No tax when omitted. |

### CheckoutShippingRate

A delivery option the local checkout offers to every address.

| Field | Type | Description |
|---|---|---|
| `handle` | string | **Required.** What Functions and the page call the option: `"standard"`. |
| `title` | string | What the buyer sees. Defaults to the handle. |
| `price` | [Money](#money) | **Required.** The cost, e.g. `"4.90"`. |
| `description` | string |  |
| `kind` | [CheckoutDeliveryKind](#checkoutdeliverykind) | Defaults to `"shipping"`. |

### CheckoutDeliveryKind

What kind of delivery a shipping rate is.

One of:

- `"shipping"`: Delivered to the address.
- `"pick_up"`: Picked up in a store.
- `"local"`: Delivered locally.

### CheckoutPaymentMethod

A payment method the local checkout offers.

| Field | Type | Description |
|---|---|---|
| `handle` | string | **Required.** What payment customization Functions and the page call the method: `"card"`. |
| `name` | string | What the buyer sees. Defaults to the handle. |
| `placements` | array of [CheckoutPaymentPlacement](#checkoutpaymentplacement) | Where the method is offered, which payment customization Functions read as `placements`. Defaults to `["PAYMENT_METHOD"]`: the list of the checkout. |

### CheckoutPaymentPlacement

Where a payment method is offered.

One of:

- `"PAYMENT_METHOD"`: The list of payment methods of the checkout.
- `"ACCELERATED_CHECKOUT"`: The express buttons (Shop Pay, Apple Pay, ...).

### CheckoutDiscount

A discount backed by a discount Function of the app.

| Field | Type | Description |
|---|---|---|
| `function` | string | **Required.** The handle of the discount Function in the app. |
| `title` | string | What the buyer sees. Defaults to the handle of the Function. |
| `code` | string | The code that triggers the discount. Without one the discount is automatic. |
| `classes` | array of [CheckoutDiscountClass](#checkoutdiscountclass) | What the discount may reduce. All three when omitted. |
| `metafields` | map of [Metafield](#metafield) | The metafields of the discount, which its Function's input query reads, by `"namespace.key"`: `{"$app:config.settings": {"type": "json", "value": {"percent": 10}}}`. |

### CheckoutDiscountClass

What a discount may reduce.

One of: `"PRODUCT"`, `"ORDER"`, `"SHIPPING"`.

### CheckoutFunction

The settings of a Function that is not a discount: cart transform, delivery or payment
customization, validation.

| Field | Type | Description |
|---|---|---|
| `metafields` | map of [Metafield](#metafield) | The metafields of the object the Function belongs to, by `"namespace.key"`. |

### Checkout scenario

A named setup of the local checkout: a file of `checkouts/`. It sets what a checkout
depends on on top of the store data, and what it does not set stays as the store data says.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/checkout-scenario.schema.json"`. |
| `title` | string | The name shown on the home page. Defaults to the name of the file. |
| `description` | string | What the scenario is for and what to look at, shown on the home page. |
| `session` | [Session](#session) | Who buys and what is in the cart, written as the `session` of the store data and applied on top of it: `customer`, `cart` (lines, quantities, selling plans, note, attributes), `country`, `company_location`. |
| `entered` | [ScenarioEntered](#scenarioentered) | What the buyer already entered: email, address, discount codes, delivery and payment. |
| `language` | string | The language of the buyer, by ISO code (`"fr"`): one of `localization.languages`. The checkout opens under its locale root. Defaults to the primary language. |
| `checkout` | [Checkout](#checkout) | Overrides of the `checkout` section of the store data: every field that is present replaces the one of the store (`shipping_rates`, `payment_methods`, `discounts`, `tax_rate`), except `functions`, which replaces the settings of the Functions it names. |
| `variant` | [ScenarioLook](#scenariolook) | The look of the page. Defaults to the one of the server (`--variant`). |
| `extensions` | array of string | The handles of the extensions (Functions and checkout UI extensions) that run, among those of the app that `--extension` allows. Every one when omitted. |

### ScenarioEntered

What the buyer already typed and chose when a scenario opens.

| Field | Type | Description |
|---|---|---|
| `email` | string | The contact email. A logged-in customer has theirs already. |
| `phone` | string |  |
| `shipping_address` | [ScenarioAddress](#scenarioaddress) | The delivery address. Its country is the country of the checkout. |
| `delivery` | string | The handle of the shipping rate chosen (`checkout.shipping_rates`). The first one available is chosen when omitted. |
| `payment` | string | The handle of the payment method chosen (`checkout.payment_methods`). |
| `discount_codes` | array of string | The discount codes entered, in order. A code that no discount of `checkout.discounts` has is entered all the same, and refused as on Shopify. |

### ScenarioAddress

A postal address the buyer already entered.

| Field | Type | Description |
|---|---|---|
| `first_name` | string |  |
| `last_name` | string |  |
| `company` | string |  |
| `address1` | string |  |
| `address2` | string |  |
| `city` | string |  |
| `country_code` | string | ISO 3166-1 alpha-2 code of one of the store's countries: `"US"`. |
| `province_code` | string |  |
| `zip` | string |  |
| `phone` | string |  |

### ScenarioLook

The look of the checkout page.

One of:

- `"standard"`: Shopify's one-page checkout.
- `"shop"`: The look of the Shop app and Shop Pay.

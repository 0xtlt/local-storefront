# shopify-local-theme

`slt` serves a Shopify theme from your machine, rendered from JSON fixtures instead of the
Shopify API.

```bash
cd my-theme
slt serve
# ready:  http://127.0.0.1:9292/
```

It exists for end-to-end tests. A theme tested against a development store is throttled by
Shopify and depends on whatever the store contains that day. Against `slt`, a test suite talks
to a local process that renders a page in a few milliseconds, never calls Shopify, has no rate
limit, and shows exactly the catalog, cart and customer each test asks for.

- **Same output as Shopify.** The Liquid engine is a port of Shopify's reference
  implementation, checked against it by 548 differential tests; Shopify's objects, filters and
  tags are checked against the examples of shopify.dev. Shopify's Horizon and Dawn themes
  render every page type without a Liquid error. See [compatibility](docs/compatibility.md).
- **Data as validated JSON.** Products, collections, pages, blogs, menus, customers, carts,
  metafields and metaobjects are plain JSON files. `slt validate` rejects typos, wrong types
  and broken references, and says where and how to fix them, in text or in JSON for tools and
  LLMs. See the [data format](docs/data-format.md) and its [field reference](docs/data-reference.md).
- **Local images.** `image_url` and `image_tag` produce URLs of Shopify's shape that point to
  the local server, which resizes and crops like Shopify's CDN. Missing files become
  placeholders of the right size, so fixtures do not need to ship images.
- **A storefront, not only pages.** Cart Ajax API, Section Rendering API, predictive search,
  product recommendations, collection filters and sorting, forms, localization, customer login.
- **Isolated sessions.** Each browser context has its own cart, customer and, if it wants,
  its own store data, set with one HTTP call. Tests run in parallel against one server. See
  [testing](docs/testing.md).

## Install

The toolchain is managed by [mise](https://mise.jdx.dev):

```bash
mise install
```

```bash
mise run install
```

The second command builds `slt` and puts it in `~/.cargo/bin`. Without mise:
`cargo install --path crates/cli` with Rust 1.98 or later.

## Quick start

To see it work on Shopify's Horizon theme with the built-in demo store:

```bash
mise run horizon:serve
```

On your own theme:

```bash
cd my-theme
```

```bash
slt init
```

```bash
slt serve --live-reload
```

`slt init` creates `shopify-local/` in the theme with a demo store to edit, JSON Schemas for
editor completion, and the guide of the format. The directory is not one of the theme's own
(`assets/`, `sections/`, ...), so it is not uploaded with the theme.

Edit the data, then check it:

```bash
slt validate
```

```text
error[unknown_field]: unknown field "titel"
  --> products/shirt.json at /titel
  hint: did you mean "title"?

error[unknown_collection]: there is no collection with the handle "aparel"
  --> products/shirt.json at /collections/0
  hint: did you mean "apparel"?
```

The smallest useful data directory is one file:

```json
{
  "shop": { "name": "My Shop", "currency": "EUR" },
  "products": [
    { "title": "Blue shirt", "price": "19.99" },
    {
      "title": "Linen overshirt",
      "price": "89.00",
      "options": ["Size"],
      "variants": [
        { "options": ["M"], "sku": "OVS-M", "inventory_quantity": 4 },
        { "options": ["L"], "sku": "OVS-L", "inventory_quantity": 0 }
      ]
    }
  ],
  "menus": { "main-menu": { "links": [{ "title": "Shop", "collection": "all" }] } }
}
```

## Commands

| Command | What it does |
|---|---|
| `slt serve` | Serves the theme. `--port`, `--host`, `--live-reload`, `--static`, `--strict`, `--quiet`. |
| `slt render <path>` | Prints the HTML of one URL, without a server. `--section-id` renders a single section. |
| `slt validate` | Checks the store data. `--format json` for a machine-readable report. Exit status 1 on errors. |
| `slt check` | Parses every Liquid and JSON file of the theme and reports what is not supported. |
| `slt init` | Creates the data directory with a demo store, schemas and the guide. |
| `slt routes` | Lists the URLs the data gives a page to. |
| `slt schema [kind]` | Prints a JSON Schema of the format; `--out <dir>` writes them all. |
| `slt docs` | Prints the guide and the field reference of the format. |

Every command takes `--theme <dir>` (default: the current directory, or `SLT_THEME`) and
`--data <dir>` (default: `<theme>/shopify-local`, or `SLT_DATA`).

## In a test

```ts
import { test, expect } from '@playwright/test';

test('adding to the cart updates the bubble', async ({ page }) => {
  await page.request.put('/__slt/session', {
    data: { cart: { items: [{ variant: 'OVS-M', quantity: 1 }] } },
  });
  await page.goto('/products/blue-shirt');
  await page.getByRole('button', { name: 'Add to cart' }).click();
  await expect(page.locator('.cart-bubble')).toHaveText('2');
});
```

The [testing guide](docs/testing.md) covers server flags, sessions, per-test data, what to
assert on and CI.

## Documentation

- [Store data format](docs/data-format.md): the guide. Also `slt docs`.
- [Field reference](docs/data-reference.md): every field, generated from the JSON Schema.
- [Testing](docs/testing.md): end-to-end tests, the control API.
- [Compatibility](docs/compatibility.md): what is covered, and the known differences.
- [Architecture](docs/architecture.md): how the code is organised, and how to work on it.

## Limits

`slt` renders themes. It does not simulate checkout, apps, discounts, selling plans, B2B,
taxes or shipping rates, and customer registration is not simulated. The full list is in
[compatibility](docs/compatibility.md#known-differences).

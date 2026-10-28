# local-storefront

Run a Shopify theme on your machine, without Shopify.

local-storefront reads your theme and a few JSON files (products, cart, customers) and serves
the storefront locally: no development store, no API calls, no rate limit. Every test gets the
exact store it needs, in milliseconds. The HTML is the HTML Shopify renders.

## Quick start

In your theme folder:

```bash
npm install --save-dev local-storefront
```

```bash
npx local-storefront serve
```

Open <http://127.0.0.1:9292>: your theme runs with a demo store.

> Write `npx local-storefront`, not `npx lsf`: `lsf` is another package on npm. In the
> `scripts` of `package.json`, `local-storefront serve` is enough.

## What you can do

### Use your own store data

```bash
npx local-storefront init
```

Creates `shopify-local/` in your theme, with a demo store to edit. A store is plain JSON:

```json
{
  "shop": { "name": "My Shop", "currency": "EUR" },
  "products": [{ "title": "Blue shirt", "price": "19.99" }]
}
```

Products, collections, pages, blogs, menus, customers, B2B companies, gift cards,
subscriptions, store pickup, swatches, metafields and metaobjects. Check what you wrote:

```bash
npx local-storefront validate
```

```text
error[unknown_field]: unknown field "titel"
  --> products/shirt.json at /titel
  hint: did you mean "title"?
```

Guide: [data format](docs/data-format.md).

### Run end-to-end tests

Each test gets its own cart and customer, so tests run in parallel. One request puts a
session in the state a test needs:

```ts
await page.request.put('/__lsf/session', {
  data: {
    customer: 'jane.doe@example.com',
    cart: { items: [{ variant: 'TEE-BLK-M', quantity: 2 }] },
  },
});
await page.goto('/cart');
```

Guide: [testing](docs/testing.md).

### Try the checkout of a Shopify app

```bash
npx local-storefront checkout --app ../my-app
```

Runs the Shopify Functions of the app and shows its checkout UI extensions on a local checkout,
without a store. Its home page lists what is wrong first (including the Functions that fail when
the scenarios run), then the checkout scenarios (carts, buyers, countries, settings) and every
Function and extension of the app, and opens a checkout for each:
`npx local-storefront checkout init --app ../my-app` writes a set made for the app.
`npx local-storefront function run --app ../my-app` runs the Functions once and prints their
input and output. Guide: [checkout](docs/checkout.md).

### Get ready for Shopify's strict Liquid parser

Shopify is moving every store to a strict Liquid parser, and a file it rejects can no longer be
uploaded. `npx local-storefront check` lists that markup with Shopify's message, and `--fix`
rewrites it:

```bash
npx local-storefront check --fix
```

```text
snippets/card.liquid:1: fixed: Shopify's strict Liquid parser rejects this {% render %}: Expected end_of_string but found pipe
  was: … alt: image.alt | default: shop.name
  now: … alt: image_alt
  before it: assign image_alt = image.alt | default: shop.name
  changes the output: `alt` gets `image.alt | default: shop.name`
```

Guide: [strict Liquid](docs/strict-liquid.md).

### Format your theme

```bash
npx local-storefront fmt
```

Formats every `.liquid` file as Prettier does with Shopify's Liquid plugin, without Node.
`npx local-storefront fmt --check` is the one for CI. Guide: [formatting](docs/formatting.md).

### Find what makes a page slow

```bash
npx local-storefront profile /collections/all
```

Shows every section, block and snippet the page renders, with its time, or with `--points`
what it would cost Shopify. Guide: [performance](docs/performance.md).

## Commands

| Command | What it does |
|---|---|
| `npx local-storefront serve` | Serves the theme at <http://127.0.0.1:9292>. |
| `npx local-storefront init` | Creates `shopify-local/` with a demo store to edit. |
| `npx local-storefront validate` | Checks the store data. `--format json` for tools. |
| `npx local-storefront check` | Checks every Liquid file, and what Shopify's strict parser rejects. `--fix` rewrites it. `--format json` for tools. |
| `npx local-storefront fmt [paths]` | Formats Liquid files as Prettier does. `--check` for CI. |
| `npx local-storefront checkout --app <dir>` | Serves the checkout of a Shopify app: its Functions run, its checkout UI extensions show. `serve --app` does it with the theme. |
| `npx local-storefront checkout init [--app <dir>]` | Writes checkout scenarios (`checkouts/` in the store data) made for the Functions and extensions of the app. |
| `npx local-storefront function run [handle]... --app <dir>` | Runs the Functions of an app once and prints each input, output and log. `--scenario <name>` runs a checkout scenario. `--format json` for tools. |
| `npx local-storefront render <path>` | Prints the HTML of one page, without a server. |
| `npx local-storefront profile <path>` | Shows what a page spends its time on, or what it would cost Shopify. |
| `npx local-storefront routes` | Lists the pages the store data creates. |
| `npx local-storefront schema [kind]` | Prints the JSON Schema of the store data. |
| `npx local-storefront docs` | Prints the data guide. |

Every option, exit code and output format: [CLI reference](docs/cli.md).

## Documentation

| I want to... | Read |
|---|---|
| Write store data | [Data format](docs/data-format.md), then the [field reference](docs/data-reference.md) |
| Write end-to-end tests | [Testing](docs/testing.md) and the [control API](docs/control-api.md) |
| Fix what Shopify's strict parser rejects | [Strict Liquid](docs/strict-liquid.md) |
| Format Liquid files | [Formatting](docs/formatting.md) |
| Make a page faster | [Performance](docs/performance.md) |
| Run the Functions and checkout extensions of a Shopify app | [Checkout](docs/checkout.md) |
| Use the storefront in a browser | [Storefront](docs/storefront.md): accounts, password page, live reload, previews |
| Know an option or an exit code | [CLI reference](docs/cli.md) |
| Drive local-storefront from an AI agent | [Agents](docs/agents.md) |
| Know what matches Shopify | [Compatibility](docs/compatibility.md) |
| Work on local-storefront itself | [Architecture](docs/architecture.md) |

What changed in each version: [changelog](CHANGELOG.md).

## What it does not do

local-storefront renders themes. It does not simulate Shopify's checkout, apps, B2B catalogs, or
discounts and shipping rates in the theme's cart. The checkout of an app is a local stand-in: it
runs its Functions and shows its extensions, and takes no payment. Customers can log in, but not
register. The full list: [compatibility](docs/compatibility.md#known-differences).

## Other ways to install

With [mise](https://mise.jdx.dev):

```bash
mise use github:0xtlt/local-storefront
```

As a single binary, from the [releases](https://github.com/0xtlt/local-storefront/releases):
macOS, Linux and Windows, on x64 and ARM64. On Linux:

```bash
curl -fsSL https://github.com/0xtlt/local-storefront/releases/latest/download/lsf-x86_64-unknown-linux-gnu.tar.gz | sudo tar -xz -C /usr/local/bin lsf
```

From source, with Rust 1.98 or later:

```bash
cargo install --path crates/cli
```

Installed these ways, the command is `lsf`: `lsf serve` is `npx local-storefront serve`.

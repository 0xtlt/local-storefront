# local-storefront

Run Shopify themes and Shopify apps on your machine, without Shopify.

local-storefront reads your theme, your app and a few JSON files of store data. From them it
serves the storefront, the checkout of your app and a local Admin API. There is no development
store, no network and no rate limit. Every run starts from the same data, and every test gets the
store it needs in milliseconds. The HTML is the HTML Shopify renders.

It is made for three jobs:

- **Theme developers** serve, test, check, format and profile a theme.
- **App developers** run their Functions and their checkout and customer account UI extensions on
  a local checkout, and assert the results in CI.
- **Backend developers** run an app's backend against a local GraphQL Admin API (2026-10). It
  comes with OAuth, webhooks, billing and the embedded admin.

## Quick start

In your theme folder:

```bash
npm install --save-dev local-storefront
npx local-storefront serve
```

Open <http://127.0.0.1:9292>. Your theme runs with a demo store.

For a Shopify app, from anywhere:

```bash
npx local-storefront app dev --app ../my-app --app-url http://localhost:3000
```

It serves the app's checkout and its Admin API. It prints their URLs and the access token for the
backend. Its home page lists what is wrong first.

> Write `npx local-storefront`, not `npx lsf`: `lsf` is another package on npm. In the `scripts`
> of `package.json`, `local-storefront serve` is enough. A release binary is called `lsf`.

## Themes

### Use your own store data

```bash
npx local-storefront init
```

This writes `shopify-local/` into your theme, with a demo store to edit. A store is plain JSON:

```json
{
  "shop": { "name": "My Shop", "currency": "EUR" },
  "products": [{ "title": "Blue shirt", "price": "19.99" }]
}
```

The store data can hold products, collections, pages, blogs, menus and customers. It can also
hold B2B companies, gift cards, subscriptions, store pickup, swatches, metafields and
metaobjects. `validate` explains each mistake:

```text
$ npx local-storefront validate
error[unknown_field]: unknown field "titel"
  --> products/shirt.json at /titel
  hint: did you mean "title"?
```

Guide: [data format](docs/data-format.md).

### Run end-to-end tests

Each test gets its own cart and customer, so tests run in parallel. One request puts a session
in the state the test needs:

```ts
// The demo store's customer and SKU (npx local-storefront init): put your own.
const response = await page.request.put('/__lsf/session', {
  data: {
    customer: 'jane.doe@example.com',
    cart: { items: [{ variant: 'TEE-BLK-M', quantity: 2 }] },
  },
});
expect(response.ok()).toBeTruthy(); // 422 when the data has no such customer or SKU
await page.goto('/cart');
```

Guides: [testing](docs/testing.md), with a Playwright setup, and the
[control API](docs/control-api.md).

### Get ready for Shopify's strict Liquid parser

Shopify is moving every store to a strict Liquid parser. After that, a file it rejects can no
longer be uploaded. `check` lists that markup with Shopify's message, and `--fix` rewrites it:

```text
$ npx local-storefront check --fix
snippets/card.liquid:1: fixed: Shopify's strict Liquid parser rejects this {% render %}: Expected end_of_string but found pipe
  was: … alt: image.alt | default: shop.name
  now: … alt: image_alt
  before it: assign image_alt = image.alt | default: shop.name
  changes the output: `alt` gets `image.alt | default: shop.name`
```

Guide: [strict Liquid](docs/strict-liquid.md).

### Format and profile

- `npx local-storefront fmt` formats every `.liquid` file as Prettier does with Shopify's Liquid
  plugin, without Node. Use `fmt --check` in CI. Guide: [formatting](docs/formatting.md).
- `npx local-storefront profile /collections/all` shows the time of every section, block and
  snippet a page renders. With `--points`, it shows what the page would cost on Shopify. Guide:
  [performance](docs/performance.md).

**Coming from `shopify theme dev`?** Add `--live-reload` to reload open pages as you edit, and
leave it off in tests. The store is your JSON files, not a development store. `check` finds the
Liquid that does not parse, the filters that do not exist, and the snippets, sections and
translation keys the theme names but does not have. Theme Check's other lint rules
(performance, accessibility) still need `shopify theme check`.

## Apps

### The checkout and the customer account of an app

`app dev` serves a local checkout that runs the app's Shopify Functions and shows its checkout
UI extensions. It also serves a local customer account (`/account`) with its customer account
UI extensions.

- **Scenarios.** A scenario is a cart, a buyer, a country and settings to open a checkout with.
  `app init --app ../my-app` writes a set of scenarios made for the app's Functions and
  extensions.
- **One run, printed.** `app function run --app ../my-app` runs the Functions once and prints
  each input, output and log.
- **CI.** A scenario's `expect` says what its checkout must look like, for example a method
  hidden from the express buttons or a total. `app check` runs every scenario and exits with 1
  when a Function fails or an expectation is not met. It needs no store and no network.
- **Builds without Node.** `app build` and `app function build` build the extensions as
  `shopify app build` does. `app function schema` writes a Function's `schema.graphql`.
- **Theme app extensions.** `serve --app ../my-app` renders the app's app blocks where the
  theme's templates name them, and its app embed blocks where the theme or the store data turns
  them on. Guide: [storefront](docs/storefront.md#theme-app-extensions).

Guide: [checkout](docs/checkout.md).

### The backend of an app: a local Admin API

With `--app`, lsf also serves a GraphQL Admin API over the store data, at
`/admin/api/2026-10/graphql.json`. The backend runs against it offline, on the same data every
run. What the backend changes shows on the storefront and in the checkout at once.

- **Admin API 2026-10.** Every mutation is simulated, along with queries, search, cost and
  throttling, and bulk operations. Add the 2027-01 release candidate with
  `--admin-versions 2026-10,2027-01`.
- **OAuth, token exchange and session tokens**, answered as Shopify answers them.
- **Webhooks**, signed as Shopify signs them, are delivered to `--app-url`. They never leave
  this machine. `app webhook trigger --topic app/uninstalled` sends one on demand.
- **The embedded admin.** The app's pages open at `http://127.0.0.1:9292/admin/apps/<handle>`,
  with a local App Bridge (session tokens, title bar, navigation, save bar, toasts and the
  resource picker). The sidebar links to the storefront and checkout previews. With
  `--polaris offline`, Polaris loads from lsf's own copy instead of Shopify's CDN.
- **Billing and managed pricing.** Declare the plans with `--pricing-plans pricing.toml`. The
  merchant picks a plan on a local plan selection page. The backend gets the subscription, its
  webhook and the welcome link, and reads the plan from lsf's Partner API (`activeSubscription`).
- **The shop's clock.** A test moves a shop's time with `POST /__lsf/admin/clock`. Payment
  schedules come due, checkouts are abandoned and scheduled listings go live, each with its
  webhooks.

`--only theme,checkout,admin` runs some of these parts alone. For example,
`serve --app ../my-app --only admin` serves the Admin API alone. See
[what runs](docs/cli.md#what-runs).

To see the whole flow with Shopify's own libraries, read
[`examples/admin-backend`](examples/admin-backend/README.md). Guide: [Admin API](docs/admin.md).

## Commands

| Command | What it does |
|---|---|
| **Themes** | |
| `serve` | Serves the theme at <http://127.0.0.1:9292>. With `--app`, it also serves the app's checkout, its customer account and its Admin API. |
| `init` | Creates `shopify-local/` with a demo store to edit. |
| `validate` | Checks the store data and the checkout scenarios, with or without a theme. |
| `check` | Checks every Liquid file, including what Shopify's strict parser rejects. `--fix` rewrites that markup. |
| `fmt [paths]` | Formats Liquid files as Prettier does. Use `--check` in CI. |
| `render <path>` | Prints the HTML of one page, without a server. |
| `profile <path>` | Shows what a page spends its time on, or what it would cost Shopify. |
| `routes` | Lists the pages the store data creates. |
| **Apps** | |
| `app dev --app <dir>` | Serves an app without a theme: its checkout, its customer account and its Admin API. |
| `app init` | Writes checkout scenarios made for the app's Functions and extensions. |
| `app check` | Runs every scenario and checks each `expect`. Exits with 1 on a failure, for CI. |
| `app build` | Builds the app's extensions as `shopify app build` does, without Node. |
| `app function run` | Runs the Functions once and prints each input, output and log. |
| `app function build` | Builds the Functions, including JavaScript and TypeScript ones, without Node. |
| `app function schema` | Writes a Function's `schema.graphql`, offline. |
| `app webhook trigger` | Sends one webhook to the backend, signed, as `shopify app webhook trigger` does. |
| **Store data** | |
| `schema [kind]` | Prints the JSON Schema of the store data. |
| `docs` | Prints the data guide. |

With npm, put `npx local-storefront` before each command. With a release binary, put `lsf`.
For tools, `validate`, `check`, `app check`, `app function run` and `app webhook trigger` print
JSON with `--format json`, and `profile` with `--json`. Every option, exit code and output is in
the [CLI reference](docs/cli.md).

## Documentation

| I want to... | Read |
|---|---|
| Write store data | [Data format](docs/data-format.md), then the [field reference](docs/data-reference.md) |
| Write end-to-end tests | [Testing](docs/testing.md) and the [control API](docs/control-api.md) |
| Fix what Shopify's strict parser rejects | [Strict Liquid](docs/strict-liquid.md) |
| Format Liquid files | [Formatting](docs/formatting.md) |
| Make a page faster | [Performance](docs/performance.md) |
| Use the storefront in a browser | [Storefront](docs/storefront.md): accounts, password page, live reload, previews, theme app extensions |
| Run the Functions and checkout extensions of an app | [Checkout](docs/checkout.md) |
| Test an app's backend: Admin API, OAuth, webhooks, billing, app proxy | [Admin API](docs/admin.md), then the [Admin API reference](docs/admin-reference.md) |
| Know an option or an exit code | [CLI reference](docs/cli.md) |
| Drive local-storefront from an AI agent | [Agents](docs/agents.md) |
| Know what matches Shopify | [Compatibility](docs/compatibility.md) |
| Work on local-storefront itself | [Architecture](docs/architecture.md) |

What changed in each version is in the [changelog](CHANGELOG.md).

## What it does not do

local-storefront does not run Shopify's own checkout or admin.

- A theme alone has no checkout: `/checkout` sums up the cart. Its cart has no discounts or
  shipping rates.
- With `--app`, the checkout is a local stand-in. It runs the app's Functions and shows its
  extensions, but takes no payment and has no B2B catalogs.
- The Admin API is a local simulation over the store data. Its admin shows the app's pages, not
  Shopify's.
- Customers can log in, but not register.

The full list is in [compatibility](docs/compatibility.md#known-differences).

## Other ways to install

With [mise](https://mise.jdx.dev):

```bash
mise use github:0xtlt/local-storefront
```

As a single binary, from the [releases](https://github.com/0xtlt/local-storefront/releases), for
macOS, Linux and Windows on x64 and ARM64. On Linux:

```bash
curl -fsSL https://github.com/0xtlt/local-storefront/releases/latest/download/lsf-x86_64-unknown-linux-gnu.tar.gz | sudo tar -xz -C /usr/local/bin lsf
```

From source, with Rust 1.97 or later:

```bash
cargo install --path crates/cli
```

Installed these ways, the command is `lsf`: `lsf serve` is `npx local-storefront serve`.

# local-storefront

Run a Shopify theme on your machine, without Shopify.

`lsf` reads your theme and a few JSON files (products, cart, customers) and serves the
storefront locally. No development store, no API calls, no rate limit. It is made for
end-to-end tests: every test gets the exact store it needs, in milliseconds.

## Quick start

In your theme folder:

```bash
npm install --save-dev local-storefront
```

```bash
npx local-storefront serve
```

Open <http://127.0.0.1:9292>. Your theme is running, with a demo store. If that port is taken,
`lsf` uses the next free one and prints the address.

> In this README, `lsf` stands for `npx local-storefront`. Do not type `npx lsf`: that is
> another package on npm. In the `scripts` of your `package.json`, plain `lsf` works.

## Use your own data

```bash
lsf init
```

This creates a `shopify-local/` folder in your theme, with the demo store for you to edit. A
store is plain JSON:

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

You can describe products, collections, pages, blogs, menus, customers, companies (B2B), gift
cards, metafields and metaobjects. Images do not have to exist: a missing image becomes a
placeholder of the right size.

After editing, check your data:

```bash
lsf validate
```

Mistakes are explained, with the file, the place and a suggestion:

```text
error[unknown_field]: unknown field "titel"
  --> products/shirt.json at /titel
  hint: did you mean "title"?
```

The full format is in the [data guide](docs/data-format.md).

## Use it in tests

Let your test runner start the server. With Playwright:

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  use: { baseURL: 'http://127.0.0.1:9292' },
  webServer: {
    command: 'npx local-storefront serve --port 9292 --static --quiet --strict',
    url: 'http://127.0.0.1:9292/__lsf/status',
    reuseExistingServer: !process.env.CI,
  },
});
```

Each test has its own session: its own cart and its own customer. Tests can run in parallel.
One call puts a session in the state you want. With the demo store:

```ts
import { test, expect } from '@playwright/test';

test('a customer sees their cart', async ({ page }) => {
  await page.request.put('/__lsf/session', {
    data: {
      customer: 'jane.doe@example.com',
      cart: { items: [{ variant: 'TEE-BLK-M', quantity: 2 }] },
    },
  });

  await page.goto('/cart');
  await expect(page.getByText('Organic Cotton T-Shirt')).toBeVisible();
});
```

A test can also bring its own data, for that test only:

```ts
await page.request.put('/__lsf/session', {
  data: {
    data: { products: [{ title: 'Sold out thing', price: '10.00', available: false }] },
  },
});
await page.goto('/products/sold-out-thing');
```

More in the [testing guide](docs/testing.md).

## Customers and B2B

Open <http://127.0.0.1:9292/account>. This page lists the customers of your data. Click one
to be logged in as them, or "Nobody" to be logged out. Every account link of your theme leads
to this page, because Shopify hosts customer accounts outside the theme.

To start logged in:

```bash
lsf serve --customer default
```

`default` is the first customer of your data. You can also give an email, or `none`.

A B2B customer is a customer with a company:

```json
{
  "companies": [
    {
      "name": "Northwind Hotels",
      "metafields": { "custom": { "payment_terms": "Net 30" } },
      "locations": [{ "name": "Portland" }, { "name": "Seattle" }]
    }
  ],
  "customers": [
    { "email": "alex.morgan@example.com", "first_name": "Alex", "company": "Northwind Hotels" }
  ]
}
```

Logged in as Alex, `customer.b2b?` is true, and `customer.current_company` and
`customer.current_location` are filled. The account page lets you change location. Company
prices (catalogs) are not simulated.

A theme with `templates/customers` (legacy accounts) keeps its own login page. More in the
[data guide](docs/data-format.md#logging-in).

## The password page

Open <http://127.0.0.1:9292/password> to see your password page. The password is `password`.
A wrong password stays on the page with the error. To change the password:

```json
{ "shop": { "password": "sesame" } }
```

The storefront is never locked: nothing redirects to that page.

## What you get

- **The same HTML as Shopify.** The Liquid engine is checked against Shopify's own, and
  against Shopify's test suite of the language. Shopify's Horizon and Dawn themes render
  every page without an error.
- **A working storefront.** Cart, section rendering, predictive search, recommendations,
  collection filters, forms, languages, customer accounts, B2B companies, the password page,
  `robots.txt` and sitemaps.
- **Images served locally.** `image_url` and `image_tag` point to your machine, which resizes
  and crops like Shopify's CDN.
- **Shopify's scripts.** Pages have the `Shopify` JavaScript object, `Shopify.actions`,
  `Shopify.loadFeatures`, the Customer Privacy API and `ShopifyAnalytics.meta`.
- **Clear errors.** Wrong data is refused with a message a person or an LLM can act on.

Details and known differences: [compatibility](docs/compatibility.md).

## Commands

| Command | What it does |
|---|---|
| `lsf serve` | Serves the theme. |
| `lsf init` | Creates `shopify-local/` with a demo store to edit. |
| `lsf validate` | Checks your data. Add `--format json` for a report a tool can read. |
| `lsf check` | Checks that every Liquid file of the theme is understood. |
| `lsf render <path>` | Prints the HTML of one page, without a server. |
| `lsf routes` | Lists the pages your data creates. |
| `lsf schema [kind]` | Prints the JSON Schema of the data format. |
| `lsf docs` | Prints the data guide. |

Every command accepts `--theme <dir>` (default: the current folder) and `--data <dir>`
(default: `<theme>/shopify-local`).

### Options of `lsf serve`

| Option | What it does |
|---|---|
| `--port <port>`, `-p` | Port to listen on, and no other: the server does not start if it is taken. `0` picks a free one. Without the option: `9292`, or the next free port up to `9391`. |
| `--host <address>` | Address to listen on. Default `127.0.0.1`. Use `0.0.0.0` in a container. |
| `--live-reload` | Reloads the page when you edit the theme or the data. For development, not for tests. |
| `--static` | Reads the files once. The fastest mode, for tests. |
| `--strict` | Refuses to start if the data has errors. |
| `--quiet`, `-q` | Does not log requests. |
| `--customer <email>` | Starts every visitor logged in as this customer. Also `default` (the first customer) or `none`. |
| `--throttle <rules>` | Answers late, to test loading states. See below. |

## Throttling

A local server answers instantly, so you never see a spinner. `--throttle` makes it answer
late.

Like a real Shopify store:

```bash
lsf serve --throttle simulated
```

Like a real store on a slow phone:

```bash
lsf serve --throttle slow
```

Your own delay, for everything or for one kind of request:

```bash
lsf serve --throttle 300ms
```

```bash
lsf serve --throttle cart-add=1s
```

A preset with a change:

```bash
lsf serve --throttle simulated,cart-add=2s
```

For one test only, without slowing the others:

```ts
await page.request.put('/__lsf/session', { data: { throttle: 'simulated' } });
```

### Kinds of requests and presets

Delays are in milliseconds.

| Kind | Requests | `simulated` | `slow` |
|---|---|---|---|
| `page` | Pages | 80 | 800 |
| `section` | Section Rendering API | 80 | 600 |
| `cart-read` | Reading the cart (`/cart.js`) | 150 | 700 |
| `cart-add` | Adding to the cart | 300 | 1200 |
| `cart-change` | Changing a quantity | 300 | 1200 |
| `cart-update` | Updating the cart | 300 | 1200 |
| `cart-clear` | Emptying the cart | 300 | 1200 |
| `search` | Predictive search | 250 | 1000 |
| `recommendations` | Product recommendations | 250 | 1000 |
| `product` | Product data (`/products/<handle>.js`) | 200 | 800 |
| `form` | Forms: contact, login, language | 400 | 1500 |
| `asset` | Theme files (CSS, JavaScript, fonts) | 30 | 300 |
| `image` | Images | 40 | 500 |

Two more names group several kinds: `cart` (the five cart kinds) and `all` (everything).

| Preset | Meaning |
|---|---|
| `simulated` | A Shopify store on a good connection. |
| `slow` | The same store on a slow mobile connection. |
| `none` | No delay. |

### Rules

- Write durations as `300ms`, `1.5s`, or a number of milliseconds.
- The most precise rule wins: `cart-add`, then `cart`, then `all`.
- Rules apply in order. A rule after a preset changes that preset.
- `0` removes the delay for a kind: `--throttle 300ms,image=0`.
- The control API (`/__lsf/...`) is never delayed.
- In a session, `throttle` is a string (`"simulated,cart-add=2s"`), a number of milliseconds,
  or an object (`{ "preset": "slow", "image": 0 }`). It replaces the server's throttle for that
  session. `"none"` removes it.

The reads of `simulated` were timed on Shopify's Horizon demo store in October 2026. The
writes (cart changes, forms) are estimates. `slow` is not measured.

## Control API

Tests talk to the server through `/__lsf`.

| Request | What it does |
|---|---|
| `PUT /__lsf/session` | Sets the session: `customer`, `company_location`, `cart`, `country`, `data`, `throttle`. |
| `GET /__lsf/login?customer=<email>` | Logs the browser in as a customer, then goes to `return_to` (default `/account`). Also `default` or `none`. A link a person can click. |
| `GET /__lsf/session` | Shows the session. |
| `DELETE /__lsf/session` | Resets the session. |
| `GET /__lsf/status` | Shows the theme, the data, its errors and every page. |
| `POST /__lsf/reload` | Reloads the data files. |
| `GET /__lsf` | The same status, as a page for a browser. |

A session follows the browser's cookies. Without cookies, name it with the header
`x-lsf-session: my-test`.

Responses carry headers a test can check:

| Header | Meaning |
|---|---|
| `x-lsf-template` | The template that rendered the page. |
| `x-lsf-liquid-errors` | The number of Liquid errors in the page. Absent when there are none. |
| `x-lsf-placeholder` | The image is a placeholder: the file is not in `shopify-local/files/`. |
| `x-lsf-throttle` | The delay that was applied. |

## Other ways to install

With [mise](https://mise.jdx.dev):

```bash
mise use github:0xtlt/local-storefront
```

As a single binary: download it from the
[releases](https://github.com/0xtlt/local-storefront/releases). There is one for macOS, Linux
and Windows, on x64 and ARM64. On Linux, for example:

```bash
curl -fsSL https://github.com/0xtlt/local-storefront/releases/latest/download/lsf-x86_64-unknown-linux-gnu.tar.gz | sudo tar -xz -C /usr/local/bin lsf
```

From source, with Rust 1.98 or later:

```bash
cargo install --path crates/cli
```

## What it does not do

`lsf` renders themes. It does not simulate checkout, apps, discounts, selling plans, B2B
catalogs, taxes or shipping rates. Customers can log in, but not register. The full list is in
[compatibility](docs/compatibility.md#known-differences).

## More documentation

- [Data guide](docs/data-format.md): how to write the store data.
- [Field reference](docs/data-reference.md): every field.
- [Testing guide](docs/testing.md): sessions, per-test data, CI.
- [Compatibility](docs/compatibility.md): what is covered, and what differs from Shopify.
- [Architecture](docs/architecture.md): how the code is organised, and how to release.

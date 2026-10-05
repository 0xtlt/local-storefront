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
cards, subscriptions, store locations, swatches, metafields and metaobjects. Images do not
have to exist: a missing image becomes a placeholder of the right size.

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

## Subscriptions

A subscription is a selling plan. Describe a group of plans, and name the products sold with
it:

```json
{
  "selling_plan_groups": [
    {
      "name": "Subscribe and save",
      "selling_plans": [
        {
          "name": "Deliver every month, 10% off",
          "price_adjustments": [{ "value_type": "percentage", "value": 10 }]
        }
      ],
      "products": ["blue-shirt"]
    }
  ]
}
```

The product now has `product.selling_plan_groups`, and each variant has its
`selling_plan_allocations` with the price of every plan. `?selling_plan=<id>` selects a plan.
Adding to the cart with `selling_plan` gives the line the plan and its price.

A product sold by subscription only has `"requires_selling_plan": true`.

## Store pickup

List the places that stock your products:

```json
{
  "locations": [
    {
      "name": "Paris shop",
      "address": { "address1": "12 rue de Rivoli", "city": "Paris", "country_code": "FR" },
      "pick_up_time": "Usually ready in 2 hours"
    }
  ]
}
```

Every variant is stocked there, and `variant.store_availabilities` says so. A variant can say
where it is in stock:

```json
{ "options": ["M"], "store_availabilities": { "Paris shop": false } }
```

`/variants/<id>?section_id=<section>` renders a section for one variant, which is how themes
load their pickup information.

## Swatches

Give a color or an image to option values, by name:

```json
{ "swatches": { "White": "#ffffff", "Floral": { "image": "swatches/floral.jpg" } } }
```

Every option value named `White` gets that swatch: `product_option_value.swatch` on products,
and `filter_value.swatch` in collection filters.

More on these three in the [data guide](docs/data-format.md#selling-plans).

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
  collection filters, forms, languages, customer accounts, B2B companies, subscriptions,
  store pickup, swatches, the password page, `robots.txt` and sitemaps.
- **Images served locally.** `image_url` and `image_tag` point to your machine, which resizes
  and crops like Shopify's CDN, and sends each browser the lightest format it reads: AVIF,
  WebP, or the one of the file. An image with `preload: true` is preloaded through the `Link`
  header, as on Shopify.
- **Minified styles and scripts.** The CSS and the JavaScript of the theme are minified, with
  a source map, as Shopify minifies them before it serves them.
- **Shopify's compression and caching.** Responses are compressed, and say how long to keep
  them with the same `Cache-Control` as Shopify, so that an audit of your theme sees what it
  would see on the store.
- **Shopify's scripts.** Pages have the `Shopify` JavaScript object, `Shopify.actions`,
  `Shopify.loadFeatures`, the Customer Privacy API and `ShopifyAnalytics.meta`.
- **Render times and costs.** Every response says how long the server took. `lsf profile`
  shows which section, block or snippet took the longest, and what the page would cost
  Shopify, where loading a product or a metafield is not free.
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
| `lsf profile <path>` | Shows what the render of one page spends its time in, or what it would cost Shopify: sections, blocks, snippets. See [Profile a page](#profile-a-page). |
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
| `--live-reload` | Reloads the page as soon as you edit the theme or the data. The page listens on a WebSocket, and asks the server every 700 ms where a socket cannot be opened. For development, not for tests. |
| `--static` | Reads the files once. The fastest mode, for tests. |
| `--strict` | Refuses to start if the data has errors. |
| `--quiet`, `-q` | Does not log requests. |
| `--no-minify` | Serves the stylesheets and the scripts of the theme as they are written. Without it they are minified, with a source map, as on Shopify. |
| `--no-compression` | Sends responses as they are. Without it, pages, styles, scripts and JSON are compressed (Brotli or gzip) for the browsers and tools that accept it, as on Shopify. |
| `--customer <email>` | Starts every visitor logged in as this customer. Also `default` (the first customer) or `none`. |
| `--throttle <rules>` | Answers late, to test loading states. See below. |
| `--timings` | Names every section and every theme block in the `Server-Timing` header, with how long it took to render. See [Render times](#render-times). |

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

## Render times

Every response says how long the server took, in the `Server-Timing` header, as Shopify does.

```text
server-timing: processing;dur=3.09, render;dur=2.507, template;dur=1.27, layout;dur=0.986, compress;dur=0.53
```

In Chrome, open DevTools, then Network, click the page, then Timing. The times are under
"Server Timing".

To see each section and each block too:

```bash
lsf serve --timings
```

```text
section;dur=1.922;desc="main-collection template--8034641070597__main"
block;dur=0.636;desc="- filters"
block;dur=1.123;desc="- _product-card product-card x8"
block;dur=0.611;desc="- - _product-card-gallery card-gallery x8"
```

A block comes under its section, with one dash per level: two dashes for a block inside a
block. `x8` means it was rendered 8 times, and the time is the time of all 8.

In a test:

```ts
const response = await page.goto('/');
const timing = response.headers()['server-timing'];
```

To measure many requests, start from zero:

```bash
curl -X DELETE http://127.0.0.1:9292/__lsf/timings
```

Load your pages, then read the totals:

```bash
curl http://127.0.0.1:9292/__lsf/timings
```

```json
{
  "templates": [
    { "template": "collection", "count": 2000, "total": 5792.9, "mean": 2.896, "p50": 2.851, "p95": 3.31, "min": 2.308, "max": 7.495 }
  ],
  "sections": [
    { "id": "template--8034641070597__main", "type": "main-collection", "count": 2000, "total": 3455.621, "mean": 1.728, "p50": 1.688, "p95": 1.982, "min": 1.281, "max": 5.537 }
  ],
  "blocks": [
    { "id": "A1456318a0478e95__product-card", "type": "_product-card", "section": "template--8034641070597__main", "count": 16000, "total": 2196.216, "mean": 0.137, "p50": 0.12, "p95": 0.223, "min": 0.064, "max": 2.549 }
  ]
}
```

What took the most time in all comes first. A block counts once each time it is rendered:
8 product cards on 2000 pages are 16000.

These are the times of `lsf` on your machine, not the times of Shopify. Use them to compare:
before and after a change, one section or one block with another. To see the snippets too,
[profile the page](#profile-a-page).

### Entries of `Server-Timing`

Times are in milliseconds.

| Entry | Meaning |
|---|---|
| `processing` | Everything the server did for the request. Shopify uses the same name. |
| `render` | Rendering the Liquid of a page or of sections. |
| `template` | The template of the page: what goes into `content_for_layout`. |
| `layout` | The layout around it, with its header and its footer. |
| `section` | One section, with `--timings`. Its description is its type, then its id: the one of `#shopify-section-<id>`. |
| `block` | One theme block, with `--timings`. Its description is a dash per level, its type, its key in the template, then `x<n>` when it was rendered more than once. |
| `omitted` | Only on a page with hundreds of blocks: how many of the fastest ones the header leaves out. `/__lsf/timings` has them all. |
| `compress` | Compressing the response. |
| `throttle` | The delay of `--throttle`. It is not part of `processing`. |

- A time includes what is inside: a section is part of `template` or of `layout`, a block is
  part of its section, and of the block it is in.
- Only theme blocks are timed: the files of `blocks/`, rendered with `content_for`. Blocks
  that a section writes itself, in a loop over `section.blocks`, are part of the section.
- `/__lsf/timings` counts pages by template, sections and blocks by id. Sections and blocks
  count wherever they are rendered: in a page, alone, or in a cart response. The id of a
  block is the one of `#shopify-block-<id>`, and ends with its key.
- `p50` and `p95` are within 2% of the exact value.

## Profile a page

To see everything a page renders, and how long each part takes:

```bash
lsf profile /collections/all
```

```text
/collections/all · template collection · 2.502 ms
The render in the middle of 15, which took from 2.325 to 2.901 ms.
Times are in ms, for all the calls of a row. "own" leaves out what is under the row.

   total      own  calls
   2.502    0.201      1  render
   1.510    0.005      1    templates/collection.json
   1.464    0.029      1      section template--8034641070597__main
   1.435    0.031      1        sections/main-collection
   0.921    0.021      8          block product-card
   0.900    0.031      8            blocks/_product-card
   0.495    0.010      8              block card-gallery
   0.485    0.022      8                blocks/_product-card-gallery
   0.398    0.112      8                  snippets/card-gallery
   0.122    0.122     15                    snippets/product-media

Slowest on their own:

     own  calls
   0.191     12  snippets/list-filter
   0.115     15  snippets/product-media
```

Each row is under the row that rendered it: a section, its file, its blocks, their snippets.

- `total` is the time of the row and of everything under it.
- `own` is the time of the row alone.
- `calls` is how many times it was rendered. Both times are for all the calls.

### In the browser

With the server running, the same report is a page:

```text
http://127.0.0.1:9292/__lsf/profile?path=/collections/all
```

It profiles the page as your session sees it: its cart, its customer.

Add `html=1` for a flame graph:

```text
http://127.0.0.1:9292/__lsf/profile?path=/collections/all&html=1
```

This is speedscope, the viewer that `shopify theme profile` opens. `lsf` embeds it: nothing
is fetched from the internet. The name of the profile, at the top of the page, switches
between time and points.

Change the parameters in the address to profile another page. The status page (`/__lsf`)
links every page to its profiles.

### What the page would cost Shopify

`lsf` loads a product or a metafield in no time. A Shopify storefront does not: a page that
is fast here can be slow there. `--points` counts what the page asks for, instead of the time
it takes:

```bash
lsf profile /collections/all --points
```

```text
/collections/all · template collection · 15564 points

   total      own  calls
   15564        0      1  render
   11556        0      1    templates/collection.json
   11251        0      1      section template--8034641070597__main
   11251      870      1        sections/main-collection
    6865        0      8          block product-card
    6865      112      8            blocks/_product-card

What the points are made of:

  points   count   each
   14244   14244      1  liquid      A tag or an output rendered.
     800       8    100  product     A product loaded.
     220      22     10  variant     A variant loaded.
     100       1    100  collection  A collection loaded.
     200       2    100  menu        A menu loaded.
```

The report is the same tree, in points. A row has the points of everything that happened
under it: a snippet that reads a metafield of 50 products has the points of 50 metafields.

| Kind | Points | Charged for |
|---|---|---|
| `liquid` | 1 | A tag or an output rendered. |
| `product` | 100 | A product loaded. |
| `variant` | 10 | A variant loaded. |
| `collection` | 100 | A collection loaded. |
| `metafield` | 100 | A metafield read. |
| `metaobject` | 100 | A metaobject loaded. |
| `page` | 100 | A page loaded. |
| `blog` | 100 | A blog loaded. |
| `article` | 100 | An article loaded. |
| `menu` | 100 | A menu loaded. |
| `search` | 1000 | A search, a predictive search or the recommendations of a product. |

- **Points are a model, not a measure.** Shopify does not publish what it spends on what.
  The unit is a tag rendered, and what a store has to fetch is taken to weigh as much as 100
  tags. Read the counts first: they are facts. The points only weigh them.
- What is loaded counts once per page, however many times the templates read it. A product
  in a loop counts once per product.
- A metafield that does not exist counts too: the store looks for it all the same.
- Points are the same at every render and on every machine.

To change the costs, for example after comparing with `shopify theme profile` on your store:

```bash
lsf profile /collections/all --points --cost product=300,metafield=50
```

### The slow line of a file

```bash
lsf profile /collections/all --lines
```

Rows such as `snippets/product-media:40` appear: a tag or an output, by its line.

### Options

| Option | In the URL | What it does |
|---|---|---|
| `--points` | `points=1` | Counts in points instead of time. In the flame graph, starts on points. |
| `--cost <kind>=<points>,...` | `cost=<kind>=<points>,...` | Changes what a kind of thing costs. |
| | `html=1` | Shows the flame graph. |
| `--lines` | `lines=1` | Adds every tag and every output, by file and line. Timing each one makes the render slower: compare the rows with each other. Points do not change. |
| `--all` | `all=1` | Shows every row. Without it, what took less than a hundredth of the render is summed up as `... 3 more`. |
| `--runs <n>` | `runs=<n>` | Renders the page `n` times. Default: 15. |
| `--section-id <id>` | `section_id=<id>` | Profiles one section alone. |
| `--json` | `format=speedscope` | Gives the file of the flame graph instead of the report. It holds both profiles. |

- The page is rendered 3 times first, so that reading the files does not count. The profile
  is the one of the render in the middle, by how long they took.
- `section <id>` and `block <key>` are a section and a block as the template places them:
  reading their settings, then their file, which is the row under them.
- The times are those of `lsf` on your machine, not of Shopify. A snippet that is slow here
  because it is rendered 200 times is rendered 200 times there too.
- Without a server, `lsf profile <path> --json > profile.json` writes the file of the flame
  graph. Drop it on <https://www.speedscope.app>.

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
| `GET /__lsf/profile?path=<path>` | Shows what the render of a page spends its time in. With `points=1`, what it would cost Shopify. With `html=1`, a flame graph. See [Profile a page](#profile-a-page). |
| `GET /__lsf/timings` | Shows how long the templates, the sections and the blocks took so far. See [Render times](#render-times). |
| `DELETE /__lsf/timings` | Forgets those times, to measure from zero. |
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
| `server-timing` | How long the server took. See [Render times](#render-times). |

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

`lsf` renders themes. It does not simulate checkout, apps, discounts, B2B catalogs, taxes or
shipping rates. Customers can log in, but not register. The full list is in
[compatibility](docs/compatibility.md#known-differences).

## More documentation

- [Data guide](docs/data-format.md): how to write the store data.
- [Field reference](docs/data-reference.md): every field.
- [Testing guide](docs/testing.md): sessions, per-test data, CI.
- [Compatibility](docs/compatibility.md): what is covered, and what differs from Shopify.
- [Architecture](docs/architecture.md): how the code is organised, and how to release.

# End-to-end testing against local-storefront

`npx local-storefront serve` gives a theme a storefront that needs no network, has no rate limit
and renders a page in a few milliseconds. This guide shows how to drive it from a test runner.
The examples use Playwright; nothing is specific to it.

## Starting the server

```bash
npx local-storefront serve --theme path/to/theme --port 9393 --static --quiet --strict
```

| Flag | Why in tests |
|---|---|
| `--static` | Reads the theme and the data once. Fastest, and nothing changes under a running test. |
| `--strict` | Refuses to start when the store data has errors, instead of serving what is valid. |
| `--quiet` | No request log. |
| `--port 9393` | The port the tests expect, or an error: without `--port`, a taken `9292` makes the server move to the next free port. |
| `--port 0` | Picks a free port; the chosen address is printed as `ready:  http://127.0.0.1:<port>/`. |
| `--host 0.0.0.0` | Listens on every interface, e.g. inside a container. |
| `--throttle <rules>` | Answers requests late: a preset (`simulated`, `slow`) or rules by kind. See [Simulating a slow network](#simulating-a-slow-network). |

Every option: [CLI reference](cli.md#serve).

Leave `--live-reload` off: it injects a script into the pages.

Responses are compressed, as a storefront's are, when the client accepts it. It costs about
half a millisecond per page. `--no-compression` turns it off.

The stylesheets and the scripts of the theme are minified, as a storefront's are, and come
with a source map: a stack trace or a coverage report still names the lines of your files.
`--no-minify` serves them as they are written.

With Playwright, let the runner start and stop the server:

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  fullyParallel: true,
  use: { baseURL: 'http://127.0.0.1:9393' },
  webServer: {
    command: 'npx local-storefront serve --port 9393 --static --quiet --strict',
    url: 'http://127.0.0.1:9393/__lsf/status',
    reuseExistingServer: !process.env.CI,
  },
});
```

The tests get a port of their own, outside the `9292` to `9391` that `serve` takes without
`--port`. A local run then never reuses your dev server, with its live reload and its data that
changes.

### An embedded app

For the backend of an app (the React Router template), start lsf and the backend, and open the
app in lsf's admin ([preview the embedded app](admin.md#preview-the-embedded-app)). The config
is the [quickstart's](quickstart-app.md#7-a-playwright-test), the same in every page of lsf's
docs:

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

// The app's values (SHOPIFY_API_KEY, SHOPIFY_API_SECRET, SCOPES): react-router-serve reads no .env.
process.loadEnvFile('.env');

export default defineConfig({
  testDir: 'e2e',
  workers: 1, // one lsf, one shop: the tests change it in turn
  use: {
    // For the test's own requests to the backend: the template answers 410 to HeadlessChrome.
    userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36',
  },
  webServer: [
    {
      command: 'npx local-storefront app dev --app . --port 9393 --app-url http://localhost:3001 --polaris offline --static --quiet',
      url: 'http://127.0.0.1:9393/__lsf/status',
    },
    {
      // A build, not the dev server: on a cold start, Vite's dev server optimizes the
      // dependencies a page asks for and reloads the page, under the test.
      command: 'npx prisma migrate deploy && npx react-router build && npx react-router-serve ./build/server/index.js',
      url: 'http://localhost:3001/',
      env: { PORT: '3001', SHOPIFY_APP_URL: 'http://localhost:3001', LSF_URL: 'http://127.0.0.1:9393' },
      timeout: 120_000,
    },
  ],
});
```

```ts
test('the app opens in lsf\'s admin', async ({ page }) => {
  await page.goto('http://127.0.0.1:9393/admin/apps/my-app/app');
  await expect(page.frameLocator('iframe').getByText('Get started with products')).toBeVisible();
});
```

The template's `isbot` answers `410 Gone` to Playwright's headless Chromium (`HeadlessChrome` in
its user agent). Pages in lsf's admin pass: the frame's proxy says Chrome. A request of the test
to `http://localhost:3001` does not go through it, hence `use.userAgent`. The backend runs from a
build: Vite's dev server, on a cold start (CI), reloads the page under the test while it
optimizes its dependencies. A build has no HMR port either, so the tests run next to your dev
server.

## Sessions: every test is isolated

The server keeps one **session** per browser: the cart, the logged-in customer, the selected
country, and optionally data of its own. A session is identified by the `_lsf_session` cookie,
which the server sets on the first response. A Playwright test gets a fresh browser context,
hence a fresh cookie jar, hence its own session: tests can run in parallel against one server
without seeing each other's carts. Their checkouts share the compiled Functions: a Function is
compiled once, and as many run at once as the machine has CPUs.

Clients without a cookie jar (curl, an API test) can name their session with a header instead:

```bash
curl -H 'x-lsf-session: my-test' http://127.0.0.1:9292/cart.js
```

## Putting a session in a known state

`PUT /__lsf/session` replaces the state of the caller's session. The body is a
[session](data-reference.md#session); every endpoint is in the [control API](control-api.md).
The examples use the demo store that `npx local-storefront init` writes: the SKU `TEE-BLK-M`,
the customer `jane.doe@example.com`, the product `ceramic-mug`. Put your own SKUs and emails.

```ts
import { test, expect } from '@playwright/test';

test('a logged-in customer sees their cart', async ({ page }) => {
  // page.request shares its cookies with the page.
  const response = await page.request.put('/__lsf/session', {
    data: {
      customer: 'jane.doe@example.com',
      cart: { items: [{ variant: 'TEE-BLK-M', quantity: 2 }], note: 'Gift wrap' },
      country: 'FR',
    },
  });
  expect(response.ok()).toBeTruthy();

  await page.goto('/cart');
  const lines = page.getByRole('main').getByRole('table');
  await expect(lines.getByRole('link', { name: 'Organic Cotton T-Shirt', exact: true })).toBeVisible();
});
```

Look in the cart's table, where Dawn and Horizon put the lines of the cart. The menu of the
header, and the products listed under the cart (Horizon's "You may also like"), often name the
same products.

Variants are referenced by SKU, by id, or by a product handle (its first variant). When the body
is wrong the answer is `422` with the same diagnostics as
`npx local-storefront validate --format json`:

```json
{
  "ok": false,
  "errors": 1,
  "warnings": 0,
  "diagnostics": [
    {
      "severity": "error",
      "code": "unknown_variant",
      "file": "(request body)",
      "path": "/cart/items/0/variant",
      "message": "\"TEE-BLK-XXL\" is neither the SKU of a variant nor the handle of a product",
      "hint": "did you mean \"TEE-BLK-XL\"?"
    }
  ]
}
```

`customer` is an email, `"default"` (the first customer of the data) or `"none"`. For a B2B
customer, `company_location` names the location they buy for:

```ts
await page.request.put('/__lsf/session', {
  data: { customer: 'alex.morgan@example.com', company_location: 'Northwind Seattle' },
});
```

To start every test logged in, start the server with `--customer default` (or an email).

Fail the test on a wrong body, so that a wrong fixture never looks like a theme bug:

```ts
import { test, expect, type Page } from '@playwright/test';

async function setSession(page: Page, session: object) {
  const response = await page.request.put('/__lsf/session', { data: session });
  if (!response.ok()) throw new Error(JSON.stringify(await response.json(), null, 2));
}
```

### Data for one test only

A session can carry its own store data. `data` is a [data file](data-format.md) applied on
top of the files on disk, for this session only:

```ts
await setSession(page, {
  data: {
    shop: { name: 'A very long shop name that must not break the header' },
    products: [
      { title: 'Sold out thing', price: '10.00', available: false },
      { title: 'Ceramic Mug', handle: 'ceramic-mug', price: '1.00', compare_at_price: '22.00' },
    ],
    now: '2025-12-24T08:00:00Z',
    theme_settings: { cart_type: 'page' },
  },
});
await page.goto('/products/sold-out-thing');
```

- A product, collection, page or blog with the handle of an existing one **replaces** it; the
  others are **added**. Customers are matched by email, menus by handle, gift cards by code,
  companies, locations and selling plan groups by name.
- `shop` is merged field by field. `localization`, `session` and `now` replace what the files
  say. `checkout` is merged as a scenario's is: each field it gives (`shipping_rates`,
  `payment_methods`, `discounts`, `tax_rate`) replaces the files', and `functions` is merged
  Function by Function.
- `theme_settings`, `files` and `swatches` are merged key by key.

Other sessions keep seeing the data on disk. This is the way to test edge cases (an empty
collection, a product with 100 variants, a sold-out state, a long title, another currency)
without growing the shared fixtures.

The same goes for a subscription, a pickup location or a swatch. This test sells a product of
the demo store by subscription, and starts with one in the cart:

```ts
await setSession(page, {
  data: {
    selling_plan_groups: [
      {
        name: 'Coffee club',
        selling_plans: [
          { name: 'Every month', price_adjustments: [{ value_type: 'percentage', value: 15 }] },
        ],
        products: ['ceramic-mug'],
      },
    ],
    locations: [{ name: 'Pop-up store', pick_up_time: 'Ready in 1 hour' }],
    swatches: { Oat: '#e8dcc4' },
  },
  cart: { items: [{ variant: 'MUG-OAT', selling_plan: 'Every month' }] },
});
await page.goto('/cart');
```

`GET /__lsf/session` returns the current state, and `DELETE /__lsf/session` resets the
session to what the data files define.

## A checkout in a test

With an app, the same server runs its checkout, its thank-you page and its customer account
(see [checkout](checkout.md)). Start it with the theme and the app, or without a theme:

```bash
npx local-storefront serve --theme . --app ../my-app --port 0 --static --quiet --strict
npx local-storefront app dev --app ../my-app --port 0 --quiet --strict    # no theme
```

```ts
// playwright.config.ts
export default defineConfig({
  webServer: {
    command: 'npx local-storefront serve --app ../my-app --port 9393 --static --quiet --strict',
    url: 'http://127.0.0.1:9393/__lsf/status',
    reuseExistingServer: !process.env.CI,
  },
  use: { baseURL: 'http://127.0.0.1:9393' },
});
```

With `--app`, the server also runs the app's [Admin API](admin.md), which records each order and
sends its webhooks. Tests of the checkout alone can leave it out: `--only theme,checkout`, or
`app dev --only checkout` without a theme ([what runs](cli.md#what-runs)). The orders are then
the session's only.

**Put the session in a state, then open the checkout.** The cart, the customer and data for this
test only go through `PUT /__lsf/session`, as above; `/checkout` opens the checkout of that cart
(`/checkout?scenario=<name>` the one of a [scenario](checkout.md#scenarios), with its cart and
buyer). A discount Function runs only for a discount that names it: give one in
`data.checkout`. Another Function runs only through an enabled owner, as on Shopify: give it
`"enabled": true` in `data.checkout.functions`, or open a scenario that names it in `extensions`
([what runs](checkout.md#owners-and-scenarios)). As in a scenario, each field it gives (`shipping_rates`, `payment_methods`,
`discounts`, `tax_rate`) replaces the store data's for the session, and the settings of the
Functions (`functions`) are merged Function by Function: what it does not give stays. A
discount has the settings of its Function in `metafields`, the metafield its
`[extensions.input.variables]` (or its input query) reads: without them, the Function has nothing
to apply.

```ts
await setSession(page, {
  customer: 'jane.doe@example.com',
  cart: { items: [{ variant: 'TEE-BLK-M', quantity: 2 }] },
  data: {
    now: '2025-01-15T10:00:00Z',
    checkout: {
      shipping_rates: [{ handle: 'standard', title: 'Standard', price: '4.90' }],
      discounts: [
        {
          function: 'discount-codes',
          title: '10% off',
          code: 'SAVE10',
          metafields: { '$app.function-configuration': { type: 'json', value: { percentage: 10 } } },
        },
      ],
    },
  },
});
await page.goto('/checkout');
await expect(page.locator('html')).toHaveAttribute('data-lsf-ready', 'true');
```

**Wait for the page, not for a time.** The checkout and the customer account set
`data-lsf-ready="true"` on `<html>` once no change is on its way to the server and every UI
extension on the page rendered (or threw, or said nothing for 5 s); it goes back to `false` while
a change is on its way, and the page has `aria-busy="true"` then. Wait for it after each action
that changes the checkout:

```ts
await page.getByRole('textbox', { name: 'Discount code' }).fill('SAVE10');
await page.getByRole('button', { name: 'Apply' }).click();
await expect(page.locator('html')).toHaveAttribute('data-lsf-ready', 'true');
```

**Select by role and name.** The fields have labels ("Email", "Address", "Discount code"), the
choices are radios named by their title, and the buttons say what they do ("Pay now",
"Complete order", "Apply"): `getByRole` finds them. What the Functions did is in the state too:
`GET /__lsf/checkout` answers the state of the session's checkout (its `runs`, `cost`,
`discounts`, `errors`), with the same session header or cookie.

**Orders are reproducible.** The orders a session places are numbered after the last one of the
store data, one after the other, whatever the other sessions do, and placed at `now` when the
data sets it: the thank-you page and the customer account (`/account`) read the same number and
date on every run. An order lives as long as its session; only the last one a session placed is
kept. `DELETE /__lsf/session` resets one session, `DELETE /__lsf/sessions` every one. With an
app, the [Admin API](admin.md) records each order too, under the same number unless another
session or the backend took it first (then under its next one, so that its orders have unique
names): the answer of `complete` gives the Admin API's. The backend of the app, its webhooks
and its tests: [test your backend](admin.md#test-your-backend). The checkout of an app in CI:
[checkout](checkout.md#in-ci).

## Simulating a slow network

A local server answers in milliseconds, so loading states flash by before a test can see
them. `--throttle` makes it answer late.

Like a real Shopify store:

```bash
npx local-storefront serve --throttle simulated
```

Like a real store on a slow phone:

```bash
npx local-storefront serve --throttle slow
```

Your own delay, for everything or for one kind of request:

```bash
npx local-storefront serve --throttle 300ms
```

```bash
npx local-storefront serve --throttle cart-add=1s
```

A preset with a change:

```bash
npx local-storefront serve --throttle simulated,cart-add=2s
```

A throttle on the command line slows the whole suite. To slow one test, give its session a
throttle of its own, which replaces the server's for that session:

```ts
test('the cart count waits for the server', async ({ page }) => {
  await setSession(page, { throttle: { 'cart-add': '1s' } });
  await page.goto('/products/ceramic-mug');
  const added = page.waitForResponse('**/cart/add*');
  await page
    .locator('form[data-type="add-to-cart-form"]')
    .getByRole('button', { name: 'Add to cart' })
    .click();
  await expect(page.getByTestId('cart-bubble')).toHaveText('0'); // the request is on its way
  expect((await added).headers()['x-lsf-throttle']).toBe('1000ms');
  await expect(page.getByTestId('cart-bubble')).toHaveText('1');
});
```

The selectors are Horizon's: its product page has a second "Add to cart" button, in a bar that
sticks to the bottom, and its cart count is `data-testid="cart-bubble"`. Put your theme's own,
and its loading state (a spinner, a disabled button) where Horizon has none.

`throttle` takes the same rules as the command line: a string (`"simulated"`,
`"simulated,cart-add=2s"`), a number of milliseconds for everything, or an object
(`{ "preset": "slow", "image": 0 }`). `"none"` lifts the server's throttle for the session.
The control API is never delayed, and a delayed response carries the header
`x-lsf-throttle: 1000ms`.

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

The reads of `simulated` were timed on Shopify's Horizon demo store in October 2026. The
writes (cart changes, forms) are estimates. `slow` is not measured.

### Rules

- Write durations as `300ms`, `1.5s`, or a number of milliseconds.
- The most precise rule wins: `cart-add`, then `cart`, then `all`.
- Rules apply in order. A rule after a preset changes that preset.
- `0` removes the delay for a kind: `--throttle 300ms,image=0`.

## What to assert on

- **The page itself.** The HTML is what Shopify would render for the same data: section and
  block wrappers (`#shopify-section-<id>`, `.shopify-block`), form markup, image URLs and
  `srcset`s, money formats and translations follow Shopify's output.
- **Liquid errors.** A page with Liquid errors renders them inline, as Shopify does
  (`Liquid error (sections/x line 12): ...`), and the response carries the header
  `x-lsf-liquid-errors: <count>`. A blanket check catches regressions on every navigation:

  ```ts
  test.beforeEach(async ({ page }) => {
    page.on('response', (response) => {
      const errors = response.headers()['x-lsf-liquid-errors'];
      if (errors) throw new Error(`${response.url()} rendered with Liquid errors: ${errors}`);
    });
  });
  ```

- **What the theme tells Shopify.** The page has the `Shopify` JavaScript object a storefront
  has, including the standard actions: `await page.evaluate(() => Shopify.actions.getCart())`
  reads the cart the way an app would. Analytics events a theme publishes with
  `Shopify.analytics.publish` are kept in `Shopify.analytics.replayQueue`, and cookie consent
  goes through `Shopify.customerPrivacy`. See
  [compatibility](compatibility.md#what-shopify-injects-into-pages).
- **The template.** `x-lsf-template` names the template that rendered the page (`product`,
  `product.alternate`, `404`, ...).
- **Pages that are not found.** A 404 renders the theme's `404` template, so a wrong URL looks
  like a page that works. The response carries `x-lsf-not-found` with the cause (`no product
  with the handle "x"`, `no page at "/x"`, `no metaobject "type/x" in the store data`, `the
  theme has no templates/metaobject/<type>.json...`, `no section "x" on the page...`) and the
  server prints it in its log. To
  fail a test on a link that leads nowhere:

  ```ts
  page.on('response', (response) => {
    const reason = response.headers()['x-lsf-not-found'];
    if (reason) throw new Error(`${response.url()} is a 404: ${reason}`);
  });
  ```

  A test of the theme's own 404 page can leave this out: a 404 is what it expects. Outside a
  browser, `npx local-storefront render <path> --strict` fails on a 404.
- **The cart.** `GET /cart.js` returns the cart in Shopify's Ajax API format.
- **Render time.** Every response carries `Server-Timing`, as on Shopify: `processing` for the
  whole request, `render` for the Liquid, and with `--timings` one entry per section and per
  theme block. `GET /__lsf/timings` adds up every page, section and block rendered since the
  server started, or since `DELETE /__lsf/timings`. These are the times of local-storefront on
  the machine that runs the tests: compare them with each other rather than with a fixed number,
  which a busy CI machine would miss. The entries are listed in
  [performance](performance.md#entries-of-server-timing). To find what makes a page slow,
  `npx local-storefront profile <path>` shows every section, block and snippet it renders, in
  time or in points: what the page would cost Shopify, where a product or a metafield has to be
  fetched. Points are the same at every render, so a test can compare them
  ([performance](performance.md#what-the-page-would-cost-shopify)).
- **Images.** A response with `x-lsf-placeholder: 1` is a generated placeholder: the data
  references a file that is not in `files/`. Placeholders have the declared size, so layout
  assertions hold; put real files in `files/` for visual regression tests. A browser gets
  images as AVIF or WebP, as on Shopify; a request that does not say it accepts them
  (`fetch`, `curl`, the HTTP client of a test) gets the format of the file.

## What the server answers

Everything a theme talks to on a storefront:

| Area | Endpoints |
|---|---|
| Pages | `/`, `/products/<handle>`, `/collections`, `/collections/<handle>`, `/collections/<handle>/<tag>`, `/collections/<handle>/products/<handle>`, `/pages/<handle>`, `/blogs/<handle>`, `/blogs/<handle>/tagged/<tag>`, `/blogs/<blog>/<article>`, `/cart`, `/search`, `/policies/<handle>`, `/gift_cards/<shop id>/<token>`, `/password`, `/account`. A 404 for the rest. Each also under a locale prefix (`/fr/...`). |
| Customer accounts | With new customer accounts, `/account` is a page of local-storefront where the visitor chooses who is logged in; `/account/login`, `/customer_authentication/login` and the other account URLs redirect to it. `/company_location/update?location_id=<id>&return_to=<path>` changes the location of a B2B customer. |
| Robots and sitemaps | `/robots.txt` (from `templates/robots.txt.liquid`, or Shopify's default rules), `/sitemap.xml` and `/sitemap_<products|pages|collections|blogs>_1.xml`. |
| Alternate templates | `?view=<suffix>` and the `template_suffix` of the resource. |
| Section Rendering API | `?section_id=<id>` and `?sections=<id>,<id>` on any page. An id the page does not have is a 404 for `section_id` and `null` in `sections`; either way `x-lsf-not-found` names it and lists the ids the page has. |
| Cart Ajax API | `GET /cart.js`, `POST /cart/add.js`, `/cart/change.js`, `/cart/update.js`, `/cart/clear.js` (with `sections` and `sections_url`), and the form posts `/cart/add`, `/cart`. Stock limits answer `422` like Shopify. |
| Product and search JSON | `/products/<handle>.js`, `/products.json`, `/collections/<handle>/products.json`, `/search/suggest.json`, `/search/suggest?section_id=`, `/recommendations/products.json`, `/recommendations/products?section_id=&product_id=`. |
| Forms | Contact, newsletter (`customer`), blog comment, customer logout, localization (country and language), storefront password. The outcome shows in `form.posted_successfully?` and `form.errors` on the next page, once. A wrong storefront password comes back to `/password` with the error; the right one (`password`, or `shop.password`) goes to the home page. |
| CDN | `/cdn/shop/t/1/assets/<file>` (including `.liquid` assets), `/cdn/shop/files/<path>` with image transformations, the compiled `{% stylesheet %}` and `{% javascript %}` bundles, fonts. |
| Control | `/__lsf` (a status page), `/__lsf/status`, `/__lsf/session`, `DELETE /__lsf/sessions`, `/__lsf/login?customer=<email>`, `/__lsf/schema/<kind>`, `POST /__lsf/reload`, `GET` and `DELETE /__lsf/timings`, `/__lsf/profile?path=<path>`. With `--app`, those of the checkout and the Admin API too ([control API](control-api.md#apps-checkout-and-admin-api)). |

Not simulated: Shopify's checkout (`/checkout` shows a summary of the cart, or, with `--app`,
the local [checkout](checkout.md) of a Shopify app, unless `--only` leaves the checkout out), classic customer accounts (the theme's `templates/customers`; its account forms lead to `/account`),
and what other apps serve (with `--app`, the [theme app extensions](storefront.md#theme-app-extensions) of that app render). See [compatibility.md](compatibility.md).

## Reproducibility

Renders are deterministic: ids, handles, asset versions (`?v=`, derived from file content) and
default dates do not depend on the machine or the run. The one moving part is the clock. Set
`"now"` in the data when the theme shows relative dates, countdowns or "new" badges.

`npx local-storefront render /products/ceramic-mug` prints the HTML of a page without starting a
server, which is handy for snapshot tests of the markup:

```bash
npx local-storefront render '/collections/all?sort_by=price-ascending' --strict > collection.html
```

`--strict` makes it exit with an error when the page has Liquid errors.

## In CI

A complete workflow, for GitHub Actions. `npm ci` installs what `package-lock.json` locks: run
`npm install --save-dev local-storefront @playwright/test` once, and commit `package.json` and
`package-lock.json`.

```yaml
# .github/workflows/storefront.yml
name: storefront
on: [push, pull_request]
jobs:
  storefront:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: actions/setup-node@v7
        with:
          node-version: 24 # 18 or later
          cache: npm
      - run: npm ci
      - run: npx local-storefront validate --format github # the store data
      - run: npx local-storefront check --format github # what Shopify refuses on upload
      - name: Render every page, and fail on a Liquid error
        run: |
          npx local-storefront routes --all > routes.txt
          failed=0
          while read -r route; do
            npx local-storefront render "$route" --strict --format github > /dev/null || failed=1
          done < routes.txt
          exit $failed
      # - run: npx local-storefront fmt --check # uncomment once `fmt` has run in its own commit
      - run: npx playwright install --with-deps chromium
      - run: npx playwright test
```

- `check` reads the files and renders none. A Liquid error at render time (a division by zero)
  shows only when the page renders: with `render --strict`, or in a test.
- `routes --all` lists every page of the store data, each again in the other languages, a
  search, and the sections the theme fetches (product recommendations, predictive search);
  `render` answers each one, so the loop renders the whole store. `/account`, a page of lsf's,
  is not listed. The step stops at once when `routes` fails (a wrong `--theme`), renders every
  route, and fails at the end if one has a Liquid error. Name the variable `route`, not `path`:
  in zsh, `path` is `PATH`, and the loop would lose `npx`.
- `--format github` shows each problem on its line of the pull request.
- `fmt --check` fails on a theme no one has formatted: Shopify's Horizon has 28 files to
  format. Run `npx local-storefront fmt` first, in a commit of its own, then uncomment the step.
- The steps before Playwright take a few seconds, and fail before the browser tests start.
- For an app: [its checkout in CI](checkout.md#in-ci), and [its backend](admin.md#test-your-backend).

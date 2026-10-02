# End-to-end testing against `lsf`

`lsf serve` gives a theme a storefront that needs no network, has no rate limit and renders a
page in a few milliseconds. This guide shows how to drive it from a test runner. The examples
use Playwright; nothing is specific to it.

## Starting the server

```bash
lsf serve --theme path/to/theme --port 9292 --static --quiet --strict
```

| Flag | Why in tests |
|---|---|
| `--static` | Reads the theme and the data once. Fastest, and nothing changes under a running test. |
| `--strict` | Refuses to start when the store data has errors, instead of serving what is valid. |
| `--quiet` | No request log. |
| `--port 0` | Picks a free port; the chosen address is printed as `ready:  http://127.0.0.1:<port>/`. |
| `--host 0.0.0.0` | Listens on every interface, e.g. inside a container. |
| `--throttle <rules>` | Answers requests late: a preset (`simulated`, `slow`) or rules by kind. See [Simulating a slow network](#simulating-a-slow-network). |

Leave `--live-reload` off: it injects a script into the pages.

With Playwright, let the runner start and stop the server:

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  fullyParallel: true,
  use: { baseURL: 'http://127.0.0.1:9292' },
  webServer: {
    command: 'npx local-storefront serve --port 9292 --static --quiet --strict',
    url: 'http://127.0.0.1:9292/__lsf/status',
    reuseExistingServer: !process.env.CI,
  },
});
```

## Sessions: every test is isolated

The server keeps one **session** per browser: the cart, the logged-in customer, the selected
country, and optionally data of its own. A session is identified by the `_lsf_session` cookie,
which the server sets on the first response. A Playwright test gets a fresh browser context,
hence a fresh cookie jar, hence its own session: tests can run in parallel against one server
without seeing each other's carts.

Clients without a cookie jar (curl, an API test) can name their session with a header instead:

```bash
curl -H 'x-lsf-session: my-test' http://127.0.0.1:9292/cart.js
```

## Putting a session in a known state

`PUT /__lsf/session` replaces the state of the caller's session. The body is a
[session](data-reference.md#session):

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
  await expect(page.getByText('Organic Cotton T-Shirt')).toBeVisible();
});
```

Variants are referenced by SKU, by id, or by a product handle (its first variant). When the
body is wrong the answer is `422` with the same diagnostics as `lsf validate --format json`:

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
  others are **added**. Customers are matched by email, menus by handle, gift cards by code.
- `shop` is merged field by field. `localization`, `session`, `now` replace what the files say.
- `theme_settings` and `files` are merged key by key.

Other sessions keep seeing the data on disk. This is the way to test edge cases (an empty
collection, a product with 100 variants, a sold-out state, a long title, another currency)
without growing the shared fixtures.

`GET /__lsf/session` returns the current state, and `DELETE /__lsf/session` resets the
session to what the data files define.

## Simulating a slow network

A local server answers in milliseconds, so loading states flash by before a test can see
them. `lsf serve --throttle simulated` answers each kind of request as late as a real Shopify
storefront does; `--throttle cart-add=1s` delays one kind. The presets, the kinds of requests
and the syntax of the rules are in the [README](../README.md#throttling).

A throttle on the command line slows the whole suite. To slow one test, give its session a
throttle of its own, which replaces the server's for that session:

```ts
test('the button shows a spinner while the item is added', async ({ page }) => {
  await page.request.put('/__lsf/session', {
    data: { throttle: { 'cart-add': '1s' } },
  });
  await page.goto('/products/ceramic-mug');
  await page.getByRole('button', { name: 'Add to cart' }).click();
  await expect(page.locator('.add-to-cart .spinner')).toBeVisible();
});
```

`throttle` takes the same rules as the command line: a string (`"simulated"`,
`"simulated,cart-add=2s"`), a number of milliseconds for everything, or an object
(`{ "preset": "slow", "image": 0 }`). `"none"` lifts the server's throttle for the session.
The control API is never delayed, and a delayed response carries the header
`x-lsf-throttle: 1000ms`.

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
      if (errors) throw new Error(`${response.url()} rendered with ${errors} Liquid error(s)`);
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
- **The cart.** `GET /cart.js` returns the cart in Shopify's Ajax API format.
- **Images.** A response with `x-lsf-placeholder: 1` is a generated placeholder: the data
  references a file that is not in `files/`. Placeholders have the declared size, so layout
  assertions hold; put real files in `files/` for visual regression tests.

## What the server answers

Everything a theme talks to on a storefront:

| Area | Endpoints |
|---|---|
| Pages | `/`, `/products/<handle>`, `/collections`, `/collections/<handle>`, `/collections/<handle>/<tag>`, `/collections/<handle>/products/<handle>`, `/pages/<handle>`, `/blogs/<handle>`, `/blogs/<handle>/tagged/<tag>`, `/blogs/<blog>/<article>`, `/cart`, `/search`, `/policies/<handle>`, `/gift_cards/<shop id>/<token>`, `/password`, `/account` and, with legacy customer accounts, `/account/login`, `/account/register`, `/account/addresses`, `/account/orders/<id>`. A 404 for the rest. Each also under a locale prefix (`/fr/...`). |
| Customer accounts | With new customer accounts, `/account` is a page of `lsf` where the visitor chooses who is logged in; `/account/login`, `/customer_authentication/login` and the other account URLs redirect to it. `/company_location/update?location_id=<id>&return_to=<path>` changes the location of a B2B customer. |
| Robots and sitemaps | `/robots.txt` (from `templates/robots.txt.liquid`, or Shopify's default rules), `/sitemap.xml` and `/sitemap_<products|pages|collections|blogs>_1.xml`. |
| Alternate templates | `?view=<suffix>` and the `template_suffix` of the resource. |
| Section Rendering API | `?section_id=<id>` and `?sections=<id>,<id>` on any page. |
| Cart Ajax API | `GET /cart.js`, `POST /cart/add.js`, `/cart/change.js`, `/cart/update.js`, `/cart/clear.js` (with `sections` and `sections_url`), and the form posts `/cart/add`, `/cart`. Stock limits answer `422` like Shopify. |
| Product and search JSON | `/products/<handle>.js`, `/products.json`, `/collections/<handle>/products.json`, `/search/suggest.json`, `/search/suggest?section_id=`, `/recommendations/products.json`, `/recommendations/products?section_id=&product_id=`. |
| Forms | Contact, newsletter (`customer`), blog comment, customer login and logout, localization (country and language), storefront password. The outcome shows in `form.posted_successfully?` and `form.errors` on the next page, once. A wrong storefront password comes back to `/password` with the error; the right one (`password`, or `shop.password`) goes to the home page. |
| CDN | `/cdn/shop/t/1/assets/<file>` (including `.liquid` assets), `/cdn/shop/files/<path>` with image transformations, the compiled `{% stylesheet %}` and `{% javascript %}` bundles, fonts. |
| Control | `/__lsf` (a status page), `/__lsf/status`, `/__lsf/session`, `/__lsf/login?customer=<email>`, `/__lsf/schema/<kind>`, `POST /__lsf/reload`. |

Not simulated: checkout (`/checkout` shows a summary of the cart and nothing else), customer
registration, password reset and address editing (the forms answer with an error saying so),
and anything served by apps. See [compatibility.md](compatibility.md).

## Reproducibility

Renders are deterministic: ids, handles, asset versions (`?v=`, derived from file content) and
default dates do not depend on the machine or the run. The one moving part is the clock. Set
`"now"` in the data when the theme shows relative dates, countdowns or "new" badges.

`lsf render /products/ceramic-mug` prints the HTML of a page without starting a server, which
is handy for snapshot tests of the markup:

```bash
lsf render '/collections/all?sort_by=price-ascending' --strict > collection.html
```

`--strict` makes it exit with an error when the page has Liquid errors.

## In CI

With `local-storefront` in the dev dependencies of the theme, `npm ci` installs `lsf`:

```yaml
- run: npm ci
- run: npx local-storefront validate   # fail fast on bad fixtures
- run: npx local-storefront check      # every Liquid file parses, every filter exists
- run: npx playwright test
```

`lsf validate` and `lsf check` take a fraction of a second, so they are worth running before
the browser tests.

# End-to-end testing against `slt`

`slt serve` gives a theme a storefront that needs no network, has no rate limit and renders a
page in a few milliseconds. This guide shows how to drive it from a test runner. The examples
use Playwright; nothing is specific to it.

## Starting the server

```bash
slt serve --theme path/to/theme --port 9292 --static --quiet --strict
```

| Flag | Why in tests |
|---|---|
| `--static` | Reads the theme and the data once. Fastest, and nothing changes under a running test. |
| `--strict` | Refuses to start when the store data has errors, instead of serving what is valid. |
| `--quiet` | No request log. |
| `--port 0` | Picks a free port; the chosen address is printed as `ready:  http://127.0.0.1:<port>/`. |
| `--host 0.0.0.0` | Listens on every interface, e.g. inside a container. |

Leave `--live-reload` off: it injects a script into the pages.

With Playwright, let the runner start and stop the server:

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  fullyParallel: true,
  use: { baseURL: 'http://127.0.0.1:9292' },
  webServer: {
    command: 'slt serve --port 9292 --static --quiet --strict',
    url: 'http://127.0.0.1:9292/__slt/status',
    reuseExistingServer: !process.env.CI,
  },
});
```

## Sessions: every test is isolated

The server keeps one **session** per browser: the cart, the logged-in customer, the selected
country, and optionally data of its own. A session is identified by the `_slt_session` cookie,
which the server sets on the first response. A Playwright test gets a fresh browser context,
hence a fresh cookie jar, hence its own session: tests can run in parallel against one server
without seeing each other's carts.

Clients without a cookie jar (curl, an API test) can name their session with a header instead:

```bash
curl -H 'x-slt-session: my-test' http://127.0.0.1:9292/cart.js
```

## Putting a session in a known state

`PUT /__slt/session` replaces the state of the caller's session. The body is a
[session](data-reference.md#session):

```ts
import { test, expect } from '@playwright/test';

test('a logged-in customer sees their cart', async ({ page }) => {
  // page.request shares its cookies with the page.
  const response = await page.request.put('/__slt/session', {
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
body is wrong the answer is `422` with the same diagnostics as `slt validate --format json`:

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

Fail the test on it, so that a wrong fixture never looks like a theme bug:

```ts
async function setSession(page: Page, session: object) {
  const response = await page.request.put('/__slt/session', { data: session });
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

`GET /__slt/session` returns the current state, and `DELETE /__slt/session` resets the
session to what the data files define.

## What to assert on

- **The page itself.** The HTML is what Shopify would render for the same data: section and
  block wrappers (`#shopify-section-<id>`, `.shopify-block`), form markup, image URLs and
  `srcset`s, money formats and translations follow Shopify's output.
- **Liquid errors.** A page with Liquid errors renders them inline, as Shopify does
  (`Liquid error (sections/x line 12): ...`), and the response carries the header
  `x-slt-liquid-errors: <count>`. A blanket check catches regressions on every navigation:

  ```ts
  test.beforeEach(async ({ page }) => {
    page.on('response', (response) => {
      const errors = response.headers()['x-slt-liquid-errors'];
      if (errors) throw new Error(`${response.url()} rendered with ${errors} Liquid error(s)`);
    });
  });
  ```

- **The template.** `x-slt-template` names the template that rendered the page (`product`,
  `product.alternate`, `404`, ...).
- **The cart.** `GET /cart.js` returns the cart in Shopify's Ajax API format.
- **Images.** A response with `x-slt-placeholder: 1` is a generated placeholder: the data
  references a file that is not in `files/`. Placeholders have the declared size, so layout
  assertions hold; put real files in `files/` for visual regression tests.

## What the server answers

Everything a theme talks to on a storefront:

| Area | Endpoints |
|---|---|
| Pages | `/`, `/products/<handle>`, `/collections`, `/collections/<handle>`, `/collections/<handle>/<tag>`, `/collections/<handle>/products/<handle>`, `/pages/<handle>`, `/blogs/<handle>`, `/blogs/<handle>/tagged/<tag>`, `/blogs/<blog>/<article>`, `/cart`, `/search`, `/policies/<handle>`, `/gift_cards/<shop id>/<token>`, `/password`, `/account`, `/account/login`, `/account/register`, `/account/addresses`, `/account/orders/<id>`, and a 404 for the rest. Each also under a locale prefix (`/fr/...`). |
| Alternate templates | `?view=<suffix>` and the `template_suffix` of the resource. |
| Section Rendering API | `?section_id=<id>` and `?sections=<id>,<id>` on any page. |
| Cart Ajax API | `GET /cart.js`, `POST /cart/add.js`, `/cart/change.js`, `/cart/update.js`, `/cart/clear.js` (with `sections` and `sections_url`), and the form posts `/cart/add`, `/cart`. Stock limits answer `422` like Shopify. |
| Product and search JSON | `/products/<handle>.js`, `/products.json`, `/collections/<handle>/products.json`, `/search/suggest.json`, `/search/suggest?section_id=`, `/recommendations/products.json`, `/recommendations/products?section_id=&product_id=`. |
| Forms | Contact, newsletter (`customer`), blog comment, customer login and logout, localization (country and language), storefront password. The outcome shows in `form.posted_successfully?` and `form.errors` on the next page, once. |
| CDN | `/cdn/shop/t/1/assets/<file>` (including `.liquid` assets), `/cdn/shop/files/<path>` with image transformations, the compiled `{% stylesheet %}` and `{% javascript %}` bundles, fonts. |
| Control | `/__slt` (a status page), `/__slt/status`, `/__slt/session`, `/__slt/schema/<kind>`, `POST /__slt/reload`. |

Not simulated: checkout (`/checkout` shows a summary of the cart and nothing else), customer
registration, password reset and address editing (the forms answer with an error saying so),
and anything served by apps. See [compatibility.md](compatibility.md).

## Reproducibility

Renders are deterministic: ids, handles, asset versions (`?v=`, derived from file content) and
default dates do not depend on the machine or the run. The one moving part is the clock. Set
`"now"` in the data when the theme shows relative dates, countdowns or "new" badges.

`slt render /products/ceramic-mug` prints the HTML of a page without starting a server, which
is handy for snapshot tests of the markup:

```bash
slt render '/collections/all?sort_by=price-ascending' --strict > collection.html
```

`--strict` makes it exit with an error when the page has Liquid errors.

## In CI

```yaml
- run: cargo install --path crates/cli   # or download a prebuilt `slt`
- run: slt validate --theme theme        # fail fast on bad fixtures
- run: slt check --theme theme           # every Liquid file parses, every filter exists
- run: npx playwright test
```

`slt validate` and `slt check` take a fraction of a second, so they are worth running before
the browser tests.

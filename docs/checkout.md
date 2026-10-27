# Checkout

Run the Shopify Functions and show the checkout UI extensions of a Shopify app, on your machine.
local-storefront serves a fake checkout for it: no store, no payment, and no network except the
requests of the Functions that have [network access](#network-access). It is a way to
see what your Functions do to a cart, and how your extensions look and behave, not a copy of
Shopify's checkout.

## Run it

```bash
npx local-storefront checkout --app ../my-app
```

Open <http://127.0.0.1:9292/checkout>. The checkout shows the cart of the session, which the
store data defines (`session.cart`, see [data format](data-format.md#customers-and-sessions)).
It needs no theme.

With a theme, `serve` does the same and the cart form of the theme leads to that checkout:

```bash
npx local-storefront serve --app ../my-app
```

The app is a directory with `shopify.app.toml` and `extensions/*/shopify.extension.toml`. API
versions 2026-10 and 2027-01 are supported. Functions run from their built Wasm module and
extensions from their built bundle, which `shopify app build` writes (`dist/function.wasm`,
`dist/<handle>.js`). Build first, or let `--build` run the `[extensions.build] command` of each
Function:

```bash
npx local-storefront checkout --app ../my-app --build
```

What is wrong with the app (a missing build, an unknown target) is printed when the server
starts, and is in the `diagnostics` of the state.

| Option | What it does |
|---|---|
| `--app <dir>` (`LSF_APP`) | The app. Required by `checkout` and `function run`. |
| `--extension <handle>` | Uses only this extension, a Function or a checkout UI extension. Repeat it for several. Without it, every extension of the app is used. |
| `--build` | Runs the build command of each Function first. |
| `--variant <standard\|shop>` | The look of the page: Shopify's one-page checkout, or the Shop app's. A URL can ask for the other with `?variant=shop`. |

`checkout` takes the other options of [`serve`](cli.md#serve) (`--port`, `--host`, `--quiet`,
`--static`, `--customer`, ...).

## Set up the checkout

The `checkout` section of the [store data](data-format.md#checkout) sets what Shopify holds
outside the app: shipping rates, payment methods, the discounts that use your Functions, the
settings of your other Functions, and a tax rate.

```json
{
  "checkout": {
    "shipping_rates": [
      { "handle": "standard", "title": "Standard", "price": "4.90" },
      { "handle": "express", "title": "Express", "price": "12.90" }
    ],
    "payment_methods": [
      { "handle": "card", "name": "Credit card" },
      { "handle": "cod", "name": "Cash on Delivery" }
    ],
    "discounts": [
      {
        "function": "volume-discount",
        "title": "Volume",
        "code": null,
        "classes": ["PRODUCT"],
        "metafields": {
          "$app:config.settings": { "type": "json", "value": { "minimumQuantity": 3, "percentage": 10 } }
        }
      }
    ],
    "functions": {
      "delivery-rules": {
        "metafields": { "$app:config.settings": { "type": "json", "value": { "hide": ["express"] } } }
      }
    },
    "tax_rate": 20
  }
}
```

- A **discount** is a discount Function with its title, its metafields and, if it has one, the
  code that triggers it. Without a code it is automatic. `classes` lists what it may reduce:
  `PRODUCT`, `ORDER`, `SHIPPING`. All three when omitted.
- A Function that is not a discount (cart transform, delivery or payment customization,
  validation) has its metafields under `functions`, by its handle.
- A metafield is written `"namespace.key"`, with a [typed value](data-format.md#metafields-and-metaobjects).
  The input query of a Function reads it; `[extensions.input.variables]` of the Function says
  which one.
- `tax_rate` is a percentage added to the discounted subtotal.

Without the section: one shipping rate, "Standard" at 5.00, the payment methods "Credit card",
"Cash on Delivery" and "Bank Deposit", and no tax.

The rest of what a Function reads comes from the other sections of the store data:

- `buyerIdentity.purchasingCompany` (company, location and `contact`, with their dates, locale
  and, for a location, `ordersCount` and `totalSpent`) from the
  [companies and the customers](data-format.md#b2b-customers).
- `localization.market` and `MailingAddress.market` from
  [`localization.markets`](data-format.md#localization): a country in no market is a market of
  its own.
- `shop.metaobject(handle: {type:, handle:})` from the
  [metaobjects](data-format.md#metafields-and-metaobjects).
- `paymentMethods { placements }` from `payment_methods[].placements` (the list of the checkout
  by default). A payment customization that hides a method for one placement only keeps the
  other, and a method left in no placement is removed.
- `cart.deliverableLines` is the lines that ship: the ones the delivery group holds.

## What runs

Every time the page changes something, the Functions run again, in the order Shopify runs
them:

1. cart transform (`cart.transform.run`)
2. product and order discounts (`cart.lines.discounts.generate.run`, and the older
   `purchase.product-discount.run` and `purchase.order-discount.run`)
3. the delivery groups, from `shipping_rates`
4. delivery customization (`cart.delivery-options.transform.run`, `purchase.delivery-customization.run`)
5. shipping discounts (`cart.delivery-options.discounts.generate.run`, `purchase.shipping-discount.run`)
6. payment customization (`cart.payment-methods.transform.run`, `purchase.payment-customization.run`)
7. validation (`cart.validations.generate.run`, `purchase.validation.run`)

A `linesMerge` of a cart transform takes `quantity` units of each cart line it names (no more than
the line holds). The merged line is as many bundles as the greatest common divisor of those
quantities, and each component is in a bundle as many times as its quantity says divided by that
number: four tees and six totes are two bundles of two tees and three totes. The price of a bundle
is the sum of the prices of its components, `percentageDecrease` applies to it, and the line costs
the price of a bundle times its quantity. The discounts, the validation and the `shopify` global
see that quantity (`lineComponents[].quantity` is the number of units of the component in the whole
line, as for `lineExpand`). The units of a cart line that the merge does not take stay in a line of
their own. Changing the quantity of a merged line in the checkout scales the lines it is made of,
by bundles. Shopify's documentation gives the examples (every merged quantity is the smallest of
the lines) and the units that stay as "a separate item", but does not say how the quantity of the
merged line follows from the quantities: the greatest common divisor is the one rule that keeps
every merged unit in a whole number of identical bundles.

Each Function sees the checkout as the steps before it left it, as on Shopify. A Function that
asks for `cart.discountApplications`, `CartLine.discountAllocations` or
`CartDeliveryGroup.discountAllocations` reads the discounts applied so far: the cart transform
sees none, the delivery customization the product and order discounts, the payment
customization and the validation all of them, shipping included. Discounts run one after the
other, so a discount sees the ones that ran before it. An application has the value of the
discount (`PricingPercentageValue` or `MoneyV2`), its allocation method (`EACH` for a percentage,
`ACROSS` for a fixed amount shared between the lines), what it targets (`EXPLICIT` for the lines
the Function named, `ALL` for the whole order, `ENTITLED` when an order discount leaves lines
out) and what it took off in total; its `metafield` is one of the discount's.

### Network access

A Function with network access has a `.fetch` target next to its `.run` target:
`cart.lines.discounts.generate.fetch`, `cart.delivery-options.discounts.generate.fetch` and
`cart.validations.generate.fetch` (the Discount and Cart and Checkout Validation APIs, in
2026-10 and 2027-01). The checkout runs the `.fetch` target right before its `.run` target, sends
the HTTP request it answers from your machine, and gives the response to the `.run` target as
`fetchResult` (`status`, `body`, `jsonBody`, `header(name:)`). The request and the response
show in the Functions panel and in `lsf function run`.

Shopify's limits apply: `GET` and `POST`; an HTTPS URL (an `http` URL is accepted for
`localhost` and loopback addresses, so a server of the app under development can answer); a
`policy.readTimeoutMs` from 100 to 2000; 100 KB for the headers and the body of the response; no
redirect followed; a UTF-8 body. A request that is out of these limits fails the `.fetch`
target, and its `.run` target does not run. A request that cannot be made is a response,
as on Shopify: `504` after the timeout, `502` when the connection or the TLS handshake fails
or the response is too big, `501` for another charset, with Shopify's body (`504 Gateway
Timeout`). A `.fetch` target that answers no request leaves `fetchResult` null, and its input
query cannot ask for `fetchResult`.

Not simulated: the JWT Shopify signs the request with (`x-shopify-request-jwt` needs the secret
of the app) and its cache of responses (every run of the Function sends its request, so a page
that changes the checkout sends it again). The pickup point and local pickup generators, which
have `.fetch` targets too, are not run.

A validation Function can check a rule at the placing of the order only: `buyerJourney.step` is
`CHECKOUT_INTERACTION` when the page loads (and for `.json`) or changes (`update`, a line included),
and `CHECKOUT_COMPLETION` for `complete` alone. The errors of an interaction show on the page and
refuse nothing; `complete` runs the Functions again, as a completion, and only its errors refuse
the order. (Shopify's documentation does not say which step blocks; this is how the local checkout
treats them.)

The input of each Function is the result of its input query, run against the cart, the buyer and
the store data. A Function that fails to run adds an error that blocks the order. The
"Functions" panel of the page shows each run: its input, its output, its logs and
its instructions (Shopify stops a Function past 11 million).

The checkout UI extensions render where their targets say: in the page, one sandboxed frame
each. They read the checkout through the `shopify` global and change it with the `apply*Change`
methods, `buyerJourney.intercept` can block the payment, and both the Polaris web components and
the older React components work.

An extension needs a supported `api_version`, or it is reported as an error and not shown. From
2027-01, each `[[extensions.metafields]]` entry needs an `owner_type` (`CART`, `CUSTOMER`, ...),
or the loader reports an error and the page says why it does not load the extension.

## Run a Function without a server

```bash
npx local-storefront function run volume-discount --app ../my-app
```

Builds the input of each Function from the store data, runs them in checkout order, and prints
each input, output and log with the checkout that results. Without a handle, every Function of
the app runs.

```text
== volume-discount · cart.lines.discounts.generate.run (cart_lines_discounts_generate_run)
input:
  { … }
output:
  { … }
logs:
  volume discount: config { minimum quantity 3 }, code None
instructions: 34977

== checkout
  3 × Organic Cotton T-Shirt (White / S)  96.00 USD → 86.40 USD
  discount 10% off 3+ items: -9.60 USD (Product, volume-discount)
  delivery * Standard (standard): 4.90 USD
  subtotal 96.00 USD · discounts -9.60 USD · shipping 4.90 USD · tax 0.00 USD · total 91.30 USD
```

| Option | What it does |
|---|---|
| `--session <file or JSON>` | The session to run for: a [session](data-reference.md#session) with a `cart`, a `customer`, a `country`. Defaults to the `session` of the store data. |
| `--checkout <file or JSON>` | What the buyer entered: `email`, `phone`, `shippingAddress`, `selectedDelivery`, `selectedPayment`, `discountCodes`, `metafields`. |
| `--locale <iso>` | The language of the buyer, one of the store's (`fr`, `pt-BR`): Functions read it as `localization.language`. Defaults to the primary language. |
| `--step interaction\|completion` | The step of the buyer journey, which validation Functions read as `buyerJourney.step`: `completion` (default) is the placing of the order, `interaction` is the page and its updates. |
| `--format json` | The checkout as the page reads it, with every run in `runs`. For tools. |
| `--build` | Runs the build command of each Function first. |

It exits with `1` when a Function fails to run (no module, a trap, an output that does not
parse). Errors that a validation Function reports are part of the checkout, not a failure.

## Test it

The checkout is part of the [control API](control-api.md). A test puts a session in a known
state, then drives the checkout like the page does:

```ts
await request.put('/__lsf/session', {
  data: { cart: { items: [{ variant: 'TEE-BLK-M', quantity: 3 }] } },
});
const checkout = await request.get('/checkout', { maxRedirects: 0 });
const path = checkout.headers()['location']; // /checkouts/cn/<token>

const state = await (await request.get(`${path}.json`)).json();
await request.post(`${path}/update`, {
  data: { operations: [{ type: 'email', value: 'buyer@example.com' }] },
});
```

| Request | What it does | Answer |
|---|---|---|
| `GET /checkout` | Leads to the checkout of the session's cart. `?variant=shop` for the other look. | A redirect to `/checkouts/cn/<token>` (`/fr/checkouts/cn/<token>` from `/fr/checkout`) |
| `GET /checkouts/cn/<token>` | The page. | HTML, with the state in it |
| `GET /checkouts/cn/<token>.json` | The state of the checkout. | JSON |
| `POST /checkouts/cn/<token>/update` | Applies `{"operations": [...]}`, in order. | `200` and the state, or `400` `{"error": "…"}` and nothing applied |
| `POST /checkouts/cn/<token>/complete` | Places the order. | `200` `{"redirect": "…/thank-you"}`, `422` and the state with `errors`, or `409` and the fresh state when the cart changed meanwhile |
| `GET /checkouts/cn/<token>/thank-you` | The page of the order. | HTML |
| `GET /__lsf/checkout?session=<id>` | The state of a session's checkout, from outside its browser. | JSON |

The token is the one of the session's cart. Another session gets a `404`.

### Language

The checkout is in the language its URL names, as the storefront is: every path above also
exists under a locale prefix (`/fr/checkout`, `/fr/checkouts/cn/<token>`,
`/fr/checkouts/cn/<token>/update`, …). The cart form of a theme leads to `/fr/checkout` from the
French storefront, the redirect keeps the prefix, and so do the page's own requests, the links
back to the storefront and the way to the thank-you page. Without a prefix the checkout is in
the primary language of the store. `/__lsf/checkout` has no prefix to read: it takes
`?locale=fr`.

The state tells both languages apart: `shop.locale` is the primary language of the store, and
`localization.language` the one the buyer reads. Extensions get the latter as
`shopify.localization.language`, translate and format numbers and money with it, and the page
sets it as the `lang` of the document. The Functions read it too, as `localization.language` of
their input. A thank-you page is read in the language of its URL, not the one the order was
placed in. A language that a session adds with `data.localization.languages` is a locale of that
session's checkout.

### Buyer and localization

`buyer` is what `shopify.buyerIdentity` reads: the `email` and `phone` the buyer typed or the
logged-in customer has, `customer` (`id`, `email`, `phone`, `firstName`, `lastName`,
`acceptsMarketing`, `ordersCount`) and, for a B2B customer, `purchasingCompany` (the `company`
and the `location` they buy for, each with `id`, `name` and `externalId`). `localization` is what
`shopify.localization` reads: `language` and `rootUrl` as above, the `country` (the one of the
shipping address, otherwise the one the visitor selected on the storefront), the `currency` and
the `market`. The store has no avatar, no consent to SMS marketing and no store credit for a
customer, nor a time zone for a buyer (`timezone` is the one of the browser).

The state has the lines, discounts, discount codes, delivery groups, payment methods, errors and
cost that the Functions produced, `runs` (a run of a `.fetch` target has its `exchange`, the
request and the response), the buyer, the address, the note, the attributes and
the metafields, the `extensions` to load and `diagnostics`. Money is `{"amount": 19.99}`.

### Operations

| `type` | Fields | Does |
|---|---|---|
| `email`, `phone` | `value` | Sets the contact. An empty value clears it. |
| `shippingAddress` | `address`: `firstName`, `lastName`, `company`, `address1`, `address2`, `city`, `countryCode`, `provinceCode`, `zip`, `phone` | Sets the address. A country of the store becomes the country of the session. |
| `deliveryOption` | `group` (`group-1`), `handle` | Chooses a shipping rate. |
| `paymentMethod` | `handle` | Chooses a payment method. No handle clears it. |
| `discountCode` | `action` (`add` or `remove`), `code` | Enters or takes back a code. |
| `cartLines` | `change`: `addCartLine`, `removeCartLine` or `updateCartLine` of the UI extension API | Changes the cart of the session, the one the theme shows. |
| `attribute` | `change`: `updateAttribute` or `removeAttribute`, with `key` and `value` | Changes the attributes of the cart. |
| `note` | `change`: `updateNote` (`note`) or `removeNote` | Changes the note of the cart. |
| `metafield` | `change`: `updateMetafield`, `updateCartMetafield`, `removeMetafield` or `removeCartMetafield`, with `namespace`, `key`, `valueType`, `value` | Sets a metafield for the Functions to read. |

`complete` refuses an empty cart, an order with errors from a Function, and one without an
email, a payment method or, for a cart that ships, an address. It empties the cart: the order is
what the thank-you page shows. It takes the cart only if the cart is still the one the Functions
ran on: if it changed meanwhile (another tab), nothing is bought and the reply is `409`. Completing
an order that was placed already leads to the same thank-you page, never to a second order.

## What it does not do

- Payments, gift cards, and payment method attributes. Choosing a payment method pays nothing.
- One delivery group holds every line that ships. No pickup points, no split shipments, no
  prices by address: the rates are the same for every address.
- Fulfillment constraints, order routing, and the pickup point and local pickup generators (with
  their `.fetch` targets) are listed and not run.
- Discounts have no `conditions` and no combinations: all of them stack.
- Tax is one rate on the discounted subtotal, with no tax lines.
- The values of the `settings` of an extension are not set: the manifest declares them, and
  `shopify.settings.value` is empty.
- No Storefront API: `query` in an extension rejects.
- Some fields of the Function input hold what the store data says and nothing more. The orders of
  the customers are not counted in the `ordersCount` of a company location, nor spent in its
  `totalSpent` (set them in the data); a `Market` is a country, or the group of countries that
  `localization.markets` declares, with no catalog or price list; the `id` of a `Metaobject`
  is made up from its type and handle.
- Customer accounts hosted by Shopify, Shop Pay and express checkout.
- With `checkout` (no theme), the links to the storefront (return to cart, continue shopping)
  lead to pages the server does not have. Use `serve --app` for those.

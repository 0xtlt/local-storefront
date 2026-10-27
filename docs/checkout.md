# Checkout

Run the Shopify Functions and show the checkout UI extensions of a Shopify app, on your machine.
local-storefront serves a fake checkout for it: no store, no network, no payment. It is a way to
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
cost that the Functions produced, `runs`, the buyer, the address, the note, the attributes and
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
- Fulfillment constraints, order routing, and the pickup point and local pickup generators are
  listed and not run. So are the `.fetch` targets.
- Discounts have no `conditions` and no combinations: all of them stack.
- Tax is one rate on the discounted subtotal, with no tax lines.
- The values of the `settings` of an extension are not set: the manifest declares them, and
  `shopify.settings.value` is empty.
- No Storefront API: `query` in an extension rejects.
- A few fields of the Function input have no local data, and a query that asks for them fails
  with the field's path: `fetchResult`, `Cart.deliverableLines`, the discount applications and
  allocations of the cart, its lines and its delivery groups, `PurchasingCompany.contact`, the
  dates, locale and totals of companies and their locations, `MailingAddress.market`,
  `PaymentCustomizationPaymentMethod.placements` and `Shop.metaobject`. A merged bundle line
  always has a quantity of 1.
- Customer accounts hosted by Shopify, Shop Pay and express checkout.
- With `checkout` (no theme), the links to the storefront (return to cart, continue shopping)
  lead to pages the server does not have. Use `serve --app` for those.

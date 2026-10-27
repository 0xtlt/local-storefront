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

Open <http://127.0.0.1:9292/>: the [home page](#the-home-page) lists the extensions of the app
and the [checkout scenarios](#scenarios), and opens a checkout for each. <http://127.0.0.1:9292/checkout>
is the checkout of the cart of the session, which the store data defines (`session.cart`, see
[data format](data-format.md#customers-and-sessions)). It needs no theme.

With a theme, `serve` does the same and the cart form of the theme leads to that checkout. The
theme owns `/`, so the home page is at <http://127.0.0.1:9292/__lsf/checkouts>, and the server
prints its address when it starts:

```bash
npx local-storefront serve --app ../my-app
```

No scenarios yet? `npx local-storefront checkout init --app ../my-app` writes a set made for
the extensions of the app, see [scenarios](#scenarios).

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

## The home page

`GET /` of `lsf checkout` (`/__lsf/checkouts` with a theme; `?format=json` answers what the
page says, for tools) shows everything the checkout can do for the app, with the look of the
checkout and without any network:

- **Every Function:** its handle and name, the API version, the Function API of each target, whether
  it is built (and the command that builds it), whether it has a `.fetch` target (network
  access) and whether a target is legacy or not run by the local checkout. What is wrong with it
  (a missing build, an unknown target, an API version that is not supported) shows as a banner.
- **Every checkout UI extension:** its handle, API version, capabilities and targets, each with
  the place it has on the page ("Checkout summary: under each line"). A target the page has no
  place for (`purchase.checkout.chat.render`, the pickup point targets) is said to have none: the
  extension does not render there.
- **Every [scenario](#scenarios):** a summary of its cart, buyer, country and language, what the
  buyer entered, discounts, settings it overrides, look and extensions, and a link that opens a
  fresh checkout for it. A scenario with errors is listed with them and has no link.
- **A link that opens a checkout where every extension renders at every target it declares**
  ([`?targets=all`](#every-extension-target)), and one for the Shop look.

An extension that `--extension` leaves out is listed and marked so. The page follows the locale
prefix of its URL (`/fr/` links to `/fr/checkout`; `/__lsf/checkouts?locale=fr` does the same).

## Scenarios

A scenario is a named checkout setup: one file of `checkouts/` in the
[store data](data-format.md#checkout-scenarios) (`<data>/checkouts/<name>.json`, the data folder
being `--data`, `LSF_DATA` or `shopify-local/`). It sets what a checkout depends on, on top of
the store data: the cart (lines, quantities, selling plans, note, attributes), the buyer (a guest,
a customer, a company location), the country, the language, what the buyer already entered
(email, address, discount codes, delivery and payment), overrides of the `checkout` section
(shipping rates, payment methods, discounts, Function settings, tax), the look and the
extensions that run. The files are reloaded when they change, and `validate` checks them.

```json
{
  "title": "Three tees in French",
  "session": { "customer": "jane.doe@example.com", "cart": { "items": [{ "variant": "TEE-BLK-M", "quantity": 3 }] } },
  "entered": { "discount_codes": ["WELCOME"] },
  "language": "fr",
  "checkout": { "tax_rate": 20 },
  "variant": "shop"
}
```

- `GET /checkout?scenario=<name>` opens it: a **new session** (a new cookie, a new checkout token,
  nothing kept from the checkout before, such as what the extensions stored) with the cart,
  buyer and entries of the scenario, then a redirect to its checkout, under the locale root of its
  language. The browser keeps that session, so the theme (with `serve --app`) shows its cart and
  customer too. A scenario that does not exist answers `404`, and one with errors `422`, with a
  page that says what is wrong.
- The checkout keeps the settings and the extensions of its scenario for the whole session, an
  order included, and follows the file when it changes. The state has `scenario`
  (`{name, title}`) and the page names it in its header.
- `lsf function run --scenario <name>` runs the Functions for a scenario.
- Only one scenario is open per browser: the session is the cookie's.

`npx local-storefront checkout init --app ../my-app` writes the scenarios, made for the app:

| The app has | Scenarios |
|---|---|
| a cart transform | `cart-transform`: several lines of more than one unit and one of three, for a merge, an expand or an update |
| a discount Function | `discount-automatic` (a cart of six, three and two units) and `discount-code` (the same cart with `SAVE10` entered; the code of a discount of the store data is kept) |
| a delivery customization | `delivery-options`: standard, express, local delivery and store pickup |
| a payment customization | `payment-methods`: card, PayPal, cash on delivery, bank deposit and two express buttons |
| a validation Function | `validation`: a big cart, for the Function to refuse |
| a checkout UI extension | `all-extensions`: a customer, several lines (one with a subscription), a code, shipping options |
| (always) | `guest`, `customer` (with a note and an attribute), `b2b` (a company customer at another location), `abroad` (another country and language), `subscription`, `pre-order` (a deposit, the rest later), `shop-look`, `large-cart` |

A scenario about one kind of Function runs that Function and the checkout UI extensions, and no
other Function (`extensions`), so that its effect shows alone; `all-extensions` has no such
list. The settings of your Functions are the store data's: the scenarios leave
`checkout.functions` alone, and the `discount-*` scenarios list the discount Functions again with
the settings (and the code) `checkout.discounts` gives them. The scenarios are a place to start:
edit them.

`checkout init` creates the data folder with the demo store when it does not exist (and a
selling plan with a deposit for `pre-order`). When it exists, it uses the products, customers,
countries, languages and selling plans it has, and says which scenarios it skipped and why. It
never overwrites a file without `--force`, and refreshes `schema/` so that editors complete the
scenarios (`"$schema": "../schema/checkout-scenario.schema.json"`).

### Every extension target

`/checkout?targets=all` (or `&targets=all` after `?scenario=`) opens a checkout where each
extension renders at every target it declares: the steps of a cart that ships are shown whatever
the cart, and the thank-you page is shown under the checkout with the same cart (no order is
placed), so the thank-you targets are there too. A panel at the top lists every target of every
extension: shown, without a place on the page, or waiting for something the cart lacks (shipping
methods need an address, a line target needs a line). The `all-extensions` scenario is made for
it.

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
- A line bought with a [selling plan](data-format.md#selling-plans) that defers a part of the
  price is [paid in two times](#deferred-payments).

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

## Deferred payments

A selling plan whose `checkout_charge` is less than the price (a pre-order with a deposit, `0`
for try before you buy) is paid in part at checkout, and the rest later. The storefront cart
reports the part paid at checkout as `cart.checkout_charge_amount`; the checkout does the same
and splits what the buyer owes:

```text
Subtotal · 3 items          96.00
Shipping                     4.90
Total                      100.90
Due today                   38.50
Due later                   62.40
```

- `total` is what the order costs in all, as without a plan. *Due today* (`cost.dueNow`) is
  what the buyer pays, and *due later* (`cost.dueLater`) what the plan defers. The page, the
  Pay button of the Shop skin and the payment line of the thank-you page charge `dueNow`.
- A discount comes off the balance first: it only reduces the deposit once the balance is gone.
  A line with a balance has `dueLater` in the state, and what it owes today is its `total` less
  that.
- Shipping is due today. Tax follows the amount it is on: the tax of the balance is due later.
- `cost.checkoutChargeAmount` is the subtotal less the balances, before discounts, shipping and
  tax: the `cart.checkout_charge_amount` of the cart, for the same lines.
- The order keeps `order.totalReceived` (what was due today) and `order.totalOutstanding` (what
  is due later), named as the `totalReceivedSet` and `totalOutstandingSet` of Shopify's `Order`.
  The thank-you page says the remaining balance is due later. Nothing is charged later.
- A cart transform cannot change a line with a selling plan, as on Shopify.

Shopify documents none of this split in the places an app can read. The Cost API of checkout UI
extensions (2026-10 and 2027-01) has the subtotal, shipping, tax and total (`totalAmount`, the
whole order) and no due-today amount; a line's `merchandise.sellingPlan` has `id` and
`recurringDeliveries`; the input of a Function has the selling plan (`id`, `name`,
`description`, `recurringDeliveries`) and its price adjustments, no checkout charge or balance.
The `shopify` global and the Function input are the same as without a plan, and an extension
reads what is due today from `cost.dueNow` of the page state only, not from the API. What
Shopify's help center says is that the deposit due at checkout shows in the order summary, and
that the remaining payment is collected later. How discounts, shipping and tax split between
today and later, and the labels, are not documented: those above are this checkout's choices.

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
customization and the validation all of them, shipping included. The discount Functions are the
exception: Shopify runs them concurrently, with no knowledge of one another, so none of them
(`cart.lines.discounts.generate.run`, `cart.delivery-options.discounts.generate.run` and the
legacy ones) sees a discount, whether it comes from another discount or an earlier stage. Their
answers are applied once all of them have run: product discounts first, then order discounts on
what is left of the lines, then shipping, so the order of the discounts in the configuration
changes nothing (see [how discounts stack](#how-discounts-stack)). An application has the value of the
discount (`PricingPercentageValue` or `MoneyV2`), its allocation method (`EACH` for a percentage,
`ACROSS` for a fixed amount shared between the lines), what it targets (`EXPLICIT` for the lines
the Function named, `ALL` for the whole order, `ENTITLED` when an order discount leaves lines
out) and what it took off in total; its `metafield` is one of the discount's.

### How discounts stack

Discounts apply class after class, as on Shopify: product discounts first, then order
discounts on the subtotal they left, then shipping. Within a class:

- **Order discounts.** Every percentage is a share of the subtotal after the product discounts,
  not of what the order discounts before it left: 10% and 20% off 100.00 take 10.00 and 20.00,
  70.00 is left. The fixed amounts come off afterwards, from what is left, and never take the
  order below zero (a percentage and a $20 discount on 350.00: 10% leaves 315.00, then 295.00).
- **Product discounts on one line.** The percentages come off before the fixed amounts. A
  percentage is a share of the price of the line before any discount, so 10% and 20% off a line
  of 100.00 take 10.00 and 20.00. A fixed amount never takes more than the line has left.
- **A fixed amount across several lines** (`appliesToEachItem: false`) is taken once: it is
  shared between the lines in proportion to their prices, and what a line cannot take (a
  discount before it took its price) goes to the others, so the amount is taken in full as long
  as the lines have that much left. 30.00 across a free 32.00 line and a 24.00 line takes the
  24.00 of the second.
- **Shipping discounts.** One applies per order, the best one: the one that takes the most off
  the delivery option the buyer selected (then off all the options). The others are not applied
  and are not dropped silently: the Functions panel lists them under "Discounts not applied"
  (`skippedDiscounts` in the state, and a `discount not applied` line in `lsf function run`)
  with their amount and why. 50% off a 4.90 Standard rate and 3.00 off it: the 3.00 applies,
  the 50% (2.45) is listed. Free Express does not beat 1.00 off Standard while the buyer has
  Standard selected, and wins when they select Express. The candidates of one discount are
  one discount: they all apply, one after the other. A tie (the same amount) goes to the
  discount that comes first by Function, title, message and code, not by its place in the
  configuration.

An amount is shared between the lines in proportion to what they cost, to the cent, and the
allocations of the lines (`discountAllocations`, in the page and for the Functions that run
later) add up to the discount.

**What is guaranteed, and what is not.** Product and order discounts come out the same whatever
the order of the discounts in the configuration, of the `[[extensions]]` of the app and of the
candidates in an answer: the checkout applies the candidates of a class in an order of its own.
Percentages first, the larger one first; then the fixed amounts, the one that covers fewer lines
first. The second rule is what makes overlapping fixed amounts predictable: two lines of 100.00,
a fixed 100.00 across both and a fixed 100.00 on the second alone leave 0.00 in either
configuration order (the narrow one first takes the second line, the general one takes the
first), and not 50.00 in one and 0.00 in the other. Candidates that cover the same lines are
told apart by what they would take from each line, their amount, their targets with the
quantities they name, Function, title, code and message, never by their place in the
configuration or in the answer; two that agree on all of these take the same from the same
lines, and only their metafields tell them apart. The order is this
checkout's choice, and so is the sharing of a fixed amount between lines: Shopify documents
neither for overlapping amounts. It is a fixed order, not the one that takes the most in every
case.

Shopify documents the order of the classes, the percentages of order discounts being all
calculated on the subtotal, the percentage before the fixed amount in the order and on a line, and
the fixed amounts after (https://help.shopify.com/en/manual/discounts/discount-combinations).
It does not say how two percentages on one line combine, how two fixed amounts combine, nor
whether a fixed amount may exceed what is left: the rules above for those are this checkout's
choices. It also says that only one shipping discount applies per order and that, when discounts
cannot combine, "the best discount for the customer's cart is always applied". It does not say
how the best shipping discount is measured, what happens to a tie, whether the rule holds for
each delivery group of a split order (this checkout has one group), nor what to show about the
others: measuring by the selected option, breaking ties as above and listing the others are
this checkout's choices.

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
its instructions (Shopify stops a Function past 11 million), and the discounts a Function
answered that were not applied.

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
| `--scenario <name>` | A [scenario](#scenarios) of the store data: its cart, buyer, language, `checkout` settings and the extensions it names. `--session`, `--checkout` and `--locale` go on top of it. |
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
| `GET /checkout` | Leads to the checkout of the session's cart. `?variant=shop` for the other look, `?targets=all` for [every extension target](#every-extension-target). | A redirect to `/checkouts/cn/<token>` (`/fr/checkouts/cn/<token>` from `/fr/checkout`) |
| `GET /checkout?scenario=<name>` | Opens a checkout for a [scenario](#scenarios), in a new session. | A redirect to its checkout with a new session cookie, or `404`/`422` and a page that says why |
| `GET /` (`lsf checkout`) or `GET /__lsf/checkouts` | The [home page](#the-home-page). `?format=json` for what it says. | HTML, or JSON |
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

- Payments, gift cards, and payment method attributes. Choosing a payment method pays nothing,
  and the balance of a [deferred payment](#deferred-payments) is never collected.
- One delivery group holds every line that ships. No pickup points, no split shipments, no
  prices by address: the rates are the same for every address.
- Fulfillment constraints, order routing, and the pickup point and local pickup generators (with
  their `.fetch` targets) are listed and not run.
- Discounts have no `conditions` and no combinations: all of them stack, as described in
  [how discounts stack](#how-discounts-stack). Shopify lets the merchant choose which discounts
  combine, and applies the best of those that cannot; here all the product and order discounts
  combine, and of the shipping discounts the best one applies.
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
- A scenario is one session per browser: opening another replaces it. The scenario cannot set
  what the theme shows outside the cart and the customer (`theme_settings`, a frozen clock).

# Quickstart: Functions and checkout UI extensions

From a Shopify app with a discount Function and a checkout UI extension to a local checkout that
runs them, and a check that fails in CI when the Function decides wrong. No store, no Partner
account, no network.

The examples use an app with two extensions, as `shopify app generate extension` makes them: a
discount Function in TypeScript (`discount-typescript`) and a checkout UI extension in Preact
(`ui-preact`). Without an app, copy these two folders of the
[playground](https://github.com/0xtlt/local-storefront/tree/main/examples/checkout-playground/extensions)
into the `extensions/` of a folder that has a `shopify.app.toml`.

lsf runs the extensions whose `api_version` is `2026-10` or `2027-01`. On another version, a
Function does not run and a UI extension is not offered to the checkout: `validate` says so, and
`app check` fails with the reason (`api_version "2026-07" is not supported (supported: 2026-10,
2027-01)`). Change the `api_version` of its `shopify.extension.toml` to one of the two.

## 1. Install and write scenarios

In the app's folder:

```bash
npm install --save-dev local-storefront
npx local-storefront app init
```

```text
./shopify-local: created, with the demo store
  wrote   checkouts/guest.json
  …
  wrote   checkouts/discount-automatic.json
  wrote   checkouts/discount-code.json
  wrote   checkouts/all-extensions.json
  skipped cart-transform: the app has no cart transform Function
  …
./shopify-local/checkouts: wrote 11 scenarios, kept 0 existing ones, for 1 Function and 1 checkout UI extension of My app
```

A scenario is a cart, a buyer, a country and the extensions to run, in one file of
`shopify-local/checkouts/`. `app init` writes a set made for the app's extensions.

## 2. Give the discount its settings

A discount Function runs only for a discount that names it. `discount-automatic.json` has one;
give it the metafield the Function reads its settings from. The playground's reads
`$app:config` / `settings`; yours reads what its input query names:

```json
"checkout": {
  "discounts": [
    {
      "function": "discount-typescript",
      "title": "Quantity discount (TypeScript)",
      "code": null,
      "metafields": {
        "$app:config.settings": { "type": "json", "value": { "percentage": 10, "minimumQuantity": 3 } }
      }
    }
  ]
}
```

## 3. Build and serve

```bash
npx local-storefront app build        # every Function and UI extension, without Node
npx local-storefront app dev --watch
```

Open <http://127.0.0.1:9292/>: the home page lists what is wrong first, then the scenarios.
Open one to get its checkout, with the Function's discount on the lines and the extension's
banner. `--watch` builds an extension again when you save it, and reloads the page:

```text
↻ building discount-typescript…
✓ discount-typescript built in 0.1s
```

## 4. Run the Function once

```bash
npx local-storefront app function run discount-typescript --scenario discount-automatic
```

It prints the input, the output, the logs and the instructions of the run, then the checkout:

```text
== checkout
  6 × Canvas Tote Bag  144.00 USD → 129.60 USD
  3 × Ceramic Mug (Oat)  66.00 USD → 59.40 USD
  2 × Linen Overshirt (S)  136.00 USD
  discount 10% off 3 or more: -21.00 USD (Product, discount-typescript)
  …
  subtotal 346.00 USD · discounts -21.00 USD · shipping 0.00 USD · tax 0.00 USD · total 325.00 USD
```

When a file of the Function changed after its module was built, `app function run` builds it
first, as `shopify app function run` does (`build:  discount-typescript: … changed after its
module was built: building it first`).

Keep the input lsf built, and run the Function on it later, as `shopify app function run
--input` does:

```bash
npx local-storefront app function run discount-typescript --scenario discount-automatic \
  --save-input tests/fixtures/six-totes.json
npx local-storefront app function run discount-typescript --input tests/fixtures/six-totes.json
```

`--save-input` creates the folders of its file. `--input -` reads the input from stdin, and
`--format json` prints the run as JSON. A Function with several targets needs `--export <name>`.
In an app with one Function, the handle can be left out.

A fixture of the unit tests of Shopify's templates (`{"payload": {"export", "target", "input",
"output"}}`) is checked: its `output` must be the run's, and its `target` and `export` the
Function's. `--input` with a folder runs each of its `*.json` fixtures:

```bash
npx local-storefront app function run --input tests/fixtures
```

```text
pass     tests/fixtures/six-totes.json
fails    tests/fixtures/fifteen.json: the output is not the one the fixture expects:
           operations[0].productDiscountsAdd.candidates[0].value.percentage.value
             expected: 15
             actual:   10
ran 2 fixtures of discount-typescript: 1 passed, 1 failed
```

## 5. Say what the checkout must look like

Add `expect` to the scenario:

```json
"expect": {
  "discounts": { "includes": [{ "title": "10% off 3 or more", "amount": "21.00" }] },
  "lines": { "includes": [{ "sku": "TOTE-NAT", "total": "129.60" }] },
  "totals": { "total": "325.00" }
}
```

```bash
npx local-storefront app check
```

```text
checked 11 of 11 scenarios in 0.0 s: every Function ran, 3 expectations met
```

When the Function decides wrong, `app check` says where, and exits with `1`:

```text
expect   discount-automatic: totals.total
         expected: "300.00"
         actual:   "325.00"
checked 11 of 11 scenarios in 0.0 s: 0 failures, 0 invalid scenarios, 1 of 3 expectations unmet
```

`expect` also checks payment methods, express buttons, delivery options and errors, each with
`includes`, `excludes` and `equals` ([expected results](checkout.md#expected-results)).

## 6. Run it in CI

Commit `package.json`, `package-lock.json` and `shopify-local/`. `dist/` is not committed:
`--build` builds the extensions first.

```yaml
# .github/workflows/checkout.yml
name: checkout
on: [push, pull_request]
jobs:
  checkout:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: actions/setup-node@v7
        with:
          node-version: 24
          cache: npm
      - run: npm ci
      - run: npx local-storefront validate          # the store data, the scenarios and the app
      - run: npx local-storefront app check --build # every Function of every scenario, and each `expect`
```

`app check` fails on a Function that traps, runs past Shopify's instruction limit or answers an
output its schema refuses, and on an `expect` that is not met. It also fails on an extension
the checkout does not offer (an `api_version` lsf does not run, no module or bundle), on a
Function whose files changed after its module was built (`stale`: give `--build`), and on what
Shopify rejects in a theme app extension. It needs no store and no network.

## Coming from Shopify's CLI

`lsf app dev --watch` stands for `shopify app dev`, `lsf app build` for `shopify app build`, and
`lsf app function run --input` for `shopify app function run` (which builds a changed Function
first too). `app function typegen` and
`app function schema` work offline. There is no development store, tunnel or deploy. The full
table, with what lsf does not do: [coming from Shopify's CLI](checkout.md#coming-from-shopifys-cli).

## Next

- [Checkout](checkout.md): scenarios, what runs and why, discounts, network access, the
  customer account.
- [Set up the checkout](checkout.md#set-up-the-checkout): shipping rates, payment methods, and
  the settings of a UI extension.
- [What to test on a real store](checkout.md#what-to-test-on-a-real-store) before a release.
- [Quickstart: an Admin API backend](quickstart-admin-api.md), for the app's backend.

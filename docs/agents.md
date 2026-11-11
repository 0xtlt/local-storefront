# Agents

How an AI agent, or any tool, drives local-storefront: the commands to run, what they print, and
how to read the result without guessing. Every command is non-interactive and exits with a code
a script can test.

## Before anything

- Run `npx local-storefront <command>` in a theme folder. Never `npx lsf`: that is another
  package on npm.
- Point at the theme with `--theme <dir>` and at the data with `--data <dir>` when the current
  folder is not the theme.
- Exit codes: `0` nothing to report, `1` something to fix, `2` the command could not run. See
  the [CLI reference](cli.md#exit-codes).
- Results go to stdout, messages and summaries to stderr.

### What each command prints

| Command | For a tool |
|---|---|
| `validate`, `check` (`--fix` too), `app check`, `app function run` | `--format json`: one JSON document on stdout. |
| `app webhook trigger` | `--format json` (`--json` is the same). |
| `render <path>` | The HTML on stdout. On stderr, each Liquid error, then `status 200 · template product · 0 Liquid errors · 0 warnings`. |
| `routes` | One path per line. |
| `fmt --check` | One file to format per line; the summary on stderr. |
| `profile <path>` | Text. `--json` is a speedscope profile for a flame graph, not a report. |
| `schema [kind]` | A JSON Schema. |
| `init`, `app init` | Text, on stderr. They take no `--format`: `init --format json` suggests `--force`, which overwrites the data. Do not follow it. |
| `app function schema` | On stderr, the file it wrote (`wrote …/schema.graphql: the payment_customization API, 2027-01`). |
| `serve`, `app dev` | Its lines on stderr; the state as JSON at `GET /__lsf/status`. |

## Tasks

### Set up a theme

```bash
npm install --save-dev local-storefront
npx local-storefront init
```

`init` writes `shopify-local/` with a demo store, and prints on stderr
`./shopify-local: wrote 16 files, kept 0 existing ones, refreshed schema/`. It keeps the
files that exist, so it is safe to run again (`wrote 0 files, kept 16 existing ones`).
`--force` overwrites them. Exit `0`.

To use your own products instead of the demo's, delete `shopify-local/products/*.json`,
`customers.json` and `gift_cards.json`, write yours (next task), then run
`npx local-storefront validate --format json`. The demo's collections, menus and subscriptions
still name its products: each one is an `unknown_product` or an `unknown_reference`, at its
`file` (`collections.json`, `menus.json`, `store.json`) and `path`. Replace or remove each.
Remove from the last `path` to the first: removing an item of a list moves the `path` of the
items after it. A collection or a selling-plan group may be left with no products
(`"products": []`).

The demo's Apparel, Home goods, Accessories and Sale collections pick products by `type`
(`Apparel`, `Home`, `Accessories`) and by the tag `sale`: give yours a `type` and `tags`, or
change the `rules` of each collection. Otherwise they are empty, and every check still passes.
The shop's name is `shop.name` in `store.json` (`Local Supply Co.`), and the pages and articles
of `pages.json` and `blogs.json` are the demo's too.

### Write store data

One file per product, `shopify-local/products/trail-runner.json`:

```json
{
  "title": "Trail Runner",
  "price": "129.00",
  "compare_at_price": "149.00",
  "options": ["Size"],
  "variants": [
    { "options": ["42"], "sku": "TRAIL-42", "inventory_quantity": 5 },
    { "options": ["43"], "sku": "TRAIL-43", "inventory_quantity": 0 }
  ],
  "images": ["products/trail-runner.jpg"]
}
```

The customers, `shopify-local/customers.json`:

```json
{
  "customers": [{
    "email": "alex.martin@example.com",
    "first_name": "Alex",
    "addresses": [{ "address1": "1 Main St", "city": "New York", "country_code": "US", "zip": "10001" }],
    "orders": [{ "line_items": [{ "variant": "TRAIL-42", "quantity": 1 }] }]
  }]
}
```

Money is a decimal string (`"129.00"`) or cents (`12900`), never a JSON float. The handle comes
from the file name (`trail-runner`). An image that is not in `files/` renders as a placeholder
of its size: test data needs no image files. The rest, in the data guide:
[conventions](data-format.md#conventions), [products](data-format.md#products),
[customers](data-format.md#customers-and-sessions), [images](data-format.md#images-and-files),
[error codes](data-format.md#error-codes).

### Check the store data

```bash
npx local-storefront validate --format json
```

```json
{
  "ok": false,
  "errors": 1,
  "warnings": 0,
  "data": "./shopify-local",
  "diagnostics": [
    {
      "severity": "error",
      "code": "unknown_field",
      "file": "products/shirt.json",
      "path": "/titel",
      "message": "unknown field \"titel\"",
      "hint": "did you mean \"title\"?"
    }
  ],
  "app": null
}
```

`path` is a JSON Pointer into `file`, in the folder `data`. Apply the `hint` when there is one,
then run the command again until `ok` is `true`. `app` is `null` outside a Shopify app. The
codes: [data guide](data-format.md#error-codes). The format of every field:
`npx local-storefront docs`, or `npx local-storefront schema <kind>` for a JSON Schema.

Fix the errors in `products/` and `customers.json` first. A product or a customer with a value
that cannot be read (a `wrong_type`, an `invalid_value`, an `invalid_format`, a
`missing_field`) is left out, so each reference to it is an error too, in its own file: in the
demo store, one float price gives 4 errors in 4 files. Run `validate` again before you edit a
reference.

### Check the theme

```bash
npx local-storefront check --format json
```

```json
{
  "ok": false,
  "files": 191,
  "problems": 1,
  "fixable": 1,
  "fixed": 0,
  "changes_output": 1,
  "diagnostics": [
    {
      "severity": "error",
      "code": "strict_liquid_rejected",
      "file": "snippets/card.liquid",
      "line": 1,
      "message": "Shopify's strict Liquid parser rejects this {% render %}: Expected end_of_string but found pipe",
      "subject": "{% render %}",
      "markup": "'image', alt: image.alt | default: shop.name",
      "fix": {
        "written": false,
        "markup": "'image', alt: image_alt",
        "before": ["assign image_alt = image.alt | default: shop.name"],
        "changes_output": "`alt` gets `image.alt | default: shop.name`"
      }
    }
  ]
}
```

This is the dry run: each `fix` is exactly what `--fix` writes. A diagnostic with `no_fix`, or
with neither key (an `unknown_filter`, which may have a `hint`), is not fixed by `--fix`: see
the rules of the next task.
Exit `1` while a problem is left. Every key and code: [CLI reference](cli.md#the-json-format).

`check` reads the files and renders none: a Liquid error at render time (a division by zero)
shows only with `render --strict`.

### Fix what Shopify's strict Liquid parser rejects

```bash
npx local-storefront check --fix --format json
```

Rules for an agent:

- Run it on a clean working tree, so that `git diff` shows exactly what it wrote.
- The fixed problems stay in `diagnostics`, with `fix.written: true`; their `severity` is still
  `error`. `problems` counts what is left: the errors without `fix.written: true` (a `warning`
  never counts).
- A fix with `changes_output` changes the rendered HTML on purpose: the code now does what it
  says. Report each one to the person, with its `file` and `line`. Do not revert it to silence
  the change.
- Two `no_fix` reasons have a rewrite in [strict Liquid](strict-liquid.md#fix-by-hand):
  `` `a.b` is not a name the strict parser reads `` and `` `=<` is read as an operator… ``.
  Apply the rewrite, and report it as you report a `changes_output`: the page changes. Any other
  `no_fix` needs a person's decision. Do not invent a rewrite: report it with its `file`,
  `line` and `no_fix`.
- An `unknown_filter` with a `hint` is a typo (`uppercas`, `did you mean "upcase"?`): apply
  the hint, and report it, since the page changes. One without a `hint` may be a filter that
  local-storefront lacks: report it.
- It is safe to run twice: the second run writes nothing.
- Run `npx local-storefront check --format json` again: `problems` is `0`, or every problem left
  is reported to the person.

Guide: [strict Liquid](strict-liquid.md).

### Render a page

```bash
npx local-storefront render /products/trail-runner --strict > page.html
```

The HTML goes to stdout, and the errors are in it too, as `Liquid error (sections/x line 12): …`.
On stderr, each error, then a summary:

```text
Liquid error (layout/theme line 34): divided by 0
status 200 · template product · 1 Liquid error · 0 warnings
```

With `--strict`, exit `1` when the page has Liquid errors, or when nothing is at the path
(`error: nothing at /x: the page is the theme's 404`). `npx local-storefront routes` lists every
page of the data, and each one renders: loop over it to check them all (`--all` adds the other
languages, a search and the sections the theme fetches). It leaves out `/account`, a page of
lsf's, not of the theme.

### Run the checks in CI

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
      - run: npx local-storefront validate --format github
      - run: npx local-storefront check --format github
      - name: Render every page, and fail on a Liquid error
        run: |
          npx local-storefront routes --all > routes.txt
          failed=0
          while read -r route; do
            npx local-storefront render "$route" --strict --format github > /dev/null || failed=1
          done < routes.txt
          exit $failed
```

`npm ci` needs `package-lock.json`: commit it with `package.json` after the
`npm install --save-dev local-storefront` of the setup. Commit `shopify-local/` too, `schema/`
and `README.md` included: the steps read the store data, and editors read `schema/`, which
`init` writes again at each run. `--format github` shows each problem on its line of the pull
request. `routes --all` adds the other languages, a search and the sections the theme fetches;
the step stops at once when `routes` fails (a wrong `--theme`), renders every route, and fails
at the end if one has a Liquid error. Add
`- run: npx local-storefront fmt --check` only once the theme is formatted: a stock theme is not.
With browser tests: [testing](testing.md#in-ci).

### Format files

```bash
npx local-storefront fmt --check
```

Lists the files that are not formatted; exit `1` if there are any. `npx local-storefront fmt`
formats them. `npx local-storefront fmt <files>` formats only those.

### Find what makes a page slow

```bash
npx local-storefront profile /collections/all --points
```

Points are deterministic: the same at every run and on every machine, so they can be compared
before and after a change. Times are not. Guide: [performance](performance.md).

### Run the Functions of a Shopify app

```bash
npx local-storefront app function run --app ../my-app --scenario <name> --format json
```

Prints the checkout the Functions produce for the cart of a scenario, a file of
`shopify-local/checkouts/` (`app init` writes a set). Without `--scenario`, it is the cart of the
store data's `session`, empty in the demo store. The JSON has `lines`,
`discounts`, `deliveryGroups`, `paymentMethods` and `expressMethods` (with their `placements`),
`errors`, `cost`, in `runs` the input, the output, the logs and the error of each Function, and in
`idle` the Functions the shop's checkout left out for want of an enabled owner (a payment
customization, …), with why. A Function named on the command line runs whether or not the shop
has an owner of it. Exits with `1` when a Function fails to run, or when a Function named on the
command line did not run; it says why. Guide: [checkout](checkout.md#owners-and-scenarios).

`npx local-storefront app function schema <handle>` writes the Function's `schema.graphql`, from
lsf's copy of the API's schema, offline.

For CI, `npx local-storefront app check --build --format json` builds the extensions, then runs
the Functions of every scenario and checks the `expect` of each, what its checkout must look like
([expected results](checkout.md#expected-results)): `ok`, `failures`, `invalid`, `expectations`
(`checked`, and in `unmet` the `scenario`, the `assertion`, `expected` and `actual` of each
expectation not met), and `ranInNoScenario`, the Functions no scenario ran (a warning: the exit
code stays `0`). Assert what a Function decides with an `expect` rather than piping
`app function run --format json` into `jq`.

### Test the backend of an app

```bash
npx local-storefront app dev --app . --app-url http://localhost:3000 --port 9393 --static --quiet
```

The local Admin API, OAuth and the app's webhooks, for its backend, with the app's checkout.
`serve --app . --only admin` runs the Admin API alone ([what runs](cli.md#what-runs)): a test
that needs neither the theme nor the checkout. Once `ready:` is printed,
`GET /__lsf/admin` gives the shop (`defaultShop`), the path of the API (`graphql`) and the token
(`shops[0].token`). Webhooks go to `--app-url`, signed with the client secret the start lines
name; without it, those that lead off this machine are `not_sent`.
`GET /__lsf/webhooks?shop=<shop>&since=<last>&wait=1` waits for one: `last` is in every answer
of `GET /__lsf/webhooks`, read it before the action. A shop for each test, orders from the
checkout, resets: [test your backend](admin.md#test-your-backend).

### Run a server

```bash
npx local-storefront serve --port 0 --static --quiet
```

- **Ready.** Run it in the background and read its stderr until a line matches
  `^ready:\s+(http://\S+)`: the URL is group 1. Every line of the server goes to stderr.
  `serving:` names the parts it runs (`theme`, `checkout`, `admin`: [what runs](cli.md#what-runs)).
  With `--app`, the `admin:` lines (the token, the client secret), `webhooks:`, `proxy:`,
  `embedded:` (the app in lsf's admin), `pricing:`, `partner:` and `polaris:` come before
  `ready:`, each when it applies, `home:` and `watch:` after it; read the token from
  `GET /__lsf/admin` rather than from the text. Without `--app`, or with `--only` leaving out
  the admin part, none of them is printed, and the Admin API is not served: its routes answer
  `404` with an `X-LSF-Reason`.
- **Failed.** It exits with `2` before `ready:` when the port is taken (`error: cannot listen
  on …`), or with `--strict` when the data has errors (on stderr; `validate --format json` gives
  them as JSON).
- **Stop.** Send `SIGTERM`: it finishes the requests it is answering, for 3 seconds at most,
  and exits with `0`.

While it runs:

- `GET /__lsf/status` gives the data diagnostics, the templates, the customers and every page.
- `PUT /__lsf/session` sets the cart, the customer and data for one session; a wrong body
  answers `422` with the same diagnostics as `npx local-storefront validate --format json`.
- The header `x-lsf-liquid-errors` counts the Liquid errors of a page; it is absent when there
  are none. `x-lsf-not-found` says why a page is a 404.

Reference: [control API](control-api.md).

## What not to expect

local-storefront renders themes, and runs your own app with `--app <dir>`: its checkout, its
Functions, its UI extensions, the Admin API and webhooks for its backend
([checkout](checkout.md), [Admin API](admin.md)). It runs no third-party app in the theme, and
the theme's cart has no discounts, taxes or shipping rates. A difference with Shopify that is
known is listed in [compatibility](compatibility.md#known-differences): check there before you
report one.

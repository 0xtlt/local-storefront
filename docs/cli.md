# CLI reference

Every command of local-storefront, its options, what it prints and how it exits.
`npx local-storefront <command> --help` prints the same options.

Installed with mise, from a release or from source, the command is `lsf`: `lsf serve` is
`npx local-storefront serve`.

## Options of every command

| Option | Environment variable | Default | What it does |
|---|---|---|---|
| `--theme <dir>` | `LSF_THEME` | `.` | The theme folder: the one with `layout/`, `sections/`, `templates/`. |
| `--data <dir>` | `LSF_DATA` | `<theme>/shopify-local` | The store data folder. Without one, the built-in demo store. |
| `--preview <name>` | `LSF_PREVIEW` | none | Turns on a Shopify feature preview. Repeat it, or separate names with commas. See [storefront](storefront.md#feature-previews). |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Done, nothing to report. |
| `1` | Done, and something to fix: data or app errors (`validate`), problems in the theme (`check`), Liquid errors in the page or nothing at its path (`render --strict`), files not formatted (`fmt --check`), a Function that failed to run or did not run (`app function run`), a scenario that fails or whose `expect` is not met (`app check`), a webhook that was not delivered (`app webhook trigger`). |
| `2` | The command could not run: a wrong option, a theme or a file that cannot be read, a file `fmt` cannot write, a port `serve` cannot listen on. The message is on stderr, after `error:`. |

## Output

Results go to stdout; messages and summaries go to stderr. For a tool, `--format json` prints
one JSON document on stdout: `validate`, `check`, `app check`, `app function run` and
`app webhook trigger` take it. The other commands print text and refuse `--format`. What each
one prints: [agents](agents.md#what-each-command-prints).

## serve

`npx local-storefront serve`

Serves the theme over HTTP. With `--app`, the checkout and the Admin API of the app too;
`--only` runs a part of them ([what runs](#what-runs)).

```bash
npx local-storefront serve --port 9292 --static --quiet --strict
```

| Option | Environment variable | What it does |
|---|---|---|
| `--port <port>`, `-p` | | Listens on this port and no other: the server does not start if it is taken. `0` picks a free one. Without the option: `9292`, or the next free port up to `9391`, which a line says (`port:   9292 is taken, using 9293`). |
| `--host <address>` | | Address to listen on. Default `127.0.0.1`. `0.0.0.0` in a container. |
| `--static` | | Reads the theme and the data once. The fastest mode, for tests. |
| `--strict` | | Refuses to start when the store data has errors. |
| `--quiet`, `-q` | | Does not log requests. |
| `--live-reload` | | Reloads open pages when the theme or the data change. It adds a script to the pages: leave it off for tests. |
| `--customer <email>` | `LSF_CUSTOMER` | Visitors arrive logged in as this customer. Also `default` (the first customer) or `none`. |
| `--throttle <rules>` | `LSF_THROTTLE` | Answers late, to see loading states: `simulated`, `slow`, `300ms`, `cart-add=1s`. See [testing](testing.md#simulating-a-slow-network). |
| `--timings` | | Names every section and theme block in the `Server-Timing` header. See [performance](performance.md#render-times). |
| `--no-minify` | | Serves styles and scripts as written. Without it, they are minified with a source map, as on Shopify. |
| `--no-compression` | | Sends responses uncompressed. Without it, they are compressed (Brotli or gzip) for clients that accept it, as on Shopify. |
| `--app <dir>` | `LSF_APP` | A Shopify app. `/checkout` then runs its Functions and shows its checkout UI extensions, `/account` shows its customer account UI extensions, and its backend gets the [Admin API](admin.md). See [checkout](checkout.md). |
| `--only <parts>` | `LSF_ONLY` | Runs only these parts: `theme`, `checkout`, `admin`, comma-separated or repeated. Without it, the theme, and all three with `--app`. `checkout` and `admin` need `--app`. See [what runs](#what-runs). |
| `--extension <handle>` | | With `--app`: uses only this extension of the app. Repeat it for several. Without it, every extension is used. |
| `--build` | | With `--app`: builds the extensions first, as `app build` does: the Functions, then the UI extensions that name their sources. |
| `--no-functions` | | With `--app`: serves the app's UI extensions and its Admin API, but runs none of its Functions. For a backend's tests, whose orders a Function that fails would block: a discount of the data whose Function is not run is left out. A Function named with `--extension` is refused; not with `--watch`. |
| `--watch` | | With `--app`: builds a Function again when its files change, and reloads the open pages (it implies `--live-reload`). At start, builds only the Functions that have no module or changed since the last watch; with `--build`, all of them. Not with `--static`. See [watch mode](checkout.md#watch-mode). |
| `--variant <standard\|shop>` | | With `--app`: the look of the checkout page, `standard` by default. |
| `--app-url <url>` | `LSF_APP_URL` | Where the backend of the app listens (`http://localhost:3000`). The webhooks of `shopify.app.toml` go there (a relative `uri` is joined to it; an absolute one on the origin of `application_url` is moved to it), and so do the OAuth redirects and the launch of the embedded app. Without it, `application_url`. See [Admin API](admin.md). |
| `--client-secret <secret>` | `LSF_CLIENT_SECRET` | The secret webhooks, OAuth and session tokens are signed with. Default: `SHOPIFY_API_SECRET` of the environment, else of the app's `.env`, else `lsf-local-client-secret`, printed at start. |
| `--admin-throttle <plan>` | `LSF_ADMIN_THROTTLE` | The cost bucket of the Admin API, per shop and app: `store` (the default: Plus for store data with companies, Standard otherwise), `standard` (2000 points, 100 a second), `advanced` (4000, 200), `plus` (20000, 1000), `enterprise` (40000, 2000), `legacy` (1000, 50), or `off`. |
| `--admin-versions <list>` | `LSF_ADMIN_VERSIONS` | The versions of the Admin API served, comma-separated, among those lsf knows: `2026-10` alone by default, `2026-10,2027-01` with the release candidate. Another version answers `404`; one Shopify no longer supports is answered by the oldest stable version served, as Shopify does. See [API versions](admin.md#api-versions). |
| `--admin-fetch-local` | `LSF_ADMIN_FETCH_LOCAL` | Lets `themeCreate`, the `URL` bodies of `themeFilesUpsert` and `urlRedirectImportCreate` download their source from this machine or its network (the loopback, private, link-local and unique-local addresses), for a theme ZIP served from localhost on purpose. Off by default: Shopify cannot reach them, and a backend could otherwise read what the machine serves. See [Sources from a URL](admin-reference.md#themes-script-tags-redirects-in-bulk-and-policies). |
| `--admin-source <url>=<file>` | | Answers a URL with a file, never downloading it: what `themeCreate`, a `URL` body of `themeFilesUpsert` or `urlRedirectImportCreate` reads there. Repeatable. For your backend's own URLs (a theme ZIP on its CDN) that nothing serves in a test, wherever they point, `localhost` included. The file must exist when lsf starts, and is read at each use. `PUT /__lsf/admin/sources?url=` stubs others while lsf runs. See [Stubbed URLs](admin-reference.md#stubbed-urls). |
| `--pricing-plans <file>` | `LSF_PRICING_PLANS` | The plans of the app's [managed pricing](admin.md#managed-pricing) (Shopify App Pricing), which Shopify keeps in the Partner Dashboard: a TOML file of `[[plans]]` (JSON for a `.json` file). lsf's admin shows them at `/admin/charges/<app handle>/pricing_plans`, where the merchant chooses one, and the Billing API creates no charge, as on Shopify. A wrong file stops the start. Read at start, and again when it changes unless `--static` ([when the data changes on disk](#when-the-data-changes-on-disk); a wrong file then keeps the plans it had, and says why); `PUT /__lsf/admin/pricing-plans` changes them while lsf runs. Printed before `ready:` as `pricing: <n> plans (<file>): <page>`, then `partner: <url>   (Partner API: …; token prtapi_lsf_…)`: where the app reads its plan ([Partner API](admin.md#read-the-plan-from-the-partner-api)). |
| `--polaris <online\|offline>` | `LSF_POLARIS` | What `/__lsf/cdn/<path>` answers, where an embedded app loads Polaris from: `online` (the default) redirects to Shopify's CDN, `offline` serves lsf's copy. See [Polaris without the internet](admin.md#polaris-without-the-internet). |

The options from `--extension` to `--variant` are the checkout's, those from `--app-url` to
`--polaris` the Admin API's. One given on the command line while its part does not run stops the
start: `error: --pricing-plans needs the admin part, which --only leaves out`, or without
`--app`, ``which needs the app: `--app DIR` (or LSF_APP)``. One set by its variable is ignored,
and said (`note: LSF_APP_URL (--app-url) is set, but the admin part is not served, …: ignored`):
a variable exported for `app dev` does not stop a `serve` of the theme alone.

Every line of the server goes to stderr; stdout stays empty. It prints the theme and the data
folder, the parts it runs (`serving: theme, checkout, admin`), then, once it takes requests, a
line that starts with `ready:  http://127.0.0.1:<port>/`. With `--port 0`, read the port from
that line. Without `--quiet`, each request gets a line (`200 GET / 39.1ms`), with the reason of
a 404 above it.

With `--app`, the lines of the app come before `ready:`: how many checkout scenarios the data
has (when the checkout runs a Function), then, with the admin part, the [Admin API](admin.md)
of the store and its token, the client secret it signs with and where that secret comes from,
the webhooks of the app, its proxy, the embedded app in lsf's admin (`embedded:`), with
`--pricing-plans` the plans and the Partner API (`pricing:`, `partner:`), and with
`--polaris offline` where Polaris is served (`polaris:`). After `ready:`, with the theme and the
checkout, the address of the home page of the checkout, then `watch:` with `--watch`:

```text
serving: theme, checkout, admin
scenarios: 10
admin:  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
        X-Shopify-Access-Token: shpat_…   (shop: local-supply-co.myshopify.com)
        client secret: from SHOPIFY_API_SECRET of ../my-app/.env
webhooks: 1 subscription of shopify.app.toml → http://localhost:3000
ready:  http://127.0.0.1:9292/   (status and control API: http://127.0.0.1:9292/__lsf)
home:   http://127.0.0.1:9292/__lsf/checkouts   (extensions and checkout scenarios of the app)
```

Without `--app`, or with `--only` leaving the admin part out, it prints no `admin:`, secret,
`webhooks:`, `proxy:`, `embedded:`, `pricing:`, `partner:` or `polaris:` line. An app without an
`extensions` directory is a backend, and its line says so: `app:    ../my-app (no extensions:
the Admin API, webhooks and the app proxy)`. The token is the same every run for a shop and an app; a tool reads it from
`GET /__lsf/admin` (`shops[0].token`). When the data changes on disk, the server reads it
again, and seeds the shops of the Admin API again only when the store's own data changed, not
for a checkout scenario ([when the data changes on disk](#when-the-data-changes-on-disk)).

While it runs, a webhook the backend refuses (`401`, `403`) gets one line, at its first refusal,
`--quiet` or not, naming the secret lsf signed it with as `client secret:` does:
`webhooks: products/create → http://localhost:3000/webhooks answered 401: lsf signed it with the
client secret from SHOPIFY_API_SECRET of ../my-app/.env: does the backend verify with the same
one?`.

It runs until it is stopped (Ctrl-C, `SIGTERM`, `SIGHUP`): then it finishes the requests it is
answering, for 3 seconds at most, abandons the rest, and exits with `0`. It exits with `2`
before `ready:` when it cannot listen on the port (`error: cannot listen on 127.0.0.1:9292:
Address already in use`), or, with `--strict`, when the store data has errors (each one, then
`error: the store data has errors (see above)`). A page whose Liquid never ends stops at its
render budget (see [the render budget](compatibility.md#the-render-budget)).

While it runs, tests talk to it through the [control API](control-api.md).

### What runs

lsf has three parts:

- **theme**: the storefront rendered from the theme: its pages, the cart, search, and the
  stand-ins of the checkout (`/checkout` lists the cart) and of the customer accounts
  (`/account`).
- **checkout**: what `--app` brings to the shop: the [checkout](checkout.md), where its
  Functions run, its UI extensions (checkout, thank-you page, order status, customer account at
  `/account`), the scenarios and the home page of the app.
- **admin**: the [Admin API](admin.md), OAuth and tokens, webhook delivery, the app proxy,
  lsf's admin (`/admin/…`, App Bridge, Polaris at `/__lsf/cdn/`), managed pricing and the
  Partner API, and their control endpoints (`/__lsf/admin/…`, `/__lsf/webhooks`,
  `/__lsf/polaris`).

`--only` (or `LSF_ONLY`) names the parts to run. Without it:

| Command | Runs |
|---|---|
| `serve` | `theme` |
| `serve --app <dir>` | `theme`, `checkout`, `admin` |
| `app dev` | `checkout`, `admin`: `serve --only checkout,admin` |

```bash
npx local-storefront serve --only theme                            # the theme alone
npx local-storefront serve --app ../my-app --only admin            # the Admin API alone, for a backend's tests
npx local-storefront serve --app ../my-app --only checkout         # the extensions and the Functions alone
npx local-storefront serve --app ../my-app --only checkout,admin   # what app dev serves
```

`checkout` and `admin` need `--app`: without it, ``error: `--only admin` needs the app: `--app
DIR` (or LSF_APP)``. `app dev` serves no theme, so `app dev --only theme` is an error. Without
the theme, `serve` needs no theme folder, as `app dev`: the store data is still looked for in
`<theme>/shopify-local`, then in the app's.

A part that does not run answers `404` on its routes, with an `X-LSF-Reason` that names the
`--only` that serves it: ``the admin part is not served (serving: theme): `serve --app <dir>
--only theme,admin` serves it`` ([control API](control-api.md)). With the theme and without the
checkout, `/checkout` and `/account` are the theme's stand-ins, as without `--app`. Without the
theme, the pages of the theme answer that `404`, `/` too unless the checkout runs (its home
page). `GET /__lsf/status`, `GET /__lsf/admin` and the page of `/__lsf` say which parts run.

The checkout without the admin part reads the owners of its Functions from the store data, and
records no order in an Admin API. The theme with the admin part shows what the backend writes,
with or without the checkout.

### When the data changes on disk

Without `--static`, the server looks at the data folder before it answers (every 300 ms at most)
and reads it again when a file changed. What it does then depends on the files, and the
terminal says which changed and what was done:

| What changed | What the server does | The line |
|---|---|---|
| The store's own data: a data file at the root (`store.json`), a file of `products/`, `collections/`, `pages/`, `blogs/`, `customers/`, `menus/`, or of `files/` | Reads everything again and seeds the shops of the Admin API again from it: what the backend changed there (a product, a subscription, a customization) is dropped. | `store data reloaded (products/tee.json changed): the shops of the Admin API were seeded again, and what the Admin API changed in local-supply-co.myshopify.com was dropped` |
| Only checkout scenarios (`checkouts/*.json`) | Reads the scenarios again on top of the same store data. The shops of the Admin API keep their state: the app's subscription, its customizations, its other writes. An open checkout follows its scenario. | `checkout scenarios reloaded (checkouts/b2b.json added): the store and what the Admin API changed are kept (no re-seed)` |
| The `--pricing-plans` file, wherever it is (looked at before every request, the control endpoints and lsf's admin included) | Reads the plans again. Plans a `PUT /__lsf/admin/pricing-plans` set stay until its `DELETE`. A file that no longer reads keeps the plans it had. | `pricing plans reloaded (pricing.toml)`, or `pricing plans not reloaded: …` |
| A file the store does not read (`schema/`, `README.md`, another folder, a name that starts with a dot) | Nothing. | none |

A scenario is a checkout input, not store data: adding one while the backend has set the shop
up must not undo that setup. A change that touches both kinds seeds again. `POST /__lsf/reload`
reads everything again and seeds the shops again whatever changed, and `DELETE /__lsf/admin`
puts them back to the store data without reading the files.

## app

The commands of a Shopify app, named as Shopify's CLI names them (`shopify app dev`,
`shopify app build`, `shopify app function run`). In the app's folder, `--app` can be left out,
and the store data defaults to the app's `shopify-local/`. Guide: [checkout](checkout.md).

| Command | What it does |
|---|---|
| [`app dev`](#app-dev) | Serves the app alone: its checkout, its customer account and its Admin API. |
| [`app init`](#app-init) | Writes checkout scenarios made for the app. |
| [`app build`](#app-build) | Builds its extensions, without Node. |
| [`app check`](#app-check) | Runs the Functions of every scenario and checks their `expect`, for CI. |
| [`app function run`](#app-function-run), [`app function build`](#app-function-build), [`app function schema`](#app-function-schema) | Runs or builds its Functions, without a server, or writes the schema of a Function's API. |
| [`app webhook trigger`](#app-webhook-trigger) | Sends one of its webhooks to its backend, signed as Shopify signs it. |

### app dev

`npx local-storefront app dev --app <dir>`

Serves a Shopify app alone, without a theme: `serve --app --only checkout,admin`. `/` is the
[home page](checkout.md#the-home-page), which lists the extensions of the app and the checkout
scenarios and opens a checkout for each; `/checkout` starts a checkout, where the Functions run
and the checkout UI extensions show; `/account` is the
[customer account](checkout.md#customer-account), where the customer account UI extensions
show. The Admin API, the control API and `/cdn` are there, other pages answer `404`. It takes
the options of [`serve`](#serve); `--only checkout` or `--only admin` runs one of its two parts
([what runs](#what-runs)), and `--only theme` is an error.

```bash
npx local-storefront app dev --app ../my-app --extension volume-discount --variant shop
```

### app init

`npx local-storefront app init [--app <dir>]`

Writes checkout scenarios, the files of `checkouts/` in the store data, made for the Functions
and checkout UI extensions of the app (a generic set outside an app, or when nothing matches).
It gives each Function the settings its input query reads (`[extensions.input.variables]`) when
the store data has none: those of the app's own `shopify-local/`, or empty values of the type of
each variable, in `app-settings.json` (to fill in). It creates the data folder with the demo store when it does not exist, refreshes `schema/`, never
overwrites a scenario or a data file without `--force`, and prints what it wrote, what it kept
and what it skipped and why. It writes to the same data folder as `init`, `--data` and
`LSF_DATA` included. Guide: [checkout](checkout.md#scenarios).

```bash
npx local-storefront app init --app ../my-app
```

| Option | Environment variable | What it does |
|---|---|---|
| `--app <dir>` | `LSF_APP` | The app the scenarios are made for. Defaults to the app the current folder is in. |
| `--force` | | Overwrites the scenarios that exist. |

### app build

`npx local-storefront app build [handle]... --app <dir>`

Builds the extensions of an app, as `shopify app build` does, without Node: its Functions (as
`app function build`) and its UI extensions, of the checkout and of the customer account, that
name their sources (`module`), bundled into `dist/<handle>.js`. Guide:
[checkout](checkout.md#what-lsf-builds-without-node).

| Option | Environment variable | What it does |
|---|---|---|
| `[handle]...` | | The extensions to build. Every extension of the app when none is given. |
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in. |

### app check

`npx local-storefront app check [--app <dir>]`

Runs the Functions of every checkout scenario, as the home page checks them, without a server:
for CI. Prints each scenario that does not validate, each Function that fails, with the
scenarios it fails in, and each expectation of a scenario's [`expect`](checkout.md#expected-results)
that its checkout does not meet. Exits with `1` when there is one, when the time ran out, or
when there is no scenario at all (`app init` writes a set). No request of a `.fetch` target is
sent.
It builds nothing without `--build`: in a fresh clone, where `dist/` is not committed, give
`--build` (or run `npx local-storefront app build` first).

A trap ends with the last message the Function logged, such as the one of a `throw`:

```text
fails    my-discount · cart.lines.discounts.generate.run: the Function trapped: Error: boom (in b2b)
```

A Function that ran in no scenario, such as a discount Function no scenario's discount names, is
a warning: nothing checked it, and the exit code stays the same. The last line, the summary,
counts it too, rather than saying that every Function ran:

```text
warning  volume-discount: ran in no scenario, so nothing checks it (a discount Function runs only for a discount that names it: …)
checked 8 of 8 scenarios in 0.4 s: no Function failed, 1 Function ran in no scenario
```

A payment or delivery customization, validation or cart transform Function gets
``(it runs where the shop has an enabled owner of it, as on Shopify, or where a scenario names
it: add it to the `extensions` of one)`` instead.

An expectation that is not met prints the scenario and the assertion (the list and the rule,
or the amount), what the scenario expects, and what the checkout has, as the scenario writes it.
The Functions run for it as the order is placed, unless the `expect` says
`"step": "interaction"`, which the first line then names:

```text
expect   express-buttons: express_methods.excludes
         expected: no entry matching "paypal"
         actual:   [{"handle": "paypal", "name": "PayPal"}, {"handle": "shop_pay", "name": "Shop Pay"}]
expect   express-buttons: totals.total
         expected: "96.00"
         actual:   "86.40"
```

When every Function ran somewhere and none failed, the summary ends with `every Function ran`,
then `12 expectations met` when scenarios have an `expect`. When something failed, it counts the
failures, the invalid scenarios and the expectations not met:
`1 failure, 0 invalid scenarios, 2 of 12 expectations unmet`.

Exiting with `0` means no Function failed where it ran, and every `expect` holds. A scenario
without `expect` checks only that its Functions run. CI: [checkout](checkout.md#in-ci).

`--extension` keeps some extensions of the app. An `expect` describes the checkout of the
Functions its scenario names (every Function of the app when it names none): when
`--extension` leaves one of them out, that checkout is another one, so the `expect` is not
checked. The scenario still runs, for the Functions that fail in it, and the summary counts
it:

```text
checked 8 of 8 scenarios in 0.1 s: every Function ran, the `expect` of 2 scenarios not checked (--extension leaves out a Function they run)
```

| Option | Environment variable | What it does |
|---|---|---|
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in. |
| `--extension <handle>` | | Only these extensions. Repeat it for several. |
| `--build` | | Builds the extensions first, as `app build` does. |
| `--format text` | | Default. One line per problem, then a summary. |
| `--format json` | | `{ok, state, checked, total, elapsedMs, failures: [{handle, target, error, logs, scenarios}], invalid: [{scenario, file, problems}], expectations: {checked, unchecked: [scenario], unmet: [{scenario, file, step, assertion, expected, actual}]}, ranInNoScenario: [handle]}`, and `error` when there is no scenario. `state` is `done`, or `partial` when the time ran out. `logs` is what the Function logged in the first scenario it failed in. `expectations.checked` counts the expectations of the scenarios' `expect`, met or not; `unchecked` names the scenarios whose `expect` was not checked, as `--extension` leaves out a Function they run; `assertion` is the list and the rule (`express_methods.excludes`) or the amount (`totals.total`), `expected` what the scenario says (the entry, the list, the amount), `actual` what the checkout has. `ranInNoScenario` is filled only when every scenario ran. |
| `--budget <seconds>` | | How long the Functions may run in all. Default 300. |

### app function run

`npx local-storefront app function run [handle]... --app <dir>`

Runs the Functions of an app once, without a server. Builds the input of each from the store
data, runs them in checkout order, and prints each input, output and log with the checkout that
results. Guide: [checkout](checkout.md#run-a-function-without-a-server).

```bash
npx local-storefront app function run volume-discount --app ../my-app --session '{"cart":{"items":[{"variant":"TEE-BLK-M","quantity":3}]}}'
```

| Option | Environment variable | What it does |
|---|---|---|
| `[handle]...` | | The Functions to run, whether or not the shop has an owner of them (a payment customization, …): one the checkout of the store data would leave out is said first (`Named: …`, `forced` in JSON). It reads the store data on disk, not a running lsf. Without them, what the checkout runs: the Functions with an enabled owner, those of a discount, or those the scenario names; the others are listed as not run, with why. |
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in; the store data to its `shopify-local/` when the current folder has none. |
| `--build` | | Builds each Function first, as `app function build` does. |
| `--scenario <name>` | | A [checkout scenario](data-format.md#checkout-scenarios) of the store data (`checkouts/<name>.json`): its cart, buyer, language, `checkout` settings and the extensions it names. `--session`, `--checkout` and `--locale` go on top of it. |
| `--session <file or JSON>` | | The session to run for, as `PUT /__lsf/session` takes it: its `data` goes on top of the store data for this run (a discount for a discount Function, settings in `checkout.functions`). Defaults to the `session` of the store data. |
| `--checkout <file or JSON>` | | What the buyer entered: `email`, `phone`, `shippingAddress`, `selectedDelivery`, `selectedPayment`, `discountCodes`, `metafields`. |
| `--locale <iso>` | | The language of the buyer, one of the store's (`fr`, `pt-BR`): Functions read it as `localization.language`. Defaults to the primary language. |
| `--step <step>` | | `interaction` or `completion` (the default): the step of the buyer journey, what validation Functions read as `buyerJourney.step`. |
| `--format text` | | Default. Each run, then the checkout. |
| `--format json` | | The checkout as the checkout page reads it, with every run in `runs`. |

Exits with `1` when a Function fails to run, or when a Function named on the command line did not
run: it says why (no discount names a discount Function, the cart is empty, the `extensions` of
the scenario leave it out, an error of the app…). A handle or a scenario that is close to an existing one is suggested. Errors a validation
Function reports are part of the checkout and do not change the exit code.

### app function build

`npx local-storefront app function build [handle]... --app <dir>`

Builds the Functions of an app, as `shopify app build` does. A Function written in JavaScript or
TypeScript without a build command is built by lsf, without Node: its types
(`generated/api.ts`), `dist/function.js`, then its Wasm module. The others run their
`[extensions.build] command`. Guide: [checkout](checkout.md#javascript-and-typescript-functions).

| Option | Environment variable | What it does |
|---|---|---|
| `[handle]...` | | The Functions to build. Every Function of the app when none is given. |
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in. |

### app function schema

`npx local-storefront app function schema [handle] [--path <dir>] [--stdout]`

Writes the GraphQL schema of a Function's API, at the `api_version` of its
`shopify.extension.toml`, to its `schema.graphql`, as `shopify app function schema` does: the
schema a Rust Function's `#[typegen("schema.graphql")]` and a JavaScript Function's
graphql-codegen read. It comes from the schemas lsf carries (the checkout's Function APIs:
cart transform, discounts, delivery and payment customizations, validations, at 2026-10 and
2027-01), without the network or a Partner account. Guide:
[checkout](checkout.md#the-schema-of-a-function).

| Option | Environment variable | What it does |
|---|---|---|
| `[handle]` | | The Function. Without it, the Function whose folder `--path` names, or the one the current folder is in. |
| `--path <dir>` | | The folder of the Function, as Shopify's `--path`. |
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app `--path` or the current folder is in. |
| `--stdout` | | Prints the schema instead of writing `schema.graphql`. |

### app webhook trigger

`npx local-storefront app webhook trigger --topic <topic> [--app <dir> --app-url <url> | --address <url>]`

Sends one webhook, as `shopify app webhook trigger` does, without Shopify: lsf builds the
payload of the topic (the REST representation Shopify sends) from a record of the store data, as
the [Admin API](admin.md) seeds it (`--data`, else the `shopify-local/` of the theme or of the
app, else the demo store): the first product for `products/update`, the first order for
`orders/create`, the shop for `app/uninstalled`. A topic whose type the data has no record of
(a refund, a draft order) is built from lsf's sample, modelled on Shopify's example payload. lsf
signs it with the client secret (`X-Shopify-Hmac-Sha256`), and posts it with Shopify's headers,
`X-Shopify-Shop-Domain` the shop of the data. It prints the request it sent and the answer of the
backend. To send the webhook of a given record, use `POST /__lsf/webhooks/trigger` with its
`id` while a server runs. In the demo store, the first order, `#1001`, is fulfilled already: to
test a fulfillment handler, [place an order](admin.md#place-an-order), or trigger
`{"topic": "orders/create", "id": "gid://shopify/Order/7888001690128"}` (`#1002`).

```bash
npx local-storefront app webhook trigger --topic products/update --app ../my-app --app-url http://localhost:3000
npx local-storefront app webhook trigger --topic orders/create --address http://localhost:3000/webhooks --client-secret "$SHOPIFY_API_SECRET"
```

Without `--address`, it goes to every subscription of `shopify.app.toml` to the topic, as an
event of the shop would: each subscription's `uri` joined to the app URL, its `include_fields`
applied. Relative `uri`s join `--app-url` (else `application_url`), and an absolute one on the
origin of `application_url` moves to `--app-url`, since `application_url` is often the tunnel of
a `shopify app dev` that is not running. As Shopify's, a triggered webhook is not retried.

Webhooks stay on this machine. One goes to another host only when that host is the scheme,
host and port of `--app-url`, or the `--address` given: otherwise the command sends nothing,
prints `error: lsf sends webhooks to this machine or to --app-url only: <url> is neither; give
--app-url http://localhost:<port> (your backend) or --address <url>` and exits with `2`. A
server logs the same delivery `not_sent`, and says to start lsf with `--app-url`. So
`application_url`, often your production backend or a dead tunnel, never gets one by accident.

The secret is the app's. Without the app (no `--app`, and run outside its folder), its `.env`
is not read: give `--client-secret` or set `SHOPIFY_API_SECRET`. Without either, lsf signs with
its own secret, and the backend answers `401`. A `401` or `403` says which secret lsf signed
with: `failed: answered 401 (1 attempt): lsf signed it with lsf-local-client-secret, its own:
give lsf the backend's secret (--client-secret, or --app for the SHOPIFY_API_SECRET of its
.env)`.

| Option | Environment variable | What it does |
|---|---|---|
| `--topic <topic>` | | The topic, as Shopify writes it (`orders/create`) or as the GraphQL enum does (`ORDERS_CREATE`). Every topic of the API version (`--api-version`) and the compliance topics (`customers/data_request`, `customers/redact`, `shop/redact`). A topic close to an existing one is suggested. |
| `--address <url>` | | Where to send it: `https://…`, or `http://localhost:<port>/<path>` (any address of this machine: `localhost`, `*.localhost`, `127.0.0.0/8`, `::1`). |
| `--api-version <version>` | | The version the payload is built in: `2026-10` (the default) or another lsf knows (`2027-01`). A topic the version does not have is refused. |
| `--delivery-method <method>` | | `http`, `google-pub-sub` or `event-bridge`; without it, what the address says. lsf delivers over HTTP only: the other two are refused. |
| `--client-secret <secret>` | `LSF_CLIENT_SECRET` | The secret of the signature. Default: `SHOPIFY_API_SECRET` of the environment, else of the app's `.env`, else lsf's local secret, `lsf-local-client-secret`. |
| `--app <dir>`, `--path <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in; may be left out with `--address`. |
| `--app-url <url>` | `LSF_APP_URL` | Where the backend of the app listens. Without it, `application_url`, when it is on this machine. |
| `--format text` | | Default. The request it sent, the answer, the state. |
| `--format json` | | Prints the deliveries as JSON: `{ok, deliveries: [...]}`, each as [`GET /__lsf/webhooks`](control-api.md#webhook-deliveries) lists it (`topic`, `url`, `request`, `attempts`, `state`, `reason`, …). `--json` and `-j`, from before `--format`, still work. |

lsf builds the payloads of the topics of products, collections, customers, orders, draft
orders, fulfillments, refunds, inventory items and levels, locations, metaobjects, discounts,
the shop, the app (`app/uninstalled`, `app/scopes_update`, `app_subscriptions/update`), bulk
operations, and the compliance topics. Another topic is refused, rather than sent with a body
made up for it.

Exits with `1` when a delivery failed: an answer out of `2xx` (a redirect included, as on
Shopify), no answer within 5 seconds, a backend that is not listening. Exits with `2` when
nothing could be sent: an unknown topic, a topic lsf does not build, no address and no
subscription to the topic, a Pub/Sub or EventBridge address, an address on another machine
that `--app-url` or `--address` does not name.

## render

`npx local-storefront render <path>`

Prints the HTML of one page, without a server.

```bash
npx local-storefront render '/collections/all?sort_by=price-ascending' --strict > collection.html
```

| Option | What it does |
|---|---|
| `<path>` | The path, with its query string. Default `/`. |
| `--section-id <id>` | Renders one section, as the Section Rendering API does. |
| `--host <host>` | The host of the URLs in the page. Default `localhost:9292`. |
| `--strict` | Exits with `1` when the page has Liquid errors, or when nothing is at the path (the theme's 404 page). |

The HTML goes to stdout. On stderr: each Liquid error and each warning, then a summary.

```text
Liquid error (layout/theme line 34): divided by 0
status 200 · template product · 1 Liquid error · 0 warnings
```

With `--strict`, a path with nothing at it adds `error: nothing at /x: the page is the theme's
404`.

## profile

`npx local-storefront profile <path>`

Prints what the render of one page spends its time on: sections, blocks, snippets. See
[performance](performance.md#profile-a-page).

| Option | What it does |
|---|---|
| `<path>` | The path, with its query string. Default `/`. |
| `--points` | Counts in points, what the page would cost Shopify, instead of time. |
| `--cost <kind>=<points>,...` | Changes what a kind of thing costs in points. |
| `--lines` | Adds every tag and every output, by file and line. |
| `--all` | Shows every row. Without it, the smallest rows are summed up. |
| `--runs <n>` | Renders the page `n` times and keeps the one in the middle. Default `15`. |
| `--section-id <id>` | Profiles one section alone. |
| `--json` | Prints the profile in speedscope's format, for a flame graph. |

## validate

`npx local-storefront validate`

Checks the store data and explains every problem, its checkout scenarios included.

The data is the one `app dev` reads: `--data`, or the folder's `shopify-local/`, or the
app's (`<app>/shopify-local`), or the built-in demo store. In a folder that is not a theme (an
app, such as [the checkout playground](https://github.com/0xtlt/local-storefront/tree/main/examples/checkout-playground)), it checks the data
alone, with the store's own languages; an app without any is checked with the demo store, as its
checkout runs. A folder that is neither a theme nor an app, and has no store data, is an error.

In a Shopify app (or with `--app`), it checks the app too: what its Functions and UI extensions
get wrong (no build, an API version lsf does not run, an unknown target, an `owner_type`), and
exits with `1` on an error of it.

| Option | What it does |
|---|---|
| `--app <dir>` | The app to check too (`LSF_APP`). Defaults to the app the current folder is in. |
| `--format text` | Default. One block per problem, for a person. |
| `--format json` | One JSON document, for a tool: `errors`, `warnings` and `diagnostics` are those of the data and of the app together (an app's has the code `app` and its extension as `file`), `app` has the app's own when there is one. |

The text format:

```text
error[unknown_field]: unknown field "titel"
  --> products/shirt.json at /titel
  hint: did you mean "title"?
```

The JSON format:

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

`data` is the folder read, and `file` is in it. `path` is a JSON Pointer into `file`. `app` is
`null` outside a Shopify app. The codes are listed in the
[data guide](data-format.md#error-codes). Exits with `1` when there is an error; warnings
alone exit with `0`.

## check

`npx local-storefront check`

Parses every Liquid file of the theme and reports what would not render on Shopify:
syntax errors, unknown filters, what Shopify's strict parser rejects. It reads `layout/`,
`templates/`, `sections/`, `blocks/`, `snippets/`, and the `liquid` settings of the JSON
templates, section groups and `config/settings_data.json`. Guide:
[strict Liquid](strict-liquid.md).

| Option | What it does |
|---|---|
| `--fix` | Rewrites what the strict parser rejects. A file is written only if it has not changed since it was read. Run twice, the second run writes nothing. |
| `--format text` | Default. One block per problem, for a person. |
| `--format json` | One JSON document, for a tool. |

### The text format

Each problem is a line `<file>:<line>: <message>`, followed by indented details. A warning,
which is not counted as a problem, has `warning: ` before its message.

| Detail | Meaning |
|---|---|
| `was:` | The markup as written. Long markup shows only the words that change. |
| `fix:` | The rewrite `--fix` writes. After `--fix`, it is `now:`. |
| `before it:` | Tags the fix adds on the lines above, separated by `;`. |
| `its body goes in:` | The `if` the fix puts the body of a `when` in. |
| `changes the output:` | What renders differently with the fix. Absent when the output stays the same. |
| `left out:` | Arguments the fix drops, which the snippet does not read. |
| `no automatic fix:` | Why there is no fix. |

For a `liquid` setting, the location is `<file>:<line>: <path of the setting>`, and
`, line <n>` when the problem is not on the first line of the value. The summary goes to
stderr.

### The JSON format

```json
{
  "ok": false,
  "files": 191,
  "problems": 2,
  "fixable": 2,
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
    },
    {
      "severity": "error",
      "code": "strict_liquid_rejected",
      "file": "templates/index.json",
      "line": 3,
      "setting": "sections.custom.settings.custom_liquid",
      "setting_line": 2,
      "message": "Shopify's strict Liquid parser rejects this {{ }}: Expected end_of_string but found id",
      "subject": "{{ }}",
      "markup": "product.title product.vendor",
      "fix": { "written": false, "markup": "product.title" }
    }
  ]
}
```

| Key | Meaning |
|---|---|
| `ok` | `true` when no problem is left. |
| `files` | How many files were read. |
| `problems` | Problems left: not fixed, or without a fix. |
| `fixable` | Problems left that `--fix` would fix. |
| `fixed` | Problems `--fix` fixed in this run. |
| `changes_output` | Fixes, written or not, that change what renders. |

Each diagnostic has a `severity` (`error`, or `warning` for what does not count as a
problem), a `code`, a `file`, a `line` when there is one, and a `message`.

Without `--fix`, the report is a dry run: each `fix` is what `--fix` writes. After `--fix`, the
problems it fixed stay in `diagnostics`, with `fix.written: true` and still `error`; `problems`
counts the others.

| `code` | What it is |
|---|---|
| `strict_liquid_rejected` | Markup Shopify's strict parser rejects. |
| `strict_liquid_reinterpreted` | Markup Shopify's strict parser reads differently from today. |
| `liquid_syntax` | A file that does not parse at all. |
| `unknown_filter` | A filter local-storefront does not know. Either a typo: Shopify skips an unknown filter, and the value comes out unchanged. A filter of Shopify one or two letters away, with the same letters, or a name of another template language (`uppercase`) is in the `hint`. Or a filter of Shopify that local-storefront lacks: [report it](https://github.com/0xtlt/local-storefront/issues). |
| `block_outside_template` | `{% block %}` outside `layout/` and `templates/` (with `--preview liquid-july-26`). |
| `invalid_json` | A JSON file that does not parse. |
| `fix_fallback` | A warning: what the code meant could not be written for this file, so only the fixes that keep the output were. |
| `classic_customer_accounts` | A warning: the theme has `templates/customers`, the pages of classic customer accounts, which lsf never renders (`/account` stands for Shopify's new customer accounts). The theme is fine on Shopify. |

The diagnostics of the strict parser have more keys:

| Key | Meaning |
|---|---|
| `setting`, `setting_line` | For Liquid in a JSON file: the setting, as its keys joined with dots, and the line in its value. `line` is then where the value starts in the file. |
| `subject` | `{% <tag> %}`, or `{{ }}` for an output. |
| `markup` | The markup as written. |
| `fix.written` | Whether `--fix` wrote the fix. |
| `fix.markup` | What replaces the markup, or the whole tag when the fix rewrites the tags around it (a `case` that becomes `if`). |
| `fix.before` | Tags the fix puts on the lines above, without their delimiters. |
| `fix.body_in_if` | The condition of the `if` the body of a `when` goes in. |
| `fix.left_out` | `{ variables, snippet }`: arguments the fix drops, which the snippet does not read. |
| `fix.changes_output` | What renders differently with the fix. Absent when the output stays the same. |
| `no_fix` | Why there is no fix, when there is none. |
| `hint` | For an `unknown_filter`, the filter of Shopify it is probably a typo of: `did you mean "upcase"?`. Absent when none is close. |

Exits with `1` when a problem is left, whatever the format.

## fmt

`npx local-storefront fmt [paths]`

Formats Liquid files as Prettier does with Shopify's Liquid plugin. Guide:
[formatting](formatting.md).

| Option | What it does |
|---|---|
| `[paths]` | Files and folders to format. Default: the theme folder. |
| `--check` | Writes nothing. Lists the files that are not formatted on stdout, the summary on stderr, and exits with `1` if there are any. |
| `--stdin` | Formats the standard input and prints it. For an editor. |
| `--stdin-filepath <path>` | With `--stdin`: the file the source comes from, for its configuration and `.prettierignore`. |
| `--option <name>=<value>`, `-o` | Sets a Prettier option: `-o printWidth=100`. Wins over the configuration files. |
| `--no-config` | Does not read `.prettierrc`, `.editorconfig` nor `.prettierignore`. |

A file that cannot be read, written or formatted is reported, the others are formatted, and
the command exits with `2`.

## init

`npx local-storefront init`

Creates the data folder with a demo store, JSON Schemas for editors, and the data guide as
`README.md`. For the checkout of a Shopify app, [`app init`](#app-init) writes the
scenarios.

It keeps the files that exist, so it is safe to run again, and refreshes `schema/`. It prints
on stderr, and exits with `0`:

```text
./shopify-local: wrote 16 files, kept 0 existing ones, refreshed schema/
next: the `serve` command shows the theme, and `validate` checks the data after you edit it
      for a Shopify app: `app init --app <dir>` writes checkout scenarios to try its Functions and extensions with
```

| Option | What it does |
|---|---|
| `--force` | Overwrites files that exist, your edits included. |

## schema

`npx local-storefront schema [kind]`

Prints the JSON Schema of the store data.

| Option | What it does |
|---|---|
| `[kind]` | `store` (default, a root data file), `product`, `collection`, `page`, `blog`, `customer`, `menu`, `session` or `checkout-scenario` (a file of `checkouts/`). |
| `--out <dir>` | Writes every schema into the folder as `<kind>.schema.json`. |

## routes

`npx local-storefront routes`

Lists the URLs the store data gives a page to, one per line. Each one renders: a loop of
`render --strict` over them checks every page. `/account`, a page of lsf's rather than of the
theme, is not listed; `GET /__lsf/status` lists the same `routes`.

## docs

`npx local-storefront docs`

Prints the data guide, with the reference of every field.

| Option | What it does |
|---|---|
| `--reference` | Prints only the field reference. |

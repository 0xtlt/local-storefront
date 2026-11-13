# Checkout

Run the Shopify Functions and show the checkout and customer account UI extensions of a Shopify
app, on your machine.
local-storefront serves a fake checkout for it: no store, no payment, and no network except the
requests of the Functions that have [network access](#network-access). It is a way to
see what your Functions do to a cart, and how your extensions look and behave, not a copy of
Shopify's checkout.

## Run it

```bash
npx local-storefront app dev --app ../my-app
```

Open <http://127.0.0.1:9292/>: the [home page](#the-home-page) lists what is wrong first, then
the [checkout scenarios](#scenarios) and the extensions of the app, and opens a checkout for each. <http://127.0.0.1:9292/checkout>
is the checkout of the cart of the session, which the store data defines (`session.cart`, see
[data format](data-format.md#customers-and-sessions)). It needs no theme.

The pages need a recent browser: Chrome 123+, Safari 17.2+ or Firefox 138+ (they import JSON
modules).

With a theme, `serve` does the same and the cart form of the theme leads to that checkout. The
theme owns `/`, so the home page is at <http://127.0.0.1:9292/__lsf/checkouts>, and the server
prints its address when it starts:

```bash
npx local-storefront serve --app ../my-app
```

No scenarios yet? `npx local-storefront app init --app ../my-app` writes a set made for
the extensions of the app, see [scenarios](#scenarios).

In the app's folder, `--app` can be left out: `lsf app dev`, `lsf app function run` and the other
commands of an app take the app the current folder is in. The store data is `--data`, else the
`shopify-local/` of the current folder (or of the theme), else the app's
(`<app>/shopify-local`), else the built-in demo store. One that `lsf init` creates while the
server runs is read at its next reload.

The app is a directory with `shopify.app.toml` and `extensions/*/shopify.extension.toml`. API
versions 2026-10 and 2027-01 are supported. Functions run from their built Wasm module and
extensions from their built bundle (`dist/function.wasm`, `dist/<handle>.js`). lsf builds them
as `shopify app build` does, without Node and without Shopify's CLI
([what it builds](#what-lsf-builds-without-node)): `lsf app build`, or `--build` before it
serves:

```bash
npx local-storefront app build --app ../my-app      # every extension
npx local-storefront app dev --app ../my-app --build
```

### Coming from Shopify's CLI

| Shopify CLI | lsf | What lsf does not do |
|---|---|---|
| `shopify app dev` | `lsf app dev --watch`: the checkout, the customer account and the Admin API, built again at each save ([watch mode](#watch-mode)) | No development store, tunnel or Partner account. The app's web server is yours to start: give its URL with `--app-url`. |
| `shopify app build` | `lsf app build` | `wasm-opt` is not run on a Rust Function ([Rust Functions](#rust-functions)). |
| `shopify app function build` | `lsf app function build` | `@shopify/shopify_function` 1.x ([JavaScript Functions](#javascript-and-typescript-functions)). |
| `shopify app function run` | `lsf app function run --input <file>` (or `-`, or a folder of fixtures), `--export <name>`, `--format json`; with `--scenario`, on an input lsf builds from the store data ([run a Function](#run-a-function-without-a-server)). As Shopify's, it builds the Function first when its files changed after its module was built. | |
| `shopify app function replay` | `lsf app function run --scenario <name> --save-input <file>`, then `--input <file>` | No run of a store to replay: lsf does not read Shopify's logs. |
| `shopify app function typegen` | `lsf app function typegen` | A Rust Function's types come from the `typegen` macro, at its build. |
| `shopify app function schema` | `lsf app function schema` | The schemas of fulfillment constraints, order routing and pickup generators. |
| `shopify app webhook trigger` | `lsf app webhook trigger` ([Admin API](admin.md)) | |
| `shopify app generate extension` | Not covered: copy an extension of the [playground](https://github.com/0xtlt/local-storefront/tree/main/examples/checkout-playground/extensions) ([add a JavaScript Function](#add-a-javascript-function)). | |
| `shopify app init`, `deploy`, `release`, `versions`, `env`, `config`, `info` | Not covered: they act on Shopify's servers. `lsf app init` writes checkout scenarios, not an app. | |

### What lsf builds without Node

| Extension | Built by lsf when | Output |
|---|---|---|
| A Function with a build command (Rust, …) | always: the command runs in its folder, then a module that calls the Function Wasm API by name is linked to it ([below](#rust-functions)) | its module |
| A Function in JavaScript or TypeScript (`src/index.js`, `src/index.ts`), no build command | always ([below](#javascript-and-typescript-functions)) | `dist/function.wasm` |
| A checkout UI extension whose targets name their sources (`module`), and the sources are there | always ([below](#checkout-ui-extensions)) | `dist/<handle>.js` |
| A checkout UI extension with only its bundle | never: the bundle is used as it is | |

The npm packages they import come from the app's `node_modules`, when it has them. Without, lsf
has its own copy of those Shopify's templates use: `@shopify/shopify_function` 2.0.1, `preact`
10.29.8, `@preact/signals` 2.11.4 and `@shopify/ui-extensions` 2026.10.0 (MIT, but
`@shopify/shopify_function`: Apache-2.0), written once in the user's cache folder
(`~/Library/Caches/lsf/` on macOS, `~/.cache/lsf/` on Linux). Any other package needs a
`node_modules`.

### JavaScript and TypeScript Functions

A Function written in JavaScript or TypeScript (`src/index.js` or `src/index.ts`) with no build
command, as `shopify app generate extension` makes it, is built by lsf the way
`shopify app function build` builds it, without Node and without Shopify's CLI:

```bash
npx local-storefront app function build --app ../my-app        # every Function
npx local-storefront app function build discount --app ../my-app
```

`--build` and `--watch` build it the same way, and `lsf app function typegen` writes its types
alone, as `shopify app function typegen` does (the `typegen` script of Shopify's templates'
`package.json`). The steps are the CLI's:

1. **Types**: the `codegen` section of its `package.json` (graphql-codegen's config) is written
   as graphql-codegen writes it (`generated/api.ts`), from `schema.graphql`, or, when the
   Function has none, lsf's schema of its API version.
2. **One JavaScript file** (`dist/function.js`), as esbuild bundles it: TypeScript removed, the
   modules it imports (with `node_modules`, when there is one), enum members inlined, and what
   nothing uses left out. `process.env.NAME` is the value of `NAME` in the environment or in the
   app's `.env`. `@shopify/shopify_function` (2.x) need not be installed: lsf has its own copy
   of the part a Function runs.
3. **The Wasm module**, as Javy writes it for Shopify's plugin: for the same JavaScript, the
   same bytes. It holds the QuickJS bytecode of the Function and imports the JavaScript engine
   from the plugin, which lsf runs it with, as Shopify does.

The module costs the instructions the CLI's would, within a few hundred. What lsf does not
build: `import()`, a `require` of an ES module, and Functions written with
`@shopify/shopify_function` 1.x (`shopify app build` builds those).

#### Add a JavaScript Function

`shopify app generate extension` needs a Partner account and the network. Without it, copy a
Function of the playground:
[`discount-typescript`](https://github.com/0xtlt/local-storefront/tree/main/examples/checkout-playground/extensions/discount-typescript)
or [`validation-javascript`](https://github.com/0xtlt/local-storefront/tree/main/examples/checkout-playground/extensions/validation-javascript).
A Function is a folder of `extensions/`:

| File | What it holds |
|---|---|
| `shopify.extension.toml` | `type = "function"`, its `handle`, a `[[extensions.targeting]]` with its `target`, `input_query` and `export`, and `[extensions.build]` with `command = ""` and `path = "dist/function.wasm"`. |
| `package.json` | `"@shopify/shopify_function": "2.0.1"` in `dependencies`: lsf needs it declared, not installed. The `codegen` section, for the types. |
| `src/index.js` (or `.ts`) | `export * from "./<target>";`, the target with `_` for `.`: `./cart_lines_discounts_generate_run` for a discount. |
| `src/<target>.graphql`, `src/<target>.js` | The input query, and the function the `export` names, in camel case (`cart-validations-generate-run`: `cartValidationsGenerateRun`). |

Then build it at each save with `npx local-storefront app dev --watch`. A discount Function runs
only for a discount that names it, and the scenarios written before it leave it out: see
[scenarios](#scenarios).

### The schema of a Function

A Function's `schema.graphql` is the GraphQL schema of its API at its `api_version`: the Rust
`shopify_function` crate generates its types from it (`#[typegen("schema.graphql")]`), and a
JavaScript Function's graphql-codegen too. `shopify app function schema` writes it from
Shopify's servers; lsf writes it from the schemas it carries, offline:

```bash
cd extensions/payment-rules && npx local-storefront app function schema   # in the Function's folder
npx local-storefront app function schema payment-rules                    # or by handle, from the app
npx local-storefront app function schema payment-rules --stdout > other.graphql
```

lsf carries the schemas of the checkout's Function APIs (cart transform, discounts, delivery and
payment customizations, cart and checkout validations) at 2026-10 and 2027-01: the ones it runs
the Functions with. Fulfillment constraints, order routing and pickup generators are not among
them. After changing `api_version`, write it again.

Before each run, the input query is checked against that schema (a field it lacks fails the run)
and against Shopify's
[input query limits](https://shopify.dev/docs/api/functions/latest#input-query-limits), which
Shopify checks when it deploys the Function:

- 3,000 bytes at most, comments left out;
- 100 elements at most in a list, given to an argument or as the value of a variable;
- a calculated cost of 30 at most, with Shopify's message past it
  (`Query has complexity of 33, which exceeds max complexity of 30`). `__typename`, the fields
  of a `Metafield`, a `HasTagResponse` or a `CollectionMembership` cost 0; the `metaobject` root
  field and any other leaf 1; a field that returns a `Metafield`, `field(key:)` of a
  `Metaobject`, `hasAnyTag`, `hasTags`, `inAnyCollection` and `inCollections` 3. Each selection
  counts once (two aliases are two). Where an object can be of several types (`merchandise`),
  the costliest type counts: Shopify's table does not say, and this is how GraphQL Ruby counts.

### Rust Functions

`cargo build` writes a module that calls the Function Wasm API of the host by name
(`shopify_function_v2::shopify_function_input_get`); `shopify app build` links it to the host's
provider with Shopify's `shopify-function-trampoline` after the build command. lsf runs that same
trampoline, as a library, at the version Shopify's CLI 5.0.0 runs (2.0.1): after the build command
of `--build`, `--watch` and `lsf app function build`, the module is the one the CLI would write, to the
byte. A module already linked is left as it is. Modules of the first version of the API
(`shopify_function_v1`) are refused, as by the trampoline: update the `shopify_function` crate.

The `typegen` macro of the `shopify_function` crate reads the input queries and
`schema.graphql` without telling cargo, so a change to one of them alone compiles nothing. When
one is newer than the module, lsf's builds (`--build`, `--watch`, `lsf app function build`)
touch `src/main.rs` and `src/lib.rs` first (their time, not their text), and cargo compiles the
crate again: `↻ delivery-rules: src/run.graphql changed: src/main.rs touched, for cargo to
compile the crate again`. A compiler cache (sccache, a `RUSTC_WRAPPER`) may still answer the old
build, as it does not see those files either: the watch then says that the module did not
change.

lsf does not run `wasm-opt -Oz`, which the CLI runs on a module before it links it: on the
playground's Rust Functions it takes 5 to 6% off the module's size, and 0.25 to 0.5% off the
instructions a run costs.

### Checkout UI extensions

A checkout or customer account UI extension whose targets name their sources
(`module = "./src/Checkout.jsx"`), in JavaScript, TypeScript or JSX, with Preact as Shopify's
templates or another runtime, is bundled by lsf the way `shopify app build` bundles it, without
Node:

```bash
npx local-storefront app build banner --app ../my-app
```

`--build` builds it too, and `--watch` builds it again when its files change, in a moment, then
reloads the open pages; at start, it builds those whose bundle is missing or older than their
files. As Shopify's CLI does:

- **The entry**: from API version 2025-10, each target's module is imported and its default
  export registered with `shopify.extend('<target>', …)`; before, each module is imported.
- **JSX** calls the automatic runtime of the `jsxImportSource` of the closest `tsconfig.json`
  (`preact` in Shopify's templates; `react` without one), in `.jsx`, `.tsx` and `.js` files. A
  Preact extension without its `tsconfig.json` fails on `react/jsx-runtime`, and the error says
  what to add: `{"compilerOptions": {"jsx": "react-jsx", "jsxImportSource": "preact"}}`.
- **The bundle** is one minified script (`dist/<handle>.js`): the modules it imports, what nothing
  uses left out, `process.env.NODE_ENV` `production` and the other values of `process.env` from
  the environment or the app's `.env`.

The bundle does what the CLI's does: the playground's tests draw both at their target and act on
them, and compare. It is not the same text (each is minified by its own tool), it is not lowered
to ES2015 (the page runs in a browser that needs none of it), and a target's `should_render`
module is not built (the page does not run it). A `.graphql` import is an error.

What is wrong with the app (a missing build, an unknown Function target, a checkout UI target
the page has no place for, an `owner_type` that is missing or not one of Shopify's) is printed
when the server starts (`app: …`), and is in the `diagnostics` of the state and on the home
page. A missing build
(``no Wasm module at …: build it with `npx local-storefront app build <handle>` ``, `not built`):
`npx local-storefront app build`, or `--build`. With `--build`,
`app dev`, `serve` and `app function run` build first, then print what is still wrong: a
module the build wrote is not reported missing.

| Option | What it does |
|---|---|
| `--app <dir>` (`LSF_APP`) | The app. The commands of `lsf app` default to the app the current folder is in. |
| `--extension <handle>` | Uses only this extension, a Function or a checkout UI extension. Repeat it for several. Without it, every extension of the app is used. |
| `--build` | Builds the extensions first, as `lsf app build` does: each Function (its build command, or lsf's build of a JavaScript Function) and the UI extensions that name their sources. With `--no-functions`, the UI extensions only. |
| `--no-functions` | Serves the UI extensions and the Admin API, and runs no Function: for a backend's tests ([Admin API](admin.md#place-an-order)). |
| `--only <parts>` (`LSF_ONLY`) | Runs only these parts of lsf: `app dev` runs `checkout,admin`, `serve --app` adds `theme` ([what runs](cli.md#what-runs)). Without `checkout`, the home page, `/__lsf/checkout` and `/__lsf/checkouts` answer `404` with an `X-LSF-Reason`, and so do `/checkout` and `/checkouts/…` without a theme; with one, `/checkout` is the theme's summary of the cart. Without `admin`, `complete` records no order in the Admin API and sends no webhook. |
| `--watch` | Develops the app as `shopify app dev` does: builds a Function again when its files change, and reloads the open pages. With `--build`, builds every Function at start. See [watch mode](#watch-mode). |
| `--variant <standard\|shop>` | The look of the page: Shopify's one-page checkout, or the Shop app's. A URL can ask for the other with `?variant=shop`. |

`app dev` takes the other options of [`serve`](cli.md#serve) (`--port`, `--host`, `--quiet`,
`--static`, `--customer`, ...).

### Watch mode

```bash
npx local-storefront app dev --app ../my-app --watch
```

It works with `serve --app` too. Three things happen:

- **A save builds the Function again**: its build command, or lsf's build of a JavaScript
  Function. The terminal says `↻ building discount-codes…`, then
  `✓ discount-codes built in 1.4s`.
- **A build that fails keeps the last module**, as `shopify app dev` does: the checkout runs it.
  The terminal, a banner of the checkout page and the home page show the error; the next build
  that works clears it.
- **The open pages reload** once the builds a change started are over: the checkout, the
  thank-you page and the home page. What the buyer entered is kept, it is in the session.

A change to a checkout UI extension's bundle, an extension added or removed,
`shopify.app.toml`, a scenario or the store data reloads the pages without a build.
`lsf app function run` has no watch mode: it runs the modules in `dist/`, as they are.

#### Details

- **At the start**, the modules that are there are used, as without `--watch`, when nothing
  changed since the last watch of the app. For each Function that has a build command
  (`[extensions.build] command`), the user's cache folder (`~/Library/Caches/lsf/watch/` on
  macOS, `~/.cache/lsf/watch/` on Linux, never the app) records what its last build was made
  from (its files, the files the builds share, its command) and a hash of the module it
  published. A module is trusted only when both still match, and no build of it runs (see the
  write lock below). Otherwise it is built: a Function with no module a run can load, one whose
  files changed, and one whose module changed since, whether a build was stopped halfway,
  failed after writing it, or another process wrote it (a build left running by a server that
  was killed, another server of the same app). A Function with nothing recorded, on the first
  watch of an app, keeps the module it has, and the terminal says `using the existing builds;
  --build rebuilds them all at start`. `--watch --build` builds every Function that has a build
  command. A build that fails does not stop the server.
- **One build at a time writes a Function's module**, across the servers of an app: each
  Function has a write lock, next to the record (`locks/<handle>.lock`). A build takes it before
  it starts, and the server lets go of it once it read, checked, published and recorded the
  module. Another build of that Function waits meanwhile, and the terminal says
  `↻ discount-codes: waiting for another build of it to end (another lsf, or one left running)`;
  Ctrl-C still stops it at once. On macOS and Linux the build holds the lock too, and so does
  what it starts (`cargo`, `rustc`): a build left running by a server that was killed holds it
  until it is over, so what it writes is never taken for the module of another build. A lock
  that only a process the build started still holds, once the build is over (a compiler cache's
  server, such as sccache), is not waited for. Limits: a build that closes the descriptors it
  inherits, or starts its writer apart from itself, does not hold the lock to its end; a
  process that stays in a killed server's build's process group is waited for while it runs. On
  Windows only the server holds the lock: a build that a killed server left running is not
  waited for there (the next start still builds again a module that is not the one recorded).
- **What is watched**: the folder of each extension (`src/`, the input queries, its
  `shopify.extension.toml`, `locales/`, and the `dist/` bundle of a checkout UI extension), what
  the builds of the Functions read in the app's own folder (a Cargo workspace's `Cargo.toml` and
  `Cargo.lock`, `rust-toolchain.toml`, `package.json` and its lock file, `pnpm-workspace.yaml`,
  `tsconfig.json`, and the folders that hold a crate or a package the Functions may share: a
  `Cargo.toml` or a `package.json` at most three levels down, as `crates/common/Cargo.toml`),
  `shopify.app.toml`, and the extension folders that are added or removed. A symbolic link is
  watched where it leads. The store data and its scenarios are watched as always.
- **What is not**: the rest of the app's own folder, which is the app's and its dev server's: its
  sources (`src/`, `app/`), its pages (`index.html`), and what its tools build or fetch at its
  root (`dist/` of trunk or Vite, `build/`, `target/`, `node_modules/`), so that an app whose
  web front end rebuilds on every save (an openshop project) does not build its Functions and
  reload the pages each time. Nor `.git/`, `.shopify/` and other dot folders, editors' temporary
  files, Markdown documents (in any folder), the store data folder (`shopify-local/`, or the one
  `--data` names), and what a build writes: the `dist/` folder of a Function that has a build
  command, its module (`[extensions.build] path`), `schema.graphql`, and the types of a
  JavaScript Function's typegen (`generated/api.ts`). A build does not start itself again.
- **A change to a Function** with a build command runs that command in its folder, as `--build`
  does. A save that writes several files builds once. One build at a time per Function: a change
  during its build makes one more build after it. Up to 3 Functions build at once (the Functions
  of one Cargo workspace wait for each other on its lock). A JavaScript Function without a build
  command counts as one with a build command: lsf builds it (`lsf app function build`, in a
  process of its own).
- **A change to the other files of the app** builds every Function that has a build command:
  `↻ Cargo.toml changed: building 12 Functions`.
- **The pages reload** once per change, however many Functions it builds. The home page runs the
  Functions of the scenarios again.
- **The checkout never runs what a build is writing.** It runs a copy of the last module of each
  Function that built, kept out of the app (in the system's temporary folder, removed when the
  server stops). Once a build is over and its module compiles as a run compiles it (and exports
  a function), the copy is replaced in one step. The checkout, the Functions dialog and the runs
  of the home page all read it. `dist/` is only written by the build command.
- **A build that fails**, or writes a module that does not compile (`the build wrote an invalid
  module: …`), leaves the last copy. The terminal prints
  `✗ discount-codes: build failed (exit status: 101)`, the first error (from its `error` line to
  the end of its source context) and the last lines of the output; the checkout page shows them
  in a banner and in the overview of the Functions dialog, and the home page lists the build
  first among its problems. A Function that never built is not built until a build works, and
  the banner and the home page say `no module of it built yet`.
- **A build that hangs** does not hold the server: it is stopped after 10 minutes and fails.
  Ctrl-C stops the builds that run, and so do `SIGTERM` and `SIGHUP` on macOS and Linux. One
  stopped halfway is built again at the next start, unless what it left (its files and the bytes
  of its module) is exactly what was last published and recorded. On Windows, only Ctrl-C
  stops the builds.

The files are watched through the system's events (FSEvents on macOS, inotify on Linux, folder
by folder, without the tools' folders): at rest, watching an app of 90 extensions costs about
0.2 % of a core. Where a file system has no events, they are polled.

## The look of the page

The standard look follows Shopify's one-page checkout, in lsf's own CSS: a header with the name
of the shop and a cart icon over a rule that runs the page's width; on a screen of 1000px or more
two columns (the form on white, 500px wide, and the order summary on a grey panel as tall as the
page, 400px wide, that stays in view while the form scrolls); below that one column with the
summary folded into a bar under the header that shows the total. Above the contact section, the
express buttons: the payment methods of the `ACCELERATED_CHECKOUT` placement that the payment
customizations left (Shop Pay, PayPal, Apple Pay, Google Pay… in their colors), then "OR". A click
says that lsf does not simulate the wallet. Fields have their label inside
the box, which floats to the top once there is something to label; payment and shipping methods
are one bordered box of rows, the chosen row tinted with its accent border; the order summary
has 64px thumbnails with the quantity hanging off a corner, the discount field with its Apply
button inside, and the Total row.

Everything that sets the look is a CSS custom property in `crates/app/assets/tokens.css`,
read by the page and by the frame of the extensions, so that an extension looks like the page
around it. The defaults are those of a store with no branding: system fonts and a 14px base text
(`--lsf-font`, `--lsf-font-size-base`, `--lsf-font-step`), black on white and a blue accent
(`--lsf-color-accent`, `--lsf-color-text`, `--lsf-color-bg`, `--lsf-color-border`, ...), a grey
order summary (`--lsf-summary-*`), corner radii of 8, 12 and 16px (`--lsf-radius-small`, `-base`,
`-large`, or one `--lsf-radius-global` for all), spacing (`--lsf-space-*`) and control sizes. The
hover color, the three light tints, the focus ring and the subdued text on a tint are derived
from the accent, so one color brands the page. The Shop skin sets its own accent, page color and
radii in the same file.

## The home page

`GET /` of `lsf app dev` (`/__lsf/checkouts` with a theme; `?format=json` answers what the
page says, for tools) has a bar at the top and six tabs. It has the look of the checkout and
needs no network: it sends no request of the Functions, only the checkout UI extensions loaded to
check them can call a server (see below).

- **The bar**: the app and its store data, how many problems there are (a link to the Problems
  tab), "More" (a checkout with [every extension target](#every-extension-target), the Shop
  look), "Admin" (the app in lsf's admin, when the Admin API runs:
  [preview the embedded app](admin.md#preview-the-embedded-app)), the
  [customer account](#customer-account) and "Open the checkout".
- **Scenarios**: the groups on the left, the scenarios on the right (see below).
- **Functions**, **Checkout UI extensions**, **Customer account UI extensions**: a tab for
  each kind of extension of the app.
- **Customers**: a card for each customer of the store data, with a checkout logged in as them
  and their customer account.
- **Problems**, errors first, each on one line. The first eight of each kind show, a "Show N
  more" opens the others. The link on the right of a problem opens a fresh checkout where it
  shows, as the Open button of a scenario does, and its name is under the link. Its details also
  link to the rows of this page it is about: a link opens the tab of what it leads to.
  - A Function that fails: "Open the run", the first scenario it fails in, with the [Functions
    dialog](#what-runs) open on that run (`?functions=<handle>`, and `:<target>` when the
    Function has several). The details link to the run in each of the other scenarios.
  - Another problem of a Function: the scenario that runs it (of those that name it in
    `extensions`, the one with the fewest; otherwise the first that runs every extension), with
    the dialog on it, on the target the message names. A Function that has a run there (a legacy target, a missing build, whose run
    fails) opens on that run, "Open the run"; one that never runs (an `api_version` that is not
    supported, an unknown or listed-only target) opens on the Overview, which says it did not
    run, "Open in checkout".
  - A checkout UI extension: the scenario that runs it, chosen the same way, without the dialog, where it shows
    on the page, with what goes wrong. One the page does not load (not built, an API version that
    is not supported, an `owner_type` that is wrong or missing) opens the Overview of the dialog on
    it.
  - An extension that no scenario runs: the checkout of the store's cart, `/checkout`, the same
    way.
  - A scenario that does not validate cannot open: the link goes to its row.
  - Errors: an extension that cannot load or run (no build, an API version that is not
    supported or missing, an unknown Function target, a missing or wrong `owner_type`, what the
    loader refuses), a scenario that does not validate or names an extension the app lacks, an
    error in the store data, and **a Function that fails when a scenario runs**.
  - Warnings: legacy targets, targets the local checkout knows of and does not run, targets the
    page has no place for, and what else the loader says that does not stop anything.
- **The scenarios** ([scenarios](#scenarios)) are in groups: the part of their titles before
  `: ` ("Discount: buy 2, get 1" is in "Discount") when two scenarios or more share it, the others
  first under "Checkouts". The menu on the left shows them all or one group, with the worst of
  their dots and how many they hold; a row in its group drops what the group says of its title.
  In a group, those with problems come first, then by name. A row has a status dot: the worst
  of its own problems (it does not validate, it names an extension the app does not have) and of the Functions that fail in it, which are errors, and of the problems of the
  extensions it runs, each at its own level (a Function that is not built makes it an error, a
  legacy target a warning). Then the title, its description (else a few words for its cart,
  buyer, country, language and codes), and Open, a fresh checkout for it. Its details say what it
  runs ("every extension of the app" without an `extensions` list), its name and file, the cart,
  the buyer, what was entered, the settings it overrides, the extensions it runs, and a link that
  opens it with every target. The discounts are listed only when the scenario sets its own
  (`checkout.discounts`); otherwise they are "the store's N discounts". A search box (it looks in
  every group) and a filter (all, with problems, Functions, UI) narrow the list. A link to a row
  (`#scenario-…`, from a problem) shows every group and opens the row.
- **The customers** ([login profiles](#log-in-as-a-customer)): name, email, company, tags,
  orders, saved addresses and country, "Checkout as …", which opens the checkout logged in as
  them (the first scenario's, for its cart, when there is one), and "Account", their customer
  account.
- **The extensions**: the Functions, the checkout UI extensions and the customer account UI
  extensions, each in its tab. An extension with targets in both the checkout and the customer
  account is in both. A problem of an extension that only renders in
  the customer account opens it ("Open the customer account"). One line each: its status, handle and
  name, Function API or capabilities, API version, how many scenarios run it, its targets, and its
  first problem. Its details list each target (the Function API, a `.fetch` target that reaches
  the network, a legacy target; for a UI extension, the place it has on the page, such as
  "Checkout summary: under each line"), how to build it, and all of its problems.

An extension that `--extension` leaves out is listed and marked so. The page follows the locale
prefix of its URL (`/fr/` links to `/fr/checkout`; `/__lsf/checkouts?locale=fr` does the same).

### The Functions that fail

The home page runs the Functions of every scenario that opens, as its checkout would when it
opens (what `lsf app function run --scenario` does), and lists each Function target that fails, with
the error and the scenarios it fails in: a trap or a panic, a run over one of
[Shopify's resource limits](#resource-limits), an input query over
[Shopify's limits](#the-schema-of-a-function), an output that does not fit the schema of its
API version, a `.fetch` request out of Shopify's limits. A target that fails in several ways is
listed once per error, each with its own scenarios and logs. The scenario and the Function get an
error status. A failure is a problem to fix even when the checkout goes on: Shopify ignores the
output of a failed run and blocks the order only for a validation or a cart transform that blocks
on failure ([network access](#network-access)).

#### Resource limits

A run fails over one of Shopify's
[resource limits](https://shopify.dev/docs/api/functions/latest#resource-limits), as on Shopify,
which ignores its output:

| Limit | Shopify's | The error |
|---|---|---|
| Compiled module | 256 KiB (262,144 bytes) | `the module is 334,239 bytes: Shopify does not deploy a Function over 262,144 bytes (256 KiB)`; `app build` warns about it too |
| Linear memory, all the memories of the run (a JavaScript Function's Javy provider's too) | 10 MiB (10,485,760 bytes) | `exceeded Shopify's limit of 10,485,760 bytes of linear memory: it asked for …`: the grow past it stops the run |
| Instructions | 11 million | `used 11234567 instructions: Shopify stops a Function past 11000000` |
| Input, as JSON | 128 kB (128,000 bytes) | `the input is 146,171 bytes: Shopify does not run a Function on an input over 128,000` |
| Output, as JSON | 40 kB (40,000 bytes) | `the output is 45,080 bytes: Shopify ignores an output over 40,000` |

The last three are for a cart of up to 200 lines; for a bigger cart they grow in proportion to
its lines, as Shopify's "dynamic limits" do (300 lines: 16.5 million instructions, 192 kB, 60 kB).
`lsf app function run --input` takes the lines of the input's `cart.lines`.

lsf stops a run at 110 million instructions, ten times Shopify's limit: an endless loop fails in
a moment (`exceeded Shopify's limit of 11,000,000 instructions: stopped at 110,000,000`), and a
run under that has its exact count.

This check sends no HTTP request: opening the home page never calls a server. A `.fetch` target
runs, and the request it asks for is checked against [Shopify's limits](#network-access) (a
request out of them fails the target), but it is not sent: the `.run` target reads what it reads
when the connection fails, `502 Bad Gateway (Connection Failed)`. A Function that only fails on
some answer of its server is found by opening its scenario, which sends the request.

Running them all takes a moment (under a second for the ~90 scenarios of the
[playground](https://github.com/0xtlt/local-storefront/tree/main/examples/checkout-playground)),
so the page does not wait for it: it shows "Running the Functions…" and asks
`GET /__lsf/checkouts/check`, which answers what it found. The check has 20 s: it looks at the
time before each Function runs (each run is bounded on its own by the instruction limit), and
once they are spent it stops and says so ("Checked 40 of 86
scenarios"). A complete check is kept until the store data or the app (a Function, its Wasm
module) changes: the page then has it at once, and the next check runs on the next request after
a change. A partial check, or one whose app changed while it ran (a module rebuilt), is not kept.
A check that fails or takes more than 30 s says so, and the rest of the page stays. Without
JavaScript, the page links to `?check=1`, which runs the check before it answers.

### The checkout UI extensions that do not load

The page also loads the bundle of each checkout UI extension in a hidden frame of the sandbox,
as the checkout does, without rendering it. Unlike the check of the Functions, this loads the
real bundles: a bundle that calls the network as it loads does so. A bundle that does not load
(no file), throws while it loads, or does not register a target it declares (`shopify.extend`),
is listed with the problems with what it threw, and the scenarios that run it take the level of
that problem, as for any extension with a problem. A frame that says nothing within 5 s is a
warning that the page could not tell; one that reported an error and never finished is that
error. An extension that throws only when it renders or when the buyer acts is not found this
way.

Messages show what is between backticks as code, and the paths inside the app relative to its
folder (the full path is in the tooltip).

## Scenarios

A scenario is a named checkout setup: one file of `checkouts/` in the
[store data](data-format.md#checkout-scenarios) (`<data>/checkouts/<name>.json`, the data folder
being `--data`, `LSF_DATA` or `shopify-local/`). It sets what a checkout depends on, on top of
the store data: the cart (lines, quantities, selling plans, note, attributes), the buyer (a guest,
a customer, a company location), the country, the language, what the buyer already entered
(email, address, discount codes, delivery and payment), overrides of the `checkout` section
(shipping rates, payment methods, discounts, Function settings, tax), the look and the
extensions that run, and what the checkout must look like once the Functions ran
([`expect`](#expected-results), which `app check` checks). The files are reloaded when they
change, and `validate` checks them: run it in the app folder, no theme needed
(`npx local-storefront validate`).

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
- `lsf app function run --scenario <name>` runs the Functions for a scenario.
- Only one scenario is open per browser: the session is the cookie's.

`npx local-storefront app init --app ../my-app` writes the scenarios, made for the app:

| The app has | Scenarios |
|---|---|
| a cart transform | `cart-transform`: several lines of more than one unit and one of three, for a merge, an expand or an update |
| a discount Function | `discount-automatic` (a cart of six, three and two units) and `discount-code` (the same cart with `SAVE10` entered; the code of a discount of the store data is kept) |
| a delivery customization | `delivery-options`: standard, express, local delivery and store pickup |
| a payment customization | `payment-methods`: card, PayPal, cash on delivery and bank deposit in the list; PayPal, Shop Pay and Apple Pay as express buttons (PayPal in both placements, as with PayPal Express on) |
| a validation Function | `validation`: a big cart, for the Function to refuse |
| a checkout UI extension | `all-extensions`: a customer, several lines (one with a subscription), shipping options, and with a discount Function, a discount of it with the code `SAVE10` entered |
| (always) | `guest`, `customer` (with a note and an attribute), `b2b` (a company customer at another location), `abroad` (another country and language), `subscription`, `pre-order` (a deposit, the rest later), `shop-look`, `large-cart` |

A scenario about one kind of Function runs that Function and the checkout UI extensions, and no
other Function (`extensions`), so that its effect shows alone; `all-extensions` names every
extension. A scenario that names a Function runs it whether or not the shop has an owner of it
(see [what runs](#owners-and-scenarios)): it tries the Function alone. The others
(`guest`, `b2b`, `abroad`, `large-cart`, …) name none: they show the shop's checkout, where only
the Functions with an enabled owner run. A Function you add later is not in those lists: add its
handle to them. The settings of your Functions are the store data's: the scenarios leave
`checkout.functions` alone, and the `discount-*` scenarios list the discount Functions again
with the settings (and the code) `checkout.discounts` gives them. The scenarios are a place to
start: edit them.

**A discount Function runs in `discount-automatic`, `discount-code` and `all-extensions` only.** It runs only for a
discount that names it ([set up the checkout](#set-up-the-checkout)), and the other scenarios
(`b2b`, `abroad`, `large-cart`, …) have none: `app check` passes on them without running it. It
warns only about a Function no scenario ran at all (`warning  <handle>: ran in no scenario, so
nothing checks it`, `1 Function ran in no scenario` in the summary, and `ranInNoScenario` in
JSON), and its exit code stays the same. To run it in
every scenario, a B2B buyer, another country and a large cart included, give it a discount in the
`checkout` section of the store data (`store.json`, or the file that has the section):

```json
{
  "checkout": {
    "discounts": [
      {
        "function": "my-discount",
        "title": "My discount",
        "metafields": { "$app.function-configuration": { "type": "json", "value": { "percentage": 10 } } }
      }
    ]
  }
}
```

The `discount-*` scenarios keep their own `checkout.discounts`.

`app init` creates the data folder with the demo store when it does not exist (and a
selling plan with a deposit for `pre-order`). When it exists, it uses the products, customers,
countries, languages and selling plans it has, and says which scenarios it skipped and why. It
never overwrites a file without `--force`, and refreshes `schema/` so that editors complete the
scenarios (`"$schema": "../schema/checkout-scenario.schema.json"`).

A Function that reads its settings as the variables of its input query
(`[extensions.input.variables]`) cannot run without them. `app init` gives each one the
settings the store data lacks: those of the app's own `shopify-local/` when it has them, or empty
values of the type of each variable (`[]`, `""`, `0`, `false`) to fill in. Those of a discount
Function go in the `metafields` of its discounts in the scenarios; the others in
`checkout.functions` of `app-settings.json`. When another data file has the `checkout` section,
it prints what to add to it instead.

### Expected results

`expect` says what the checkout of a scenario must look like once its Functions ran, and
`npx local-storefront app check` fails when it does not. An app that hides PayPal from the express
buttons, and only from them, asserts that PayPal is gone from the buttons and still in the list:

```json
{
  "title": "PayPal Express hidden, PayPal kept",
  "session": { "cart": { "items": [{ "variant": "TEE-BLK-M", "quantity": 3 }] } },
  "checkout": {
    "payment_methods": [
      { "handle": "card", "name": "Credit card" },
      { "handle": "paypal", "name": "PayPal", "placements": ["PAYMENT_METHOD", "ACCELERATED_CHECKOUT"] },
      { "handle": "shop_pay", "name": "Shop Pay", "placements": ["ACCELERATED_CHECKOUT"] }
    ]
  },
  "extensions": ["payment-rules"],
  "expect": {
    "express_methods": { "excludes": ["paypal"], "equals": ["shop_pay"] },
    "payment_methods": { "includes": ["paypal"] },
    "errors": { "equals": [] },
    "totals": { "total": "96.00" }
  }
}
```

Each list takes `includes` (each of these matches an entry of the list, in any order),
`excludes` (none of these matches one) and `equals` (the whole list, in order; `[]` is an empty
list, so `"errors": { "equals": [] }` says there is no error):

| List | What it is | A string matches | An object can give |
|---|---|---|---|
| `payment_methods` | The payment methods of the list (`PAYMENT_METHOD`), in order, once the payment customizations ran | the handle or the name | `handle`, `name` |
| `express_methods` | The express buttons (`ACCELERATED_CHECKOUT`), in order | the handle or the name | `handle`, `name` |
| `delivery_options` | The delivery options, in the order the buyer sees them, once the delivery customizations and the shipping discounts ran | the handle or the title | `handle`, `title`, `cost` |
| `discounts` | The discounts applied: product, order and shipping ones | the title or the code | `title`, `code`, `class` (`PRODUCT`, `ORDER`, `SHIPPING`), `amount` |
| `lines` | The lines, once the cart transforms ran | the SKU or the title | `sku`, `title`, `quantity`, `price` (of one unit), `total` (after its discounts), `components`, `attributes` |
| `errors` | The errors that block the order: those validation Functions report, and the failures of the validations and cart transforms that [block on failure](#the-functions-that-fail) | the message | `message`, `target` |

A string names an entry, so a renamed method is matched by its new name (`"Card (secure)"`) or by
its handle. An object gives the fields the entry must have, and leaves the others free:
`{ "handle": "standard", "cost": "0.00" }` is the option `standard`, free, whatever its title.
Amounts are written as in the store data: cents (`490`) or a decimal string (`"4.90"`). A line's
`components` are the lines a cart transform merged into it or split it into, in order, each by
its SKU or title (`[]`: none); its `attributes`, those it must have, by key. `totals` takes
amounts, each checked exactly: `subtotal`, `discounts` (product and order ones), `shipping`
(after the shipping discounts), `tax`, `total`, `due_now` and `due_later`.

The Functions run for `expect` as the order is placed (`buyerJourney.step` is
`CHECKOUT_COMPLETION`), as `app function run` runs them. `"step": "interaction"` checks the page
as the buyer sees it before: a validation Function that waits for "Pay now" reports nothing then.
Only a validation Function tells the two apart.

`app check` prints each expectation that is not met: the scenario and the assertion, what the
scenario expects, and what the checkout has. With a Function that hides nothing:

```text
expect   express-buttons: express_methods.excludes
         expected: no entry matching "paypal"
         actual:   [{"handle": "paypal", "name": "PayPal"}, {"handle": "shop_pay", "name": "Shop Pay"}]
expect   express-buttons: express_methods.equals
         expected: ["shop_pay"]
         actual:   [{"handle": "paypal", "name": "PayPal"}, {"handle": "shop_pay", "name": "Shop Pay"}]
checked 1 of 1 scenario in 0.0 s: 0 failures, 0 invalid scenarios, 2 of 5 expectations unmet
```

`actual` shows each entry with the fields the expectation names, amounts as decimal strings, so
that it can be pasted into the scenario. Each entry of `includes` and `excludes`, each `equals`
and each amount is one expectation: when all are met, the summary says `5 expectations met`.
`--format json` has them in `expectations` ([`app check`](cli.md#app-check)), where `expected`
is the entry, the list or the amount as the scenario writes it:

```json
"expectations": {
  "checked": 5,
  "unchecked": [],
  "unmet": [
    {
      "scenario": "express-buttons",
      "file": "checkouts/express-buttons.json",
      "step": "completion",
      "assertion": "express_methods.excludes",
      "expected": "paypal",
      "actual": [{ "handle": "paypal", "name": "PayPal" }, { "handle": "shop_pay", "name": "Shop Pay" }]
    }
  ]
}
```

A scenario without `expect` is checked as before: its Functions must run without an error. With
`--extension`, the `expect` of a scenario that runs a Function left out is not checked (its
checkout is another one). The scenario still runs, its expectations are not counted in
`checked`, `expectations.unchecked` names it, and the exit code does not change. The summary
says so: ``the `expect` of 1 scenario not checked (--extension leaves out a Function it runs)``.

A name in `excludes` that the checkout does not offer, such as `"paypall"`, is never in the list,
so the expectation would always pass: `validate` warns about it (`expect_checks_nothing`), with
the closest handle. The scenario still opens.

## Log in as a customer

The customers of the [store data](data-format.md#customers-and-sessions) are the login profiles
of the checkout. "Log in", at the right of the Contact heading, opens a menu of them: name,
email, and what tells them apart (B2B company and its locations, tags, orders, saved addresses,
country). Pick one and the checkout is logged in as them; "Continue as a guest" logs it out. The
menu is a menu button: the arrows, Home and End move between the customers, Escape closes it and
gives the focus back to the button, Tab and a click elsewhere close it.

Logged in, the checkout does what Shopify's does for a customer who has an account:

- **Contact** shows the account (name and email) instead of the email field, with "Switch" (the
  same menu) and "Log out". The email and the phone are the customer's. A contact email that
  is not the account's (a scenario's `entered.email`, or one typed later) is the one the
  Functions, the extensions and the order get, so Contact shows it under the account, as "Email
  for this order", where it can be corrected, with "Use &lt;account email&gt;" to go back to the
  account's.
- **Delivery** starts with the customer's default address (the first of their `addresses`) and
  offers the others in a "Saved addresses" select, with "Use a new address" for one they type.
  The fields below hold the chosen address and can be edited.
- **A B2B customer** shows their company and chooses the location they buy for, among the ones
  they have access to; the order ships to the address of that location ("Ship to the location").
- Everything that reads the buyer follows: the Functions (`buyerIdentity.customer` with `id`,
  `email`, `displayName`, `hasTags`, `hasAnyTag`, `metafield`, `numberOfOrders`, `amountSpent`;
  `buyerIdentity.purchasingCompany`; `isAuthenticated`), the checkout UI extensions
  (`shopify.buyerIdentity.customer`, `email`, `phone`, `purchasingCompany`, whose subscribers are
  called; the `sub` of the session token), the shipping rates (an address means rates), the
  discounts and the payment methods the Functions decide on.

A login changes the session, the way `session.customer` of the data, a scenario or
`PUT /__lsf/session` set it: the theme (with `serve --app`) shows the same customer. The cart,
the checkout token, the choices of delivery and payment, the codes and the scenario the checkout
opened from stay. What belonged to the buyer before goes: the email and the phone they typed,
and an address the customer before had saved (an address typed as a guest stays for a customer
who has none). The checkout follows a login made anywhere else too (`/__lsf/login`, the control
API): the next time it is read, it is the new customer's.

- `GET /checkout?customer=<email>` opens the checkout logged in as a customer (`default` for the
  first one, `none` for a guest); `/checkout?scenario=<name>&customer=<email>` opens a scenario
  as someone else. Someone the data does not have answers `404` with the list of customers.
- A login from a URL is the login of the menu: `?scenario=<name>&customer=<email>` gives the same
  checkout as opening `?scenario=<name>` and picking the customer in the menu. The scenario is
  set up first (its `entered` included), then the login applies what is written above: the
  contact (email and phone) becomes the customer's, the address their default one, and the
  scenario's address stays only for a customer without one. When `customer` is the one the
  scenario already logs in as, nothing changes, and everything the scenario entered stays.
- The page sends the [operations](#operations) `customer` and `companyLocation`.
- The state has `account`: `profiles` (the customers to log in as), `addresses` (the saved
  addresses of the customer logged in, the default one first, each with an `id` and the
  `address`; for a B2B customer, the one of their location) and `locations` (the locations a B2B
  customer can buy for, with `current`). Extensions do not see it.

The Shop skin has the same menu, account and select.

The demo store (`lsf init`, `lsf app init`, and the built-in data) has a profile of each
kind:

| Customer | What for |
|---|---|
| `jane.doe@example.com` | A returning customer: two saved addresses, two orders, the tag `vip`, a `custom.loyalty_tier` metafield |
| `new.customer@example.com` | A new customer: no order, no address |
| `alex.morgan@example.com` | A B2B buyer of Northwind Hotels, with two locations (Portland, Seattle) |
| `camille.martin@example.com` | A customer in France: an address in Lyon, one order |

Not done: logging in with a password or a one-time code (any customer of the data, at once),
Shop Pay, store credit, the customer's language (the checkout is in the language of its URL),
and the B2B settings that depend on the location in Shopify (catalogs, price lists, payment
terms, deposits, and whether a buyer may ship to another address): a B2B buyer pays what the
store data says and may type an address.

### Where each target renders

Every target of Shopify's checkout UI extensions has a place (`assets/api.json`), besides those
of the [customer account](#customer-account):

- **Pickup**: the shipping rates of kind `pick_up` are a "Pick up" list of their own in each
  delivery group, between `pickup-location-list.render-before` and `-after`, with
  `pickup-location-option-item.render-after` under each. The pickup points come from Shopify's
  pickup point Functions, which do not run here: with an extension at `pickup-point-list.render-*`,
  the page shows the list with no point in it.
- **Payment apps**: under the selected method, `payment-option-item.details.render`, and under the
  card fields of the credit card, `payment-option-item.hosted-fields.render-after`.
  `payment-option-item.action-required.render` is a dialog over the page that opens when the buyer
  selects a method ("Complete the payment form" opens it again). `shopify.target` is the payment
  option; there are no payment method attributes.
- **Gift cards**: with an extension at `gift-card.render`, the payment step has "Pay with a gift
  card"; ticking it shows the extension. `applyGiftCardChange` and `applyRedeemableChange` are
  refused.
- **Bundles**: a line a cart transform expanded lists its products in the summary, or
  `purchase.cart-line-item.line-components.render` renders in their place, on the checkout and the
  thank-you page.
- **Chat**: `purchase.checkout.chat.render` and `purchase.thank-you.chat.render` sit over the page
  in the bottom right corner.
- **Address autocomplete**: `purchase.address-autocomplete.suggest` renders nothing. As the buyer
  types the address field, the page runs it (in a hidden frame) with `shopify.target` (`value`,
  `field`, `selectedCountryCode`) and shows the suggestions it returns under the field. Picking one
  fills the address from its `formattedAddress`, or from what
  `purchase.address-autocomplete.format-suggestion` returns for it. A new request aborts the one
  before (`shopify.signal`).

### Every extension target

`/checkout?targets=all` (or `&targets=all` after `?scenario=`) opens a checkout where each
extension renders at every target it declares: the steps of a cart that ships are shown whatever
the cart, and the thank-you page is shown under the checkout with the same cart (no order is
placed), so the thank-you targets are there too. A panel at the top lists every target of every
extension: shown, without a place on the page, waiting for something the cart lacks (shipping
methods need an address, a line target needs a line, a pickup target a rate of kind `pick_up`),
run as the buyer types (the address autocomplete), or in the customer account. The
`all-extensions` and `ui-every-target` scenarios are made for it.

## Customer account

With an app, `/account` is the customer account where its customer account UI extensions render:
the pages of Shopify's new customer accounts, filled with the customers and orders of the store
data. The home page links to it ("Customer account").

| Path | Page | Targets |
|---|---|---|
| `/account` | The orders of the customer logged in | `order-index.announcement.render`, `order-index.block.render`, `order.action.menu-item.render` on each order |
| `/account/orders/<id>` | The status of an order: its shipments, what is not shipped, its returns, the details, the payment and the summary | the 9 `order-status.*` targets, `order.action.menu-item.render` |
| `/account/profile` | Name, contact, addresses, payment methods; for a B2B customer, the company and each location it buys for (addresses, payment, staff) | the 8 `profile.*` targets |
| `/account/pages/<handle>/…` | The full page of an extension | `customer-account.page.render` |
| `/account/orders/<id>/pages/<handle>/…` | The full page of an extension for an order | `customer-account.order.page.render` |

`customer-account.footer.render-after` is at the bottom of every page. Pressing the button of an
extension's `order.action.menu-item.render` opens its `order.action.render` in a dialog, which
`shopify.close()` closes; a menu item with an `href` leads there instead.

- **Logged out**, the page lists the customers of the store data to log in as
  ([log in as a customer](#log-in-as-a-customer)); the avatar in the header switches customer or
  logs out.
- **The orders** are those of the customer in the store data (`orders` of a customer), the most
  recent first, and the last one the session placed in the local checkout, logged in as them.
  The orders of a session are numbered after the last one of the store data (#1003 when its last
  is #1002), one after the other, whatever the other sessions do, and placed at the frozen clock
  of the data (`now`) if it has one: a test reads the same number and date every time. They live
  as long as the session (`DELETE /__lsf/sessions` forgets every one). An order's shipments come
  from its `fulfillment_status` (`fulfilled`: one shipment of every line; `partial`: its first
  line), its returns from its `returns` ([data reference](data-reference.md#orderreturn)).
- **The `shopify` global** of a customer account target has what every target has (`i18n`,
  `storage`, `sessionToken`, `settings`, `localization`, `extension`, `analytics`, and `version`,
  the extension's `api_version`), and:
  `authenticatedAccount` (`customer`, `purchasingCompany`); `navigation`; `toast.show`, which shows
  a toast at the bottom of the page; `intents.invoke`, which answers `closed`. The order status
  targets and the full page of an order have the Order Status API (`order`, `lines`, `cost`,
  `shippingAddress`, `billingAddress`, `buyerIdentity`, `discountCodes`, `authenticationState`,
  …) and `appMetafields`, with the order's own metafields (`owner_type = "ORDER"`, see
  `shopify.appMetafields` below), the line, shipment, return, location and order targets their
  `target`, `fulfillmentId`, `returnId`, `locationId` and `orderId`.
- **Navigation**: `shopify:customer-account/orders`, `…/orders/<id>` and `…/profile` lead to the
  pages; `extension://<handle>` (or `extension:<handle>/<path>`) to the full page of another
  extension; `extension://<route>` and `/<route>`, on a full page, to a route of its own. The page
  loads again: `navigation.currentEntry.url` is `extension:<handle>/<route>`.
- **Components**: the ones of checkout, and those only customer accounts have: `s-avatar`,
  `s-button-group`, `s-customer-account-action`, `s-image-group`, `s-menu`, `s-page`.
- `?targets=all` adds a panel that lists the customer account targets of each extension, and where
  each one renders.
- **An uninstalled app** (`POST /__lsf/admin/uninstall`) renders no customer account extension: a
  banner and the panel say "the app is not installed on the shop", and a full page answers `404`
  with that reason. They render again once it is installed ([uninstall](admin.md#tokens)).

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
          "$app.function-configuration": { "type": "json", "value": { "minimumQuantity": 3, "percentage": 10 } }
        }
      }
    ],
    "functions": {
      "delivery-rules": {
        "metafields": { "$app.function-configuration": { "type": "json", "value": { "hide": ["express"] } } }
      }
    },
    "tax_rate": 20
  }
}
```

- A **discount** is a discount Function with its title, its metafields and, if it has one, the
  code that triggers it. Without a code it is automatic. `classes` lists what it may reduce:
  `PRODUCT`, `ORDER`, `SHIPPING`. All three when omitted.
- **A discount Function runs only for a discount that names it**, as on Shopify, where the
  merchant creates the discount: one that no discount of the store data (nor of a scenario)
  names never runs, and the home page and `lsf app function run` say so. The demo store of
  `lsf init` has none: add yours here, in a scenario, or for one test in the `data.checkout` of
  `PUT /__lsf/session` (see [testing](testing.md#a-checkout-in-a-test)).
- A Function that is not a discount (cart transform, delivery or payment customization,
  validation) has its metafields under `functions`, by its handle, and `"enabled"` when the shop
  has an owner of it: see [what runs](#owners-and-scenarios).
- A metafield is written `"namespace.key"`, with a [typed value](data-format.md#metafields-and-metaobjects):
  the namespace and the key the input query reads, joined by a dot. Shopify's templates read
  `metafield(namespace: "$app", key: "function-configuration")`, and name it in
  `[extensions.input.variables]` (`namespace = "$app"`, `key = "function-configuration"`):
  that is `"$app.function-configuration": {"type": "json", "value": {…}}`. A namespace
  `$app:<name>` keeps its colon: `"$app:config.settings"` is the key `settings` of
  `$app:config`. Shopify's list of `{namespace, key, type, value}`, as `metafieldsSet` takes
  it, is read too:
  `"metafields": [{"namespace": "$app", "key": "function-configuration", "type": "json", "value": "{\"percentage\": 10}"}]`.
- The **settings of a UI extension** are what the merchant fills in in the checkout editor:
  `checkout.extensions.<handle>.settings`, by the `key` of each field of its
  `[extensions.settings]`, read as `shopify.settings` (`useSettings`). A scenario's replace the
  store data's for the extensions it names. A field without a value is left out, as on Shopify.
  `validate` and the home page warn about a key the extension does not declare, a value not of
  its field's type (`number_integer`: a whole number, `boolean`, text and dates: a string) or
  not among its `choices`:
  `{"checkout": {"extensions": {"gift-banner": {"settings": {"title": "Free gift wrap", "max_items": 3}}}}}`.
- A **payment method** is in the list of the page by default (`PAYMENT_METHOD`). Its
  `placements` can put it among the express buttons above the contact section
  (`["ACCELERATED_CHECKOUT"]`), or in both, as PayPal with PayPal Express on
  (`["PAYMENT_METHOD", "ACCELERATED_CHECKOUT"]`).
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

## Orders submitted for review

A payment customization that answers `orderReviewAdd` on a B2B checkout (the buyer is a customer
of a company in the store data, the `purchasingCompany` of the Function input) submits the
checkout as a draft for the merchant's review, as on Shopify: "the checkout will be submitted as
a draft, requiring merchant review and approval to finalize the order" (Payment Customization
API).

- The state lists each requirement in `reviews` (`function`, `reason`), and the Overview of the
  Functions dialog shows them.
- `complete` places no order. The Admin API gets a draft order (`draftOrderCreate`, so the
  backend receives `draft_orders/create`, not `orders/create`) with the cart's lines, the buyer
  and their company location, the email, phone, shipping address, note, attributes and delivery
  option. The answer has `draftOrder` (`id`, `name`) instead of `order`, and so does the state
  of the thank-you page. Without the Admin API, the draft is only in the session.
- On a D2C checkout (a guest, or a customer with no company), the operation adds no review
  requirement and the order is placed: "review requirements aren't applicable to D2C checkouts".

Shopify does not document what the buyer sees, so the thank-you page's "Your order was
submitted for review" is lsf's wording. The draft takes the shop's prices of the variants: the
discounts the Functions applied are not on it. Nor is the Plus plan modelled: Shopify makes the
operation "only available for B2B purchases on stores with the Plus plan", and lsf applies it on
every store.

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

Not every Function of the app runs: see [owners and scenarios](#owners-and-scenarios).

The cart transforms run on the theme's cart too, as Shopify runs them on the online store's
cart: Liquid `cart`, `/cart.js` and the cart page show a line an expand or an update changed,
with its new title and price, and its components in `item.item_components`
([compatibility](compatibility.md#known-differences) says what they leave out).

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

Several delivery customizations, and several payment customizations, run one after the other in the
order of their folders in `extensions/` (by name), and each sees what the ones before it did: a
method that one hides is not in the input of the next, and a method one renames comes to the next
with its new name; when two rename the same method, the last one wins. Shopify's documentation does
not say how several of them combine (the Delivery Customization and Payment Customization Function
API pages only say that a store can activate 25 of each, and the cart transform page is the only
one that says how the operations of several Functions are merged), so lsf keeps this chain. A store
with one customization of each kind, the common case, sees no difference.

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

### Owners and scenarios

On Shopify, a Function runs only through an object of the shop that runs it, its **owner**, which
the app creates once installed: a payment customization (`paymentCustomizationCreate`), a
delivery customization, a cart and checkout validation (`validationCreate`), a cart transform
(`cartTransformCreate`); a discount Function, through a discount. A shop the app was just
installed on has none, and its checkout runs none of the app's Functions until the app sets
them up. lsf follows that rule at the shop's checkout (`/checkout` from the storefront's cart,
and a scenario that names no `extensions`):

| Function | Runs when |
|---|---|
| payment customization | the shop has an enabled `PaymentCustomization` of it |
| delivery customization | the shop has an enabled `DeliveryCustomization` of it |
| cart and checkout validation | the shop has an enabled `Validation` of it (`validationCreate` takes `enable`, `false` by default) |
| cart transform | the shop has a `CartTransform` of it (it has no `enabled`: deleting it stops it) |
| discount | a discount names it that is active by the shop's clock (`startsAt` ≤ now < `endsAt`), automatic or with a code the buyer entered |
| fulfillment constraints, order routing, pickup generators | never: the local checkout lists them and does not run them |

The owners come from two places:

- **The app**, through the [Admin API](admin.md): what it creates, updates (`enabled`), deletes
  or turns on and off (`paymentCustomizationActivation`) is what the next checkout runs, with the
  metafields it gives them.
- **The store data**, for a shop already set up: `"enabled": true` on the Function's entry of
  `checkout.functions` (`false`: an owner that is turned off). An entry without `enabled` is
  settings alone: the shop has no owner of the Function.

```json
{ "checkout": { "functions": { "payment-rules": { "enabled": true, "metafields": { "$app.function-configuration": { "type": "json", "value": { "hide": ["paypal"] } } } } } } }
```

lsf creates no owner at install: `paymentCustomizations` answers none, as Shopify does, and an
app that checks whether it is set up sees that it is not.

**Uninstalled, the app runs nothing.** Shopify removes an uninstalled app's Functions, and a
Function goes with its owners: `POST /__lsf/admin/uninstall` (or `appUninstall`) deletes every
owner of the app's Functions, those of the store data and the app discounts included. While the
app is not installed, the shop's checkout runs none of its Functions, whatever a scenario
declares or names in its `extensions`, and lists each as not run because "the app is not
installed on the shop". Installing it again brings no owner back: the app creates them, as after
a first install ([uninstall](admin.md#tokens)). Nor does the checkout load any of the app's UI
extensions while it is not installed, the thank-you page of an order placed before included: a
banner, the Functions dialog ("Extensions not loaded") and `?targets=all` say why, and the state
has `"extensions": []` with `"uninstalled": {"reason", "extensions"}`. They render again once
the app is installed again.

**Trying a Function alone** does not need an owner. A [scenario](#scenarios) that names it in
`extensions` runs it, and so does `lsf app function run <handle>`, with the metafields of
`checkout.functions` (the scenario's, or the store data's). That is how the scenarios of
`app init` try each Function; the shop's checkout shows what the app has set up. `--extension`
of `app dev` only narrows the extensions: the shop's owners still decide which Functions run.

A Function the shop's checkout leaves out is said: the home page warns that the shop has no
enabled owner of it when no scenario runs it either (the scenarios of `app init` name each
Function, so it says nothing after them), the "Functions" dialog of the checkout lists it under "Functions not run",
with why, and so does `lsf app function run` (`Not run: …`; `idle` in JSON).

A scenario added or edited while lsf runs is read again without seeding the shops of the Admin
API: the owners the app created stay ([when the data changes on disk](cli.md#when-the-data-changes-on-disk)).

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
  and are not dropped silently: the Functions dialog lists them under "Discounts not applied"
  (`skippedDiscounts` in the state, and a `discount not applied` line in `lsf app function run`)
  with their amount and why. 50% off a 4.90 Standard rate and 3.00 off it: the 3.00 applies,
  the 50% (2.45) is listed. Free Express does not beat 1.00 off Standard while the buyer has
  Standard selected, and wins when they select Express. The candidates of one discount are
  one discount: they all apply, one after the other. A tie (the same amount) goes to the
  discount that comes first by Function, title, message and code, not by its place in the
  configuration.
- **A discount of 0.** A candidate whose value is 0 (a `percentage` of 0, a `fixedAmount` of
  0.00) takes nothing off and still applies: its code is accepted (`applicable` in
  `discountCodes`), and it shows at 0 on the lines or the delivery group it targets, in
  `discounts` and `discountAllocations`, in the page and in `lsf app function run`. A Function
  answers one to accept a code before the line it stands for (a gift) reaches the cart. A
  shipping discount of 0 applies to the rate the buyer selected (none is before an address with a
  country) when no other one takes something off that rate, and is listed as not applied
  otherwise: it beats one that only cuts the rates the buyer did not select, which would apply
  nothing. An automatic discount (one without a code) of 0 applies the same way and shows at 0:
  the playground's `discount-vip-customer`, whose percentage comes from settings, shows "VIP: 0%
  off −€0.00" while its settings are not set. A candidate of more than 0 that finds nothing left
  to take off (a line another discount made free) is not applied.

**Codes of Shopify's own discount types.** With the [Admin API](admin.md), a code of a
`DiscountCodeBasic` or a `DiscountCodeFreeShipping` the backend created applies without a
Function ([what the checkout sees](admin.md#what-the-storefront-and-the-checkout-see)): an
amount off the order is an order discount, one off listed products, variants or collections (or
off each item) a product discount, free shipping a shipping discount, and each takes its place
among the discounts of the Functions by the rules above. The code is checked when the
checkout is computed: the discount's dates and usage limit, its buyers, one use per customer,
its minimum, and for free shipping the country of the address (a code is taken before there is
one). A minimum subtotal counts the prices the discounts before it left, as Shopify's do
([amount off](https://help.shopify.com/en/manual/discounts/discount-types/percentage-fixed-amount),
[free shipping](https://help.shopify.com/en/manual/discounts/discount-types/free-shipping),
"Understanding minimum purchase requirements"): for an amount off, the items it applies to
after the product discounts of the Functions; for free shipping, the whole cart after the
product and order discounts. A refused code shows "Enter a valid discount code", and its entry
of `discountCodes` in the state has a `reason` (`"its minimum purchase is 50.00 USD, and the
items it applies to come to 32.00 USD after the product discounts"`). One code of these types
applies per checkout, the first one entered that qualifies: a second is refused, as on Shopify
when a discount combines with no other (lsf does not read `combinesWith`). Its
`DiscountApplication` is `ACROSS` (`EACH` off each item), `ALL` for the order and `ENTITLED`
for listed items; free shipping is 100% off the shipping line, `EACH`.

A free shipping code that takes nothing off (the selected rate costs more than its
`maximumShippingPrice`, nothing in the cart ships, or a shipping discount of a Function beat it)
stays applicable on the page, as entered, but it is not the order's: the order does not carry it
and it is not counted as a use. The codes are checked again as the order is recorded: when
another order took the last use of the discount (`usageLimit`) or the buyer's one
(`appliesOncePerCustomer`) after the checkout was priced, the order is not placed. The
completion answers 422 with the checkout as it now is (the code refused, with its `reason`, the
price without it) and the error "`<code>` discount code isn't available to you right now" on
`$.cart.discountCodes`; the buyer pays again for what they now see.

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
this checkout's choices. Shopify's checkout applies a code whose product discount is 0 and shows
it at 0; that an order or a shipping discount of 0 does the same, and that an automatic discount
of 0 shows, are this checkout's choices: Shopify does not say what its checkout shows for them.

### Network access

A Function with network access has a `.fetch` target next to its `.run` target:
`cart.lines.discounts.generate.fetch`, `cart.delivery-options.discounts.generate.fetch` and
`cart.validations.generate.fetch` (the Discount and Cart and Checkout Validation APIs, in
2026-10 and 2027-01). The checkout runs the `.fetch` target right before its `.run` target, sends
the HTTP request it answers from your machine, and gives the response to the `.run` target as
`fetchResult` (`status`, `body`, `jsonBody`, `header(name:)`). The request and the response
show in the Functions dialog and in `lsf app function run`.

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
the order. Shopify says that "validation errors that your function returns always block checkout"
([Cart and Checkout Validation API](https://shopify.dev/docs/api/functions/latest/cart-and-checkout-validation));
it does not say what happens when the completion run returns none after an interaction run
returned some. So a validation that errs at `CHECKOUT_INTERACTION` alone shows its errors here
and lets the order through: return the error at both steps, or test that case on a store.

The input of each Function is the result of its input query, run against the cart, the buyer and
the store data. A metafield whose value is over 10,000 bytes is `null` there, as Shopify returns
none ("Metafields with values exceeding 10,000 bytes in size will not be returned",
[input query limits](https://shopify.dev/docs/api/functions/latest#input-query-limits)): keep a
Function's configuration under that size.

A run that fails (a trap, more than 11 million instructions, an output out of its schema, a
request out of Shopify's limits) is ignored, as on Shopify: its output is not applied and the
checkout goes on without it. A failed discount applies no discount, a failed delivery or payment
customization leaves the options as they were, and the buyer can pay. Only a validation or a cart
transform whose owner has `blockOnFailure: true` blocks the order when it fails, with the error
`` Function `<handle>` failed: … ``: `validationCreate`, `validationUpdate` and
`cartTransformCreate` set it through the [Admin API](admin.md), and `block_on_failure` of its
entry of `checkout.functions` in the store data ([data format](data-format.md#checkout)). The
failure is still the run's error: the Functions badge turns red, the dialog shows it, and
`lsf app check` and `lsf app function run` fail on it ([the Functions that
fail](#the-functions-that-fail)).

The "Functions" badge in the bottom left corner of every page (the checkout, the thank-you page,
`?targets=all`) says how many Functions ran, and turns red when one failed or the checkout has
errors. Below 1644px wide it is just "ƒ" and the number of runs, so that it stays clear of the
checkout; its tooltip says the rest. It opens the Functions dialog, which takes most of the window (all of it on a narrow
screen): the runs on the left, and for the one picked its error, its logs, its instructions
(Shopify stops a Function past 11 million) and its input and output side by side, each with a
Copy button; a `.fetch` run adds its HTTP request and response. It opens on the first run that
failed. Its Overview lists the errors that block the checkout, the discounts a Function answered
that were not applied, the checkout UI extensions, and a link to the other skin. Escape, the
close button or a click outside closes it.

`?functions=` on the URL of a checkout opens the dialog as the page loads, which is what the
links of the [home page](#the-home-page) problems do. `/checkout?scenario=<name>` and
`/checkout` keep it through their redirect, as they keep `variant` and `targets`.

| Value | The dialog opens on |
|---|---|
| `<handle>` | the run of that Function that failed first, else its first run |
| `<handle>:<target>` | the same, among the runs of that target |
| empty (`?functions=`) | the Overview |

A value with no run in the checkout (a Function that does not run there, an unknown handle, a
checkout UI extension) opens the Overview: what it names is outlined there (an error, an
extension), or a line says it did not run or load in this checkout. The page then drops the
parameter from its URL (`history.replaceState`), so a reload does not open the dialog again;
once closed, nothing reopens it. The run stays picked across updates, as a run picked by hand
does.

The checkout UI extensions render where their targets say: in the page, one sandboxed frame
each. They read the checkout through the `shopify` global and change it with the `apply*Change`
methods, `buyerJourney.intercept` can block the payment, and they draw with the Polaris web
components (`s-text`, `s-button`, ...), as from API version 2025-10. A block shows the buyer its
`errors` (at the field their `target` names, else above the button) and never its `reason`,
which Shopify keeps for its own debugging: a block with no `errors` stops the payment without a
word, so the extension says why in its own UI. The Overview of the Functions dialog lists each
block with its `reason`. React and remote-ui
extensions (2025-07 and older) do not run: lsf runs 2026-10 and 2027-01 only.

What an extension throws, while its bundle loads, when a target renders or in a listener, and
the promises it rejects with nobody to catch them, show above its frame with their message and
stack, and in the overview of the Functions dialog. The frame keeps an opaque origin. The page
fetches the bundle (same origin) and hands its text to the frame, which runs it from a `blob:`
URL of its own: the browser does not hide those errors ("Script error."), and lsf serves the
bundles without CORS, so another site cannot read them.

An extension needs a supported `api_version`, or it is reported as an error and not shown. From
2027-01, each `[[extensions.metafields]]` entry needs an `owner_type` (`CART`, `CUSTOMER`,
`ORDER`, ...), or the loader reports an error and the page says why it does not load the
extension.

`shopify.appMetafields` has the metafields the extension declares: those of
`[[extensions.metafields]]`, and those of `[[extensions.targeting.metafields]]` at the target
they are declared for. Each entry is a metafield with the resource it belongs to, its `target`:
a `type` (`shop`, `product`, `variant`, `customer`, `company`, `companyLocation`, `cart`, and
`order` on the order status page) and the `id` of the resource, a GID. They are read from the store data, with what the backend of the
app sets through the [Admin API](admin.md) (`metafieldsSet`): the shop's, those of the products
and variants of the lines (and of the components of a bundle a cart transform made), of the
customer logged in and of the company and location they buy for, and the cart's, which the
extensions write with `applyMetafieldChange`. A declaration with an `owner_type` reads that
resource only, one without reads every resource that has the metafield. The app's own namespace
is written `$app` (or `$app:<name>`) in the toml, in the store data and in what the extension
reads; Shopify's `app--<id>--<name>`, which the Admin API writes, is the same namespace. The toml
takes `$app` only, as on Shopify: a declaration written `app--<id>--<name>` reads nothing, and the
loader warns with the spelling to use.

```toml
[[extensions.metafields]]
namespace = "$app"
key = "gift-codes"
owner_type = "SHOP"
```

```json
{ "shop": { "metafields": { "$app": { "gift-codes": { "type": "json", "value": ["GIFT-1"] } } } } }
```

```js
// [{target: {type: 'shop', id: 'gid://shopify/Shop/…'},
//   metafield: {namespace: '$app', key: 'gift-codes', value: '["GIFT-1"]', type: 'json', valueType: 'json_string'}}]
shopify.appMetafields.value;
```

On the order status page of the customer account, the products and variants are those of the
order, the company location is the one the order was placed for (whichever the buyer buys for
now; an order of the store data records none, and reads the customer's first location), and the
cart is the one of the order placed in the local checkout. As in Shopify's types, a `value` there
is typed by its `valueType` (`true`, `2`, `1.5`; a JSON value stays serialized), where the
checkout's is the text whatever the type (`"true"`).

The order's own metafields are `order` entries (`target: {type: 'order', id: 'gid://shopify/Order/…'}`),
for a declaration of `owner_type = "ORDER"` or of no owner. They are read from the order in the
[Admin API](admin.md#what-the-storefront-and-the-checkout-see): what the backend sets on the
order (`metafieldsSet`), on an order placed in the local checkout or on one of the store data,
and the cart metafields copied onto an order placed here. As on
Shopify, a cart metafield is copied when the order is placed if an ORDER definition has its
namespace and key with `cartToOrderCopyable` enabled, made by the backend or declared in
`shopify.app.toml`; without one, it stays the cart's only:

```toml
# shopify.app.toml
[order.metafields.app.gift_message]
type = "single_line_text_field"
capabilities.cart_to_order_copyable = true

# shopify.extension.toml
[[extensions.metafields]]
namespace = "$app"
key = "gift_message"
owner_type = "ORDER"
```

The checkout and the thank-you page have no `order` entry (Shopify's `AppMetafieldEntryTarget`
of checkout UI extensions has no such type): an `ORDER` declaration reads nothing there, without
a warning. Shopify lists `CART` as an owner type of checkout UI extensions only and `ORDER` of
customer account UI extensions only; lsf takes both on either, as one extension can target both.

### Session tokens

`shopify.sessionToken.get()`, in a checkout or a customer account extension, gives the token
Shopify gives, which the backend verifies as it verifies Shopify's (`authenticate.public.checkout`
and `authenticate.public.customerAccount` of Shopify's libraries, `decodeSessionToken`): an
HS256 JWT signed with the client secret of the [Admin API](admin.md#start)
(`--client-secret`, `SHOPIFY_API_SECRET` of the environment or of the app's `.env`, else
`lsf-local-client-secret`; the same without the admin part, without `--client-secret`). Its
claims are those of shopify.dev's Session Token API, for
[checkout](https://shopify.dev/docs/api/checkout-ui-extensions/latest/target-apis/platform-apis/session-token-api)
and [customer account](https://shopify.dev/docs/api/customer-account-ui-extensions/latest/target-apis/platform-apis/session-token-api)
extensions:

```json
{
  "dest": "local-supply-co.myshopify.com",
  "aud": "<client id>",
  "exp": 1794511949,
  "nbf": 1794511649,
  "iat": 1794511649,
  "jti": "7f63880b-b45d-49d4-beb1-af926c85653c",
  "sub": "gid://shopify/Customer/6523834743460"
}
```

`dest` is the shop's domain, bare, as in shopify.dev's example (the session token of the admin
has `https://`): the `permanent_domain` of the store data, the default shop of the Admin API.
`aud` is the client id, the token lives five minutes, `jti` is a new UUID each time, and `sub`
is there only when a customer is logged in (whether the app may read customer data is not
checked). There is no `iss` and no `sid`. The page asks lsf for the token
(`POST /__lsf/extensions/<handle>/session-token`, [control API](control-api.md#apps-checkout-and-admin-api))
and hands it to the extension: the secret never reaches the browser.

## Run a Function without a server

```bash
npx local-storefront app function run volume-discount --app ../my-app --scenario discount-automatic
```

Builds the input of each Function from the store data and the [scenario](#scenarios), runs them
in checkout order, and prints each input, output and log with the checkout that results. The
Functions it names run whether or not the shop has an owner of them, and a named one that the
checkout of the store data would leave out (its payment customization is disabled, or there is
none) is said first (`Named: …`; `forced` in JSON), so that its run is not taken for the
checkout's. It reads the store data on disk, not a running lsf: a customization the app created
or enabled there through the Admin API is not seen. To see what a running lsf's checkout does, open
its checkout (`/__lsf/checkouts`, a scenario), whose debug panel lists each run.
Without a handle, it runs those the checkout runs (an enabled owner, a discount, or the
`extensions` of the scenario), and the others are listed as not run, with why. Without `--scenario` or `--session`, the cart is the
`session` of the store data, empty in the demo store: a Function that runs gets an empty cart
and changes nothing. Give `--scenario`.

```text
== scenario discount-automatic: An automatic discount

== volume-discount · cart.lines.discounts.generate.run (cart-lines-discounts-generate-run)
input:
  { … }
output:
  { … }
logs:
  Canvas Tote Bag: 6 units
  Ceramic Mug: 3 units
instructions: 337817

== checkout
  6 × Canvas Tote Bag  144.00 USD → 129.60 USD
  3 × Ceramic Mug (Oat)  66.00 USD → 59.40 USD
  2 × Linen Overshirt (S)  136.00 USD
  discount 10% off 3 or more: -21.00 USD (Product, volume-discount)
  delivery   Standard (standard): 5.00 USD
  payment: Credit card [PAYMENT_METHOD], Cash on Delivery [PAYMENT_METHOD], Bank Deposit [PAYMENT_METHOD]
  subtotal 346.00 USD · discounts -21.00 USD · shipping 0.00 USD · tax 0.00 USD · total 325.00 USD
```

`*` marks the selected shipping rate: none until the address has a country (the scenario's
`entered.shipping_address`, or `--checkout`), so the shipping is not in the total yet. Each
payment method has its placements: `[ACCELERATED_CHECKOUT]` is an express button.

| Option | What it does |
|---|---|
| `--scenario <name>` | A [scenario](#scenarios) of the store data: its cart, buyer, language, `checkout` settings and the extensions it names. `--session`, `--checkout` and `--locale` go on top of it. The cart is priced as the served checkout of the scenario prices it: in the currency of the buyer's country, at its market's `rate` (`presentmentCurrencyRate`). |
| `--session <file or JSON>` | The session to run for: a [session](data-reference.md#session) with a `cart`, a `customer`, a `country`. Defaults to the `session` of the store data. |
| `--checkout <file or JSON>` | What the buyer entered: `email`, `phone`, `shippingAddress`, `selectedDelivery`, `selectedPayment`, `discountCodes`, `metafields`. |
| `--locale <iso>` | The language of the buyer, one of the store's (`fr`, `pt-BR`): Functions read it as `localization.language`. Defaults to the primary language. |
| `--step interaction\|completion` | The step of the buyer journey, which validation Functions read as `buyerJourney.step`: `completion` (default) is the placing of the order, `interaction` is the page and its updates. |
| `--format json` | The checkout as the page reads it, with every run in `runs`. For tools. |
| `--build` | Builds the extensions first, as `lsf app build` does: each Function (its build command, or lsf's build of a JavaScript Function), then each UI extension that names its sources. |
| `--input <file>` | Runs one Function on this input instead, as `shopify app function run --input` does: a JSON file, or `-` for stdin (see below). |
| `--export <name>` | The target to run, by its Wasm export or its name, when the Function has several: for `--input` and `--save-input`. |
| `--save-input <file>` | Writes the input lsf built for the run of the Function named to this file. |

It exits with `1` when a Function fails to run (no module, a trap, an output that does not
parse), or when a Function named on the command line did not run: it says why (no discount
names a discount Function, the cart is empty, the scenario's `extensions` leaves it out…).
Errors that a validation Function reports are part of the checkout, not a failure.

### Run a Function on an input

`--save-input` keeps the input lsf builds for a scenario, and `--input` runs a Function on an
input, without the store data, as `shopify app function run --input` does:

```bash
npx local-storefront app function run volume-discount --scenario discount-code \
  --export cart.lines.discounts.generate.run --save-input tests/fixtures/code.json
npx local-storefront app function run volume-discount --input tests/fixtures/code.json
cat input.json | npx local-storefront app function run volume-discount --input -
```

It prints the run (input, output, logs, instructions), and `--format json` the run as JSON. The
output is checked as in a checkout: against the schema of the API version, and the instruction
limit. A fixture of the unit tests of Shopify's templates (`tests/fixtures/*.json`,
`{"payload": {"export", "target", "input", "output"}}`) runs the target it names, and fails
when its `output` is not the run's, with each path that differs (it says `the output is the one
the fixture expects` when it is). Its `target` and `export` must be the Function's: a wrong one
is an error, not left aside. A Function with
several targets needs `--export` (its export, `cart-lines-discounts-generate-run`, or its name).
In the Function's folder, with a fixture in it, or in an app with one Function, the handle can
be left out. `--input` with a folder runs each of its `*.json` fixtures and exits with `1` when
one fails, as the unit tests of the templates do:

```bash
npx local-storefront app function run volume-discount --input extensions/volume-discount/tests/fixtures
```

`--save-input` creates the folders of its file. When a file of the Function changed after its
module was built, the run builds it first, as `shopify app function run` does.

## Test it

The checkout is part of the [control API](control-api.md). A test puts a session in a known
state, then drives the checkout like the page does, and places the order. The checkout belongs to
the session: Playwright's `request` keeps its cookie; a client without cookies names its session
with an `x-lsf-session` header.

```ts
await request.put('/__lsf/session', {
  data: { cart: { items: [{ variant: 'TEE-BLK-M', quantity: 3 }] } },
});
const checkout = await request.get('/checkout', { maxRedirects: 0 });
const path = checkout.headers()['location']; // /checkouts/cn/<token>

const state = await (await request.get(`${path}.json`)).json();
await request.post(`${path}/update`, {
  data: {
    operations: [
      { type: 'email', value: 'buyer@example.com' },
      { type: 'shippingAddress', address: { lastName: 'Buyer', address1: '1 Main St', city: 'New York', countryCode: 'US', provinceCode: 'NY', zip: '10001' } },
      { type: 'paymentMethod', handle: 'card' },
    ],
  },
});
const { order } = await (await request.post(`${path}/complete`)).json(); // { id, number, name }
```

The order is an order of the [Admin API](admin.md#place-an-order), and its `orders/create`
reaches the backend of the app. A test that only needs an order (for its webhooks) serves the
Functions it needs and no others, `--extension <handle>` repeated, or none, `--no-functions`: a
Function that fails blocks `complete`.

| Request | What it does | Answer |
|---|---|---|
| `GET /checkout` | Leads to the checkout of the session's cart. `?variant=shop` for the other look, `?targets=all` for [every extension target](#every-extension-target). | A redirect to `/checkouts/cn/<token>` (`/fr/checkouts/cn/<token>` from `/fr/checkout`) |
| `GET /checkout?scenario=<name>` | Opens a checkout for a [scenario](#scenarios), in a new session. | A redirect to its checkout with a new session cookie, or `404`/`422` and a page that says why |
| `GET /checkout?customer=<email>` | Logs the session in as a [customer of the data](#log-in-as-a-customer) (`default`, `none` too), then leads to its checkout. With `scenario=`, opens the scenario as them. | A redirect, or `404` and a page that lists the customers |
| `GET /` (`lsf app dev`) or `GET /__lsf/checkouts` | The [home page](#the-home-page). `?format=json` for what it says, `?check=1` to [run the Functions of the scenarios](#the-functions-that-fail) first. | HTML, or JSON |
| `GET /__lsf/checkouts/check` | Runs the Functions of every scenario (or takes the last run on the same app and data): [the ones that fail](#the-functions-that-fail). | JSON: `failures` (`handle`, `target`, `error`, `logs`, `scenarios`), and what the page shows of it |
| `GET /checkouts/cn/<token>` | The page. | HTML, with the state in it |
| `GET /checkouts/cn/<token>.json` | The state of the checkout. | JSON |
| `POST /checkouts/cn/<token>/update` | Applies `{"operations": [...]}`, in order. | `200` and the state, or `400` `{"error": "…"}` and nothing applied, or `409` and the state as it is now when the order was placed meanwhile or the session changed under it |
| `POST /checkouts/cn/<token>/complete` | Places the order. | `200` `{"redirect": "…/thank-you", "order": {"id", "number", "name"}}`, the order as the Admin API recorded it: `number` is an integer (`1004`), `name` the string (`"#1004"`), `id` the `admin_graphql_api_id` of its `orders/create` (without the Admin API, `--only checkout`: `gid://shopify/Order/<number>`, the session's number). The state's `order.number` is a string, the session's, which can differ (below). Or `422` and the state with `errors`, or `409` and the fresh state when the cart changed meanwhile. Asked again once the order is placed: `200` and the same `redirect` and `order` |
| `GET /checkouts/cn/<token>/thank-you` | The page of the order. | HTML |
| `GET /__lsf/checkout?session=<id>` | The state of a session's checkout, from outside its browser. | JSON |

The token is the one of the session's cart. Another session gets a `404`.

### Language

The buyer reads the checkout in the language its URL names, as the storefront is: every path above also
exists under a locale prefix (`/fr/checkout`, `/fr/checkouts/cn/<token>`,
`/fr/checkouts/cn/<token>/update`, …). The cart form of a theme leads to `/fr/checkout` from the
French storefront, the redirect keeps the prefix, and so do the page's own requests, the links
back to the storefront and the way to the thank-you page. Without a prefix the checkout is in
the primary language of the store. `/__lsf/checkout` has no prefix to read: it takes
`?locale=fr`.

The state tells both languages apart: `shop.locale` is the primary language of the store, and
`localization.language` the one the buyer reads. Extensions get the latter as
`shopify.localization.language`, translate and format numbers and money with it. The Functions
read it too, as `localization.language` of their input. The words of lsf's own pages (the
checkout, the thank-you page, the customer account: "Delivery", "Pay now", "Orders") are not
translated: they are English, and the document says so (`lang="en"`), whatever the language of
the URL. A thank-you page is read in the language of its URL, not the one the order was
placed in. A language that a session adds with `data.localization.languages` is a locale of that
session's checkout.

### Buyer and localization

`buyer` is what `shopify.buyerIdentity` reads: the `email` and `phone` the buyer typed or the
logged-in customer has, `customer` (`id`, `email`, `phone`, `firstName`, `lastName`,
`acceptsMarketing`, `ordersCount`) and, for a B2B customer, `purchasingCompany` (the `company`
and the `location` they buy for, each with `id`, `name` and `externalId`). `localization` is what
`shopify.localization` reads: `language` and `rootUrl` as above, the `country` (the one of the
shipping address, otherwise the one the visitor selected on the storefront), the `currency` and
the `market` of that country. The store has no avatar, no consent to SMS marketing and no store
credit for a customer, nor a time zone for a buyer (`timezone` is the one of the browser).

A buyer with no address and no selected country is in the shop's country
(`shop.address.country_code`), the one of its primary market, as on Shopify. The Functions read
it as `localization.country`, the extensions as `shopify.localization.country`, and the
address form starts on it. Both read the same `market`. A shop in a country the store does not
sell to uses the first country of the store. The storefront shows the same country to a visitor
who selected none (Liquid's `localization.country`, `Shopify.country`), so the checkout opens in
the country they browsed in.

The state has the lines, discounts, discount codes, delivery groups, payment methods (each with
its `placements`), express buttons (`expressMethods`), errors and cost that the Functions
produced, `runs` (a run of a `.fetch` target has its `exchange`, the request and the response),
the buyer, the address, the note, the attributes and
the metafields (the cart's), `appMetafields` (the metafields the extensions declare of the shop,
the products and variants of the lines and the buyer), the `extensions` to load and
`diagnostics`. Money is `{"amount": 19.99}`.

No shipping rate is selected until the address has a country, as on Shopify. Until then, the
page shows "Enter shipping address", and no total has the shipping: `cost.shipping` is 0,
`cost.total` and `cost.dueNow` leave it out, and so does `shopify.cost.totalAmount`.
`shopify.cost.totalShippingAmount` is undefined. The Functions read `selectedDeliveryOption:
null` and a `cart.cost.totalAmount` without shipping. The rates are still offered, so a delivery
customization runs on them.

### Operations

| `type` | Fields | Does |
|---|---|---|
| `customer` | `customer`: an email, `default`, or `null` to log out; `location` (optional): a company location by id or name | Logs the checkout in as a [customer of the data](#log-in-as-a-customer). The session gets the customer; the contact and the address become theirs. |
| `companyLocation` | `location`: a `gid://shopify/CompanyLocation/…`, its number or its name | Chooses the location the logged-in B2B customer buys for; the order ships there. |
| `email`, `phone` | `value` | Sets the contact. An empty value clears it. |
| `shippingAddress` | `address`: `firstName`, `lastName`, `company`, `address1`, `address2`, `city`, `countryCode`, `provinceCode`, `zip`, `phone` | Sets the address. A country of the store becomes the country of the session. A `countryCode` that is not a country (ISO 3166-1 alpha-2, or `XK`, `AC`, `TA` as Shopify takes them; not `ZZ`) is refused and changes nothing. The store data has no shipping zones: any real country is taken. |
| `deliveryOption` | `group` (`group-1`), `handle` | Chooses a shipping rate. |
| `paymentMethod` | `handle` | Chooses a payment method. No handle clears it. |
| `discountCode` | `action` (`add` or `remove`), `code` | Enters or takes back a code. Each entered code is in the `discountCodes` of the state, `applicable` or not; a code of a discount of Shopify's own types that was refused has a `reason` ([how discounts stack](#how-discounts-stack)). |
| `cartLines` | `change`: `addCartLine`, `removeCartLine` or `updateCartLine` of the UI extension API | Changes the cart of the session, the one the theme shows. |
| `attribute` | `change`: `updateAttribute` or `removeAttribute`, with `key` and `value` | Changes the attributes of the cart. |
| `note` | `change`: `updateNote` (`note`) or `removeNote` | Changes the note of the cart. |
| `metafield` | `change`: `updateMetafield`, `updateCartMetafield`, `removeMetafield` or `removeCartMetafield`, with `namespace`, `key`, `valueType`, `value` | Sets a metafield for the Functions to read. |

`complete` refuses an empty cart, an order with errors from a Function, and one without an
email, a payment method or, for a cart that ships, an address with a last name, a first line and
a city. It empties the cart: the order is what the thank-you page shows, and the state's `order`
(`id`, `number`, `processedAt`, `totalReceived`, `totalOutstanding`) once `status` is `completed`.
That `number` is a string, the session's (`"#1004"` every run). The `order` of `complete` is the
Admin API's, and there `number` is an integer (`1004`) and `name` the string (`"#1004"`); they
are `1005` and `"#1005"` when the shop has a `#1004` already. To match the two, use `id`, which
both have; never compare the two `number`s. The state's `number` is a string since 0.2.0, and
stays one. `complete` takes the cart only if the cart is still the one the Functions ran on: if
it changed meanwhile (another tab), nothing is bought and the reply is `409`. Completing an order that was placed already leads to the same thank-you page, never to a
second order: `200`, with the same `redirect` and `order`. An `update` that comes once the order
is placed (a line changed in another tab, a request sent late) changes nothing either: the reply
is `409` and the state as it is now, whose `status` is `completed`.

### In CI

Once, in the app's folder: `npm install --save-dev local-storefront`, then commit
`package.json` and `package-lock.json`. In the job, `actions/setup-node` and `npm ci` come
before these steps, as in [testing](testing.md#in-ci). `dist/` is not committed: build the
extensions before checking them.

```yaml
- run: npx local-storefront app build      # every Function and UI extension, without Node
- run: npx local-storefront validate       # the store data, the scenarios and the app
- run: npx local-storefront app check      # every Function of every scenario runs, and each `expect` holds (`--build` builds first)
```

`app check` fails on a scenario that does not validate, on a Function that traps or panics,
runs over one of Shopify's [resource limits](#resource-limits) or answers an output out of its
schema, and on an
[expectation](#expected-results) of a scenario that is not met: a discount of the wrong amount,
PayPal left in the express buttons. It also fails on an extension the checkout does not offer
(an `api_version` other than `2026-10` and `2027-01`, no module or bundle), on a Function whose
files changed after its module was built (`stale`, unless `--build`: its folder, and the app's
files every build reads, such as a workspace's `Cargo.toml` or a shared package; a lock file
the build writes itself does not count), and on what Shopify rejects in a theme app extension.
`--extension` naming theme app extensions only checks those. A trap ends with the last message the Function logged
(`the Function trapped: Error: boom (in b2b)` for a `throw new Error("boom")`), and the JSON has
the `logs` of each failure. For what `expect` cannot say, `app function run --format json` has the
whole checkout (`lines`, `discounts`, `deliveryGroups`, `paymentMethods`, `expressMethods`,
`errors`, `cost`, and each run in `runs`) for `jq`, and a test can drive the checkout, as above.
The browser tests of a theme in CI: [testing](testing.md#in-ci).

## What it does not do

- Payments, gift cards, and payment method attributes. Choosing a payment method pays nothing,
  and the balance of a [deferred payment](#deferred-payments) is never collected.
- One delivery group holds every line that ships. No pickup points (their list stays empty), no
  split shipments, no prices by address: the rates are the same for every address.
- Fulfillment constraints, order routing, and the pickup point and local pickup generators (with
  their `.fetch` targets) are listed and not run.
- Discounts have no `conditions` and no combinations: all of them stack, as described in
  [how discounts stack](#how-discounts-stack). Shopify lets the merchant choose which discounts
  combine, and applies the best of those that cannot; here all the product and order discounts
  combine, and of the shipping discounts the best one applies. Of Shopify's own discount types,
  only the codes of `DiscountCodeBasic` and `DiscountCodeFreeShipping` apply, one per checkout,
  and only with the Admin API (which makes them): not buy X get Y, not the automatic ones, not a
  `context` of markets nor `appliesOnSubscription`.
- Tax is one rate on the discounted subtotal, with no tax lines.
- No Storefront API: `query` in an extension rejects. No Customer Account API either: a customer
  account extension reads the account through its `shopify` global only.
- `shopify.appMetafields` has no `shopUser` entries (the store data has no staff accounts).
- The customer account shows; it changes nothing (no address to edit, no order to cancel or
  return), and its intents (`shopify.intents.invoke`) are closed at once.
- Some fields of the Function input hold what the store data says and nothing more. The orders of
  the customers are not counted in the `ordersCount` of a company location, nor spent in its
  `totalSpent` (set them in the data); a `Market` is a country, or the group of countries that
  `localization.markets` declares, with no catalog or price list; the `id` of a `Metaobject`
  is made up from its type and handle.
- Shop Pay and express checkout: the express buttons show (the methods of the
  `ACCELERATED_CHECKOUT` placement that the payment customizations left), but a click only says
  that lsf does not simulate the wallet. Logging in is picking a customer of the data, see
  [log in as a customer](#log-in-as-a-customer).
- With `app dev` (no theme), the links to the storefront (return to cart, continue shopping)
  lead to pages the server does not have. Use `serve --app` for those.
- A scenario is one session per browser: opening another replaces it. The scenario cannot set
  what the theme shows outside the cart and the customer (`theme_settings`, a frozen clock).

## What to test on a real store

What lsf leaves out, or decides where Shopify does not document it. Test these on a development
store before a release:

- **Discount combinations and conditions**: lsf combines every product and order discount
  ([how discounts stack](#how-discounts-stack)).
- **Codes of Shopify's own discount types**: one applies per checkout whatever its
  `combinesWith`, a minimum subtotal counts the prices after every discount before it (Shopify
  counts only those that combine with it), and a refused code says lsf's reason, where
  Shopify's checkout has messages of its own.
- **Currencies and price lists**: a buyer pays in the currency of the country of the shipping
  address (else of the one selected on the storefront), at its market's `rate` or fixed `prices`
  ([localization](data-format.md#localization)); the Functions read that rate as
  `presentmentCurrencyRate`. The served checkout, `lsf app check` and
  `lsf app function run --scenario` price a scenario alike, so an `expect` on totals reads the
  amounts the page shows. Shopify's rounding rules of converted prices and B2B catalogs are
  not applied ([markets](compatibility.md#known-differences)).
- **B2B catalogs, pickup points, local pickup, fulfillment constraints and order routing**: not
  run.
- **Network access**: the request JWT and Shopify's cache of responses
  ([network access](#network-access)).
- **The settings of an extension** (`shopify.settings`): those of the store data or the
  scenario here, not the checkout editor's.
- **lsf's choices**: how several customizations chain and how `linesMerge` counts
  ([what runs](#what-runs)), what an error a validation returns at `CHECKOUT_INTERACTION`
  alone does to the order: Shopify says its errors "always block checkout", lsf refuses only on
  the errors of the completion run ([network access](#network-access)), the split between due today and due later
  ([deferred payments](#deferred-payments)).
- **Instruction counts near the 11 million limit**: lsf does not run `wasm-opt`, so a Rust
  Function costs 0.25 to 0.5% more here ([Rust Functions](#rust-functions)).
- **Payments**, Shop Pay and express checkout.
- **Orders submitted for review** (`orderReviewAdd`): what the buyer sees, the prices on the draft
  and the Plus plan ([orders submitted for review](#orders-submitted-for-review)).

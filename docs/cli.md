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
| `1` | Done, and something to fix: data or app errors (`validate`), problems in the theme (`check`), Liquid errors in the page or nothing at its path (`render --strict`), files not formatted (`fmt --check`), a Function that failed to run or did not run (`function run`), a scenario that fails (`checkout check`). |
| `2` | The command could not run: a wrong option, a theme or a file that cannot be read, a file `fmt` cannot write. The message is on stderr, after `error:`. |

## serve

`npx local-storefront serve`

Serves the theme over HTTP.

```bash
npx local-storefront serve --port 9292 --static --quiet --strict
```

| Option | Environment variable | What it does |
|---|---|---|
| `--port <port>`, `-p` | | Listens on this port and no other: the server does not start if it is taken. `0` picks a free one. Without the option: `9292`, or the next free port up to `9391`. |
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
| `--app <dir>` | `LSF_APP` | A Shopify app. `/checkout` then runs its Functions and shows its checkout UI extensions. See [checkout](checkout.md). |
| `--extension <handle>` | | With `--app`: uses only this extension of the app. Repeat it for several. Without it, every extension is used. |
| `--build` | | With `--app`: runs the build command of each Function first. |
| `--watch` | | With `--app`: builds a Function again when its files change, and reloads the open pages (it implies `--live-reload`). At start, builds only the Functions that have no module or changed since the last watch; with `--build`, all of them. See [watch mode](checkout.md#watch-mode). |
| `--variant <standard\|shop>` | | With `--app`: the look of the checkout page, `standard` by default. |

When it is ready, the server prints a line to stderr that starts with
`ready:  http://127.0.0.1:<port>/`. With `--port 0`, read the port from that line. It runs
until it is stopped. With `--app`, it also prints how many checkout scenarios the data has and,
with a theme, the address of the home page of the checkout
(`home:   http://127.0.0.1:<port>/__lsf/checkouts`).

While it runs, tests talk to it through the [control API](control-api.md).

## checkout

`npx local-storefront checkout --app <dir>`

Serves the checkout of a Shopify app alone, without a theme: `serve --app` without the
storefront. `/` is the [home page](checkout.md#the-home-page), which lists the extensions of the
app and the checkout scenarios and opens a checkout for each; `/checkout` starts a checkout. The
control API and `/cdn` are there, other pages answer `404`. It takes the options of
[`serve`](#serve). `--app` defaults to the app the current folder is in, and the store data to
the app's `shopify-local/`. Guide: [checkout](checkout.md).

```bash
npx local-storefront checkout --app ../my-app --extension volume-discount --variant shop
```

### checkout init

`npx local-storefront checkout init [--app <dir>]`

Writes checkout scenarios, the files of `checkouts/` in the store data, made for the Functions
and checkout UI extensions of the app (a generic set outside an app, or when nothing matches).
It gives each Function the settings its input query reads (`[extensions.input.variables]`) when
the store data has none: those of the app's own `shopify-local/`, or empty values of the type of
each variable, in `app-settings.json` (to fill in). It creates the data folder with the demo store when it does not exist, refreshes `schema/`, never
overwrites a scenario or a data file without `--force`, and prints what it wrote, what it kept
and what it skipped and why. It is a subcommand of `checkout` because the scenarios are the
checkout's, and the same data folder as `init`, `--data` and `LSF_DATA` included. Guide:
[checkout](checkout.md#scenarios).

```bash
npx local-storefront checkout init --app ../my-app
```

| Option | Environment variable | What it does |
|---|---|---|
| `--app <dir>` | `LSF_APP` | The app the scenarios are made for. Defaults to the app the current folder is in. |
| `--force` | | Overwrites the scenarios that exist. |

## checkout build

`npx local-storefront checkout build [handle]... --app <dir>`

Builds the extensions of an app, as `shopify app build` does, without Node: its Functions (as
`function build`) and its checkout UI extensions that name their sources (`module`), bundled into
`dist/<handle>.js`. Guide: [checkout](checkout.md#what-lsf-builds-without-node).

| Option | Environment variable | What it does |
|---|---|---|
| `[handle]...` | | The extensions to build. Every extension of the app when none is given. |
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in. |

## checkout check

`npx local-storefront checkout check [--app <dir>]`

Runs the Functions of every checkout scenario, as the home page checks them, without a server:
for CI. Prints each scenario that does not validate and each Function that fails, with the
scenarios it fails in, and exits with `1` when there is one, when the time ran out, or when
there is no scenario at all (`checkout init` writes a set). No request of a `.fetch` target is
sent.

| Option | Environment variable | What it does |
|---|---|---|
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in. |
| `--extension <handle>` | | Only these extensions. Repeat it for several. |
| `--format text` | | Default. One line per problem, then a summary. |
| `--format json` | | `{ok, checked, total, elapsedMs, failures: [{handle, target, error, scenarios}], invalid: [{scenario, file, problems}]}`, and `error` when there is no scenario. |
| `--budget <seconds>` | | How long the Functions may run in all. Default 300. |

## function build

`npx local-storefront function build [handle]... --app <dir>`

Builds the Functions of an app, as `shopify app build` does. A Function written in JavaScript or
TypeScript without a build command is built by lsf, without Node: its types
(`generated/api.ts`), `dist/function.js`, then its Wasm module. The others run their
`[extensions.build] command`. Guide: [checkout](checkout.md#javascript-and-typescript-functions).

| Option | Environment variable | What it does |
|---|---|---|
| `[handle]...` | | The Functions to build. Every Function of the app when none is given. |
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in. |

## function run

`npx local-storefront function run [handle]... --app <dir>`

Runs the Functions of an app once, without a server. Builds the input of each from the store
data, runs them in checkout order, and prints each input, output and log with the checkout that
results. Guide: [checkout](checkout.md#run-a-function-without-a-server).

```bash
npx local-storefront function run volume-discount --app ../my-app --session '{"cart":{"items":[{"variant":"TEE-BLK-M","quantity":3}]}}'
```

| Option | Environment variable | What it does |
|---|---|---|
| `[handle]...` | | The Functions to run. Every Function of the app when none is given. |
| `--app <dir>` | `LSF_APP` | The app. Defaults to the app the current folder is in; the store data to its `shopify-local/` when the current folder has none. |
| `--build` | | Builds each Function first, as `function build` does. |
| `--scenario <name>` | | A [checkout scenario](data-format.md#checkout-scenarios) of the store data (`checkouts/<name>.json`): its cart, buyer, language, `checkout` settings and the extensions it names. `--session`, `--checkout` and `--locale` go on top of it. |
| `--session <file or JSON>` | | The session to run for, as `PUT /__lsf/session` takes it. Defaults to the `session` of the store data. |
| `--checkout <file or JSON>` | | What the buyer entered: `email`, `phone`, `shippingAddress`, `selectedDelivery`, `selectedPayment`, `discountCodes`, `metafields`. |
| `--locale <iso>` | | The language of the buyer, one of the store's (`fr`, `pt-BR`): Functions read it as `localization.language`. Defaults to the primary language. |
| `--step <step>` | `completion` | `interaction` or `completion`: the step of the buyer journey, what validation Functions read as `buyerJourney.step`. |
| `--format text` | | Default. Each run, then the checkout. |
| `--format json` | | The checkout as the checkout page reads it, with every run in `runs`. |

Exits with `1` when a Function fails to run, or when a Function named on the command line did not
run: it says why (no discount names a discount Function, the cart is empty, an error of the
app…). A handle or a scenario that is close to an existing one is suggested. Errors a validation
Function reports are part of the checkout and do not change the exit code.

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

The data is the one `lsf checkout` reads: `--data`, or the folder's `shopify-local/`, or the
app's (`<app>/shopify-local`), or the built-in demo store. In a folder that is not a theme (an
app, such as [the checkout playground](../examples/checkout-playground)), it checks the data
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
  "diagnostics": [
    {
      "severity": "error",
      "code": "unknown_field",
      "file": "products/shirt.json",
      "path": "/titel",
      "message": "unknown field \"titel\"",
      "hint": "did you mean \"title\"?"
    }
  ]
}
```

`path` is a JSON Pointer into `file`. The codes are listed in the
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
| `--fix` | Rewrites what the strict parser rejects. A file is written only if it has not changed since it was read. |
| `--format text` | Default. One block per problem, for a person. |
| `--format json` | One JSON document, for a tool. |

### The text format

Each problem is a line `<file>:<line>: <message>`, followed by indented details:

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

| `code` | What it is |
|---|---|
| `strict_liquid_rejected` | Markup Shopify's strict parser rejects. |
| `strict_liquid_reinterpreted` | Markup Shopify's strict parser reads differently from today. |
| `liquid_syntax` | A file that does not parse at all. |
| `unknown_filter` | A filter that does not exist here. Shopify ignores unknown filters. |
| `block_outside_template` | `{% block %}` outside `layout/` and `templates/` (with `--preview liquid-july-26`). |
| `invalid_json` | A JSON file that does not parse. |
| `fix_fallback` | A warning: what the code meant could not be written for this file, so only the fixes that keep the output were. |

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

Exits with `1` when a problem is left, whatever the format.

## fmt

`npx local-storefront fmt [paths]`

Formats Liquid files as Prettier does with Shopify's Liquid plugin. Guide:
[formatting](formatting.md).

| Option | What it does |
|---|---|
| `[paths]` | Files and folders to format. Default: the theme folder. |
| `--check` | Writes nothing. Lists the files that are not formatted, and exits with `1` if there are any. |
| `--stdin` | Formats the standard input and prints it. For an editor. |
| `--stdin-filepath <path>` | With `--stdin`: the file the source comes from, for its configuration and `.prettierignore`. |
| `--option <name>=<value>`, `-o` | Sets a Prettier option: `-o printWidth=100`. Wins over the configuration files. |
| `--no-config` | Does not read `.prettierrc`, `.editorconfig` nor `.prettierignore`. |

A file that cannot be read, written or formatted is reported, the others are formatted, and
the command exits with `2`.

## init

`npx local-storefront init`

Creates the data folder with a demo store, JSON Schemas for editors, and the data guide as
`README.md`. For the checkout of a Shopify app, [`checkout init`](#checkout-init) writes the
scenarios.

| Option | What it does |
|---|---|
| `--force` | Overwrites files that exist. |

## schema

`npx local-storefront schema [kind]`

Prints the JSON Schema of the store data.

| Option | What it does |
|---|---|
| `[kind]` | `store` (default, a root data file), `product`, `collection`, `page`, `blog`, `customer`, `menu`, `session` or `checkout-scenario` (a file of `checkouts/`). |
| `--out <dir>` | Writes every schema into the folder as `<kind>.schema.json`. |

## routes

`npx local-storefront routes`

Lists the URLs the store data gives a page to, one per line.

## docs

`npx local-storefront docs`

Prints the data guide, with the reference of every field.

| Option | What it does |
|---|---|
| `--reference` | Prints only the field reference. |

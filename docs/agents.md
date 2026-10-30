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

## Tasks

### Check the store data

```bash
npx local-storefront validate --format json
```

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

`path` is a JSON Pointer into `file`. Apply the `hint` when there is one, then run the command
again until `ok` is `true`. The codes: [data guide](data-format.md#error-codes). The format of
every field: `npx local-storefront docs`, or `npx local-storefront schema <kind>` for a JSON
Schema.

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

A diagnostic with a `fix` is fixed by `--fix`; one with `no_fix` needs a person. Exit `1` while
a problem is left. Every key and code: [CLI reference](cli.md#the-json-format).

### Fix what Shopify's strict Liquid parser rejects

```bash
npx local-storefront check --fix
```

Rules for an agent:

- Run it on a clean working tree, so that `git diff` shows exactly what it wrote.
- Fixes marked `changes the output:` change the rendered HTML on purpose: the code now does
  what it says. Report each one to the person, with its line from the output. Do not revert
  it to silence the change.
- A problem reported with `no automatic fix:` needs a person's decision. Do not invent a
  rewrite: the reason says what the strict parser cannot express.
- `npx local-storefront check --fix --format json` lists what it wrote: `fix.written` is
  `true`, and `fix.changes_output` says what renders differently.
- Run `npx local-storefront check` again afterwards: it must exit `0`.

Guide: [strict Liquid](strict-liquid.md).

### Render a page

```bash
npx local-storefront render /products/blue-shirt --strict
```

Prints the HTML to stdout. With `--strict`, exit `1` when the page has Liquid errors; the errors
are in the HTML, as `Liquid error (sections/x line 12): …`. `npx local-storefront routes` lists
every path the data gives a page to.

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
before and after a change. Times are not. `--json` prints a speedscope profile. Guide:
[performance](performance.md).

### Run the Functions of a Shopify app

```bash
npx local-storefront app function run --app ../my-app --format json
```

Prints the checkout the Functions produce for the cart of the store data (`--session` sets
another): `lines`, `discounts`, `deliveryGroups`, `errors`, `cost`, and in `runs` the input,
the output, the logs and the error of each Function. Exits with `1` when a Function fails to
run. Guide: [checkout](checkout.md).

### Drive a running server

`npx local-storefront serve --port 0 --static --quiet` prints `ready:  http://127.0.0.1:<port>/`
to stderr when it can take requests. Then:

- `GET /__lsf/status` gives the data diagnostics, the templates, the customers and every page.
- `PUT /__lsf/session` sets the cart, the customer and data for one session; a wrong body
  answers `422` with the same diagnostics as `npx local-storefront validate --format json`.
- The header `x-lsf-liquid-errors` counts the Liquid errors of a page, and `x-lsf-not-found`
  says why a page is a 404.

Reference: [control API](control-api.md).

## What not to expect

local-storefront renders themes. Its checkout is a summary of the cart, unless it runs with
`--app <dir>`: then it runs the Functions and shows the checkout UI extensions of a Shopify app
([checkout](checkout.md)). It has no apps, no discounts, no taxes and no shipping rates in the
theme. A difference with Shopify that is known is listed in
[compatibility](compatibility.md#known-differences): check there before you report one.

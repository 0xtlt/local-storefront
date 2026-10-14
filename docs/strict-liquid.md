# Strict Liquid

Shopify is moving every store from its lenient Liquid parser to a strict one. A file with
markup the strict parser rejects can no longer be uploaded:

```text
Liquid syntax error (line 12): Expected end_of_string but found pipe
```

`npx local-storefront check` finds that markup in your theme, and
`npx local-storefront check --fix` rewrites it.

## Find what the strict parser rejects

```bash
npx local-storefront check
```

```text
snippets/card.liquid:1: Shopify's strict Liquid parser rejects this {% render %}: Expected end_of_string but found pipe
  was: … alt: image.alt | default: shop.name
  fix: … alt: image_alt
  before it: assign image_alt = image.alt | default: shop.name
  changes the output: `alt` gets `image.alt | default: shop.name`
snippets/card.liquid:8: Shopify's strict Liquid parser rejects this {% when %}: Expected end_of_string but found id
  was: 'collection' and collection.handle
  fix: 'collection'
2 file(s) checked, 2 problem(s)
2 of them can be fixed: run again with `--fix`
1 of the fixes change the output, where the code now does what it says: review the diff
```

Each problem gives the message Shopify gives, the markup, and the fix. `changes the output`
marks the fixes that render something else than today. Every line of the report is described
in the [CLI reference](cli.md#check).

`npx local-storefront check` reads every `.liquid` file of `layout/`, `templates/`, `sections/`,
`blocks/` and `snippets/`, and the `liquid` settings stored in JSON: a "Custom Liquid" section
of a template, a section group, `config/settings_data.json`.

## Fix it

```bash
npx local-storefront check --fix
```

Then review the diff before you push it:

```bash
git diff
```

A fix does one of two things.

### What the code meant, where that is clear

The lenient parser silently ignores what it does not understand. A theme can carry code for
years that never did what it says. Where the intent is obvious, the fix makes the code do it:

```liquid
{% render 'image', alt: image.alt | default: shop.name %}
```

becomes

```liquid
{% assign image_alt = image.alt | default: shop.name -%}
{% render 'image', alt: image_alt %}
```

The lenient parser never applied that filter. Now an image without alt text gets the shop's
name. The `-%}` removes the line break the fix adds, so nothing else changes in the HTML.

| Written | Fixed | What changes |
|---|---|---|
| `{% render 'card', on_sale: a and b %}` | A `{% liquid %}` block above the tag computes `true` or `false`, then `on_sale: card_on_sale` | `on_sale` is false when `b` is |
| `{% assign label = ('X' \| downcase) %}` | `{% assign label = 'X' \| downcase %}` | `label` is `x`. The lenient parser read the variable `X`. |
| `{% for i in items limit: 2 reversed %}` | `{% for i in items reversed limit: 2 %}` | The loop runs in reverse |
| `{% if a = b %}` | `{% if a == b %}` | The condition is evaluated. It rendered an error. |
| `{% when 'x' and y %}…` | `{% when 'x' %}{% if y %}…{% endif %}` | The body renders only when `y` is true |

A `when` with `and`, in a `case` that has an `{% else %}`, meant the `else` to run when the
condition is false. An `if` inside the `when` would not do that, so the `case` becomes
`if`/`elsif`/`else`:

```liquid
{%- case template.name -%}
  {%- when 'product' -%}P
  {%- when 'collection' and collection.handle -%}C
  {%- else -%}Z
{%- endcase -%}
```

becomes

```liquid
{%- if template.name == 'product' -%}P
  {%- elsif template.name == 'collection' and collection.handle -%}C
  {%- else -%}Z
{%- endif -%}
```

This happens only when the `when`s compare with distinct literals, since a `case` renders
every `when` that matches and `if` only the first.

### Today's output, everywhere else

Where the intent is not clear, the fix keeps exactly what renders today:

| Written | Fixed |
|---|---|
| `{{ product.title product.vendor }}` | `{{ product.title }}` |
| `{{ 'hi' \| append, '!' }}` | `{{ 'hi' \| append: '!' }}` |
| `{{ [key].title }}` | `{{ self[key].title }}` |

## What a fix guarantees

- **The strict parser accepts it.** Each rewritten file is parsed again before it is written.
- **A fix that keeps the output is read the same by both parsers** as the original is read
  today. These fixes were rendered by Shopify's reference implementation on 1,548 templates:
  the HTML is identical.
- **Nothing else in the file changes.** Arguments written one per line stay one per line, and
  the tags around keep their `{%-` and `-%}`. In JSON, only the value of the setting changes,
  with the escaping of the file (`<`, `\/`).
- **A variable a fix adds gets a name the file does not use.** `image_alt` becomes
  `image_alt_2` if the file already has an `image_alt`.
- **A file is written only if it has not changed** since `npx local-storefront check` read it.

Some markup has no fix, and the report says why. `{% assign a.b = 1 %}`, for example, names a
variable the strict parser cannot write.

## In CI

```yaml
- run: npx local-storefront check
```

`npx local-storefront check` exits with `1` while a problem is left, so the build fails before
Shopify refuses the upload.

## How it is checked

The rules come from Shopify's reference implementation, the `liquid` gem 5.14, in its
`strict2` mode. They are checked against Shopify's test suite of the language: 6,314 specs,
every rejection with the gem's message. Details:
[compatibility](compatibility.md#shopifys-strict-liquid-parser).

## From Rust

The audit is in the `lsf-liquid` crate, in `lsf_liquid::strict2`: `audit` returns the findings
of a template, `apply` writes the fixes that keep the output, and `apply_intents` writes what
`npx local-storefront check --fix` writes. A tag of your own says how the strict parser reads it
with `Environment::register_syntax`.

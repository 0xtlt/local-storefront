# Writing an issue

This page is the standard for issues filed against local-storefront. An issue that follows it
can be reproduced, ranked and fixed without a round trip with its author.

The words **must**, **should** and **may** are used as in
[RFC 2119](https://www.rfc-editor.org/rfc/rfc2119): *must* is required, *should* is expected
unless there is a stated reason, *may* is optional.

## What an issue is for

Most issues report a **divergence**: for the same theme, app and data, lsf does not do what a
Shopify store does. The reference is always the real store, then shopify.dev, never what seems
right.

An issue also may report a crash, an error message that misleads, a missing feature, or a
documentation error. The same structure applies; the *Expected* section then says what should
happen and why.

## Rules

1. One issue **must** cover one divergence. Two symptoms with the same cause or in the same
   feature **may** share an issue, numbered `1.`, `2.` in each section.
2. An issue **must** be reproduced on the latest release, and name it.
3. The repro **must** be minimal: only the files and fields the divergence needs, written out in
   full. No link to a private theme, no "use Dawn".
4. The *Expected* section **must** rest on evidence: an observation on a development store,
   dated, or a quoted shopify.dev page with its URL. A guess **must** be labelled as one.
5. Outputs **must** be pasted as text, not as screenshots.
6. The issue **should** say what a theme loses: what renders wrongly, what passes locally and
   fails on the store.
7. The author **must not** propose a fix in the body. A cause found in the code **may** go in a
   comment.

## Title

```text
[<severity>] <what lsf does>, <what Shopify does>
```

- Severity in brackets, lowercase: `[high]`, `[medium]` or `[low]` (see [Severity](#severity)).
- Code, paths, filters and routes in backticks.
- A statement of fact, not a question or a wish. Under 120 characters.

Examples:

```text
[high] `lsf check` passes a `visible_if` with a Liquid syntax error, which Shopify's upload refuses
[low] `/cart/update.js` with an unknown variant answers 404 (Shopify 422)
```

## Body

The sections come in this order, with these headings.

### Context (follow-ups only)

The first line of a follow-up names the earlier issue and what its fix settled, checked again on
the current release:

```markdown
Follow-up to #189. Its fix covers items 2, 3 and 5 there; that holds on 0.3.0-rc.7.
```

Do not reopen a closed issue for a leftover: file a follow-up.

### `## What happens`

What lsf outputs, exactly, in a code block. Then, in one or two sentences, what a theme
gets wrong because of it.

### `## Repro`

Every file the repro needs, each under its path, then the command:

````markdown
`./data/store.json`:

```json
{ "shop": { "name": "Repro Shop", "currency": "USD" } }
```

`./theme/layout/theme.liquid`: a layout with `{{ content_for_header }}` and `{{ content_for_layout }}`.

```sh
lsf --theme ./theme --data ./data render /products/widget
```
````

- Prefer `lsf render` over `serve` + `curl`. Use `serve` when the divergence needs a cookie, a
  method or a header.
- Write a trivial file in one line of prose instead of a code block, as above for the layout.
- Give the shell the commands were run in when it changes them (`MSYS_NO_PATHCONV=1` on Git
  Bash).

### `## Expected`

What the store gives, with the source:

```markdown
A development store answered `422` with the same body (observed 2026-11-13).
```

- An observation names its setup when it matters: the shop's timezone, published languages,
  `shopify theme dev`, a form or a JSON body.
- A documentation source is quoted and linked.
- When the store and the documentation disagree, the store wins and the issue says so.

### Footer

One italic line: how the issue was checked, the release, the OS.

```markdown
_Repro on lsf 0.3.0-rc.7, macOS 15. Checked with an AI assistant (Claude)._
```

Saying that an AI assistant helped is required when one did.

## Severity

| Severity | Meaning | Examples |
|---|---|---|
| `high` | lsf says a theme or app is fine when Shopify refuses it, or a common page or flow breaks. | `lsf check` passes a section the upload refuses; a cart call crashes. |
| `medium` | Visible output differs in a case a theme can easily hit. | An unescaped `search.terms`; English strings on `/fr/`. |
| `low` | An edge case, an error body or status, an ordering. | A 404 where Shopify answers 422; `search.types` order. |

When in doubt, pick the lower one. A maintainer may change it.

## Labels

Maintainers set the labels during triage. The author does not need to.

| Label | Set when |
|---|---|
| `bug` | Behaviour is wrong. `enhancement` for a missing feature, `documentation` for the docs. |
| `severity: high` / `medium` / `low` | Always, matching the title. |
| `shopify-parity` | lsf differs from a real store. |
| `follow-up` | The issue is a leftover of an earlier one. |
| `area: liquid` | Tags, filters, objects. |
| `area: cart-api` | `/cart/*.js` and cart forms. |
| `area: search` | The search page and the `search` object. |
| `area: i18n` | Locales, translated built-in strings, localized routes. |
| `area: metafields` | Metafield types, values and `metafield_tag`. |
| `area: check` | `lsf check` and theme validation. |
| `area: functions` | Shopify Functions and checkout extensions. |

An issue may have several `area:` labels. A new area gets a new `area:` label, added here in the
same change.

## Before filing

- [ ] Reproduced on the latest release.
- [ ] Searched open and closed issues; a leftover is filed as a follow-up.
- [ ] Title is `[severity] …`.
- [ ] *What happens*, *Repro* and *Expected* are present, in that order.
- [ ] Every repro file is written out; the command runs as pasted.
- [ ] *Expected* has a dated observation or a linked source.
- [ ] Footer names the release and the OS.

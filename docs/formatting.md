# Formatting

`npx local-storefront fmt` formats Liquid files the way [Prettier](https://prettier.io) does
with Shopify's Liquid plugin (`@shopify/prettier-plugin-liquid`). Neither is needed:
local-storefront does it itself, and the 283 files of Shopify's Horizon theme take less than a
tenth of a second.

## Format the theme

```bash
npx local-storefront fmt
```

Formats every `.liquid` file of the theme: Liquid, HTML, the JSON of `{% schema %}`, the CSS
of `<style>`, `{% style %}` and `{% stylesheet %}`, and the JavaScript of `<script>` and
`{% javascript %}`. A theme that Prettier has formatted comes out unchanged.

Format some files only:

```bash
npx local-storefront fmt sections/header.liquid snippets
```

Format the files a commit touches:

```bash
npx local-storefront fmt $(git diff --name-only)
```

A file named on its own is formatted if it is a `.liquid` file that `.prettierignore` does not
name, as Prettier does: the other files of that list are left alone.

## Check in CI

```bash
npx local-storefront fmt --check
```

Writes nothing. Lists the files that are not formatted, and exits with `1` if there are any.

## From an editor

```bash
npx local-storefront fmt --stdin --stdin-filepath sections/header.liquid < sections/header.liquid
```

Reads the source from the standard input and prints it formatted. `--stdin-filepath` gives
the configuration and the `.prettierignore` of that file; an ignored file is printed as it is.

## Configuration

Your Prettier configuration applies. It is looked for as Prettier looks for it: from the folder
of each file, up. These files are read:

- `.prettierrc` (JSON or YAML), `.prettierrc.json`, `.prettierrc.yaml`, `.prettierrc.yml`;
- the `prettier` key of `package.json`;
- their `overrides`;
- `.editorconfig`;
- `.prettierignore`, the nearest one from the theme folder up.

A configuration written in JavaScript (`prettier.config.js`) cannot be read:
`npx local-storefront fmt` says so, and the files of its folder get the default options, as
Prettier's search stops there too.

To set an option from the command line, under the name Prettier gives it:

```bash
npx local-storefront fmt -o printWidth=100 -o singleQuote=true
```

It wins over the configuration files. `--no-config` ignores the files.

### Options

| Prettier option | Default | What it changes |
|---|---|---|
| `printWidth` | `120` | The width lines are kept under. |
| `tabWidth` | `2` | Spaces per level of indentation. |
| `useTabs` | `false` | Indents with tabs. |
| `endOfLine` | `lf` | `lf`, `crlf`, `cr`, or `auto` for what the file uses. |
| `singleQuote` | `false` | Single quotes around HTML attributes. |
| `liquidSingleQuote` | `true` | Single quotes around the strings of Liquid. |
| `embeddedSingleQuote` | `true` | Single quotes in the CSS and the JavaScript of `<style>`, `<script>` and the `{% style %}`, `{% stylesheet %}` and `{% javascript %}` tags. |
| `bracketSameLine` | `false` | Puts the `>` of a tag at the end of its last attribute. |
| `singleAttributePerLine` | `false` | One attribute per line as soon as a tag has two. |
| `htmlWhitespaceSensitivity` | `css` | Where whitespace may be added or removed: `css`, `strict`, `ignore`. |
| `captureWhitespaceSensitivity` | `strict` | `ignore` formats what a `{% capture %}` holds. |
| `singleLineLinkTags` | `false` | Keeps `<link>` tags on one line. |
| `indentSchema` | `false` | Indents the JSON of `{% schema %}`. |
| `liquidDocParamDash` | `true` | A dash between a `@param` of `{% doc %}` and its description. |
| `embeddedLanguageFormatting` | `auto` | `off` leaves JSON, CSS and JavaScript as they are written. |
| `semi` | `true` | A semicolon at the end of each statement of JavaScript. |
| `jsxSingleQuote` | `false` | Single quotes around the attributes of JSX. |
| `quoteProps` | `as-needed` | Quotes around the keys of objects: `as-needed`, `consistent`, `preserve`. |
| `trailingComma` | `all` | A comma after the last item of a list on several lines: `all`, `es5`, `none`. |
| `bracketSpacing` | `true` | Spaces inside the braces of an object, in JavaScript and JSON. |
| `objectWrap` | `preserve` | `collapse` puts an object on one line when it fits, even if it started on several. |
| `arrowParens` | `always` | Parentheses around the only parameter of an arrow function: `always`, `avoid`. |
| `experimentalOperatorPosition` | `end` | `start` begins a broken line with its operator. |

Other options of Prettier are ignored in a configuration file.

## Safety

- A file is replaced in one step, with its permissions: an interrupted run leaves no
  half-written template, and a read-only file is not replaced.
- In a folder, `node_modules`, `.git`, links to folders and what `.prettierignore` names are
  left out.
- A file or a folder that cannot be read, written or formatted is reported, the others are
  formatted all the same, and the command exits with `2`.

## What differs from Prettier

How the output is compared with Prettier's, and the few things it does not format:
[compatibility](compatibility.md#formatting).

# Polaris, as Shopify's CDN serves it

The files of this directory are copies of files of `https://cdn.shopify.com/`, at the same paths:
Polaris, the web components of an embedded app's pages (`<s-page>`, `<s-button>`, …), in its
1.x and 2.x versions, and what they load from the CDN as they run: their icons and their fonts.
They are not modified. `lsf` embeds them, and serves them at `/__lsf/cdn/<path>` when its Polaris
mode is `offline` (`--polaris offline`, or `PUT /__lsf/polaris`), with the CDN's URLs inside the
scripts and stylesheets pointed at lsf: an app shown in lsf's admin then renders its components,
icons and fonts without the internet. See `docs/admin.md` ("Polaris without the internet") and
`crates/cli/src/server/polaris.rs`.

Copyright Shopify Inc. The scripts and the icons carry no license header, and Shopify publishes no
license for them; they are included here by the decision of lsf's maintainer, for a tool that
previews Shopify apps on a developer's machine. The fonts, Inter and Shopify Inter, are © The
Inter Project Authors, under the SIL Open Font License 1.1, as their own metadata says (their
`name` table, which carries the notice and the license's URL wherever they go).

<!-- refresh: start -->
Fetched on 2026-11-08 from https://cdn.shopify.com/: 862 files, 2544 KB.

| File | Version | Also served at | Size | SHA-256 |
|---|---|---|---|---|
| `shopifycloud/polaris-2.0.js` | 2.0.0 | `polaris-2.js` | 898 KB | `af6e068300a7ec498092ecd2c4feb90bd93c43a2d8f75d27f65f86ae1f93d9f2` |
| `shopifycloud/polaris-1.1.js` | 1.1.0 | `polaris.js`, `polaris-1.js` | 723 KB | `912455ad068713e7595f5a506fb7433a078554c327cf0fd30ce86ccb214818fe` |
| `static/fonts/inter/v4/styles.css` | | | 2 KB | `0bc0e9702fae47de7df775afecd50830b7da95bb6cfbd06bdb62ba0276e012d6` |
| `static/fonts/shopifyinter/v0/styles.css` | | | 2 KB | `0fafae9efbc63816c60af543b85b3db0752e3901cd0b3f42f51b6e986e56b681` |

And 844 icons (`shopifycloud/admin-ui-foundations/{icons,internal-only,experimental}/<hash>.svg`, 509 KB), 14 fonts (`static/fonts/…/*.woff2`, 410 KB). The SHA-256 of every file is in `SHA256SUMS`.
<!-- refresh: end -->

## Refreshing the copy

For the maintainers of lsf: lsf itself never downloads these files.

```bash
node tools/polaris/refresh.mjs
cargo test -p lsf-cli polaris
```

The script fetches each major of Polaris (`shopifycloud/polaris-1.js`, `polaris-2.js`), keeps it
under the name of the version it says it is (`polaris-2.0.js` for 2.0.0, which the CDN must
serve the same), and records which unversioned names served it (`polaris.js` too). Then it
fetches every icon the scripts' tables name, their stylesheets of fonts and the fonts those
point at, and fails on a URL of the CDN it does not know how to follow. It replaces
`shopifycloud/` and `static/` here, and writes `SHA256SUMS`, the table above and
`crates/cli/src/server/polaris/files.rs`, the list of the files the binary embeds. The tests
check that the list, the files and `SHA256SUMS` agree.

A new major of Polaris (a `polaris-3.js`) is added to `MAJORS` in the script; the aliases lsf
serves come from what the CDN served when the script ran.

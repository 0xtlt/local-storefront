# Third-party data

`shopify_system_translations.json` is a subset (everything outside `shopify.checkout.*`) of
`data/shopify_system_translations.json` from
[Shopify/theme-liquid-docs](https://github.com/Shopify/theme-liquid-docs), © Shopify Inc.,
distributed under the MIT license. It provides the English text of the translation keys Shopify
ships with every store (`shopify.pagination.next`, `shopify.sentence.words_connector`, ...).

`placeholders/image.svg`, `placeholders/collection-2.svg` and `placeholders/lifestyle-2.svg` are
the illustrations Shopify's `placeholder_svg_tag` filter renders for those names, taken from the
expected outputs of [Shopify/liquid-spec](https://github.com/Shopify/liquid-spec) (commit
`84bf25e`), © Shopify Inc., distributed under the MIT license. Shopify has not published its
other placeholder illustrations: `placeholder_svg_tag` draws a neutral shape for them.

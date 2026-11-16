# Third-party data

`shopify_system_translations.json` is a subset (everything outside `shopify.checkout.*`) of
`data/shopify_system_translations.json` from
[Shopify/theme-liquid-docs](https://github.com/Shopify/theme-liquid-docs), © Shopify Inc.,
distributed under the MIT license. It provides the English text of the translation keys Shopify
ships with every store (`shopify.pagination.next`, `shopify.notices.tags.add_products`, ...).
It was then checked against what `t` prints for each key on Shopify's demo store, through the
renderer of shopify.dev's Liquid playground (2026-11-16): four texts were reworded to the
store's, and six keys the store does not have (`shopify.sentence.*`,
`shopify.notices.cart.not_found`, ...) were removed.

`shopify_system_translations.fr.json` is what `t` prints for the same keys on a French page
(`/fr`) of that store, read the same way.

`placeholders/image.svg`, `placeholders/collection-2.svg` and `placeholders/lifestyle-2.svg` are
the illustrations Shopify's `placeholder_svg_tag` filter renders for those names, taken from the
expected outputs of [Shopify/liquid-spec](https://github.com/Shopify/liquid-spec) (commit
`84bf25e`), © Shopify Inc., distributed under the MIT license. Shopify has not published its
other placeholder illustrations: `placeholder_svg_tag` draws a neutral shape for them.

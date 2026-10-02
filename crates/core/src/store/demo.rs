//! The built-in demo store: used when a theme has no data directory yet, and written to disk
//! by `lsf init` as a starting point.

/// The files of the demo data directory, as `(relative path, content)`.
pub const FILES: &[(&str, &str)] = &[
    ("store.json", include_str!("../../data/demo/store.json")),
    ("menus.json", include_str!("../../data/demo/menus.json")),
    (
        "collections.json",
        include_str!("../../data/demo/collections.json"),
    ),
    ("pages.json", include_str!("../../data/demo/pages.json")),
    ("blogs.json", include_str!("../../data/demo/blogs.json")),
    (
        "customers.json",
        include_str!("../../data/demo/customers.json"),
    ),
    (
        "gift_cards.json",
        include_str!("../../data/demo/gift_cards.json"),
    ),
    (
        "products/organic-cotton-t-shirt.json",
        include_str!("../../data/demo/products/organic-cotton-t-shirt.json"),
    ),
    (
        "products/linen-overshirt.json",
        include_str!("../../data/demo/products/linen-overshirt.json"),
    ),
    (
        "products/canvas-tote-bag.json",
        include_str!("../../data/demo/products/canvas-tote-bag.json"),
    ),
    (
        "products/ceramic-mug.json",
        include_str!("../../data/demo/products/ceramic-mug.json"),
    ),
    (
        "products/scented-candle.json",
        include_str!("../../data/demo/products/scented-candle.json"),
    ),
    (
        "products/wool-beanie.json",
        include_str!("../../data/demo/products/wool-beanie.json"),
    ),
    (
        "products/merino-crew-socks.json",
        include_str!("../../data/demo/products/merino-crew-socks.json"),
    ),
    (
        "products/gift-card.json",
        include_str!("../../data/demo/products/gift-card.json"),
    ),
];

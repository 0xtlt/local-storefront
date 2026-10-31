//! What a mutation costs on a large store: a store of `LSF_COST_PRODUCTS` products (3000 by
//! default) of 12 variants each, generated, then mutations through `AdminApi::handle`, and a
//! rollback on the state, timed.
//!
//! Ignored by default (seeding the store takes seconds); run it in release:
//! `cargo test --release -p lsf-admin --test mutation_cost -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::state::Shops;
use lsf_core::store::load::{DataSource, LoadOptions, load};
use serde_json::{Value, json};

const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

/// The data of a store of `products` products of 12 variants (3 sizes × 4 colors).
fn generated_store(products: usize) -> Value {
    let sizes = ["S", "M", "L"];
    let colors = ["Red", "Blue", "Green", "Black"];
    let products: Vec<Value> = (0..products)
        .map(|n| {
            let variants: Vec<Value> = sizes
                .iter()
                .flat_map(|size| colors.iter().map(move |color| (size, color)))
                .map(|(size, color)| {
                    json!({
                        "options": [size, color],
                        "price": format!("{}.00", 10 + n % 90),
                        "sku": format!("P{n}-{size}-{color}"),
                        "inventory_quantity": 5,
                    })
                })
                .collect();
            json!({
                "title": format!("Product {n}"),
                "vendor": "Generated",
                "type": "Shirts",
                "tags": ["generated"],
                "options": ["Size", "Color"],
                "variants": variants,
            })
        })
        .collect();
    json!({
        "now": "2026-01-01T00:00:00Z",
        "shop": {"name": "Large shop", "permanent_domain": "large-shop.myshopify.com"},
        "products": products,
    })
}

fn graphql(api: &AdminApi, token: &str, query: &str) -> Value {
    let response = api
        .handle(&HttpRequest {
            method: "POST".to_string(),
            path: GRAPHQL.to_string(),
            headers: [
                ("content-type".to_string(), "application/json".to_string()),
                ("x-shopify-access-token".to_string(), token.to_string()),
            ]
            .into_iter()
            .collect(),
            body: json!({ "query": query }).to_string().into_bytes(),
            ..HttpRequest::default()
        })
        .expect("routed");
    serde_json::from_slice(&response.body).expect("JSON")
}

/// The fastest and the median of `runs` runs of `run`.
fn time(runs: usize, mut run: impl FnMut(usize)) -> (Duration, Duration) {
    let mut times: Vec<Duration> = (0..runs)
        .map(|i| {
            let start = Instant::now();
            run(i);
            start.elapsed()
        })
        .collect();
    times.sort();
    (times[0], times[runs / 2])
}

#[test]
#[ignore = "seeds a store of 3000 products: run it in release with --ignored --nocapture"]
fn a_mutation_on_a_large_store() {
    let products = std::env::var("LSF_COST_PRODUCTS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);
    let dir = std::env::temp_dir().join(format!("lsf-mutation-cost-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("store.json"),
        generated_store(products).to_string(),
    )
    .unwrap();
    let (store, diagnostics) = load(&DataSource::Directory(dir.clone()), &LoadOptions::default());
    std::fs::remove_dir_all(&dir).ok();
    assert!(!diagnostics.has_errors(), "{diagnostics:?}");

    let start = Instant::now();
    let api = AdminApi::new(
        &store,
        AdminOptions {
            client_secret: "test-secret".to_string(),
            token_seed: Some(7),
            throttle: ThrottleSetting::Off,
            ..AdminOptions::default()
        },
    );
    let seeded = start.elapsed();
    let token = api.token(&api.default_shop());
    let first = graphql(&api, &token, "{ products(first: 1) { nodes { id } } }");
    let product = first["data"]["products"]["nodes"][0]["id"]
        .as_str()
        .expect("a product")
        .to_string();
    let records = graphql(&api, &token, "{ productsCount { count } }");
    println!(
        "{products} products × 12 variants ({} products counted), seeded in {seeded:?}",
        records["data"]["productsCount"]["count"]
    );

    let runs = 15;
    let (fastest, median) = time(runs, |i| {
        let answer = graphql(
            &api,
            &token,
            &format!(
                r#"mutation {{ productUpdate(product: {{id: "{product}", title: "Renamed {i}"}}) {{ product {{ title }} userErrors {{ message }} }} }}"#
            ),
        );
        assert_eq!(
            answer["data"]["productUpdate"]["product"]["title"],
            format!("Renamed {i}")
        );
    });
    println!("productUpdate that succeeds: fastest {fastest:?}, median {median:?}");

    // Writes the product, then fails on the variant that does not exist: rolled back.
    let (fastest, median) = time(runs, |_| {
        let answer = graphql(
            &api,
            &token,
            &format!(
                r#"mutation {{ productVariantsBulkUpdate(productId: "{product}", variants: [{{id: "gid://shopify/ProductVariant/1", price: "1.00"}}]) {{ userErrors {{ message }} }} }}"#
            ),
        );
        assert!(
            !answer["data"]["productVariantsBulkUpdate"]["userErrors"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{answer}"
        );
    });
    println!("productVariantsBulkUpdate that fails: fastest {fastest:?}, median {median:?}");

    let (fastest, median) = time(runs, |_| {
        graphql(&api, &token, "{ shop { name } }");
    });
    println!("a query: fastest {fastest:?}, median {median:?}");

    // The mutations validate before they write, so a failure through the API rolls nothing
    // back. What a failure after writes costs, on the state itself: a product and its 12
    // variants changed, one variant removed, one product created, events, then a rollback.
    let mut shops = Shops::new(&store, None);
    let shop = shops.default_shop_mut().expect("the default shop");
    let variants: Vec<String> = shop
        .keys("ProductVariant")
        .take(12)
        .map(str::to_string)
        .collect();
    let (fastest, median) = time(runs, |i| {
        let savepoint = shop.savepoint();
        if let Some(record) = shop.get_mut(&product) {
            record.insert("title".into(), json!(format!("Undone {i}")));
        }
        for variant in &variants {
            shop.touch(variant);
            shop.emit("products/update", variant);
        }
        shop.delete(&variants[0], "variants/delete");
        shop.insert(
            serde_json::from_value(json!({"__typename": "Product", "title": "New"})).unwrap(),
        );
        shop.rollback(savepoint);
    });
    println!("writes then a rollback: fastest {fastest:?}, median {median:?}");
    assert_eq!(shop.count("ProductVariant"), products * 12);

    // What a mutation paid before its first write when the rollback was a copy of the shop.
    let (fastest, median) = time(5, |_| {
        std::hint::black_box(shop.clone());
    });
    println!("a copy of the shop: fastest {fastest:?}, median {median:?}");
}

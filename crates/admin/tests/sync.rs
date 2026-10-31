//! Store sync: records changed the way the mutations change them, projected to an overlay of the
//! store data, then loaded with it. What the backend changes through the Admin API must show in
//! the storefront and the checkout, and nothing else may change.

mod support;

use std::path::Path;
use std::sync::Arc;

use lsf_admin::domains::{app, catalog, content, customers};
use lsf_admin::state::{Record, ShopState, value};
use lsf_admin::sync;
use lsf_core::Store;
use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::load::{DataSource, LoadOptions, Overlay, load_with_overlays};
use lsf_core::store::model;
use serde_json::{Value, json};

fn rich() -> (Store, ShopState) {
    let store = support::fixture_store("rich");
    let app = support::fixture_app();
    let state = ShopState::seeded(&store, Some(&app));
    (store, state)
}

/// The rich fixture loaded with the overlays, which must not have errors.
fn load(overlays: &[Overlay]) -> (Store, Diagnostics) {
    let (store, diagnostics) = load_with_overlays(
        &DataSource::Directory(support::fixture("rich")),
        &LoadOptions::default(),
        overlays,
    );
    assert!(
        !diagnostics.has_errors(),
        "the overlay gives errors:\n{diagnostics}\n{overlays:#?}"
    );
    (store, diagnostics)
}

/// Projects the changes of the shop and loads the store with them.
fn synced(state: &ShopState, base: &Store) -> (Store, Overlay) {
    let overlay = sync::overlay(state, base).expect("something to sync");
    let (store, diagnostics) = load(std::slice::from_ref(&overlay));
    let own: Vec<_> = diagnostics
        .items
        .iter()
        .filter(|item| item.file == sync::OVERLAY_NAME)
        .collect();
    assert!(
        own.is_empty(),
        "the overlay has warnings: {own:?}\n{overlay:#?}"
    );
    (store, overlay)
}

fn object(value: Value) -> Record {
    value::object(value)
}

fn set(state: &mut ShopState, gid: &str, field: &str, value: Value) {
    state
        .get_mut(gid)
        .unwrap_or_else(|| panic!("no record {gid}"))
        .insert(field.to_string(), value);
}

fn push(state: &mut ShopState, gid: &str, field: &str, item: &str) {
    if let Some(Value::Array(items)) = state.get_mut(gid).and_then(|record| record.get_mut(field)) {
        items.push(json!(item));
    }
}

fn product_gid(store: &Store, handle: &str) -> String {
    catalog::product_gid(store.product_by_handle(handle).expect("product"))
}

/// A metafield set on an owner, as `metafieldsSet` makes it.
fn set_metafield(
    state: &mut ShopState,
    owner: &str,
    owner_type: &str,
    namespace: &str,
    key: &str,
    kind: &str,
    value: &str,
) -> String {
    let id = content::metafield_gid(owner, namespace, key);
    state.insert(object(json!({
        "__typename": "Metafield",
        "id": id,
        "namespace": namespace,
        "key": key,
        "type": kind,
        "value": value,
        "jsonValue": content::json_value(kind, value),
        "owner": owner,
        "ownerType": owner_type,
        "createdAt": "2026-03-15T10:00:00Z",
        "updatedAt": "2026-03-15T10:00:00Z",
    })));
    if let Some(Value::Array(ids)) = state
        .get_mut(owner)
        .and_then(|record| record.get_mut("metafields"))
        && !ids.contains(&json!(id))
    {
        ids.push(json!(id));
    }
    id
}

/// A product with one variant stocked at a location, as `productCreate` then
/// `productVariantsBulkUpdate` and `inventorySetQuantities` make it, published on the Online Store.
fn create_product(
    state: &mut ShopState,
    title: &str,
    handle: &str,
    price: &str,
    stock: i64,
    location: &str,
) -> String {
    let product = state.new_gid("Product");
    let variant = state.new_gid("ProductVariant");
    let item = state.new_gid("InventoryItem");
    let level = state.new_gid("InventoryLevel");
    let option = state.new_gid("ProductOption");
    let option_value = state.new_gid("ProductOptionValue");
    let now = state.timestamp();
    state.insert(object(json!({
        "__typename": "ProductOptionValue", "id": option_value, "name": "Default Title", "hasVariants": true,
    })));
    state.insert(object(json!({
        "__typename": "ProductOption", "id": option, "name": "Title", "position": 1,
        "values": ["Default Title"], "optionValues": [option_value],
    })));
    state.insert(object(json!({
        "__typename": "InventoryLevel", "id": level, "item": item, "location": location,
        "quantities": [{"name": "available", "quantity": stock}, {"name": "on_hand", "quantity": stock}],
    })));
    state.insert(object(json!({
        "__typename": "InventoryItem", "id": item, "sku": "HAT-1", "tracked": true,
        "requiresShipping": true, "inventoryLevels": [level], "variant": variant,
        "measurement": {"weight": {"unit": "GRAMS", "value": 120.0}},
    })));
    state.insert(object(json!({
        "__typename": "ProductVariant", "id": variant, "title": "Default Title", "sku": "HAT-1",
        "price": price, "compareAtPrice": null, "position": 1,
        "selectedOptions": [{"name": "Title", "value": "Default Title"}],
        "inventoryItem": item, "inventoryPolicy": "DENY", "inventoryQuantity": stock,
        "availableForSale": stock > 0, "taxable": true, "product": product, "media": [],
        "createdAt": now, "updatedAt": now, "metafields": [],
    })));
    state.insert(object(json!({
        "__typename": "Product", "id": product, "title": title, "handle": handle,
        "descriptionHtml": "<p>Warm.</p>", "vendor": "Atelier", "productType": "Hats",
        "tags": ["new", "wool"], "status": "ACTIVE", "createdAt": now, "updatedAt": now,
        "publishedAt": now, "options": [option], "variants": [variant], "media": [],
        "collections": [], "sellingPlanGroups": [], "metafields": [], "isGiftCard": false,
        "requiresSellingPlan": false, "templateSuffix": null,
    })));
    product
}

fn renderer() -> lsf_core::Renderer {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let env = Arc::new(lsf_core::environment());
    let theme = lsf_core::Theme::open(&directory, env, lsf_core::theme::Revalidate::Never)
        .expect("the fixture theme of lsf-core");
    lsf_core::Renderer::new(Arc::new(theme))
}

fn page(renderer: &lsf_core::Renderer, store: &Arc<Store>, path: &str) -> lsf_core::Rendered {
    renderer.render(
        store.clone(),
        lsf_core::Request::new("shop.test", path),
        lsf_core::Session::initial(store),
        &lsf_core::render::Target::Page,
    )
}

/// Renders a piece of Liquid as part of the page at `path`.
fn liquid(renderer: &lsf_core::Renderer, store: &Arc<Store>, path: &str, source: &str) -> String {
    let site = renderer.site(
        store.clone(),
        lsf_core::Request::new("shop.test", path),
        lsf_core::Session::initial(store),
    );
    let resolved = lsf_core::render::routes::resolve(&site);
    let (output, errors) = renderer
        .render_liquid(&site, resolved, source, &[])
        .expect("the Liquid parses");
    assert!(errors.is_empty(), "{errors:?}");
    output
}

#[test]
fn nothing_changed_gives_no_overlay() {
    let (store, mut state) = rich();
    assert!(sync::overlay(&state, &store).is_none());
    // What a mutation recomputes without changing anything, and what it reads to change: not
    // a change.
    let dress = product_gid(&store, "linen-dress");
    catalog::refresh_product(&mut state, &dress);
    let _ = state.get_mut(&dress);
    assert!(sync::overlay(&state, &store).is_none());
    // A derived field that the store data does not have (an order placed at the checkout makes
    // the customer's `amountSpent` change): nothing to write either.
    let zoe = customers::customer_gid(store.customer_by_email("zoe@example.com").unwrap());
    set(
        &mut state,
        &zoe,
        "amountSpent",
        json!({"amount": "999.0", "currencyCode": "EUR"}),
    );
    set(&mut state, &zoe, "numberOfOrders", json!("12"));
    assert!(sync::overlay(&state, &store).is_none());
}

#[test]
fn a_created_product_is_on_the_storefront() {
    let (base, mut state) = rich();
    let paris = catalog::location_gid(&base.locations[0]);
    let hat = create_product(&mut state, "Wool Hat", "wool-hat", "35.00", 6, &paris);
    let summer = catalog::collection_gid(base.collection_by_handle("summer").unwrap());
    push(&mut state, &summer, "products", &hat);
    set_metafield(
        &mut state,
        &hat,
        "PRODUCT",
        "custom",
        "care",
        "single_line_text_field",
        "Hand wash",
    );

    let (store, overlay) = synced(&state, &base);
    assert!(overlay.removed.is_empty(), "{:?}", overlay.removed);
    let product = store
        .product_by_handle("wool-hat")
        .expect("the new product");
    // The id the Admin API gave it, so that both agree on its GID.
    assert_eq!(catalog::product_gid(product), hat);
    let variant = &product.variants[0];
    assert_eq!(
        (
            variant.price,
            variant.inventory_quantity,
            variant.available,
            variant.sku.as_str(),
            variant.weight
        ),
        (3500, 6, true, "HAT-1", 120)
    );
    assert_eq!(variant.store_availabilities, vec![(0, true)]);
    assert!(product.has_only_default_variant());
    assert_eq!(
        (product.vendor.as_str(), product.product_type.as_str()),
        ("Atelier", "Hats")
    );
    assert_eq!(product.tags, ["new", "wool"]);
    assert_eq!(
        product.metafields["custom"]["care"].value,
        json!("Hand wash")
    );
    let collection = store.collection_by_handle("summer").unwrap();
    assert!(
        collection
            .products
            .iter()
            .any(|index| store.products[*index].handle == "wool-hat")
    );

    // On the theme's pages: its own, and the collection's.
    let store = Arc::new(store);
    let renderer = renderer();
    let rendered = page(&renderer, &store, "/products/wool-hat");
    assert_eq!(rendered.status, 200);
    assert!(
        rendered.body.contains("<h1>Wool Hat</h1>"),
        "{}",
        rendered.body
    );
    let rendered = page(&renderer, &store, "/collections/summer");
    assert!(rendered.body.contains("Wool Hat"), "{}", rendered.body);
    assert_eq!(
        liquid(
            &renderer,
            &store,
            "/products/wool-hat",
            "{{ product.metafields.custom.care.value }}"
        ),
        "Hand wash"
    );
}

#[test]
fn a_changed_product_keeps_what_the_records_do_not_hold() {
    let (base, mut state) = rich();
    let dress = product_gid(&base, "linen-dress");
    set(&mut state, &dress, "title", json!("Linen Dress, new"));
    set(&mut state, &dress, "tags", json!(["linen"]));
    let (store, overlay) = synced(&state, &base);
    // Only the dress is written.
    let data = overlay.data.as_object().unwrap();
    assert_eq!(data.keys().collect::<Vec<_>>(), ["products"]);
    assert_eq!(data["products"].as_array().unwrap().len(), 1);

    let before = base.product_by_handle("linen-dress").unwrap();
    let after = store.product_by_handle("linen-dress").unwrap();
    assert_eq!(
        (after.title.as_str(), after.tags.as_slice()),
        ("Linen Dress, new", &["linen".to_string()][..])
    );
    assert_eq!(after.id, before.id);
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.description, before.description);
    assert_eq!(after.collections, before.collections);
    for (old, new) in before.variants.iter().zip(&after.variants) {
        assert_eq!(
            (
                new.id,
                new.price,
                new.compare_at_price,
                new.inventory_quantity,
                new.available
            ),
            (
                old.id,
                old.price,
                old.compare_at_price,
                old.inventory_quantity,
                old.available
            )
        );
        assert_eq!(
            (new.weight, new.weight_unit.as_str()),
            (old.weight, old.weight_unit.as_str())
        );
        assert_eq!(new.store_availabilities, old.store_availabilities);
        assert_eq!(new.unit_price, old.unit_price);
        assert_eq!(new.options, old.options);
        assert_eq!(new.media_index, old.media_index);
    }
    assert_eq!(after.media.len(), before.media.len());
    for (old, new) in before.media.iter().zip(&after.media) {
        assert_eq!((new.id, new.media_type()), (old.id, old.media_type()));
        assert_eq!(
            new.preview
                .as_ref()
                .map(|image| (image.id, image.src.as_str())),
            old.preview
                .as_ref()
                .map(|image| (image.id, image.src.as_str()))
        );
    }
    // The swatches of the option values, and the metafields with their references.
    assert_eq!(
        after.options[1]
            .swatch("Red")
            .and_then(|swatch| swatch.color.as_deref()),
        Some("#c0392b")
    );
    assert_eq!(
        after.metafields["custom"]["pairs_with"].value,
        json!("coffee-subscription")
    );
    assert_eq!(
        after.metafields["custom"]["related"].value,
        json!(["coffee-subscription", "gift-card"])
    );
    assert_eq!(
        after.metafields["custom"]["size_guide"].value,
        json!("docs/size-guide.pdf")
    );
    assert_eq!(
        after.metafields["custom"]["designer"].value,
        json!("designer/ines")
    );
    assert_eq!(
        after.metafields["$app:config"]["hidden"].value,
        json!(false)
    );
    assert_eq!(
        after.metafields["custom"]["specs"].value,
        json!({"fit": "relaxed"})
    );
    // The customer's order of the dress is still there.
    let zoe = store.customer_by_email("zoe@example.com").unwrap();
    assert_eq!(zoe.orders.len(), 4);
}

#[test]
fn stock_set_through_the_admin_api_is_the_stock_of_the_storefront() {
    let (base, mut state) = rich();
    let dress = base.product_by_handle("linen-dress").unwrap();
    let variant = &dress.variants[0];
    let berlin = catalog::inventory_level_gid(variant.id, base.locations[1].id);
    let paris = catalog::inventory_level_gid(variant.id, base.locations[0].id);
    for (level, quantity) in [(&paris, 0), (&berlin, 2)] {
        let record = state.get_mut(level).unwrap();
        for entry in record["quantities"].as_array_mut().unwrap() {
            if entry["name"] == json!("available") || entry["name"] == json!("on_hand") {
                entry["quantity"] = json!(quantity);
            }
        }
    }
    catalog::refresh_product(&mut state, &catalog::product_gid(dress));
    let (store, _) = synced(&state, &base);
    let after = &store.product_by_handle("linen-dress").unwrap().variants[0];
    assert_eq!(after.inventory_quantity, 2);
    assert!(after.available);
    // In stock where there are some left.
    assert_eq!(after.store_availabilities, vec![(0, false), (1, true)]);
}

#[test]
fn a_draft_or_unpublished_product_leaves_the_storefront() {
    let (base, mut state) = rich();
    let coffee = product_gid(&base, "coffee-subscription");
    set(&mut state, &coffee, "status", json!("DRAFT"));
    let boots = product_gid(&base, "preorder-boots");
    set(&mut state, &boots, "publishedAt", Value::Null);
    let (store, overlay) = synced(&state, &base);
    assert!(overlay.removed.is_empty());
    assert!(store.product_by_handle("coffee-subscription").is_none());
    assert!(store.product_by_handle("preorder-boots").is_none());
    let unlisted: Vec<(&str, model::ProductStatus, bool)> = store
        .unlisted_products
        .iter()
        .map(|product| (product.handle.as_str(), product.status, product.published))
        .collect();
    assert_eq!(
        unlisted,
        [
            ("coffee-subscription", model::ProductStatus::Draft, true),
            ("preorder-boots", model::ProductStatus::Active, false)
        ]
    );
    // What named them leaves them out: the summer collection, the orders of the data.
    let summer = store.collection_by_handle("summer").unwrap();
    assert_eq!(summer.products.len(), 1);
    let zoe = store.customer_by_email("zoe@example.com").unwrap();
    assert_eq!(zoe.orders[0].line_items.len(), 1);
    assert!(zoe.orders[3].line_items.is_empty());
    // Its page is not found.
    let store = Arc::new(store);
    assert_eq!(
        page(&renderer(), &store, "/products/coffee-subscription").status,
        404
    );
}

#[test]
fn a_renamed_product_is_renamed_in_what_names_it() {
    let (base, mut state) = rich();
    // A change that does not show in the store, to an entity that names the product: looked at
    // first, and written all the same once the product is renamed.
    let zoe = customers::customer_gid(base.customer_by_email("zoe@example.com").unwrap());
    set(&mut state, &zoe, "numberOfOrders", json!("9"));
    let coffee = product_gid(&base, "coffee-subscription");
    set(&mut state, &coffee, "handle", json!("coffee-club"));
    let (store, overlay) = synced(&state, &base);
    // Replaced by its id, where it was.
    assert!(overlay.removed.is_empty(), "{:?}", overlay.removed);
    let product = store.product_by_handle("coffee-club").expect("renamed");
    assert_eq!(store.products[1].handle, "coffee-club");
    assert_eq!(catalog::product_gid(product), coffee);
    assert!(store.product_by_handle("coffee-subscription").is_none());
    // The collection, the selling plan group, the metafields and the orders that named it.
    let summer = store.collection_by_handle("summer").unwrap();
    assert!(
        summer
            .products
            .iter()
            .any(|index| store.products[*index].handle == "coffee-club")
    );
    assert_eq!(product.selling_plan_groups.len(), 1);
    let dress = store.product_by_handle("linen-dress").unwrap();
    assert_eq!(
        dress.metafields["custom"]["pairs_with"].value,
        json!("coffee-club")
    );
    let zoe = store.customer_by_email("zoe@example.com").unwrap();
    assert_eq!(zoe.orders[0].line_items.len(), 2);
    // Its variant keeps its id, which carts name.
    assert_eq!(
        product.variants[0].id,
        base.product_by_handle("coffee-subscription")
            .unwrap()
            .variants[0]
            .id
    );
}

#[test]
fn removals_remove_and_only_them() {
    let (base, mut state) = rich();
    // `productDelete`: the product and what belongs to it, and the collections that listed it.
    let coffee = base.product_by_handle("coffee-subscription").unwrap();
    let coffee_gid = catalog::product_gid(coffee);
    let record = state.get(&coffee_gid).unwrap().clone();
    for key in ["variants", "options", "media", "metafields"] {
        for child in record[key].as_array().unwrap() {
            state.remove(child.as_str().unwrap());
        }
    }
    for variant in &coffee.variants {
        state.remove(&catalog::inventory_item_gid(variant.id));
    }
    state.delete(&coffee_gid, "products/delete");
    let summer = catalog::collection_gid(base.collection_by_handle("summer").unwrap());
    let products = state.get_mut(&summer).unwrap().get_mut("products").unwrap();
    products
        .as_array_mut()
        .unwrap()
        .retain(|product| product != &json!(coffee_gid));
    // `customerDelete`, `metaobjectDelete`, a page deleted.
    let buyer = customers::customer_gid(base.customer_by_email("buyer@acme.example").unwrap());
    state.remove(&buyer);
    state.remove(&content::metaobject_gid("designer", "marc"));
    let about = content::page_gid(&base.pages[0]);
    state.remove(&about);
    // The menu links to the page: `menuUpdate` drops the item.
    let menu = content::menu_gid("main-menu");
    let items = state.get_mut(&menu).unwrap().get_mut("items").unwrap();
    items
        .as_array_mut()
        .unwrap()
        .retain(|item| item["title"] != json!("About"));
    // FAQ names Marc: `metaobjectUpdate` drops him.
    let faq = content::metaobject_gid("faq", "returns");
    for field in state.get_mut(&faq).unwrap()["fields"]
        .as_array_mut()
        .unwrap()
    {
        if field["key"] == json!("designers") {
            field["value"] =
                json!(json!([content::metaobject_gid("designer", "ines")]).to_string());
        }
    }

    let (store, overlay) = synced(&state, &base);
    assert_eq!(overlay.removed.products, ["coffee-subscription"]);
    assert_eq!(overlay.removed.customers, ["buyer@acme.example"]);
    assert_eq!(overlay.removed.pages, ["about"]);
    assert_eq!(overlay.removed.metaobjects["designer"], ["marc"]);
    assert!(store.product_by_handle("coffee-subscription").is_none());
    assert!(store.customer_by_email("buyer@acme.example").is_none());
    assert!(store.page_index("about").is_none());
    assert!(store.metaobject("designer", "marc").is_none());
    // Only them: the rest is there, with its ids.
    assert_eq!(store.products.len(), base.products.len() - 1);
    assert_eq!(store.customers.len(), base.customers.len() - 1);
    assert_eq!(store.metaobjects.len(), base.metaobjects.len() - 1);
    for product in &store.products {
        let old = base.product_by_handle(&product.handle).unwrap();
        assert_eq!((product.id, product.created_at), (old.id, old.created_at));
        let ids: Vec<u64> = product.variants.iter().map(|variant| variant.id).collect();
        assert_eq!(
            ids,
            old.variants
                .iter()
                .map(|variant| variant.id)
                .collect::<Vec<_>>()
        );
    }
    let zoe = store.customer_by_email("zoe@example.com").unwrap();
    assert_eq!(
        zoe.id,
        base.customer_by_email("zoe@example.com").unwrap().id
    );
    // Her order of the coffee keeps its other line.
    assert_eq!(zoe.orders[0].line_items.len(), 1);
    assert_eq!(store.companies.len(), 1);
    assert_eq!(store.gift_cards.len(), base.gift_cards.len());
}

#[test]
fn a_created_customer_can_log_in() {
    let (base, mut state) = rich();
    let id = state.new_gid("Customer");
    state.insert(object(json!({
        "__typename": "Customer", "id": id, "firstName": "Noa", "lastName": "Lee",
        "email": "noa@example.com", "phone": null, "state": "DISABLED", "tags": ["vip"],
        "taxExempt": false, "emailMarketingConsent": {"marketingState": "SUBSCRIBED"},
        "addresses": [], "addressesV2": [
            {"id": "gid://shopify/MailingAddress/77?model_name=CustomerAddress", "address1": "1 Main St",
             "city": "Lyon", "countryCodeV2": "FR", "zip": "69001", "firstName": "Noa"}
        ],
        "defaultAddress": {"id": "gid://shopify/MailingAddress/77?model_name=CustomerAddress"},
        "metafields": [], "createdAt": "2026-03-15T10:00:00Z", "updatedAt": "2026-03-15T10:00:00Z",
    })));
    set_metafield(
        &mut state,
        &id,
        "CUSTOMER",
        "custom",
        "shoe_size",
        "number_integer",
        "41",
    );
    let (store, _) = synced(&state, &base);
    // Sessions and the checkout log in by email.
    let noa = store
        .customer_named("noa@example.com")
        .unwrap()
        .expect("a customer");
    assert_eq!(customers::customer_gid(noa), id);
    assert!(noa.accepts_marketing && !noa.has_account);
    assert_eq!(noa.tags, ["vip"]);
    assert_eq!(
        (noa.addresses[0].id, noa.addresses[0].city.as_str()),
        (77, "Lyon")
    );
    assert_eq!(noa.metafields["custom"]["shoe_size"].value, json!(41));
}

#[test]
fn metafields_reach_liquid_and_the_function_inputs() {
    let (base, mut state) = rich();
    let boots = product_gid(&base, "preorder-boots");
    let dress = product_gid(&base, "linen-dress");
    set_metafield(
        &mut state,
        &boots,
        "PRODUCT",
        "custom",
        "pairs_with",
        "product_reference",
        &dress,
    );
    let hero = catalog::file_gid("lookbook/hero.jpg");
    set_metafield(
        &mut state,
        &boots,
        "PRODUCT",
        "custom",
        "lookbook",
        "file_reference",
        &hero,
    );
    set_metafield(
        &mut state,
        &boots,
        "PRODUCT",
        "custom",
        "sizes",
        "list.single_line_text_field",
        r#"["38","39"]"#,
    );
    let shop = state.shop_gid().unwrap().to_string();
    set_metafield(
        &mut state,
        &shop,
        "SHOP",
        "custom",
        "banner",
        "single_line_text_field",
        "Spring sale",
    );
    // A metafield of the shop removed: `metafieldsDelete`.
    state.remove(&content::metafield_gid(&shop, "custom", "ratio"));
    let europe = content::market_gid("europe");
    set_metafield(
        &mut state,
        &europe,
        "MARKET",
        "custom",
        "free_shipping_over",
        "number_integer",
        "8000",
    );

    let (store, overlay) = synced(&state, &base);
    assert_eq!(overlay.removed.shop_metafields, ["custom.ratio"]);
    let product = store.product_by_handle("preorder-boots").unwrap();
    assert_eq!(
        product.metafields["custom"]["pairs_with"].value,
        json!("linen-dress")
    );
    assert_eq!(
        product.metafields["custom"]["lookbook"].value,
        json!("lookbook/hero.jpg")
    );
    assert_eq!(
        store.shop.metafields["custom"]["banner"].value,
        json!("Spring sale")
    );
    assert!(!store.shop.metafields["custom"].contains_key("ratio"));
    assert_eq!(
        store.shop.metafields["custom"]["founded"].value,
        json!(2019)
    );
    let market = store
        .markets
        .iter()
        .find(|market| market.handle == "europe")
        .unwrap();
    assert_eq!(
        market.metafields["custom"]["free_shipping_over"].value,
        json!(8000)
    );
    assert_eq!(market.countries, ["FR", "DE"]);
    assert_eq!(store.countries.len(), base.countries.len());

    // Liquid reads them as it reads those of the files.
    let store = Arc::new(store);
    let renderer = renderer();
    assert_eq!(
        liquid(
            &renderer,
            &store,
            "/products/preorder-boots",
            "{{ product.metafields.custom.pairs_with.value.title }}|{{ product.metafields.custom.sizes.value | join: ',' }}|{{ shop.metafields.custom.banner }}"
        ),
        "Linen Dress|38,39|Spring sale"
    );
    // The Function inputs read the metafields of the store.
    let query = r#"query Input { cart { lines { merchandise { ... on ProductVariant { product { metafield(namespace: "custom", key: "pairs_with") { value } } } } } } shop { metafield(namespace: "custom", key: "banner") { value } } }"#;
    let input = function_input(&store, "preorder-boots", query);
    assert_eq!(
        input["cart"]["lines"][0]["merchandise"]["product"]["metafield"]["value"],
        json!("linen-dress")
    );
    assert_eq!(input["shop"]["metafield"]["value"], json!("Spring sale"));
}

/// The input a validation Function with `query` reads for a cart of one unit of a product.
fn function_input(store: &Store, handle: &str, query: &str) -> Value {
    use lsf_checkout::input::{FetchInput, InputContext, Owner, OwnerKind};
    let product = store.product_by_handle(handle).unwrap();
    let line = lsf_checkout::pipeline::order_line(store, product.variants[0].id, 1).unwrap();
    let session = lsf_core::Session::initial(store);
    let state = lsf_checkout::CheckoutState::default();
    let ctx = InputContext {
        version: lsf_checkout::version::ApiVersion::latest(),
        api: lsf_checkout::version::FunctionApi::CartCheckoutValidation,
        store,
        session: &session,
        state: &state,
        lines: &[line],
        discounts: &[],
        delivery_groups: &[],
        payment_methods: &[],
        owner: Owner {
            kind: OwnerKind::Validation,
            id: "gid://shopify/Validation/1".to_string(),
            metafields: &[],
            discount_classes: &[],
        },
        entered_codes: &[],
        triggering_code: None,
        fetch: FetchInput::Response(None),
        currency: "EUR",
        now: chrono::DateTime::parse_from_rfc3339("2026-03-15T10:00:00+00:00").unwrap(),
    };
    lsf_checkout::input::build(query, None, &ctx).expect("the input builds")
}

#[test]
fn an_app_discount_runs_its_function_with_its_metafields() {
    let (base, mut state) = rich();
    let app = support::fixture_app();
    let function = app::function_id(Some(&app), "volume-discount");
    // `discountAutomaticAppCreate` with metafields.
    let id = state.new_gid("DiscountAutomaticNode");
    state.insert(object(json!({
        "__typename": "DiscountAutomaticNode",
        "id": id,
        "automaticDiscount": {
            "__typename": "DiscountAutomaticApp",
            "title": "Spring 15",
            "discountId": id,
            "appDiscountType": {"functionId": function, "title": "volume-discount"},
            "discountClasses": ["ORDER"],
            "startsAt": "2026-03-01T00:00:00Z",
            "endsAt": null,
            "status": "ACTIVE",
        },
        "metafields": [],
    })));
    let namespace = content::admin_namespace(app::app_id(Some(&app)), "$app:config");
    set_metafield(
        &mut state,
        &id,
        "DISCOUNT",
        &namespace,
        "settings",
        "json",
        r#"{"percent":15}"#,
    );
    // One that has not started yet does not run.
    let later = state.new_gid("DiscountAutomaticNode");
    state.insert(object(json!({
        "__typename": "DiscountAutomaticNode", "id": later,
        "automaticDiscount": {
            "__typename": "DiscountAutomaticApp", "title": "Summer",
            "appDiscountType": {"functionId": function}, "discountClasses": ["ORDER"],
            "startsAt": "2026-06-01T00:00:00Z",
        },
        "metafields": [],
    })));

    let (store, _) = synced(&state, &base);
    let discounts = &store.checkout.discounts;
    assert_eq!(discounts.len(), 3);
    // The discounts of the data keep their ids; the new one has the Admin API's.
    assert_eq!(
        discounts
            .iter()
            .map(|discount| discount.id)
            .collect::<Vec<_>>(),
        [1, 2, lsf_admin::state::parse_gid(&id).unwrap().1]
    );
    let created = &discounts[2];
    assert_eq!(
        (
            created.function.as_str(),
            created.title.as_str(),
            created.code.as_deref()
        ),
        ("volume-discount", "Spring 15", None)
    );
    assert_eq!(created.classes, [model::CheckoutDiscountClass::Order]);
    assert_eq!(
        created.metafields,
        [lsf_core::store::CheckoutMetafield {
            namespace: "$app:config".to_string(),
            key: "settings".to_string(),
            kind: "json".to_string(),
            value: r#"{"percent":15}"#.to_string(),
        }]
    );
    // The discounts of the data are as they were.
    assert_eq!(
        discounts[0].metafields,
        base.checkout.discounts[0].metafields
    );
    assert_eq!(discounts[1].code.as_deref(), Some("WELCOME10"));
    assert_eq!(discounts[1].metafields[0].value, "linen-dress");
}

#[test]
fn the_settings_of_functions_follow_their_owners() {
    let (base, mut state) = rich();
    let app = support::fixture_app();
    let index = app
        .functions
        .iter()
        .position(|function| function.handle == "payment-rules")
        .unwrap();
    let owner = app::owner_gid("PaymentCustomization", index);
    let namespace = content::admin_namespace(app::app_id(Some(&app)), "$app:config");
    set_metafield(
        &mut state,
        &owner,
        "PAYMENT_CUSTOMIZATION",
        &namespace,
        "rules",
        "json",
        r#"{"hide":["card"]}"#,
    );
    let (store, overlay) = synced(&state, &base);
    let checkout = overlay.data["checkout"].as_object().unwrap();
    // Only the Function whose settings changed.
    assert_eq!(checkout.keys().collect::<Vec<_>>(), ["functions"]);
    assert_eq!(
        store.checkout.functions["payment-rules"][0].value,
        r#"{"hide":["card"]}"#
    );
    assert_eq!(
        store.checkout.functions["order-limits"],
        base.checkout.functions["order-limits"]
    );
    assert_eq!(
        store.checkout.discounts.len(),
        base.checkout.discounts.len()
    );
}

#[test]
fn content_syncs() {
    let (base, mut state) = rich();
    // A page, an article, a menu item, a location, a gift card, a company, a metaobject.
    let about = content::page_gid(&base.pages[0]);
    set(&mut state, &about, "body", json!("<p>Who we are now.</p>"));
    let spring = content::article_gid(&base.blogs[0].articles[0]);
    set(&mut state, &spring, "title", json!("Spring, again"));
    let menu = content::menu_gid("main-menu");
    state.get_mut(&menu).unwrap()["items"][0]["title"] = json!("Start");
    let paris = catalog::location_gid(&base.locations[0]);
    set(&mut state, &paris, "name", json!("Paris Rivoli"));
    let card = catalog::gift_card_gid(&base.gift_cards[0]);
    set(
        &mut state,
        &card,
        "balance",
        json!({"amount": "5.0", "currencyCode": "EUR"}),
    );
    let acme = customers::company_gid(&base.companies[0]);
    set(&mut state, &acme, "name", json!("Acme Stores"));
    let ines = content::metaobject_gid("designer", "ines");
    for field in state.get_mut(&ines).unwrap()["fields"]
        .as_array_mut()
        .unwrap()
    {
        if field["key"] == json!("name") {
            field["value"] = json!("Inès D.");
        }
    }

    let (store, overlay) = synced(&state, &base);
    assert!(overlay.removed.is_empty(), "{:?}", overlay.removed);
    assert_eq!(store.pages[0].content, "<p>Who we are now.</p>");
    assert_eq!(store.blogs[0].articles[0].title, "Spring, again");
    assert_eq!(store.blogs[0].articles[0].comments.len(), 1);
    let main = store.menu("main-menu").unwrap();
    assert_eq!(main.links[0].title, "Start");
    assert_eq!(
        main.links.len(),
        base.menu("main-menu").unwrap().links.len()
    );
    assert_eq!(main.links[2].links[0].url, "/products/linen-dress");
    // The location is renamed, and the variants it stocks name it so.
    assert_eq!(store.locations[0].name, "Paris Rivoli");
    assert_eq!(store.locations[0].id, base.locations[0].id);
    let dress = store.product_by_handle("linen-dress").unwrap();
    assert_eq!(
        dress.variants[0].store_availabilities,
        vec![(0, true), (1, true)]
    );
    assert_eq!(store.gift_cards[0].balance, 500);
    assert_eq!(store.gift_cards[0].code, "GIFT1234ABCD5678");
    // The company is renamed, and its buyer still buys for it.
    assert_eq!(store.companies[0].name, "Acme Stores");
    let buyer = store.customer_by_email("buyer@acme.example").unwrap();
    assert_eq!(buyer.company, Some(0));
    assert_eq!(buyer.company_locations, [0]);
    assert_eq!(
        store.metaobject("designer", "ines").unwrap().fields["name"].value,
        json!("Inès D.")
    );
}

#[test]
fn created_collections_metaobjects_and_markets_are_in_the_store() {
    let (base, mut state) = rich();
    // `collectionCreate` with products.
    let collection = state.new_gid("Collection");
    let dress = product_gid(&base, "linen-dress");
    let boots = product_gid(&base, "preorder-boots");
    state.insert(object(json!({
        "__typename": "Collection", "id": collection, "title": "Picks", "handle": "picks",
        "descriptionHtml": "<p>Ours.</p>", "sortOrder": "MANUAL", "products": [boots, dress],
        "createdAt": "2026-03-15T10:00:00Z", "updatedAt": "2026-03-15T10:00:00Z", "metafields": [],
    })));
    // `metaobjectCreate`, with the id lsf-core gives its type and handle; a handle changed.
    state.insert(object(json!({
        "__typename": "Metaobject", "id": content::metaobject_gid("designer", "lea"),
        "type": "designer", "handle": "lea",
        "fields": [{"key": "name", "type": "single_line_text_field", "value": "Léa"},
                   {"key": "signature_piece", "type": "product_reference", "value": boots}],
        "capabilities": {"publishable": {"status": "ACTIVE"}},
    })));
    let marc = content::metaobject_gid("designer", "marc");
    set(&mut state, &marc, "handle", json!("marc-b"));
    // `marketCreate` for Great Britain, with the id lsf-core gives its handle.
    state.insert(object(json!({
        "__typename": "Market", "id": content::market_gid("uk"), "name": "UK", "handle": "uk",
        "status": "ACTIVE", "enabled": true, "regions": [{"code": "GB"}], "metafields": [],
    })));

    let (store, overlay) = synced(&state, &base);
    assert_eq!(overlay.removed.metaobjects["designer"], ["marc"]);
    let picks = store.collection_by_handle("picks").expect("created");
    assert_eq!(catalog::collection_gid(picks), collection);
    let handles: Vec<&str> = picks
        .products
        .iter()
        .map(|index| store.products[*index].handle.as_str())
        .collect();
    assert_eq!(handles, ["preorder-boots", "linen-dress"]);
    let lea = store.metaobject("designer", "lea").expect("created");
    assert_eq!(lea.fields["signature_piece"].value, json!("preorder-boots"));
    assert!(store.metaobject("designer", "marc-b").is_some());
    // The FAQ that named Marc names him by his new handle.
    assert_eq!(
        store.metaobject("faq", "returns").unwrap().fields["designers"].value,
        json!(["designer/ines", "designer/marc-b"])
    );
    let uk = store
        .markets
        .iter()
        .find(|market| market.handle == "uk")
        .expect("created");
    assert_eq!(uk.countries, ["GB"]);
    assert_eq!(
        store.market_of("GB").map(|market| market.handle.as_str()),
        Some("uk")
    );
    assert_eq!(store.markets.len(), 2);
}

#[test]
fn a_customer_whose_email_changed_stays_where_it_was() {
    let (base, mut state) = rich();
    let zoe = customers::customer_gid(base.customer_by_email("zoe@example.com").unwrap());
    set(&mut state, &zoe, "email", json!("zoe.martin@example.com"));
    let (store, overlay) = synced(&state, &base);
    assert!(overlay.removed.is_empty());
    let position = store
        .customers
        .iter()
        .position(|customer| customer.email == "zoe.martin@example.com");
    assert_eq!(position, Some(1));
    assert_eq!(store.customers[1].id, base.customers[1].id);
    assert_eq!(store.customers[1].orders.len(), 4);
    // Her gift card names her by her new email.
    assert_eq!(store.gift_cards[0].customer, Some(store.customers[1].id));
}

#[test]
fn a_deleted_product_that_records_still_name_leaves_no_error() {
    let (base, mut state) = rich();
    // A product deleted without the collection and the group that list it being told.
    let coffee = product_gid(&base, "coffee-subscription");
    state.remove(&coffee);
    let (store, overlay) = synced(&state, &base);
    assert_eq!(overlay.removed.products, ["coffee-subscription"]);
    assert!(store.product_by_handle("coffee-subscription").is_none());
    let summer = store.collection_by_handle("summer").unwrap();
    assert_eq!(summer.products.len(), 1);
    // The metafields that named it name nothing now.
    let dress = store.product_by_handle("linen-dress").unwrap();
    assert_eq!(
        dress.metafields["custom"]["related"].value,
        json!(["gift-card"])
    );
}

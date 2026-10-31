//! The shops of the server: the default one, those created for isolation, resets; and what a
//! shop records when an order is placed in the local checkout.

mod support;

use lsf_admin::domains::{catalog, customers, orders};
use lsf_admin::state::{ShopState, Shops, gid, parse_gid};
use lsf_checkout::{Address, CheckoutResult, Cost, DeliveryGroup, DeliveryOption, ResultLine};
use serde_json::{Value, json};

#[test]
fn shops_are_seeded_from_the_same_data_and_isolated() {
    let store = support::demo_store();
    let mut shops = Shops::new(&store, None);
    let default = store.shop.permanent_domain.clone();
    assert_eq!(shops.default_domain(), default);
    assert_eq!(shops.domains().collect::<Vec<_>>(), [default.as_str()]);

    let test = shops.create("test-1").unwrap();
    assert_eq!(test.domain(), "test-1.myshopify.com");
    let shop = test.shop().unwrap().clone();
    assert_eq!(shop["myshopifyDomain"], json!("test-1.myshopify.com"));
    assert_eq!(shop["url"], json!("https://test-1.myshopify.com"));
    // A shop of its own: its id, and what pointed at the old one points at it.
    assert_ne!(shop["id"], json!(gid("Shop", store.shop.id)));
    let announcement = test.get(shop["metafields"][0].as_str().unwrap()).unwrap();
    assert_eq!(announcement["owner"], shop["id"]);
    let tee = store.product_by_handle("organic-cotton-t-shirt").unwrap();
    let product = catalog::product_gid(tee);
    assert_eq!(
        test.get(&product).unwrap()["onlineStoreUrl"],
        json!("https://test-1.myshopify.com/products/organic-cotton-t-shirt")
    );
    assert!(shops.create("test-1.myshopify.com").is_err());
    assert!(shops.create("not a shop").is_err());

    // A change in one shop is not in the other.
    let test = shops.get_mut("test-1.myshopify.com").unwrap();
    test.get_mut(&product)
        .unwrap()
        .insert("title".into(), json!("Changed"));
    test.emit("products/update", &product);
    let first_event = test.events()[0].id.clone();
    assert_eq!(
        shops.get(&default).unwrap().get(&product).unwrap()["title"],
        json!(tee.title)
    );

    // A reset puts every shop back to the seed, and keeps the shops.
    shops.reset();
    let test = shops.get_mut("test-1.myshopify.com").unwrap();
    assert_eq!(test.get(&product).unwrap()["title"], json!(tee.title));
    assert!(test.events().is_empty());
    assert_eq!(
        test.shop().unwrap()["myshopifyDomain"],
        json!("test-1.myshopify.com")
    );
    // Event ids never come back.
    let event = test.emit("products/update", &product).id.clone();
    assert_ne!(event, first_event);

    assert!(shops.remove("test-1.myshopify.com"));
    assert!(!shops.remove(&default));
    assert_eq!(shops.len(), 1);
}

#[test]
fn reseeding_follows_the_data() {
    let store = support::demo_store();
    let mut shops = Shops::new(&store, None);
    shops.create("test-2").unwrap();
    let rich = support::fixture_store("rich");
    shops.reseed(&rich, None);
    assert_eq!(shops.default_domain(), "rich-fixture.myshopify.com");
    assert_eq!(
        shops.domains().collect::<Vec<_>>(),
        ["rich-fixture.myshopify.com", "test-2.myshopify.com"]
    );
    let test = shops.get("test-2.myshopify.com").unwrap();
    assert_eq!(test.shop().unwrap()["name"], json!("Rich Fixture"));
    // A custom primary domain stays: only the myshopify domain is the shop's own.
    assert_eq!(
        test.shop().unwrap()["url"],
        json!("https://shop.rich.example")
    );
}

#[test]
fn new_ids_are_above_the_seeded_ones() {
    let mut state = ShopState::seeded(&support::demo_store(), None);
    let highest = state
        .keys("Product")
        .filter_map(|key| parse_gid(key).map(|(_, id)| id))
        .max()
        .unwrap();
    let new = state.new_gid("Product");
    assert_eq!(parse_gid(&new).unwrap().1, highest + 1);
    assert!(!state.contains(&new));
}

#[test]
fn an_order_placed_in_the_checkout_is_an_order_of_the_shop() {
    let store = support::demo_store();
    let mut state = ShopState::seeded(&store, None);
    let jane = store.customer_by_email("jane.doe@example.com").unwrap();
    let mug = store.product_by_handle("ceramic-mug").unwrap();
    let variant = &mug.variants[0];
    let result = CheckoutResult {
        lines: vec![ResultLine {
            id: "gid://shopify/CartLine/1".into(),
            variant_id: variant.id,
            product_id: mug.id,
            title: mug.title.clone(),
            variant_title: Some(variant.title.clone()),
            quantity: 2,
            unit_price: 2000,
            subtotal: 4000,
            total: 3600,
            requires_shipping: true,
            ..ResultLine::default()
        }],
        delivery_groups: vec![DeliveryGroup {
            handle: "group-1".into(),
            line_ids: vec!["gid://shopify/CartLine/1".into()],
            options: vec![DeliveryOption {
                handle: "standard".into(),
                title: "Standard".into(),
                description: None,
                kind: Default::default(),
                original_cost: 500,
                cost: 500,
            }],
            selected: Some("standard".into()),
            discount_allocations: Vec::new(),
        }],
        cost: Cost {
            subtotal: 3600,
            discounts: 400,
            shipping: 500,
            tax: 0,
            total: 4100,
            due_now: 4100,
            due_later: 0,
            checkout_charge_amount: 3600,
            currency_code: "USD".into(),
        },
        ..CheckoutResult::default()
    };
    let address = Address {
        first_name: Some("Jane".into()),
        last_name: Some("Doe".into()),
        address1: Some("1 Main St".into()),
        city: Some("Portland".into()),
        country_code: Some("us".into()),
        province_code: Some("OR".into()),
        zip: Some("97201".into()),
        ..Address::default()
    };
    let before: Vec<String> = state.keys("Order").map(str::to_string).collect();
    let order = orders::record_placed_order(
        &mut state,
        &orders::PlacedOrder {
            id: Some("gid://shopify/Order/1004"),
            number: 1004,
            processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
            result: &result,
            customer_id: Some(jane.id),
            email: Some(&jane.email),
            phone: None,
            shipping_address: Some(&address),
            note: Some("Gift wrap"),
            attributes: &[("gift".to_string(), "yes".to_string())],
            payment: Some("Credit card"),
        },
    );
    assert_eq!(order, "gid://shopify/Order/1004");
    assert!(!before.contains(&order));
    let record = state.get(&order).unwrap();
    assert_eq!(record["name"], json!("#1004"));
    assert_eq!(record["processedAt"], json!("2026-05-01T09:30:00Z"));
    assert_eq!(record["sourceName"], json!("web"));
    assert_eq!(
        record["subtotalPriceSet"]["shopMoney"]["amount"],
        json!("36.0")
    );
    assert_eq!(
        record["totalPriceSet"]["shopMoney"]["amount"],
        json!("41.0")
    );
    assert_eq!(
        record["totalDiscountsSet"]["shopMoney"]["amount"],
        json!("4.0")
    );
    assert_eq!(
        record["shippingLine"]["shippingRateHandle"],
        json!("standard")
    );
    assert_eq!(record["shippingAddress"]["countryCodeV2"], json!("US"));
    assert_eq!(
        record["customAttributes"],
        json!([{"key": "gift", "value": "yes"}])
    );
    assert_eq!(record["displayFinancialStatus"], json!("PAID"));
    assert_eq!(record["displayFulfillmentStatus"], json!("UNFULFILLED"));
    let line = state.get(record["lineItems"][0].as_str().unwrap()).unwrap();
    assert_eq!(line["variant"], json!(catalog::variant_gid(variant)));
    assert_eq!(
        line["discountedTotalSet"]["shopMoney"]["amount"],
        json!("36.0")
    );
    let sale = state
        .get(record["transactions"][0].as_str().unwrap())
        .unwrap();
    assert_eq!(sale["gateway"], json!("Credit card"));

    // The customer counts it, and the webhooks will report it.
    let customer = state.get(&customers::customer_gid(jane)).unwrap();
    assert_eq!(customer["numberOfOrders"], json!("3"));
    assert_eq!(customer["lastOrder"], json!(order));
    let topics: Vec<String> = state
        .take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect();
    assert_eq!(topics, ["orders/create", "orders/paid"]);

    // The checkout numbers the orders of each session on its own: an id that is taken gets a
    // new one.
    let again = orders::record_placed_order(
        &mut state,
        &orders::PlacedOrder {
            id: Some("gid://shopify/Order/1004"),
            number: 1004,
            processed_at: "2026-05-01T09:31:00Z".parse().unwrap(),
            result: &result,
            customer_id: None,
            email: None,
            phone: None,
            shipping_address: None,
            note: None,
            attributes: &[],
            payment: None,
        },
    );
    assert_ne!(again, order);
    assert_eq!(state.get(&again).unwrap()["customer"], Value::Null);
}

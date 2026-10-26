use lsf_core::store::load::{DataSource, LoadOptions, load};
use lsf_core::store::{
    Adjustment, CartLine, CheckoutCharge, Metafield as StoreMetafield, PriceAdjustment,
    SellingPlan, SellingPlanGroup,
};
use lsf_core::{Session, Store};
use serde_json::{Value, json};

use super::*;
use crate::{Address, DeliveryKind, DeliveryOption, KeyValue};

struct Fixture {
    store: Store,
    session: Session,
    state: CheckoutState,
    lines: Vec<ResultLine>,
    groups: Vec<DeliveryGroup>,
    methods: Vec<PaymentMethod>,
    metafields: Vec<Metafield>,
    classes: Vec<DiscountClass>,
    codes: Vec<String>,
}

impl Fixture {
    /// The demo store with a cart of two t-shirts (product 0, variant 0) and a mug, one
    /// delivery group and two payment methods.
    fn new() -> Fixture {
        let (mut store, _) = load(&DataSource::Demo, &LoadOptions::default());
        let shirt = store.products[0].clone();
        let mug = store
            .products
            .iter()
            .find(|product| product.handle == "ceramic-mug")
            .unwrap()
            .clone();
        let line = |n: u32, product: &lsf_core::store::Product, quantity: u32| {
            let variant = &product.variants[0];
            ResultLine {
                id: format!("gid://shopify/CartLine/{n}"),
                variant_id: variant.id,
                product_id: product.id,
                title: product.title.clone(),
                variant_title: Some(variant.title.clone()),
                sku: Some(variant.sku.clone()),
                quantity,
                unit_price: variant.price,
                subtotal: variant.price * i64::from(quantity),
                total: variant.price * i64::from(quantity),
                requires_shipping: true,
                ..ResultLine::default()
            }
        };
        let mut lines = vec![line(1, &shirt, 2), line(2, &mug, 1)];
        lines[0].attributes = vec![KeyValue {
            key: "engraving".to_string(),
            value: "JD".to_string(),
        }];
        store.products[0].tags = vec!["Bestseller".to_string(), "cotton".to_string()];
        let mug_index = store.product_index("ceramic-mug").unwrap();
        store.products[mug_index].tags = vec!["ceramic".to_string()];
        store.shop.metafields.insert(
            "custom".to_string(),
            [(
                "announcement".to_string(),
                StoreMetafield {
                    kind: "single_line_text_field".to_string(),
                    value: json!("Free shipping"),
                },
            )]
            .into(),
        );
        let mut session = Session::initial(&store);
        session.cart_lines = vec![
            CartLine {
                variant_id: lines[0].variant_id,
                quantity: 2,
                properties: Default::default(),
                selling_plan: None,
            },
            CartLine {
                variant_id: lines[1].variant_id,
                quantity: 1,
                properties: Default::default(),
                selling_plan: None,
            },
        ];
        session
            .cart_attributes
            .insert("gift".to_string(), "yes".to_string());
        let group = DeliveryGroup {
            handle: "group-1".to_string(),
            line_ids: lines.iter().map(|line| line.id.clone()).collect(),
            options: vec![
                DeliveryOption {
                    handle: "standard".to_string(),
                    title: "Standard".to_string(),
                    description: Some("3 to 5 days".to_string()),
                    kind: DeliveryKind::Shipping,
                    original_cost: 490,
                    cost: 490,
                },
                DeliveryOption {
                    handle: "pickup".to_string(),
                    title: "Pick up".to_string(),
                    description: None,
                    kind: DeliveryKind::PickUp,
                    original_cost: 0,
                    cost: 0,
                },
            ],
            selected: Some("standard".to_string()),
        };
        Fixture {
            store,
            session,
            state: CheckoutState {
                email: Some("buyer@example.com".to_string()),
                shipping_address: Some(Address {
                    first_name: Some("Ada".to_string()),
                    last_name: Some("Lovelace".to_string()),
                    address1: Some("1 Rue Test".to_string()),
                    city: Some("Paris".to_string()),
                    country_code: Some("fr".to_string()),
                    zip: Some("75001".to_string()),
                    ..Address::default()
                }),
                ..CheckoutState::default()
            },
            lines,
            groups: vec![group],
            methods: vec![
                PaymentMethod {
                    handle: "card".to_string(),
                    name: "Credit card".to_string(),
                },
                PaymentMethod {
                    handle: "cod".to_string(),
                    name: "Cash on Delivery".to_string(),
                },
            ],
            metafields: vec![
                Metafield {
                    namespace: "$app:config".to_string(),
                    key: "settings".to_string(),
                    kind: "json".to_string(),
                    value: r#"{"percent": 10, "tags": ["vip"]}"#.to_string(),
                },
                Metafield {
                    namespace: "custom".to_string(),
                    key: "count".to_string(),
                    kind: "number_integer".to_string(),
                    value: "7".to_string(),
                },
            ],
            classes: vec![DiscountClass::Product, DiscountClass::Order],
            codes: vec!["SAVE10".to_string()],
        }
    }

    fn run_as(
        &self,
        kind: OwnerKind,
        query: &str,
        variables: Option<&Value>,
    ) -> Result<Value, String> {
        let ctx = InputContext {
            store: &self.store,
            session: &self.session,
            state: &self.state,
            lines: &self.lines,
            delivery_groups: &self.groups,
            payment_methods: &self.methods,
            owner: Owner {
                kind,
                id: "gid://shopify/DiscountAutomaticNode/1".to_string(),
                metafields: &self.metafields,
                discount_classes: &self.classes,
            },
            entered_codes: &self.codes,
            triggering_code: Some("SAVE10"),
            currency: "EUR",
            now: chrono::DateTime::parse_from_rfc3339("2026-03-14T23:30:00+00:00").unwrap(),
        };
        build(query, variables, &ctx)
    }

    fn run(&self, query: &str) -> Value {
        self.run_as(OwnerKind::Discount, query, None)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    fn fail(&self, query: &str) -> String {
        self.run_as(OwnerKind::Discount, query, None).unwrap_err()
    }
}

#[test]
fn money_is_a_decimal_string_as_shopify_writes_it() {
    assert_eq!(decimal(1999), "19.99");
    assert_eq!(decimal(1990), "19.9");
    assert_eq!(decimal(1000), "10.0");
    assert_eq!(decimal(5), "0.05");
    assert_eq!(decimal(0), "0.0");
    assert_eq!(decimal(-250), "-2.5");
}

#[test]
fn cart_lines_carry_ids_quantities_and_costs() {
    let f = Fixture::new();
    let shirt = &f.lines[0];
    let out = f.run(
        r#"{ cart { lines { id quantity cost {
            amountPerQuantity { amount currencyCode }
            subtotalAmount { amount } totalAmount { amount }
            compareAtAmountPerQuantity { amount } } } } }"#,
    );
    let lines = out["cart"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["id"], "gid://shopify/CartLine/1");
    assert_eq!(lines[0]["quantity"], 2);
    assert_eq!(
        lines[0]["cost"]["amountPerQuantity"],
        json!({"amount": decimal(shirt.unit_price), "currencyCode": "EUR"})
    );
    assert_eq!(
        lines[0]["cost"]["subtotalAmount"]["amount"],
        decimal(shirt.subtotal)
    );
    assert_eq!(
        lines[0]["cost"]["totalAmount"]["amount"],
        decimal(shirt.subtotal)
    );
    assert_eq!(lines[0]["cost"]["compareAtAmountPerQuantity"], Value::Null);
}

#[test]
fn a_line_shows_the_compare_at_price_only_when_it_is_higher() {
    let mut f = Fixture::new();
    let price = f.lines[0].unit_price;
    f.store.products[0].variants[0].compare_at_price = Some(price + 500);
    let out = f.run("{ cart { lines { cost { compareAtAmountPerQuantity { amount } } } } }");
    assert_eq!(
        out["cart"]["lines"][0]["cost"]["compareAtAmountPerQuantity"]["amount"],
        decimal(price + 500)
    );
    assert_eq!(
        out["cart"]["lines"][1]["cost"]["compareAtAmountPerQuantity"],
        Value::Null
    );
}

#[test]
fn cart_cost_sums_the_lines() {
    let f = Fixture::new();
    let subtotal: i64 = f.lines.iter().map(|line| line.subtotal).sum();
    let out = f.run(
        "{ cart { cost { subtotalAmount { amount currencyCode } totalAmount { amount } totalTaxAmount { amount } totalDutyAmount { amount } } } }",
    );
    assert_eq!(
        out["cart"]["cost"],
        json!({
            "subtotalAmount": {"amount": decimal(subtotal), "currencyCode": "EUR"},
            "totalAmount": {"amount": decimal(subtotal)},
            "totalTaxAmount": null,
            "totalDutyAmount": null,
        })
    );
}

#[test]
fn merchandise_is_a_product_variant_with_its_product() {
    let f = Fixture::new();
    let shirt = &f.store.products[0];
    let variant = &shirt.variants[0];
    let out = f.run(
        r#"{ cart { lines { merchandise { __typename
            ... on ProductVariant { id sku title requiresShipping weight weightUnit
              product { id handle title productType vendor isGiftCard } } } } } }"#,
    );
    let merchandise = &out["cart"]["lines"][0]["merchandise"];
    assert_eq!(merchandise["__typename"], "ProductVariant");
    assert_eq!(
        merchandise["id"],
        format!("gid://shopify/ProductVariant/{}", variant.id)
    );
    assert_eq!(merchandise["sku"], variant.sku.as_str());
    assert_eq!(merchandise["title"], variant.title.as_str());
    assert_eq!(merchandise["requiresShipping"], true);
    assert_eq!(merchandise["weightUnit"], "KILOGRAMS");
    assert!(merchandise["weight"].is_f64() || merchandise["weight"].is_u64());
    assert_eq!(
        merchandise["product"]["id"],
        format!("gid://shopify/Product/{}", shirt.id)
    );
    assert_eq!(merchandise["product"]["handle"], shirt.handle.as_str());
    assert_eq!(merchandise["product"]["title"], shirt.title.as_str());
    assert_eq!(merchandise["product"]["vendor"], shirt.vendor.as_str());
    assert_eq!(merchandise["product"]["isGiftCard"], false);
}

#[test]
fn a_custom_product_fragment_does_not_match_a_variant() {
    let f = Fixture::new();
    let out = f.run(
        "{ cart { lines { merchandise { ... on CustomProduct { title } ... on ProductVariant { sku } } } } }",
    );
    let merchandise = &out["cart"]["lines"][0]["merchandise"];
    assert_eq!(
        merchandise.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["sku"]
    );
}

#[test]
fn product_tags_are_tested_never_listed() {
    let f = Fixture::new();
    let out = f.run(
        r#"{ cart { lines { merchandise { ... on ProductVariant { product {
            yes: hasAnyTag(tags: ["bestseller", "nope"])
            no: hasAnyTag(tags: ["nope"])
            each: hasTags(tags: ["COTTON", "nope"]) { tag hasTag } } } } } } }"#,
    );
    let product = &out["cart"]["lines"][0]["merchandise"]["product"];
    assert_eq!(product["yes"], true);
    assert_eq!(product["no"], false);
    assert_eq!(
        product["each"],
        json!([{"tag": "COTTON", "hasTag": true}, {"tag": "nope", "hasTag": false}])
    );
    assert!(
        f.fail("{ cart { lines { merchandise { ... on ProductVariant { product { tags } } } } } }")
            .contains("unknown field `tags` on type `Product`")
    );
}

#[test]
fn collection_membership_reads_the_collections_of_the_product() {
    let f = Fixture::new();
    let shirt = &f.store.products[0];
    let member = f.store.collections[shirt.collections[0]].id;
    let other = f
        .store
        .collections
        .iter()
        .map(|collection| collection.id)
        .find(|id| {
            shirt
                .collections
                .iter()
                .all(|index| f.store.collections[*index].id != *id)
        })
        .unwrap();
    let vars = json!({
        "in": [format!("gid://shopify/Collection/{member}")],
        "out": [format!("gid://shopify/Collection/{other}")],
        "both": [format!("gid://shopify/Collection/{other}"), format!("gid://shopify/Collection/{member}")],
    });
    let out = f
        .run_as(
            OwnerKind::Discount,
            r#"query Input($in: [ID!], $out: [ID!], $both: [ID!]) { cart { lines { merchandise {
                ... on ProductVariant { product {
                  a: inAnyCollection(ids: $in) b: inAnyCollection(ids: $out)
                  c: inCollections(ids: $both) { collectionId isMember } } } } } } }"#,
            Some(&vars),
        )
        .unwrap();
    let product = &out["cart"]["lines"][0]["merchandise"]["product"];
    assert_eq!(product["a"], true);
    assert_eq!(product["b"], false);
    assert_eq!(
        product["c"],
        json!([
            {"collectionId": format!("gid://shopify/Collection/{other}"), "isMember": false},
            {"collectionId": format!("gid://shopify/Collection/{member}"), "isMember": true},
        ])
    );
}

#[test]
fn attributes_of_the_cart_and_of_a_line() {
    let f = Fixture::new();
    let out = f.run(
        r#"{ cart {
            gift: attribute(key: "gift") { key value }
            missing: attribute(key: "nope") { value }
            lines { engraving: attribute(key: "engraving") { value } } } }"#,
    );
    assert_eq!(out["cart"]["gift"], json!({"key": "gift", "value": "yes"}));
    assert_eq!(out["cart"]["missing"], Value::Null);
    assert_eq!(out["cart"]["lines"][0]["engraving"], json!({"value": "JD"}));
    assert_eq!(out["cart"]["lines"][1]["engraving"], Value::Null);
}

#[test]
fn buyer_identity_without_a_customer() {
    let f = Fixture::new();
    let out = f.run("{ cart { buyerIdentity { email phone isAuthenticated customer { id } purchasingCompany { company { id } } } } }");
    assert_eq!(
        out["cart"]["buyerIdentity"],
        json!({"email": "buyer@example.com", "phone": null, "isAuthenticated": false, "customer": null, "purchasingCompany": null})
    );
}

#[test]
fn buyer_identity_with_a_customer() {
    let mut f = Fixture::new();
    let jane = f.store.customer_by_email("jane.doe@example.com").unwrap();
    let (id, orders) = (jane.id, jane.orders.len());
    f.session.customer_id = Some(id);
    f.state.email = None;
    f.store
        .customers
        .iter_mut()
        .find(|c| c.id == id)
        .unwrap()
        .metafields
        .insert(
            "custom".to_string(),
            [(
                "tier".to_string(),
                StoreMetafield {
                    kind: "single_line_text_field".to_string(),
                    value: json!("gold"),
                },
            )]
            .into(),
        );
    let out = f.run(
        r#"{ cart { buyerIdentity { email isAuthenticated customer {
            id email firstName lastName displayName numberOfOrders
            amountSpent { amount currencyCode }
            vip: hasAnyTag(tags: ["VIP"]) other: hasAnyTag(tags: ["wholesale"])
            tags: hasTags(tags: ["vip"]) { tag hasTag }
            tier: metafield(namespace: "custom", key: "tier") { value type } } } } }"#,
    );
    let identity = &out["cart"]["buyerIdentity"];
    // The email falls back on the customer's when nothing was entered.
    assert_eq!(identity["email"], "jane.doe@example.com");
    assert_eq!(identity["isAuthenticated"], true);
    let customer = &identity["customer"];
    assert_eq!(customer["id"], format!("gid://shopify/Customer/{id}"));
    assert_eq!(customer["firstName"], "Jane");
    assert_eq!(customer["displayName"], "Jane Doe");
    assert_eq!(customer["numberOfOrders"], orders);
    assert_eq!(customer["amountSpent"]["currencyCode"], "EUR");
    assert_eq!(customer["vip"], true);
    assert_eq!(customer["other"], false);
    assert_eq!(customer["tags"], json!([{"tag": "vip", "hasTag": true}]));
    assert_eq!(
        customer["tier"],
        json!({"value": "gold", "type": "single_line_text_field"})
    );
}

#[test]
fn a_company_customer_has_a_purchasing_company() {
    let mut f = Fixture::new();
    let alex = f
        .store
        .customer_by_email("alex.morgan@example.com")
        .unwrap();
    f.session.customer_id = Some(alex.id);
    let out = f.run(
        r#"{ cart { buyerIdentity { purchasingCompany {
            company { id name externalId } location { name externalId } } } } }"#,
    );
    let company = &out["cart"]["buyerIdentity"]["purchasingCompany"];
    assert_eq!(company["company"]["name"], "Northwind Hotels");
    assert_eq!(company["company"]["externalId"], "NW-001");
    assert!(
        company["company"]["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/Company/")
    );
    assert!(
        company["location"]["name"]
            .as_str()
            .unwrap()
            .starts_with("Northwind")
    );
}

#[test]
fn delivery_groups_options_and_address() {
    let f = Fixture::new();
    let out = f.run(
        r#"{ cart { deliveryGroups {
            id cartLines { id quantity }
            selectedDeliveryOption { handle }
            deliveryAddress { firstName lastName name address1 address2 city countryCode zip provinceCode phone company latitude }
            deliveryOptions { handle title description deliveryMethodType cost { amount currencyCode } } } } }"#,
    );
    let group = &out["cart"]["deliveryGroups"][0];
    assert_eq!(group["id"], delivery_group_id("group-1"));
    assert_eq!(
        group["cartLines"],
        json!([{"id": "gid://shopify/CartLine/1", "quantity": 2}, {"id": "gid://shopify/CartLine/2", "quantity": 1}])
    );
    assert_eq!(
        group["selectedDeliveryOption"],
        json!({"handle": "standard"})
    );
    assert_eq!(
        group["deliveryAddress"],
        json!({"firstName": "Ada", "lastName": "Lovelace", "name": "Ada Lovelace", "address1": "1 Rue Test",
               "address2": null, "city": "Paris", "countryCode": "FR", "zip": "75001", "provinceCode": null,
               "phone": null, "company": null, "latitude": null})
    );
    assert_eq!(
        group["deliveryOptions"],
        json!([
            {"handle": "standard", "title": "Standard", "description": "3 to 5 days", "deliveryMethodType": "SHIPPING", "cost": {"amount": "4.9", "currencyCode": "EUR"}},
            {"handle": "pickup", "title": "Pick up", "description": null, "deliveryMethodType": "PICK_UP", "cost": {"amount": "0.0", "currencyCode": "EUR"}},
        ])
    );
    assert_eq!(
        delivery_group_handle(group["id"].as_str().unwrap()),
        Some("group-1")
    );
}

#[test]
fn a_group_without_address_or_choice_is_null() {
    let mut f = Fixture::new();
    f.state.shipping_address = None;
    f.groups[0].selected = None;
    let out = f.run("{ cart { deliveryGroups { deliveryAddress { city } selectedDeliveryOption { handle } } } }");
    assert_eq!(
        out["cart"]["deliveryGroups"][0],
        json!({"deliveryAddress": null, "selectedDeliveryOption": null})
    );
}

#[test]
fn selling_plan_allocation_follows_the_session_cart_line() {
    let mut f = Fixture::new();
    let group = f.store.selling_plan_groups.len();
    f.store.selling_plan_groups.push(SellingPlanGroup {
        id: "abc".to_string(),
        name: "Subscribe".to_string(),
        app_id: None,
        options: vec!["Delivery".to_string()],
        plans: vec![SellingPlan {
            id: 42,
            name: "Monthly".to_string(),
            description: Some("Every month".to_string()),
            options: vec!["Monthly".to_string()],
            price_adjustments: vec![PriceAdjustment {
                adjustment: Adjustment::FixedAmount(100),
                order_count: None,
            }],
            recurring_deliveries: true,
            checkout_charge: CheckoutCharge::Percentage(100),
        }],
    });
    f.store.products[0].selling_plan_groups = vec![group];
    f.session.cart_lines[0].selling_plan = Some(42);
    let price = f.lines[0].unit_price;
    let out = f.run(
        r#"{ cart { lines { sellingPlanAllocation {
            sellingPlan { id name description recurringDeliveries }
            priceAdjustments { price { amount } perDeliveryPrice { amount } } } } } }"#,
    );
    assert_eq!(
        out["cart"]["lines"][0]["sellingPlanAllocation"],
        json!({
            "sellingPlan": {"id": "gid://shopify/SellingPlan/42", "name": "Monthly", "description": "Every month", "recurringDeliveries": true},
            "priceAdjustments": [{"price": {"amount": decimal(price - 100)}, "perDeliveryPrice": {"amount": decimal(price - 100)}}],
        })
    );
    assert_eq!(
        out["cart"]["lines"][1]["sellingPlanAllocation"],
        Value::Null
    );
}

#[test]
fn payment_methods_and_discount_codes() {
    let f = Fixture::new();
    let out = f.run(
        "{ paymentMethods { id name } enteredDiscountCodes { code rejectable } triggeringDiscountCode }",
    );
    assert_eq!(
        out,
        json!({
            "paymentMethods": [
                {"id": payment_method_id("card"), "name": "Credit card"},
                {"id": payment_method_id("cod"), "name": "Cash on Delivery"},
            ],
            "enteredDiscountCodes": [{"code": "SAVE10", "rejectable": true}],
            "triggeringDiscountCode": "SAVE10",
        })
    );
    assert_eq!(
        payment_method_handle(&payment_method_id("cod")),
        Some("cod")
    );
}

#[test]
fn localization_follows_the_session_country() {
    let mut f = Fixture::new();
    let query = "{ localization { country { isoCode } language { isoCode } market { id handle regions { __typename ... on MarketRegionCountry { code } } } } }";
    // No country selected: the shop's.
    let out = f.run(query);
    assert_eq!(out["localization"]["country"]["isoCode"], "CA");
    assert_eq!(out["localization"]["language"]["isoCode"], "EN");
    assert_eq!(
        out["localization"]["market"]["regions"],
        json!([{"__typename": "MarketRegionCountry", "code": "CA"}])
    );
    assert!(
        out["localization"]["market"]["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/Market/")
    );
    f.session.country = Some("fr".to_string());
    let out = f.run(query);
    assert_eq!(out["localization"]["country"]["isoCode"], "FR");
    assert_eq!(out["localization"]["market"]["regions"][0]["code"], "FR");
}

#[test]
fn shop_local_time_is_in_the_shop_time_zone() {
    let f = Fixture::new();
    // 23:30 UTC on 14 March 2026 is 19:30 in New York (daylight saving time).
    let out = f.run(
        r#"{ shop { localTime {
            date
            a: timeAfter(time: "19:00:00") b: timeBefore(time: "19:00:00")
            c: timeBetween(startTime: "19:00:00", endTime: "20:00:00")
            d: dateTimeAfter(dateTime: "2026-03-14T19:29:59")
            e: dateTimeBefore(dateTime: "2026-03-14T19:29:59")
            f: dateTimeBetween(startDateTime: "2026-03-14T00:00:00", endDateTime: "2026-03-14T23:59:59") } } }"#,
    );
    assert_eq!(
        out["shop"]["localTime"],
        json!({"date": "2026-03-14", "a": true, "b": false, "c": true, "d": true, "e": false, "f": true})
    );
}

#[test]
fn shop_metafield() {
    let f = Fixture::new();
    let out = f.run(r#"{ shop { metafield(namespace: "custom", key: "announcement") { value type jsonValue } } }"#);
    assert_eq!(
        out["shop"]["metafield"],
        json!({"value": "Free shipping", "type": "single_line_text_field", "jsonValue": "Free shipping"})
    );
}

#[test]
fn presentment_currency_rate_and_buyer_journey() {
    let f = Fixture::new();
    assert_eq!(
        f.run("{ presentmentCurrencyRate buyerJourney { step } }"),
        json!({"presentmentCurrencyRate": "1.0", "buyerJourney": {"step": "CHECKOUT_COMPLETION"}})
    );
}

#[test]
fn the_discount_owner_has_classes_and_metafields() {
    let f = Fixture::new();
    let out = f.run(
        r#"{ discount { discountClasses
            config: metafield(namespace: "$app:config", key: "settings") { type value jsonValue }
            absent: metafield(namespace: "$app:config", key: "nope") { value } } }"#,
    );
    let discount = &out["discount"];
    assert_eq!(discount["discountClasses"], json!(["PRODUCT", "ORDER"]));
    assert_eq!(discount["config"]["type"], "json");
    assert_eq!(
        discount["config"]["value"],
        r#"{"percent": 10, "tags": ["vip"]}"#
    );
    assert_eq!(
        discount["config"]["jsonValue"],
        json!({"percent": 10, "tags": ["vip"]})
    );
    assert_eq!(discount["absent"], Value::Null);
}

#[test]
fn the_reserved_app_namespace_has_three_spellings() {
    let mut f = Fixture::new();
    f.metafields.push(Metafield {
        namespace: "$app".to_string(),
        key: "config".to_string(),
        kind: "single_line_text_field".to_string(),
        value: "reserved".to_string(),
    });
    for query in [
        r#"{ discount { metafield(key: "config") { value } } }"#,
        r#"{ discount { metafield(namespace: "$app", key: "config") { value } } }"#,
        r#"{ discount { metafield(namespace: "app--123456", key: "config") { value } } }"#,
    ] {
        assert_eq!(
            f.run(query)["discount"]["metafield"]["value"],
            "reserved",
            "{query}"
        );
    }
    let out = f.run(
        r#"{ discount { metafield(namespace: "app--123456--config", key: "settings") { type } } }"#,
    );
    assert_eq!(out["discount"]["metafield"]["type"], "json");
}

#[test]
fn metafield_json_values_follow_the_type() {
    let mut f = Fixture::new();
    for (key, kind, value) in [
        ("n", "number_integer", "7"),
        ("b", "boolean", "true"),
        ("d", "number_decimal", "1.5"),
        ("l", "list.single_line_text_field", r#"["a","b"]"#),
        (
            "r",
            "rating",
            r#"{"value":"4.5","scale_min":"1.0","scale_max":"5.0"}"#,
        ),
        ("t", "multi_line_text_field", "two\nlines"),
    ] {
        f.metafields.push(Metafield {
            namespace: "typed".to_string(),
            key: key.to_string(),
            kind: kind.to_string(),
            value: value.to_string(),
        });
    }
    let fields: String = ["n", "b", "d", "l", "r", "t"]
        .iter()
        .map(|key| {
            format!(r#"{key}: metafield(namespace: "typed", key: "{key}") {{ jsonValue }} "#)
        })
        .collect();
    let out = f.run(&format!("{{ discount {{ {fields} }} }}"));
    let values: Vec<&Value> = ["n", "b", "d", "l", "r", "t"]
        .iter()
        .map(|k| &out["discount"][*k]["jsonValue"])
        .collect();
    assert_eq!(values[0], &json!(7));
    assert_eq!(values[1], &json!(true));
    assert_eq!(values[2], &json!("1.5"));
    assert_eq!(values[3], &json!(["a", "b"]));
    assert_eq!(
        values[4],
        &json!({"value": "4.5", "scale_min": "1.0", "scale_max": "5.0"})
    );
    assert_eq!(values[5], &json!("two\nlines"));
}

#[test]
fn product_and_variant_metafields_come_from_the_store() {
    let f = Fixture::new();
    let out = f.run(
        r#"{ cart { lines { merchandise { ... on ProductVariant {
            sku nothing: metafield(namespace: "custom", key: "nope") { value }
            product { care: metafield(namespace: "custom", key: "care") { value type }
                      rating: metafield(namespace: "reviews", key: "rating") { jsonValue } } } } } } }"#,
    );
    let merchandise = &out["cart"]["lines"][0]["merchandise"];
    assert_eq!(merchandise["nothing"], Value::Null);
    assert_eq!(
        merchandise["product"]["care"],
        json!({"value": "Machine wash cold. Tumble dry low.", "type": "single_line_text_field"})
    );
    assert_eq!(
        merchandise["product"]["rating"]["jsonValue"]["scale_max"],
        "5.0"
    );
}

#[test]
fn every_owner_kind_answers_on_its_own_root_field() {
    let f = Fixture::new();
    for kind in [
        OwnerKind::Discount,
        OwnerKind::DiscountNode,
        OwnerKind::CartTransform,
        OwnerKind::DeliveryCustomization,
        OwnerKind::PaymentCustomization,
        OwnerKind::Validation,
    ] {
        let query = format!(
            r#"{{ {} {{ __typename config: metafield(namespace: "custom", key: "count") {{ jsonValue }} }} }}"#,
            kind.field()
        );
        let out = f
            .run_as(kind, &query, None)
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        assert_eq!(out[kind.field()]["config"]["jsonValue"], 7, "{kind:?}");
    }
    let typename = |kind: OwnerKind| {
        f.run_as(
            kind,
            &format!("{{ {} {{ __typename }} }}", kind.field()),
            None,
        )
        .unwrap()[kind.field()]["__typename"]
            .clone()
    };
    assert_eq!(typename(OwnerKind::DiscountNode), "DiscountNode");
    assert_eq!(typename(OwnerKind::Validation), "Validation");
    assert_eq!(typename(OwnerKind::CartTransform), "CartTransform");
}

#[test]
fn another_functions_owner_is_an_error() {
    let f = Fixture::new();
    let error = f
        .run_as(
            OwnerKind::Validation,
            "{ discount { discountClasses } }",
            None,
        )
        .unwrap_err();
    assert!(
        error.contains("owner is the `validation` root field, not `discount`"),
        "{error}"
    );
    assert!(error.contains("at `discount`"), "{error}");
}

#[test]
fn aliases_name_the_response_keys() {
    let f = Fixture::new();
    let out = f.run(
        r#"{ basket: cart { items: lines { n: quantity id } } cart2: cart { lines { quantity } } }"#,
    );
    assert_eq!(
        out["basket"]["items"][0],
        json!({"n": 2, "id": "gid://shopify/CartLine/1"})
    );
    assert_eq!(out["cart2"]["lines"][1], json!({"quantity": 1}));
    assert!(out.get("cart").is_none());
}

#[test]
fn the_same_field_twice_with_different_arguments_needs_aliases() {
    let f = Fixture::new();
    let error =
        f.fail(r#"{ cart { attribute(key: "a") { value } attribute(key: "b") { value } } }"#);
    assert!(error.contains("conflict"), "{error}");
    assert!(error.contains("cart.attribute"), "{error}");
}

#[test]
fn named_fragments_and_inline_fragments_merge() {
    let f = Fixture::new();
    let out = f.run(
        r#"query Input {
            cart { ...CartFields lines { ...LineFields } }
            ... on Input { triggeringDiscountCode }
        }
        fragment CartFields on Cart { lines { id } }
        fragment LineFields on CartLine {
            quantity
            merchandise { __typename ...Variant }
        }
        fragment Variant on Merchandise { ... on ProductVariant { sku } }"#,
    );
    let line = &out["cart"]["lines"][0];
    assert_eq!(line["id"], "gid://shopify/CartLine/1");
    assert_eq!(line["quantity"], 2);
    assert_eq!(line["merchandise"]["__typename"], "ProductVariant");
    assert!(line["merchandise"]["sku"].is_string());
    assert_eq!(out["triggeringDiscountCode"], "SAVE10");
}

#[test]
fn typename_works_on_every_object() {
    let f = Fixture::new();
    let out = f.run(
        "{ __typename cart { __typename cost { __typename subtotalAmount { __typename } } lines { __typename cost { __typename } } } }",
    );
    assert_eq!(out["__typename"], "Input");
    assert_eq!(out["cart"]["__typename"], "Cart");
    assert_eq!(
        out["cart"]["cost"]["subtotalAmount"]["__typename"],
        "MoneyV2"
    );
    assert_eq!(out["cart"]["lines"][0]["__typename"], "CartLine");
    assert_eq!(
        out["cart"]["lines"][0]["cost"]["__typename"],
        "CartLineCost"
    );
}

#[test]
fn skip_and_include_directives() {
    let f = Fixture::new();
    let vars = json!({"yes": true, "no": false});
    let out = f
        .run_as(
            OwnerKind::Discount,
            r#"query Input($yes: Boolean!, $no: Boolean!) {
                a: triggeringDiscountCode @include(if: $yes)
                b: triggeringDiscountCode @include(if: $no)
                c: triggeringDiscountCode @skip(if: $yes)
                d: triggeringDiscountCode @skip(if: false)
                ... @include(if: $no) { e: triggeringDiscountCode }
                ...Frag @skip(if: $no)
            }
            fragment Frag on Input { g: presentmentCurrencyRate }"#,
            Some(&vars),
        )
        .unwrap();
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["a", "d", "g"]);
}

#[test]
fn variables_come_from_the_json_value_of_the_owner_metafield() {
    let f = Fixture::new();
    let vars = json!({"key": "gift", "tags": ["bestseller"], "ignored": 1});
    let out = f
        .run_as(
            OwnerKind::Discount,
            r#"query Input($key: String!, $tags: [String!]) {
                cart { attribute(key: $key) { value }
                  lines { merchandise { ... on ProductVariant { product { hasAnyTag(tags: $tags) } } } } } }"#,
            Some(&vars),
        )
        .unwrap();
    assert_eq!(out["cart"]["attribute"]["value"], "yes");
    assert_eq!(
        out["cart"]["lines"][0]["merchandise"]["product"]["hasAnyTag"],
        true
    );
    assert_eq!(
        out["cart"]["lines"][1]["merchandise"]["product"]["hasAnyTag"],
        false
    );
}

#[test]
fn variables_have_defaults_and_optional_ones_may_be_missing() {
    let f = Fixture::new();
    let query = r#"query Input($key: String = "gift", $ns: String) {
        discount { m: metafield(namespace: $ns, key: "count") { value } }
        cart { attribute(key: $key) { value } } }"#;
    // `$ns` is not given: the argument is left out and the namespace defaults to `$app`.
    let out = f.run_as(OwnerKind::Discount, query, None).unwrap();
    assert_eq!(out["cart"]["attribute"]["value"], "yes");
    assert_eq!(out["discount"]["m"], Value::Null);
    let vars = json!({"ns": "custom"});
    let out = f.run_as(OwnerKind::Discount, query, Some(&vars)).unwrap();
    assert_eq!(out["discount"]["m"]["value"], "7");
}

#[test]
fn a_required_variable_without_a_value_is_an_error() {
    let f = Fixture::new();
    let query = "query Input($key: String!) { cart { attribute(key: $key) { value } } }";
    let error = f.run_as(OwnerKind::Discount, query, None).unwrap_err();
    assert!(
        error.contains("variable `$key` of type `String!`"),
        "{error}"
    );
    let error = f
        .run_as(OwnerKind::Discount, query, Some(&json!({"key": null})))
        .unwrap_err();
    assert!(error.contains("variable `$key`"), "{error}");
    let error = f
        .run_as(OwnerKind::Discount, query, Some(&json!(["key"])))
        .unwrap_err();
    assert!(error.contains("must be a JSON object"), "{error}");
    // An undeclared variable is an error where it is used... as a missing argument.
    let error = f
        .run_as(
            OwnerKind::Discount,
            "{ cart { attribute(key: $key) { value } } }",
            Some(&json!({"key": "gift"})),
        )
        .unwrap_err();
    assert!(error.contains("missing required argument `key`"), "{error}");
}

#[test]
fn unknown_fields_name_their_path_and_type() {
    let f = Fixture::new();
    let error = f.fail(
        "{ cart { lines { merchandise { ... on ProductVariant { product { colour } } } } } }",
    );
    assert!(error.starts_with("input query: "), "{error}");
    assert!(
        error.contains("unknown field `colour` on type `Product`"),
        "{error}"
    );
    assert!(
        error.contains("at `cart.lines.merchandise.product.colour`"),
        "{error}"
    );
    let error = f.fail("{ nothing }");
    assert!(
        error.contains("unknown field `nothing` on type `Input`"),
        "{error}"
    );
    let error = f.fail("{ cart { basket: lines { weight } } }");
    assert!(error.contains("at `cart.basket.weight`"), "{error}");
    // Fields Shopify does not have in the Function input, even when the Admin API does.
    for field in ["note", "discountAllocations"] {
        assert!(
            f.fail(&format!("{{ cart {{ {field} }} }}"))
                .contains("unknown field"),
            "{field}"
        );
    }
}

#[test]
fn argument_mistakes_are_reported() {
    let f = Fixture::new();
    let error = f.fail(r#"{ cart { attribute(key: "a", other: 1) { value } } }"#);
    assert!(
        error.contains("unknown argument `other` on field `attribute` of `Cart`"),
        "{error}"
    );
    let error = f.fail("{ cart { attribute { value } } }");
    assert!(error.contains("missing required argument `key`"), "{error}");
    let error = f.fail("{ cart { attribute(key: 12) { value } } }");
    assert!(error.contains("must be a string"), "{error}");
    let error = f.fail(r#"{ shop { localTime { timeAfter(time: "noon") } } }"#);
    assert!(error.contains("must be a time"), "{error}");
}

#[test]
fn selections_must_match_the_kind_of_field() {
    let f = Fixture::new();
    let error = f.fail("{ cart { lines { quantity { x } } } }");
    assert!(
        error.contains("`quantity` of `CartLine` is a scalar and takes no selection"),
        "{error}"
    );
    let error = f.fail("{ cart { lines } }");
    assert!(
        error.contains("`lines` of `Cart` needs a selection of subfields"),
        "{error}"
    );
    let error = f.fail("{ cart { cost { subtotalAmount } } }");
    assert!(error.contains("needs a selection of subfields"), "{error}");
}

#[test]
fn fragment_mistakes_are_reported() {
    let f = Fixture::new();
    assert!(
        f.fail("{ cart { ...Missing } }")
            .contains("unknown fragment `Missing`")
    );
    assert!(
        f.fail("{ cart { ... on Nothing { lines { id } } } }")
            .contains("unknown type `Nothing`")
    );
    assert!(
        f.fail("{ ...A } fragment A on Input { ...B } fragment B on Input { ...A }")
            .contains("spreads itself")
    );
}

#[test]
fn only_a_single_query_is_accepted() {
    let f = Fixture::new();
    assert!(f.fail("{ cart {").contains("invalid GraphQL"));
    assert!(
        f.fail("mutation { cart { lines { id } } }")
            .contains("not a mutation")
    );
    assert!(
        f.fail("query A { cart { lines { id } } } query B { shop { localTime { date } } }")
            .contains("several operations")
    );
    assert!(
        f.fail("fragment A on Input { cart { lines { id } } }")
            .contains("no operation")
    );
}

#[test]
fn the_output_follows_the_order_of_the_query() {
    let f = Fixture::new();
    let out =
        f.run("{ triggeringDiscountCode cart { lines { quantity id } } presentmentCurrencyRate }");
    let keys: Vec<&str> = out
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["triggeringDiscountCode", "cart", "presentmentCurrencyRate"]
    );
    let line: Vec<&str> = out["cart"]["lines"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(line, ["quantity", "id"]);
}

#[test]
fn an_empty_cart_has_no_lines() {
    let mut f = Fixture::new();
    f.lines.clear();
    f.groups.clear();
    f.codes.clear();
    let out = f.run("{ cart { lines { id } deliveryGroups { id } cost { subtotalAmount { amount } } } enteredDiscountCodes { code } }");
    assert_eq!(
        out,
        json!({"cart": {"lines": [], "deliveryGroups": [], "cost": {"subtotalAmount": {"amount": "0.0"}}}, "enteredDiscountCodes": []})
    );
}

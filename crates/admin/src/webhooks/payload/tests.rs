use serde_json::{Value, json};

use super::*;
use crate::version::{ApiVersion, adapt};
use crate::webhooks::samples;

/// Shopify's example payloads of the topics lsf builds in a version
/// (`tools/admin/webhook_examples.mjs --version <v>`).
fn examples(version: ApiVersion) -> Map<String, Value> {
    let file: Value = serde_json::from_str(version.files().webhook_examples).unwrap();
    file["examples"].as_object().unwrap().clone()
}

/// How a payload differs from an example: keys it lacks, keys in another order, keys it adds,
/// values that differ. Arrays compare their first items for their keys, and every item for
/// their values when both have as many.
#[derive(Default, Debug)]
struct Differences {
    missing: Vec<String>,
    order: Vec<String>,
    extra: Vec<String>,
    values: Vec<String>,
}

fn compare(ours: &Value, theirs: &Value, path: &str, differences: &mut Differences) {
    match (ours, theirs) {
        (Value::Object(ours), Value::Object(theirs)) => {
            for (key, value) in theirs {
                let path = format!("{path}.{key}");
                match ours.get(key) {
                    Some(own) => compare(own, value, &path, differences),
                    None => differences.missing.push(path),
                }
            }
            for key in ours.keys().filter(|key| !theirs.contains_key(*key)) {
                differences.extra.push(format!("{path}.{key}"));
            }
            let shared_ours: Vec<&String> = ours
                .keys()
                .filter(|key| theirs.contains_key(*key))
                .collect();
            let shared_theirs: Vec<&String> = theirs
                .keys()
                .filter(|key| ours.contains_key(*key))
                .collect();
            if shared_ours != shared_theirs {
                differences.order.push(path.to_string());
            }
        }
        (Value::Array(ours), Value::Array(theirs)) => {
            if ours.len() == theirs.len() {
                for (index, (own, their)) in ours.iter().zip(theirs).enumerate() {
                    compare(own, their, &format!("{path}[{index}]"), differences);
                }
            } else if let (Some(own), Some(their)) = (ours.first(), theirs.first()) {
                // Only the keys: the items are not the same ones.
                let mut keys = Differences::default();
                compare(own, their, &format!("{path}[0]"), &mut keys);
                differences.missing.extend(keys.missing);
                differences.order.extend(keys.order);
                differences.extra.extend(keys.extra);
            }
        }
        (ours, theirs) if ours != theirs => differences
            .values
            .push(format!("{path}: {ours} instead of {theirs}")),
        _ => {}
    }
}

/// The keys lsf adds to Shopify's example, and why.
const EXTRA: &[(&str, &str)] = &[
    // The original `customers/update` payload of the same reference has them.
    ("customers/*", ".tags"),
    ("customers/*", ".email_marketing_consent"),
    ("customers/*", ".sms_marketing_consent"),
];

/// The values lsf builds differently from Shopify's example, and why.
const VALUES: &[(&str, &str)] = &[
    // The examples of an update have other values than those of the creation; the sample is the
    // record of the creation.
    ("discounts/update", ".title"),
    ("draft_orders/update", "."),
    // REST's `merchant_business_entity_id` is an encoding of the business entity GraphQL does
    // not document: lsf leaves it null.
    ("orders/*", ".merchant_business_entity_id"),
    // GraphQL's `OrderTransaction` has no `message`; the example's transactions have no
    // currency, lsf writes that of their amount, as REST's transactions do.
    ("refunds/create", ".transactions[0].message"),
    ("refunds/create", ".transactions[0].currency"),
    ("refunds/create", ".transactions[1].currency"),
    // REST names the time zone as Rails does (`Eastern Time (US & Canada)`): lsf writes its
    // IANA name. The example's shop has no IANA zone nor domain; the sample's has.
    ("app/uninstalled", ".timezone"),
    ("app/uninstalled", ".iana_timezone"),
    ("app/uninstalled", ".myshopify_domain"),
    ("shop/update", ".timezone"),
    ("shop/update", ".iana_timezone"),
    ("shop/update", ".myshopify_domain"),
    // Not in GraphQL: `plan_name` comes from the plan's flags, the email consent at checkout is
    // false.
    ("app/uninstalled", ".plan_name"),
    (
        "app/uninstalled",
        ".marketing_email_consent_enabled_at_checkout",
    ),
    ("shop/update", ".plan_name"),
    (
        "shop/update",
        ".marketing_email_consent_enabled_at_checkout",
    ),
    // `CompanyLocationTaxSettings.taxExempt` is a `Boolean!`: lsf writes it, where the example
    // has `null`.
    ("company_locations/*", ".tax_settings.tax_exempt"),
    (
        "company_contact_roles/*",
        ".company_location.tax_settings.tax_exempt",
    ),
    // The example's definition has no id (nor dates): lsf sends the id of the definition, which
    // `metafield_definitions/delete` names by its GID.
    ("metafield_definitions/create", ".id"),
    ("metafield_definitions/update", ".id"),
    // The compliance examples are of another shop and customer (`{shop}.myshopify.com`), and of
    // a data request lsf has no record of.
    // The examples of an update and a delete are of other groups than the creation's, which
    // the sample is; the app that made a group is not in GraphQL's `SellingPlanGroup`.
    ("selling_plan_groups/update", ".id"),
    ("selling_plan_groups/update", ".admin_graphql_api_id"),
    ("selling_plan_groups/delete", "."),
    ("selling_plan_groups/*", ".admin_graphql_api_app"),
    // The examples of a feed have no number in their id and an empty status; the syncs' are of
    // another shop than the other examples' (`gid://shopify/Shop/12345`). lsf computes no
    // delivery estimates of a variant.
    ("product_feeds/create", ".id"),
    ("product_feeds/create", ".status"),
    ("product_feeds/update", ".id"),
    ("product_feeds/update", ".status"),
    ("product_feeds/*", ".productFeed.shop_id"),
    (
        "product_feeds/*",
        ".product.variants.edges[0].node.deliveryOptionEstimates",
    ),
    // The example of `out_of_stock` is the same variant with no stock; the sample has some.
    ("variants/out_of_stock", ".inventory_quantity"),
    ("variants/out_of_stock", ".old_inventory_quantity"),
    ("customers/data_request", "."),
    ("customers/redact", "."),
    ("shop/redact", "."),
];

fn allowed(list: &[(&str, &str)], topic: &str, path: &str) -> bool {
    list.iter().any(|(topics, prefix)| {
        let topic_matches = match topics.strip_suffix("/*") {
            Some(resource) => topic.split('/').next() == Some(resource),
            None => *topics == topic,
        };
        topic_matches && path.starts_with(prefix)
    })
}

/// In every version lsf knows, each payload it builds (with the version's adapters) has the keys
/// of Shopify's example of the version, in its order. Values are compared with the examples of
/// the default version, which the samples are made from: the examples of another version are of
/// other records (other ids, other draft orders).
#[test]
fn every_payload_has_the_keys_of_shopifys_example_in_its_order() {
    let mut failures = Vec::new();
    for version in ApiVersion::all() {
        let examples = examples(version);
        for topic in BUILT_TOPICS {
            let example = examples
                .get(*topic)
                .unwrap_or_else(|| panic!("no example for {topic} in {version}"));
            let sample = samples::fixed(topic).unwrap();
            let built = adapt::payload(version.adapters(), topic, &sample.payload(topic).unwrap());
            let mut differences = Differences::default();
            compare(&built, example, "", &mut differences);
            for missing in differences.missing {
                failures.push(format!("{version} {topic}: no {missing}"));
            }
            for order in differences.order {
                failures.push(format!(
                    "{version} {topic}: the keys of {order} are in another order"
                ));
            }
            for extra in differences.extra {
                if !allowed(EXTRA, topic, &extra) {
                    failures.push(format!(
                        "{version} {topic}: {extra} is not in Shopify's example"
                    ));
                }
            }
            if version != ApiVersion::default_version() {
                continue;
            }
            for value in differences.values {
                if !allowed(VALUES, topic, &value) {
                    failures.push(format!("{version} {topic}: {value}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn record(value: Value) -> Record {
    value.as_object().unwrap().clone()
}

#[test]
fn a_product_from_a_record_of_the_state() {
    // As the state holds it: the variants by GID, prices as `Money` strings.
    let product = record(json!({
        "__typename": "Product",
        "id": "gid://shopify/Product/1",
        "title": "Organic cotton t-shirt",
        "handle": "organic-cotton-t-shirt",
        "descriptionHtml": "<p>Soft.</p>",
        "status": "DRAFT",
        "tags": ["cotton", "summer"],
        "vendor": "Acme",
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": "2026-01-02T00:00:00Z",
        "variants": ["gid://shopify/ProductVariant/11", "gid://shopify/ProductVariant/12"],
        "options": [{"id": "gid://shopify/ProductOption/5", "name": "Size", "position": 1, "values": ["S", "M"]}],
        "media": [{
            "__typename": "MediaImage",
            "id": "gid://shopify/MediaImage/7",
            "alt": "Front",
            "mediaContentType": "IMAGE",
            "image": {"url": "https://cdn.example/front.jpg", "width": 800, "height": 600},
        }],
    }));
    let variant = |id: u64, size: &str, price: &str, updated: &str| {
        record(json!({
            "id": format!("gid://shopify/ProductVariant/{id}"),
            "title": size,
            "price": price,
            "compareAtPrice": null,
            "sku": format!("TEE-{size}"),
            "selectedOptions": [{"name": "Size", "value": size}],
            "inventoryItem": format!("gid://shopify/InventoryItem/{id}"),
            "inventoryQuantity": 3,
            "inventoryPolicy": "CONTINUE",
            "image": "gid://shopify/MediaImage/7",
            "updatedAt": updated,
        }))
    };
    let lookup = Records::of(
        [
            variant(11, "S", "19.0", "2026-01-03T00:00:00Z"),
            variant(12, "M", "21.5", "2026-01-04T00:00:00Z"),
        ],
        None,
    );
    let payload = payload("products/update", &product, &lookup).unwrap();
    assert_eq!(payload["id"], json!(1));
    assert_eq!(payload["admin_graphql_api_id"], "gid://shopify/Product/1");
    assert_eq!(payload["status"], "draft");
    assert_eq!(payload["tags"], "cotton, summer");
    assert_eq!(payload["body_html"], "<p>Soft.</p>");
    assert_eq!(payload["product_type"], "");
    let variants = payload["variants"].as_array().unwrap();
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0]["price"], "19.00");
    assert_eq!(variants[1]["price"], "21.50");
    assert_eq!(variants[0]["option1"], "S");
    assert_eq!(variants[0]["option2"], Value::Null);
    assert_eq!(variants[0]["product_id"], json!(1));
    assert_eq!(variants[0]["inventory_item_id"], json!(11));
    assert_eq!(variants[0]["inventory_policy"], "continue");
    assert_eq!(variants[1]["position"], json!(2));
    assert_eq!(variants[0]["image_id"], json!(7));
    assert_eq!(payload["options"][0]["values"], json!(["S", "M"]));
    assert_eq!(payload["options"][0]["product_id"], json!(1));
    assert_eq!(payload["images"][0]["src"], "https://cdn.example/front.jpg");
    assert_eq!(payload["images"][0]["variant_ids"], json!([11, 12]));
    assert_eq!(payload["image"], payload["images"][0]);
    assert_eq!(payload["media"][0]["media_content_type"], "IMAGE");
    // The most recently updated variant first, its date in milliseconds.
    assert_eq!(
        payload["variant_gids"],
        json!([
            {"admin_graphql_api_id": "gid://shopify/ProductVariant/12", "updated_at": "2026-01-04T00:00:00.000Z"},
            {"admin_graphql_api_id": "gid://shopify/ProductVariant/11", "updated_at": "2026-01-03T00:00:00.000Z"},
        ])
    );
}

#[test]
fn references_may_be_gids_embedded_objects_or_connections() {
    let order = |line_items: Value| {
        record(json!({
            "id": "gid://shopify/Order/9",
            "currencyCode": "EUR",
            "lineItems": line_items,
            "totalPriceSet": {"shopMoney": {"amount": "12.5", "currencyCode": "EUR"}},
        }))
    };
    let line = json!({
        "id": "gid://shopify/LineItem/3",
        "title": "Tee",
        "quantity": 2,
        "originalUnitPriceSet": {"shopMoney": {"amount": "6.25", "currencyCode": "EUR"}},
    });
    let lookup = Records::of([record(line.clone())], None);
    let by_gid = payload(
        "orders/create",
        &order(json!(["gid://shopify/LineItem/3"])),
        &lookup,
    )
    .unwrap();
    let embedded = payload("orders/create", &order(json!([line.clone()])), &lookup).unwrap();
    let nodes = payload(
        "orders/create",
        &order(json!({"nodes": [line.clone()]})),
        &lookup,
    )
    .unwrap();
    let edges = payload(
        "orders/create",
        &order(json!({"edges": [{"node": line}]})),
        &lookup,
    )
    .unwrap();
    assert_eq!(by_gid, embedded);
    assert_eq!(by_gid, nodes);
    assert_eq!(by_gid, edges);
    assert_eq!(by_gid["line_items"][0]["price"], "6.25");
    assert_eq!(by_gid["total_price"], "12.50");
    // The line items' original total, computed: 2 × 6.25.
    assert_eq!(by_gid["total_line_items_price"], "12.50");
    assert_eq!(
        by_gid["total_line_items_price_set"]["shop_money"]["currency_code"],
        "EUR"
    );
    // What the order does not say is a zero in its currency, never a made-up amount.
    assert_eq!(by_gid["total_tax"], "0.00");
    assert_eq!(
        by_gid["total_tax_set"]["presentment_money"]["currency_code"],
        "EUR"
    );
    // A GID that names nothing is left out.
    let missing = payload(
        "orders/create",
        &order(json!(["gid://shopify/LineItem/404"])),
        &lookup,
    )
    .unwrap();
    assert_eq!(missing["line_items"], json!([]));
}

#[test]
fn an_order_with_a_discount_code_lists_what_it_took_off() {
    let order = record(json!({
        "id": "gid://shopify/Order/1",
        "number": 1001,
        "currencyCode": "EUR",
        "displayFinancialStatus": "PAID",
        "displayFulfillmentStatus": "PARTIALLY_FULFILLED",
        "discountApplications": [{
            "__typename": "DiscountCodeApplication",
            "index": 0,
            "code": "SUMMER10",
            "value": {"percentage": 10.0},
            "allocationMethod": "ACROSS",
            "targetSelection": "ALL",
            "targetType": "LINE_ITEM",
        }],
        "lineItems": [{
            "id": "gid://shopify/LineItem/1",
            "quantity": 1,
            "originalUnitPriceSet": {"shopMoney": {"amount": "20.0", "currencyCode": "EUR"}},
            "discountAllocations": [{
                "allocatedAmountSet": {"shopMoney": {"amount": "2.0", "currencyCode": "EUR"}},
                "discountApplication": {"index": 0},
            }],
        }],
    }));
    let payload = payload("orders/paid", &order, &Records::default()).unwrap();
    assert_eq!(
        payload["discount_codes"],
        json!([{"code": "SUMMER10", "amount": "2.00", "type": "percentage"}])
    );
    assert_eq!(
        payload["discount_applications"][0],
        json!({
            "target_type": "line_item",
            "type": "discount_code",
            "value": "10.0",
            "value_type": "percentage",
            "allocation_method": "across",
            "target_selection": "all",
            "code": "SUMMER10",
        })
    );
    assert_eq!(
        payload["line_items"][0]["discount_allocations"][0],
        json!({
            "amount": "2.00",
            "discount_application_index": 0,
            "amount_set": {
                "shop_money": {"amount": "2.00", "currency_code": "EUR"},
                "presentment_money": {"amount": "2.00", "currency_code": "EUR"},
            },
        })
    );
    assert_eq!(payload["financial_status"], "paid");
    assert_eq!(payload["fulfillment_status"], "partial");
    assert_eq!(
        (payload["number"].clone(), payload["order_number"].clone()),
        (json!(1), json!(1001))
    );
}

#[test]
fn deletes_send_what_shopify_sends() {
    let product = record(json!({"id": "gid://shopify/Product/5", "title": "Gone"}));
    assert_eq!(
        payload("products/delete", &product, &Records::default()),
        Some(json!({"id": 5}))
    );
    let collection = record(json!({"id": "gid://shopify/Collection/6", "title": "Gone"}));
    assert_eq!(
        payload("collections/delete", &collection, &Records::default()),
        Some(
            json!({"id": 6, "published_scope": "web", "admin_graphql_api_id": "gid://shopify/Collection/6"})
        )
    );
    let discount = record(
        json!({"id": "gid://shopify/DiscountCodeNode/2", DELETED_AT: "2026-02-01T00:00:00Z"}),
    );
    assert_eq!(
        payload("discounts/delete", &discount, &Records::default()),
        Some(
            json!({"admin_graphql_api_id": "gid://shopify/DiscountCodeNode/2", "deleted_at": "2026-02-01T00:00:00Z"})
        )
    );
    let level = record(json!({
        "id": "gid://shopify/InventoryLevel/3?inventory_item_id=4",
        "item": "gid://shopify/InventoryItem/4",
        "location": {"id": "gid://shopify/Location/3"},
    }));
    assert_eq!(
        payload("inventory_levels/disconnect", &level, &Records::default()),
        Some(json!({"inventory_item_id": 4, "location_id": 3}))
    );
}

#[test]
fn a_topic_without_a_builder_has_no_payload() {
    let record = record(json!({"id": "gid://shopify/Cart/1"}));
    assert_eq!(payload("carts/create", &record, &Records::default()), None);
    assert!(!is_built("carts/create"));
    assert!(is_built("orders/create"));
    // Every built topic is a topic of the default version or a compliance topic.
    for topic in BUILT_TOPICS {
        assert!(
            crate::webhooks::topics::written_form(topic) == Some(*topic),
            "{topic}"
        );
    }
}

#[test]
fn include_fields_keeps_only_the_fields_named() {
    let payload = json!({
        "id": 9554194432293u64,
        "title": "T-Shirt",
        "status": "active",
        "updated_at": "2025-04-22T14:30:00-05:00",
        "variants": [
            {"id": 123456789, "title": "Default Title", "price": "29.99", "sku": "TSHIRT-001"},
            {"id": 123456790, "title": "Large", "price": "31.99", "sku": "TSHIRT-002"},
        ],
        "tags": "cotton, comfortable",
    });
    let fields = |list: &[&str]| {
        list.iter()
            .map(|field| field.to_string())
            .collect::<Vec<_>>()
    };
    // The example of Shopify's delivery structure guide.
    assert_eq!(
        include_fields(
            &payload,
            &fields(&["id", "variants.id", "variants.price", "updated_at"])
        ),
        json!({
            "id": 9554194432293u64,
            "updated_at": "2025-04-22T14:30:00-05:00",
            "variants": [{"id": 123456789, "price": "29.99"}, {"id": 123456790, "price": "31.99"}],
        })
    );
    // A field named whole after one of its fields keeps all of it; a field the payload does not
    // have is not added.
    assert_eq!(
        include_fields(
            &payload,
            &fields(&["variants.id", "variants", "missing", "title.x"])
        ),
        json!({"title": "T-Shirt", "variants": payload["variants"]})
    );
    assert_eq!(include_fields(&payload, &[]), payload);
}

#[test]
fn prices_and_enums_are_written_as_rest_writes_them() {
    assert_eq!(price(&json!("19")), json!("19.00"));
    assert_eq!(price(&json!("19.9")), json!("19.90"));
    assert_eq!(price(&json!("0.125")), json!("0.125"));
    assert_eq!(price(&json!(5)), json!("5.00"));
    assert_eq!(price(&Value::Null), Value::Null);
    assert_eq!(tags(&json!(["a", "b c"])), json!("a, b c"));
    assert_eq!(tags(&Value::Null), json!(""));
    assert_eq!(lower(&json!("PARTIALLY_PAID")), json!("partially_paid"));
    assert_eq!(
        grams(&json!({"unit": "KILOGRAMS", "value": 0.2})),
        json!(200)
    );
    assert_eq!(grams(&json!({"unit": "POUNDS", "value": 1.0})), json!(454));
}

#[test]
fn the_compliance_payloads_name_the_shop() {
    let shop =
        record(json!({"id": "gid://shopify/Shop/7", "myshopifyDomain": "my-shop.myshopify.com"}));
    let customer = record(json!({
        "id": "gid://shopify/Customer/3",
        "email": "jane@example.com",
        "orders": ["gid://shopify/Order/10", {"id": "gid://shopify/Order/11"}],
    }));
    let lookup = Records::of([], Some(shop.clone()));
    assert_eq!(
        payload("customers/redact", &customer, &lookup),
        Some(json!({
            "shop_id": 7,
            "shop_domain": "my-shop.myshopify.com",
            "customer": {"id": 3, "email": "jane@example.com", "phone": null},
            "orders_to_redact": [10, 11],
        }))
    );
    assert_eq!(
        payload("shop/redact", &shop, &Records::default()),
        Some(json!({"shop_id": 7, "shop_domain": "my-shop.myshopify.com"}))
    );
    let request = payload("customers/data_request", &customer, &lookup).unwrap();
    assert_eq!(request["orders_requested"], json!([10, 11]));
    assert_eq!(request["data_request"], json!({"id": 3}));
}

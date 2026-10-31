//! The records `lsf app webhook trigger` builds a payload from when it has no shop to read them
//! from: one record per family of topics, in the shape of its GraphQL type, with the values of
//! Shopify's example payload of the topic (the T-shirt of `products/create`, the order `#9999` of
//! `orders/create`, …), so that the payload is the example's for the keys they share.
//!
//! [`Samples`] is the hook: the server, which has the state of the shop seeded from the store
//! data, gives records of the store instead of [`FixedSamples`].

use serde_json::{Value, json};

use super::payload::{self, DELETED_AT, PREVIOUS_ACCESS_SCOPES, Records};
use crate::state::Record;

/// The record a webhook is about, and what it refers to.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub record: Record,
    pub lookup: Records,
}

impl Sample {
    /// The payload of `topic` for this sample.
    pub fn payload(&self, topic: &str) -> Option<Value> {
        payload::payload(topic, &self.record, &self.lookup)
    }
}

/// Where the records of a triggered webhook come from.
pub trait Samples {
    /// The domain of the shop: `X-Shopify-Shop-Domain`.
    fn shop(&self) -> String;

    /// The record a webhook of `topic` is about, `None` when there is none for it.
    fn sample(&self, topic: &str) -> Option<Sample>;
}

/// The built-in records, from Shopify's example payloads.
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedSamples;

/// The domain Shopify's sample webhooks come from.
pub const SAMPLE_SHOP: &str = "shop.myshopify.com";

impl Samples for FixedSamples {
    fn shop(&self) -> String {
        SAMPLE_SHOP.to_string()
    }

    fn sample(&self, topic: &str) -> Option<Sample> {
        fixed(topic)
    }
}

/// The built-in sample of a topic.
pub fn fixed(topic: &str) -> Option<Sample> {
    if !payload::is_built(topic) {
        return None;
    }
    let (resource, _) = topic.split_once('/').unwrap_or((topic, ""));
    let the_shop = Some(record(shop()));
    let (mut main, others): (Value, Vec<Value>) = match resource {
        "products" => (product(), variants()),
        "collections" => (collection(), Vec::new()),
        "customers" => (customer(), Vec::new()),
        "orders" if topic == "orders/edited" => (order_edit(), Vec::new()),
        "orders" => (order(), order_parts(false)),
        "order_transactions" => (order_transaction(), vec![order()]),
        "fulfillment_events" => (fulfillment_event(), vec![fulfillment()]),
        "fulfillment_holds" => (fulfillment_hold(topic), Vec::new()),
        "fulfillment_orders" => fulfillment_order_sample(topic),
        "returns" => (return_sample(topic), Vec::new()),
        "fulfillments" => (fulfillment(), [vec![order()], order_parts(true)].concat()),
        "refunds" => (refund(), [vec![order()], order_parts(true)].concat()),
        "draft_orders" => (draft_order(), Vec::new()),
        "inventory_items" => (inventory_item(), Vec::new()),
        "inventory_levels" => (inventory_level(), Vec::new()),
        "locations" => (location(), Vec::new()),
        "app" if topic == "app/scopes_update" => (installation(), Vec::new()),
        "app" | "shop" => (shop(), Vec::new()),
        "app_subscriptions" => (app_subscription(), Vec::new()),
        "bulk_operations" => (bulk_operation(), Vec::new()),
        "metaobjects" => (metaobject(), Vec::new()),
        "discounts" => (discount(), Vec::new()),
        _ => return None,
    };
    if topic.ends_with("/delete") {
        main = main.with(DELETED_AT, json!("2018-08-29T08:00:00-04:00"));
    }
    let main = record(main);
    let lookup = Records::of(
        std::iter::once(main.clone()).chain(others.into_iter().map(record)),
        the_shop,
    );
    Some(Sample {
        record: main,
        lookup,
    })
}

/// `value.with(key, field)`: the object with one more field.
trait With {
    fn with(self, key: &str, field: Value) -> Value;

    /// The object with the fields of `other` after its own.
    fn merged(self, other: Value) -> Value;
}

impl With for Value {
    fn with(mut self, key: &str, field: Value) -> Value {
        self[key] = field;
        self
    }

    fn merged(mut self, other: Value) -> Value {
        if let (Value::Object(object), Value::Object(other)) = (&mut self, other) {
            object.extend(other);
        }
        self
    }
}

fn record(value: Value) -> Record {
    match value {
        Value::Object(record) => record,
        _ => Record::new(),
    }
}

fn money(amount: &str) -> Value {
    json!({"amount": amount, "currencyCode": "USD"})
}

fn bag(amount: &str) -> Value {
    json!({"shopMoney": money(amount), "presentmentMoney": money(amount)})
}

fn product() -> Value {
    json!({
        "__typename": "Product",
        "id": "gid://shopify/Product/788032119674292922",
        "title": "Example T-Shirt",
        "handle": "example-t-shirt",
        "descriptionHtml": "An example T-Shirt",
        "productType": "Shirts",
        "vendor": "Acme",
        "status": "ACTIVE",
        "tags": ["example", "mens", "t-shirt"],
        "createdAt": null,
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "publishedAt": "2021-12-31T19:00:00-05:00",
        "templateSuffix": null,
        "variants": [
            "gid://shopify/ProductVariant/642667041472713922",
            "gid://shopify/ProductVariant/757650484644203962",
        ],
        "options": [],
        "media": [],
        "hasVariantsThatRequiresComponents": false,
        "category": null,
    })
}

fn variants() -> Vec<Value> {
    [
        (
            "642667041472713922",
            "Small",
            1,
            75,
            "2021-12-30T19:00:00-05:00",
        ),
        (
            "757650484644203962",
            "Medium",
            2,
            50,
            "2021-12-31T19:00:00-05:00",
        ),
    ]
    .into_iter()
    .map(|(id, title, position, quantity, updated)| {
        json!({
            "__typename": "ProductVariant",
            "id": format!("gid://shopify/ProductVariant/{id}"),
            "title": title,
            "price": "19.99",
            "compareAtPrice": "24.99",
            "sku": null,
            "barcode": null,
            "position": position,
            "inventoryPolicy": "DENY",
            "taxable": true,
            "inventoryQuantity": quantity,
            "selectedOptions": [{"name": "Title", "value": title}],
            "inventoryItem": null,
            "createdAt": "2021-12-29T19:00:00-05:00",
            "updatedAt": updated,
            "product": "gid://shopify/Product/788032119674292922",
        })
    })
    .collect()
}

fn collection() -> Value {
    json!({
        "__typename": "Collection",
        "id": "gid://shopify/Collection/408372092144951419",
        "title": "My New Collection",
        "handle": "mynewcollection",
        "descriptionHtml": "<b>Some HTML</b>",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "publishedAt": "2021-12-31T16:00:00-05:00",
        "sortOrder": null,
        "templateSuffix": null,
    })
}

fn customer() -> Value {
    json!({
        "__typename": "Customer",
        "id": "gid://shopify/Customer/706405506930370084",
        "firstName": "Bob",
        "lastName": "Biller",
        "email": "bob@biller.com",
        "phone": null,
        "state": "DISABLED",
        "note": "This customer loves ice cream",
        "verifiedEmail": true,
        "multipassIdentifier": null,
        "taxExempt": false,
        "taxExemptions": [],
        "tags": [],
        "amountSpent": money("0.0"),
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "addressesV2": [],
        "defaultAddress": {
            "id": "gid://shopify/MailingAddress/12321",
            "firstName": "Bob",
            "lastName": "Biller",
            "company": null,
            "address1": "151 O'Connor Street",
            "address2": null,
            "city": "Ottawa",
            "province": "ON",
            "provinceCode": "ON",
            "country": "CA",
            "countryCodeV2": "CA",
            "zip": "K2P 2L8",
            "phone": "555-555-5555",
            "name": "Bob Biller",
        },
        "emailMarketingConsent": null,
        "smsMarketingConsent": null,
        "orders": [
            "gid://shopify/Order/299938",
            "gid://shopify/Order/280263",
            "gid://shopify/Order/220458",
        ],
    })
}

fn shipping_address() -> Value {
    json!({
        "firstName": "Steve",
        "lastName": "Shipper",
        "address1": "123 Shipping Street",
        "address2": null,
        "phone": "555-555-SHIP",
        "city": "Shippington",
        "zip": "40003",
        "province": "Kentucky",
        "provinceCode": "KY",
        "country": "United States",
        "countryCodeV2": "US",
        "company": "Shipping Company",
        "latitude": null,
        "longitude": null,
        "name": "Steve Shipper",
    })
}

fn order() -> Value {
    json!({
        "__typename": "Order",
        "id": "gid://shopify/Order/820982911946154508",
        "name": "#9999",
        "number": 1234,
        "email": "jon@example.com",
        "phone": null,
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "processedAt": "2021-12-31T19:00:00-05:00",
        "cancelledAt": "2021-12-31T19:00:00-05:00",
        "cancelReason": "CUSTOMER",
        "closedAt": null,
        "confirmed": false,
        "test": true,
        "currencyCode": "USD",
        "presentmentCurrencyCode": "USD",
        "customerLocale": "en",
        "customerAcceptsMarketing": true,
        "displayFinancialStatus": "VOIDED",
        "displayFulfillmentStatus": "UNFULFILLED",
        "statusPageUrl": "https://jsmith.myshopify.com/548380009/orders/123456abcd/authenticate?key=abcdefg",
        "tags": ["tag1", "tag2"],
        "note": null,
        "customAttributes": [],
    })
    // In parts: one `json!` of every field is deeper than the macro may go.
    .merged(json!({
        "paymentGatewayNames": ["visa", "bogus"],
        "sourceName": "web",
        "taxesIncluded": false,
        "taxExempt": false,
        "taxLines": [],
        "dutiesIncluded": false,
        "estimatedTaxes": false,
        "totalWeight": 0,
        "subtotalPriceSet": bag("404.95"),
        "currentSubtotalPriceSet": bag("414.95"),
        "totalPriceSet": bag("404.95"),
        "currentTotalPriceSet": bag("414.95"),
        "totalDiscountsSet": bag("20.00"),
        "currentTotalDiscountsSet": bag("0.00"),
        "totalTaxSet": bag("0.00"),
        "currentTotalTaxSet": bag("0.00"),
        "totalShippingPriceSet": bag("10.00"),
        "currentShippingPriceSet": bag("0.00"),
        "totalOutstandingSet": bag("414.95"),
        "totalTipReceivedSet": bag("0.00"),
    }))
    .merged(json!({
        "customer": {
            "id": "gid://shopify/Customer/115310627314723954",
            "firstName": "John",
            "lastName": "Smith",
            "email": "john@example.com",
            "phone": null,
            "state": "DISABLED",
            "note": null,
            "verifiedEmail": true,
            "multipassIdentifier": null,
            "taxExempt": false,
            "taxExemptions": [],
            "createdAt": null,
            "updatedAt": null,
            "amountSpent": money("0.0"),
            "defaultAddress": {
                "id": "gid://shopify/MailingAddress/715243470612851245",
                "firstName": "John",
                "lastName": "Smith",
                "company": null,
                "address1": "123 Elm St.",
                "address2": null,
                "city": "Ottawa",
                "province": "Ontario",
                "provinceCode": "ON",
                "country": "Canada",
                "countryCodeV2": "CA",
                "zip": "K2H7A8",
                "phone": "123-123-1234",
                "name": "John Smith",
            },
        },
        "billingAddress": shipping_address(),
        "shippingAddress": shipping_address(),
        "lineItems": line_item_gids(),
        "shippingLines": [{
            "id": "gid://shopify/ShippingLine/271878346596884015",
            "title": "Generic Shipping",
            "code": null,
            "source": "shopify",
            "carrierIdentifier": null,
            "phone": null,
            "isRemoved": false,
            "originalPriceSet": bag("10.00"),
            "discountedPriceSet": bag("0.00"),
            "currentDiscountedPriceSet": bag("0.00"),
            "taxLines": [],
            "discountAllocations": [],
        }],
        "discountApplications": [],
        "fulfillments": [],
        "refunds": [],
    }))
}

/// A line item of the example order.
struct Line {
    id: &'static str,
    title: &'static str,
    sku: &'static str,
    price: &'static str,
    grams: u32,
    /// Whether its product still exists.
    exists: bool,
    /// Its line item group (a bundle).
    group: Option<&'static str>,
}

/// The line items of the example order: the five of `orders/create`. In the examples of
/// `fulfillments/create` and `refunds/create`, the fourth has two discounts of 5.00.
const LINE_ITEMS: [Line; 5] = [
    Line {
        id: "487817672276298554",
        title: "Aviator sunglasses",
        sku: "SKU2006-001",
        price: "89.99",
        grams: 100,
        exists: true,
        group: None,
    },
    Line {
        id: "789012345678901234",
        title: "Lens Protection Plan (2 Year)",
        sku: "LENS-PROTECT-2YR",
        price: "19.99",
        grams: 0,
        exists: false,
        group: Some("234567890123456789"),
    },
    Line {
        id: "890123456789012345",
        title: "Premium Leather Case",
        sku: "CASE-LEATHER-PREM",
        price: "24.99",
        grams: 0,
        exists: false,
        group: Some("234567890123456789"),
    },
    Line {
        id: "976318377106520349",
        title: "Mid-century lounger",
        sku: "SKU2006-020",
        price: "159.99",
        grams: 1000,
        exists: true,
        group: Some("142831562"),
    },
    Line {
        id: "315789986012684393",
        title: "Coffee table",
        sku: "SKU2006-035",
        price: "119.99",
        grams: 500,
        exists: true,
        group: Some("142831562"),
    },
];

fn order_parts(discounted: bool) -> Vec<Value> {
    LINE_ITEMS
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let Line {
                id,
                title,
                sku,
                price,
                grams,
                exists,
                group,
            } = line;
            let allocation = |application: u32| {
                json!({"allocatedAmountSet": bag("5.00"), "discountApplication": {"index": application}})
            };
            let discounts = if discounted && index == 3 {
                vec![allocation(0), allocation(2)]
            } else {
                Vec::new()
            };
            json!({
                "__typename": "LineItem",
                "id": format!("gid://shopify/LineItem/{id}"),
                "name": title,
                "title": title,
                "sku": sku,
                "quantity": 1,
                "currentQuantity": 1,
                "unfulfilledQuantity": 1,
                "variantTitle": null,
                "vendor": null,
                "variant": null,
                "product": if exists { json!({"id": "gid://shopify/Product/788032119674292922"}) } else { Value::Null },
                "staffMember": if index == 0 { json!("gid://shopify/StaffMember/902541635") } else { Value::Null },
                "lineItemGroup": group.map(|group| json!({"id": format!("gid://shopify/LineItemGroup/{group}")})),
                "requiresShipping": true,
                "taxable": true,
                "isGiftCard": false,
                "customAttributes": [],
                "originalUnitPriceSet": bag(price),
                "totalDiscountSet": bag(if discounts.is_empty() { "0.00" } else { "5.00" }),
                "weight": {"unit": "GRAMS", "value": grams},
                "taxLines": [],
                "discountAllocations": discounts,
            })
        })
        .collect()
}

fn line_item_gids() -> Vec<String> {
    LINE_ITEMS
        .iter()
        .map(|line| format!("gid://shopify/LineItem/{}", line.id))
        .collect()
}

fn fulfillment() -> Value {
    json!({
        "__typename": "Fulfillment",
        "id": "gid://shopify/Fulfillment/123456",
        "name": "#9999.1",
        "order": "gid://shopify/Order/820982911946154508",
        "status": "PENDING",
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "service": null,
        "location": null,
        "trackingInfo": [{
            "company": "UPS",
            "number": "1z827wk74630",
            "url": "https://www.ups.com/WebTracking?loc=en_US&requester=ST&trackNums=1z827wk74630",
        }],
        "fulfillmentLineItems": line_item_gids()
            .into_iter()
            .enumerate()
            .map(|(index, line)| json!({
                "id": format!("gid://shopify/FulfillmentLineItem/{}", index + 1),
                "quantity": 1,
                "lineItem": line,
            }))
            .collect::<Vec<_>>(),
    })
}

fn refund() -> Value {
    json!({
        "__typename": "Refund",
        "id": "gid://shopify/Refund/890088186047892319",
        "order": "gid://shopify/Order/820982911946154508",
        "note": "Things were damaged",
        "createdAt": "2021-12-31T19:00:00-05:00",
        "processedAt": "2021-12-31T19:00:00-05:00",
        "staffMember": "gid://shopify/StaffMember/548380009",
        "totalRefundedSet": bag("85.00"),
        "refundLineItems": ([
            ("487817672276298627", "89.99"),
            ("789012345678901307", "19.99"),
            ("890123456789012418", "24.99"),
            ("976318377106520422", "159.99"),
            ("315789986012684466", "119.99"),
        ]
        .into_iter()
        .zip(line_item_gids())
        .map(|((id, subtotal), line)| json!({
            "id": format!("gid://shopify/RefundLineItem/{id}"),
            "quantity": 1,
            "lineItem": line,
            "location": null,
            "restockType": "NO_RESTOCK",
            "restocked": false,
            "subtotalSet": bag(subtotal),
            "totalTaxSet": bag("0.00"),
        }))
        .collect::<Vec<_>>()),
        "transactions": ([
            ("245135271310201194", "2019-12-31T19:00:00-05:00", "75.00"),
            ("839212141605670573", "2020-01-01T19:00:00-05:00", "10.00"),
        ]
        .into_iter()
        .map(|(id, at, amount)| json!({
            "id": format!("gid://shopify/OrderTransaction/{id}"),
            "kind": "REFUND",
            "gateway": "bogus",
            "status": "SUCCESS",
            "createdAt": at,
            "test": false,
            "amountSet": bag(amount),
            "paymentId": "#9999.",
            "manualPaymentGateway": false,
        }))
        .collect::<Vec<_>>()),
    })
}

fn draft_order() -> Value {
    json!({
        "__typename": "DraftOrder",
        "id": "gid://shopify/DraftOrder/890612572568261625",
        "name": "#D151",
        "email": "jon@doe.ca",
        "note2": null,
        "status": "OPEN",
        "currencyCode": "USD",
        "taxesIncluded": false,
        "taxExempt": false,
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "invoiceSentAt": null,
        "completedAt": null,
        "invoiceUrl": "https://jsmith.myshopify.com/548380009/invoices/abcd1234abcd1234abcd1234abcd1234",
        "allowDiscountCodesInCheckout": false,
        "tags": [],
        "customAttributes": [],
        "lineItems": ([
            ("1056548", "49148385", "Red", "IPOD2008RED", 6),
            ("4194725", "457924702", "Black", "IPOD2008BLACK", 4),
            ("3386024", "808950810", "Pink", "IPOD2008PINK", 10),
        ]
        .into_iter()
        .map(|(id, variant, color, sku, quantity)| json!({
            "id": format!("gid://shopify/DraftOrderLineItem/{id}"),
            "variant": format!("gid://shopify/ProductVariant/{variant}"),
            "product": "gid://shopify/Product/632910392",
            "title": "IPod Nano - 8GB",
            "variantTitle": color,
            "name": format!("IPod Nano - 8GB - {color}"),
            "sku": sku,
            "vendor": "Apple",
            "quantity": quantity,
            "requiresShipping": true,
            "taxable": true,
            "isGiftCard": false,
            "custom": false,
            "weight": {"unit": "GRAMS", "value": 567},
            "originalUnitPriceSet": bag("199.00"),
            "taxLines": [],
            "appliedDiscount": null,
            "customAttributes": [],
        }))
        .collect::<Vec<_>>()),
    })
    // In parts: one `json!` of every field is deeper than the macro may go.
    .merged(json!({
        "shippingAddress": shipping_address().with("zip", json!("40150")),
        "billingAddress": {
            "firstName": "Bob",
            "lastName": "Biller",
            "address1": "123 Billing Street",
            "address2": null,
            "phone": "555-555-BILL",
            "city": "Billtown",
            "zip": "K2P0B0",
            "province": "Kentucky",
            "provinceCode": "KY",
            "country": "United States",
            "countryCodeV2": "US",
            "company": "My Company",
            "latitude": null,
            "longitude": null,
            "name": "Bob Biller",
        },
        "appliedDiscount": {
            "description": "ABC 123",
            "value": 4.0,
            "title": "ABC 123",
            "amountSet": bag("2.00"),
            "valueType": "PERCENTAGE",
        },
        "shippingLine": {"custom": true, "shippingRateHandle": null, "title": "ABC 123", "originalPriceSet": bag("5.00")},
        "taxLines": (["4.00", "2.00", "1.00"]
            .into_iter()
            .map(|price| json!({"rate": 0.019980504574941826, "title": "State tax", "priceSet": bag(price)}))
            .collect::<Vec<_>>()),
        "totalPriceSet": bag("486.00"),
        "subtotalPriceSet": bag("453.00"),
        "totalTaxSet": bag("408.00"),
        "order": null,
    }))
    .merged(json!({
        "paymentTerms": {
            "id": "gid://shopify/PaymentTerms/706405506930370084",
            "paymentTermsName": "Net 7",
            "paymentTermsType": "NET",
            "dueInDays": 7,
            "createdAt": "2021-01-01T00:00:00-05:00",
            "updatedAt": "2021-01-01T00:00:01-05:00",
            "paymentSchedules": [{
                "id": "gid://shopify/PaymentSchedule/606405506930370084",
                "createdAt": "2021-01-01T00:00:00-05:00",
                "updatedAt": "2021-01-01T00:00:01-05:00",
                "issuedAt": "2021-01-01T00:00:00-05:00",
                "dueAt": "2022-01-07T19:00:00-05:00",
                "completedAt": null,
                "amount": money("486.00"),
                "balanceDue": money("486.00"),
                "totalBalance": money("486.00"),
            }],
        },
        "customer": {
            "id": "gid://shopify/Customer/706405506930370084",
            "firstName": "John",
            "lastName": "Smith",
            "email": "john@doe.ca",
            "phone": null,
            "state": "DISABLED",
            "note": null,
            "verifiedEmail": true,
            "multipassIdentifier": null,
            "taxExempt": false,
            "taxExemptions": [],
            "createdAt": null,
            "updatedAt": null,
            "amountSpent": money("0.0"),
            "defaultAddress": {
                "id": null,
                "firstName": "John",
                "lastName": "Smith",
                "company": null,
                "address1": "123 Elm St.",
                "address2": null,
                "city": "Ottawa",
                "province": "Ontario",
                "provinceCode": "ON",
                "country": "Canada",
                "countryCodeV2": "CA",
                "zip": "K2H7A8",
                "phone": "123-123-1234",
                "name": "John Smith",
            },
        },
    }))
}

/// The transaction of `order_transactions/create`'s example.
fn order_transaction() -> Value {
    json!({
        "__typename": "OrderTransaction",
        "id": "gid://shopify/OrderTransaction/120560818172775265",
        "order": "gid://shopify/Order/820982911946154508",
        "kind": "AUTHORIZATION",
        "gateway": "visa",
        "status": "SUCCESS",
        "createdAt": "2021-12-31T19:00:00-05:00",
        "test": false,
        "authorizationCode": "1001",
        "processedAt": null,
        "paymentDetails": {
            "__typename": "CardPaymentDetails",
            "number": "•••• •••• •••• 1234",
            "company": "Visa",
            "paymentMethodName": "visa",
        },
        "amountSet": bag("419.95"),
        "paymentId": "#9999.1",
        "totalUnsettledSet": {
            "presentmentMoney": {"amount": "419.95", "currencyCode": "USD"},
            "shopMoney": {"amount": "419.95", "currencyCode": "USD"},
        },
        "manualPaymentGateway": true,
    })
}

/// The committed edit of `orders/edited`'s example.
fn order_edit() -> Value {
    json!({
        "__typename": "CalculatedOrder",
        "id": "gid://shopify/CalculatedOrder/78912328793123782",
        "originalOrder": "gid://shopify/Order/820982911946154508",
        "committed": true,
        "createdAt": "2021-12-31T19:00:00-05:00",
        payload::EDIT_COMMITTED_AT: "2021-12-31T19:00:00-05:00",
        payload::EDIT_NOTIFY_CUSTOMER: false,
        payload::EDIT_STAFF_NOTE: "",
        payload::EDIT_ADDITIONS: [{"id": "gid://shopify/LineItem/78643924236718232", "delta": 1}],
        payload::EDIT_REMOVALS: [{"id": "gid://shopify/LineItem/487817672276298554", "delta": 1}],
    })
}

/// The event of `fulfillment_events/create`'s example, of the example fulfillment.
fn fulfillment_event() -> Value {
    json!({
        "__typename": "FulfillmentEvent",
        "id": "gid://shopify/FulfillmentEvent/1234567",
        "fulfillment": "gid://shopify/Fulfillment/123456",
        "status": "IN_TRANSIT",
        "message": "Item is now in transit",
        "happenedAt": "2021-12-31T19:00:00-05:00",
        "country": "CA",
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "estimatedDeliveryAt": null,
    })
}

/// The hold of the `fulfillment_holds/*` examples.
fn fulfillment_hold(topic: &str) -> Value {
    json!({
        "__typename": "FulfillmentHold",
        "id": "gid://shopify/FulfillmentHold/1",
        "fulfillmentOrder": "gid://shopify/FulfillmentOrder/1",
        "reason": "OTHER",
        "reasonNotes": "Waiting for some more details from the customer before this can be fulfilled.",
        "heldByRequestingApp": false,
        "handle": if topic == "fulfillment_holds/added" { "test_handle" } else { "fulfillment_hold_1" },
        "heldByApp": "gid://shopify/App/12345",
    })
}

/// A fulfillment order of the `fulfillment_orders/*` examples, with the records its topic names.
fn fulfillment_order_sample(topic: &str) -> (Value, Vec<Value>) {
    let fulfillment_order = |id: u64, status: &str, location: u64| {
        json!({
            "__typename": "FulfillmentOrder",
            "id": format!("gid://shopify/FulfillmentOrder/{id}"),
            "status": status,
            "assignedLocation": {"location": format!("gid://shopify/Location/{location}")},
            "fulfillAt": "2021-12-31T19:00:00-05:00",
        })
    };
    let hold = |id: u64, reason: &str, notes: &str, handle: &str| {
        json!({
            "__typename": "FulfillmentHold",
            "id": format!("gid://shopify/FulfillmentHold/{id}"),
            "reason": reason,
            "reasonNotes": notes,
            "heldByRequestingApp": false,
            "handle": handle,
            "heldByApp": "gid://shopify/App/12345",
        })
    };
    let request = |message: &str| {
        json!({
            "__typename": "FulfillmentOrderMerchantRequest",
            "id": "gid://shopify/FulfillmentOrderMerchantRequest/1",
            "message": message,
        })
    };
    let event = payload::FULFILLMENT_ORDER_EVENT;
    match topic {
        "fulfillment_orders/placed_on_hold" => (
            fulfillment_order(1, "ON_HOLD", 0)
                .with("fulfillmentHolds", json!(["gid://shopify/FulfillmentHold/1", "gid://shopify/FulfillmentHold/2"]))
                .with(event, json!({
                    "hold": "gid://shopify/FulfillmentHold/1",
                    "remaining": "gid://shopify/FulfillmentOrder/2",
                    "lineItems": [{"id": "gid://shopify/FulfillmentOrderLineItem/3", "quantity": 4}],
                })),
            vec![
                fulfillment_order(2, "OPEN", 0),
                hold(1, "OTHER", "example", "example-hold-1"),
                hold(2, "INVENTORY_OUT_OF_STOCK", "Stacked hold", "example-hold-2"),
            ],
        ),
        "fulfillment_orders/cancelled" => (
            fulfillment_order(1, "CANCELLED", 0)
                .with(event, json!({"replacement": "gid://shopify/FulfillmentOrder/2"})),
            vec![fulfillment_order(2, "OPEN", 0)],
        ),
        "fulfillment_orders/moved" => (
            fulfillment_order(2, "OPEN", 1).with(event, json!({
                "original": "gid://shopify/FulfillmentOrder/1",
                "source": "gid://shopify/Location/0",
                "lineItems": [{"id": "gid://shopify/FulfillmentOrderLineItem/1", "quantity": 1}],
            })),
            vec![fulfillment_order(1, "CLOSED", 0)],
        ),
        "fulfillment_orders/rescheduled" => (fulfillment_order(1, "SCHEDULED", 0), Vec::new()),
        "fulfillment_orders/fulfillment_request_submitted" => (
            fulfillment_order(1, "OPEN", 0)
                .with("requestStatus", json!("UNSUBMITTED"))
                .with(event, json!({
                    "original": "gid://shopify/FulfillmentOrder/1",
                    "submitted": "gid://shopify/FulfillmentOrder/1",
                    "request": "gid://shopify/FulfillmentOrderMerchantRequest/1",
                })),
            vec![request("Fragile")],
        ),
        "fulfillment_orders/cancellation_request_submitted" => (
            fulfillment_order(1, "IN_PROGRESS", 0)
                .with("requestStatus", json!("CANCELLATION_REQUESTED"))
                .with(event, json!({"request": "gid://shopify/FulfillmentOrderMerchantRequest/1"})),
            vec![request("Customer cancelled their order")],
        ),
        "fulfillment_orders/fulfillment_request_accepted" => (
            fulfillment_order(1, "IN_PROGRESS", 0)
                .with("requestStatus", json!("ACCEPTED"))
                .with(event, json!({"message": "We will ship the item tomorrow."})),
            Vec::new(),
        ),
        "fulfillment_orders/fulfillment_request_rejected" => (
            fulfillment_order(1, "OPEN", 0)
                .with("requestStatus", json!("REJECTED"))
                .with(event, json!({"message": "Can't fulfill due to no inventory on product."})),
            Vec::new(),
        ),
        "fulfillment_orders/cancellation_request_accepted" => (
            fulfillment_order(1, "CLOSED", 0)
                .with(event, json!({"message": "Order has not been shipped yet."})),
            Vec::new(),
        ),
        "fulfillment_orders/cancellation_request_rejected" => (
            fulfillment_order(1, "IN_PROGRESS", 0)
                .with("requestStatus", json!("CANCELLATION_REJECTED"))
                .with(event, json!({"message": "Order has already been shipped."})),
            Vec::new(),
        ),
        "fulfillment_orders/fulfillment_service_failed_to_complete" => (
            fulfillment_order(1, "CLOSED", 0)
                .with(event, json!({"message": "We broke the last item."})),
            Vec::new(),
        ),
        "fulfillment_orders/split" => (
            fulfillment_order(2, "OPEN", 0).with(event, json!({
                "original": "gid://shopify/FulfillmentOrder/1",
                "replacement": "gid://shopify/FulfillmentOrder/3",
                "lineItems": [{"id": "gid://shopify/FulfillmentOrderLineItem/1", "quantity": 1}],
            })),
            vec![fulfillment_order(1, "OPEN", 0), fulfillment_order(3, "OPEN", 0)],
        ),
        _ => (fulfillment_order(1, "OPEN", 0), Vec::new()),
    }
}

/// The return of the `returns/*` examples, in the status of the topic.
fn return_sample(topic: &str) -> Value {
    let status = match topic {
        "returns/request" => "REQUESTED",
        "returns/decline" => "DECLINED",
        "returns/close" => "CLOSED",
        "returns/cancel" => "CANCELED",
        _ => "OPEN",
    };
    let lines = if topic == "returns/approve" || topic == "returns/request" {
        json!([
            "gid://shopify/ReturnLineItem/1",
            "gid://shopify/ReturnLineItem/2"
        ])
    } else {
        json!([])
    };
    json!({
        "__typename": "Return",
        "id": "gid://shopify/Return/123134564567890",
        "order": "gid://shopify/Order/4783296544821",
        "status": status,
        "name": null,
        "returnLineItems": lines,
        "exchangeLineItems": if topic == "returns/approve" { json!(["gid://shopify/ExchangeLineItem/987654321283476"]) } else { json!([]) },
        "requestedAt": "2026-01-15T07:00:00-05:00",
        "createdAt": "2026-01-15T07:00:00-05:00",
        "decline": {
            "reason": "RETURN_PERIOD_ENDED",
            "note": "As discussed on the phone, the 30-day return window has ended.",
        },
    })
}

fn inventory_item() -> Value {
    json!({
        "__typename": "InventoryItem",
        "id": "gid://shopify/InventoryItem/271878346596884015",
        "sku": "example-sku",
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "requiresShipping": true,
        "unitCost": null,
        "countryCodeOfOrigin": null,
        "provinceCodeOfOrigin": null,
        "harmonizedSystemCode": null,
        "tracked": true,
        "countryHarmonizedSystemCodes": [],
        "measurement": {"weight": {"unit": "POUNDS", "value": 0.0}},
    })
}

fn inventory_level() -> Value {
    json!({
        "__typename": "InventoryLevel",
        "id": "gid://shopify/InventoryLevel/24826418?inventory_item_id=271878346596884015",
        "item": "gid://shopify/InventoryItem/271878346596884015",
        "location": "gid://shopify/Location/24826418",
        "quantities": [{"name": "available", "quantity": 10}],
        "updatedAt": "2021-12-31T19:00:00-05:00",
    })
}

fn location() -> Value {
    json!({
        "__typename": "Location",
        "id": "gid://shopify/Location/866550311766439020",
        "name": "Example Shop",
        "address": {
            "address1": "34 Example Street",
            "address2": "Next to example",
            "city": "ottawa",
            "zip": "k1n5t5",
            "province": "ontario",
            "provinceCode": "ON",
            "country": "Canada",
            "countryCode": "CA",
            "phone": "555-555-5555",
        },
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "isFulfillmentService": false,
        "isActive": true,
    })
}

fn shop() -> Value {
    json!({
        "__typename": "Shop",
        "id": "gid://shopify/Shop/548380009",
        "name": "Super Toys",
        "email": "super@supertoys.com",
        "contactEmail": "super@supertoys.com",
        "myshopifyDomain": SAMPLE_SHOP,
        "primaryDomain": null,
        "shopAddress": {
            "address1": "190 MacLaren Street",
            "address2": null,
            "city": "Houston",
            "zip": "37178",
            "province": "Tennessee",
            "provinceCode": "TN",
            "country": "United States",
            "countryCodeV2": "US",
            "phone": "3213213210",
            "latitude": null,
            "longitude": null,
        },
        "currencyCode": "USD",
        "enabledPresentmentCurrencies": ["USD"],
        "ianaTimezone": "America/New_York",
        "timezoneOffset": "-0500",
        "shopOwnerName": "John Smith",
        "currencyFormats": {
            "moneyFormat": "${{amount}}",
            "moneyWithCurrencyFormat": "${{amount}} USD",
            "moneyInEmailsFormat": "${{amount}}",
            "moneyWithCurrencyInEmailsFormat": "${{amount}} USD",
        },
        "weightUnit": "KILOGRAMS",
        "plan": {"publicDisplayName": "Shopify Plus", "shopifyPlus": true, "partnerDevelopment": false},
        "checkoutApiSupported": true,
        "setupRequired": false,
        "features": {"giftCards": true},
        "primaryLocation": "gid://shopify/Location/655441491",
        "createdAt": null,
        "updatedAt": null,
    })
}

fn installation() -> Value {
    json!({
        "__typename": "AppInstallation",
        "id": "gid://shopify/AppInstallation/1",
        "accessScopes": [{"handle": "read_products"}, {"handle": "write_products"}],
        "updatedAt": "2024-06-25T00:00:00Z",
    })
    .with(PREVIOUS_ACCESS_SCOPES, json!([{"handle": "read_products"}]))
}

fn app_subscription() -> Value {
    json!({
        "__typename": "AppSubscription",
        "id": "gid://shopify/AppSubscription/1029266965",
        "name": "Webhook Test",
        "status": "PENDING",
        "createdAt": "2021-12-31T19:00:00-05:00",
        "updatedAt": "2021-12-31T19:00:00-05:00",
        "lineItems": [
            {"id": "gid://shopify/AppSubscriptionLineItem/1", "plan": {"pricingDetails": {
                "__typename": "AppRecurringPricing",
                "price": money("10.0"),
                "interval": "EVERY_30_DAYS",
                "planHandle": "plan-123",
            }}},
            {"id": "gid://shopify/AppSubscriptionLineItem/2", "plan": {"pricingDetails": {
                "__typename": "AppUsagePricing",
                "cappedAmount": money("20.0"),
                "terms": "Usage",
            }}},
        ],
    })
}

fn bulk_operation() -> Value {
    json!({
        "__typename": "BulkOperation",
        "id": "gid://shopify/BulkOperation/147595010",
        "status": "COMPLETED",
        "type": "QUERY",
        "errorCode": null,
        "createdAt": "2026-10-01T10:20:57-04:00",
        "completedAt": "2024-01-01T07:34:56-05:00",
    })
}

fn metaobject() -> Value {
    json!({
        "__typename": "Metaobject",
        "id": "gid://shopify/Metaobject/123",
        "type": "example-type",
        "handle": "example-metaobject",
        "displayName": "Example Metaobject",
        "createdAt": "1975-10-31T00:55:12-05:00",
        "updatedAt": "1985-07-13T01:55:12-04:00",
        "definition": "gid://shopify/MetaobjectDefinition/222",
        "fields": [{"key": "example-key", "value": "example-value", "type": "single_line_text_field"}],
        "createdByStaff": "gid://shopify/StaffMember/444",
        "createdByApp": "gid://shopify/ApiClient/333",
        "capabilities": {"publishable": {"status": "DRAFT"}},
    })
}

fn discount() -> Value {
    json!({
        "__typename": "DiscountAutomaticNode",
        "id": "gid://shopify/DiscountAutomaticNode/1",
        "automaticDiscount": {
            "__typename": "DiscountAutomaticFreeShipping",
            "title": "Automatic free shipping",
            "status": "ACTIVE",
            "createdAt": "2016-08-29T08:00:00-04:00",
            "updatedAt": "2016-08-29T08:00:00-04:00",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_topic_has_a_sample() {
        for topic in payload::BUILT_TOPICS {
            let sample = fixed(topic).unwrap_or_else(|| panic!("no sample for {topic}"));
            assert!(sample.payload(topic).is_some(), "{topic}");
        }
        assert!(fixed("carts/create").is_none());
        assert_eq!(FixedSamples.shop(), "shop.myshopify.com");
    }
}

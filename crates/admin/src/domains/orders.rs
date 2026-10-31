//! Orders: their line items, and what follows from their state: fulfillment orders, the
//! fulfillment of a fulfilled order, the transactions of a paid one, returns.
//!
//! The orders of the store data and the orders placed in the local checkout become records the
//! same way: each is first an [`OrderDraft`], which [`insert_order`] turns into records. The
//! records and their ids: the "Records" section of `crates/admin/docs/design.md`.
//!
//! What the Admin API does with them is in the submodules: [`model`] (what an order's records
//! derive from one another, the money and the inventory they move), [`queries`] (the search
//! filters, sort keys and resolvers of the family), and one module per group of mutations.

mod drafts;
mod edits;
mod fulfillments;
mod lifecycle;
pub mod model;
pub mod payments;
mod queries;
mod refunds;
mod returns;
pub mod risk;
mod services;
mod store_credit;

#[cfg(test)]
mod payments_tests;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use lsf_core::store::{Order, Product, Variant};
use lsf_core::util::stable_id;
use serde_json::{Value, json};

use super::catalog::{self, gids};
use super::customers;
use crate::state::seed::Seed;
use crate::state::value::{self, date_time, text};
use crate::state::{ShopState, gid};

/// An order about to become records.
#[derive(Clone, Debug, Default)]
pub struct OrderDraft {
    /// The GID of the order.
    pub id: String,
    /// `#1001`.
    pub name: String,
    /// `1001`: what the name is made of.
    pub number: u64,
    /// When it was placed (`createdAt`, `processedAt`).
    pub created_at: DateTime<Utc>,
    /// The GID of the customer.
    pub customer: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub accepts_marketing: bool,
    pub tax_exempt: bool,
    pub lines: Vec<LineDraft>,
    /// `MailingAddress`es, as [`value::mailing_address`] makes them.
    pub shipping_address: Option<Value>,
    pub billing_address: Option<Value>,
    pub shipping: Option<ShippingDraft>,
    /// The tax of the order, in cents.
    pub tax: i64,
    /// What the discounts of the order (not those of its lines) took off, in cents.
    pub order_discount: i64,
    pub discount_codes: Vec<String>,
    pub note: Option<String>,
    pub attributes: Vec<(String, String)>,
    pub financial: FinancialStatus,
    pub fulfillment: FulfillmentState,
    pub cancelled: bool,
    /// What was paid at checkout, when the order is paid and that is not its whole price (the
    /// deposit of a pre-order): the total otherwise.
    pub paid: Option<i64>,
    /// The name of the payment gateway: what the checkout calls the payment method.
    pub gateway: Option<String>,
    /// `web` for an order of the checkout.
    pub source_name: Option<String>,
    /// The returns: their status (`requested`, `open`, `closed`) and the lines sent back, as
    /// `(index of the line, quantity)`.
    pub returns: Vec<(String, Vec<(usize, u32)>)>,
    /// The GID of the location that fulfills the order.
    pub location: Option<String>,
}

/// A line of an [`OrderDraft`].
#[derive(Clone, Debug, Default)]
pub struct LineDraft {
    pub product: Option<String>,
    pub variant: Option<String>,
    pub inventory_item: Option<String>,
    /// The product title.
    pub title: String,
    /// The variant title, `None` for the default variant.
    pub variant_title: Option<String>,
    pub sku: Option<String>,
    pub vendor: Option<String>,
    pub quantity: u32,
    /// The price of one unit, before discounts, in cents.
    pub unit_price: i64,
    /// What the discounts took off the line in all, in cents.
    pub discount: i64,
    pub requires_shipping: bool,
    pub taxable: bool,
    pub gift_card: bool,
    /// An `Image`.
    pub image: Option<Value>,
    /// A `Weight`, and the weight of one unit in grams.
    pub weight: Option<Value>,
    pub grams: u64,
    pub attributes: Vec<(String, String)>,
}

/// The shipping line of an [`OrderDraft`].
#[derive(Clone, Debug, Default)]
pub struct ShippingDraft {
    pub title: String,
    /// The handle of the rate chosen, for an order of the checkout.
    pub handle: Option<String>,
    /// Before and after the shipping discounts, in cents.
    pub original: i64,
    pub discounted: i64,
}

/// The financial status of an order, as the store data and Shopify's `OrderDisplayFinancialStatus`
/// name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FinancialStatus {
    #[default]
    Paid,
    Pending,
    Authorized,
    PartiallyPaid,
    PartiallyRefunded,
    Refunded,
    Voided,
    Expired,
}

impl FinancialStatus {
    /// The status the store data writes (`paid`, `partially_refunded`, …); paid when unknown.
    pub fn parse(text: &str) -> FinancialStatus {
        match text.to_ascii_lowercase().replace('-', "_").as_str() {
            "pending" => FinancialStatus::Pending,
            "authorized" => FinancialStatus::Authorized,
            "partially_paid" => FinancialStatus::PartiallyPaid,
            "partially_refunded" => FinancialStatus::PartiallyRefunded,
            "refunded" => FinancialStatus::Refunded,
            "voided" => FinancialStatus::Voided,
            "expired" => FinancialStatus::Expired,
            _ => FinancialStatus::Paid,
        }
    }

    pub fn graphql(self) -> &'static str {
        match self {
            FinancialStatus::Paid => "PAID",
            FinancialStatus::Pending => "PENDING",
            FinancialStatus::Authorized => "AUTHORIZED",
            FinancialStatus::PartiallyPaid => "PARTIALLY_PAID",
            FinancialStatus::PartiallyRefunded => "PARTIALLY_REFUNDED",
            FinancialStatus::Refunded => "REFUNDED",
            FinancialStatus::Voided => "VOIDED",
            FinancialStatus::Expired => "EXPIRED",
        }
    }

    /// Whether the money was taken.
    fn captured(self) -> bool {
        matches!(
            self,
            FinancialStatus::Paid
                | FinancialStatus::PartiallyPaid
                | FinancialStatus::PartiallyRefunded
                | FinancialStatus::Refunded
        )
    }
}

/// How much of an order was fulfilled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FulfillmentState {
    #[default]
    Unfulfilled,
    /// Some of it: which lines the data does not say, so the first line is (or, for an order of
    /// one line, half of its units).
    Partial,
    Fulfilled,
}

impl FulfillmentState {
    /// The status the store data writes (`fulfilled`, `unfulfilled`, `partial`).
    pub fn parse(text: &str) -> FulfillmentState {
        match text.to_ascii_lowercase().as_str() {
            "fulfilled" | "shipped" => FulfillmentState::Fulfilled,
            "partial" | "partially_fulfilled" => FulfillmentState::Partial,
            _ => FulfillmentState::Unfulfilled,
        }
    }
}

pub fn order_gid(order: &Order) -> String {
    gid("Order", order.id)
}

/// A line item: from the order's id and the line's position, which lsf-core gives no id.
pub fn line_item_gid(order_id: u64, position: usize) -> String {
    gid(
        "LineItem",
        stable_id("line_item", &format!("{order_id}/{position}")),
    )
}

/// An id made of the order and a key, for the records an order implies.
fn derived(type_name: &str, kind: &str, order: &str, key: &str) -> String {
    gid(type_name, stable_id(kind, &format!("{order}/{key}")))
}

/// The setting of the shop that holds the checkout's tax rate, in basis points (2000 is 20 %):
/// draft orders compute their taxes with it.
pub const TAX_RATE_SETTING: &str = "taxRateBps";

pub fn seed(seed: &mut Seed) {
    let store = seed.store;
    seed.state
        .set_setting(TAX_RATE_SETTING, json!(store.checkout.tax_rate_bps));
    payments::seed(&mut seed.state);
    let location = catalog::locations(store).first().map(catalog::location_gid);
    for customer in &store.customers {
        for order in &customer.orders {
            let lines = order
                .line_items
                .iter()
                .map(|line| {
                    let product = &store.products[line.variant.0];
                    let variant = &product.variants[line.variant.1];
                    line_draft(seed, product, variant, line.quantity)
                })
                .collect();
            // The rate of the checkout that costs what the order paid for shipping names it.
            let shipping =
                (order.shipping_price > 0 || order.shipping_address.is_some()).then(|| {
                    let rate = store
                        .checkout
                        .shipping_rates
                        .iter()
                        .find(|rate| rate.price == order.shipping_price);
                    ShippingDraft {
                        title: rate
                            .map_or_else(|| "Shipping".to_string(), |rate| rate.title.clone()),
                        handle: rate.map(|rate| rate.handle.clone()),
                        original: order.shipping_price,
                        discounted: order.shipping_price,
                    }
                });
            let draft = OrderDraft {
                id: order_gid(order),
                name: order.name.clone(),
                number: order.order_number,
                created_at: order.created_at,
                customer: Some(customers::customer_gid(customer)),
                email: Some(customer.email.clone()).filter(|email| !email.is_empty()),
                phone: Some(customer.phone.clone()).filter(|phone| !phone.is_empty()),
                accepts_marketing: customer.accepts_marketing,
                tax_exempt: customer.tax_exempt,
                lines,
                shipping_address: order
                    .shipping_address
                    .as_ref()
                    .map(|address| value::mailing_address(address, "Address")),
                billing_address: order
                    .billing_address
                    .as_ref()
                    .map(|address| value::mailing_address(address, "Address")),
                shipping,
                tax: order.tax_price,
                order_discount: 0,
                discount_codes: Vec::new(),
                note: Some(order.note.clone()).filter(|note| !note.is_empty()),
                attributes: Vec::new(),
                financial: FinancialStatus::parse(&order.financial_status),
                fulfillment: FulfillmentState::parse(&order.fulfillment_status),
                cancelled: order.cancelled,
                paid: None,
                gateway: None,
                source_name: None,
                returns: order
                    .returns
                    .iter()
                    .map(|sent| (sent.status.clone(), sent.lines.clone()))
                    .collect(),
                location: location.clone(),
            };
            insert_order(&mut seed.state, &draft);
        }
    }
    for customer in &store.customers {
        customers::refresh_customer(&mut seed.state, &customers::customer_gid(customer));
    }
}

/// A line of an order for a variant of the store.
fn line_draft(seed: &Seed, product: &Product, variant: &Variant, quantity: u32) -> LineDraft {
    let image = variant
        .media_index
        .and_then(|index| product.media.get(index))
        .and_then(|media| media.preview.as_ref())
        .or_else(|| product.images().next())
        .map(|image| seed.image(image, "ImageSource"));
    LineDraft {
        product: Some(catalog::product_gid(product)),
        variant: Some(catalog::variant_gid(variant)),
        inventory_item: Some(catalog::inventory_item_gid(variant.id)),
        title: product.title.clone(),
        variant_title: (!product.has_only_default_variant()).then(|| variant.title.clone()),
        sku: Some(variant.sku.clone()).filter(|sku| !sku.is_empty()),
        vendor: Some(product.vendor.clone()).filter(|vendor| !vendor.is_empty()),
        quantity,
        unit_price: variant.price,
        discount: 0,
        requires_shipping: variant.requires_shipping,
        taxable: variant.taxable,
        gift_card: product.gift_card,
        image,
        weight: Some(json!({
            "unit": catalog::weight_unit(&variant.weight_unit),
            "value": lsf_core::drops::product::weight_in_unit(variant.weight, &variant.weight_unit),
        })),
        grams: variant.weight,
        attributes: Vec::new(),
    }
}

/// The quantity of each line a fulfillment of the order holds.
fn fulfilled_quantities(draft: &OrderDraft) -> Vec<u32> {
    match draft.fulfillment {
        FulfillmentState::Unfulfilled => vec![0; draft.lines.len()],
        FulfillmentState::Fulfilled => draft.lines.iter().map(|line| line.quantity).collect(),
        FulfillmentState::Partial if draft.lines.len() == 1 => vec![draft.lines[0].quantity / 2],
        FulfillmentState::Partial => draft
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| if index == 0 { line.quantity } else { 0 })
            .collect(),
    }
}

/// Turns a draft into records: the order, its line items, its fulfillment order, its
/// fulfillment and transactions as its state implies, its returns. Updates the customer's
/// derived fields. Returns the GID of the order.
pub fn insert_order(state: &mut ShopState, draft: &OrderDraft) -> String {
    let currency = state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();
    let taxes_included = state
        .shop()
        .and_then(|shop| shop.get("taxesIncluded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let order = draft.id.clone();
    let created = date_time(draft.created_at);
    let fulfilled = fulfilled_quantities(draft);
    let attributes = |attributes: &[(String, String)]| -> Vec<Value> {
        attributes
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect()
    };

    // The line items.
    let mut line_items = Vec::new();
    let mut subtotal = 0;
    let mut line_discounts = 0;
    let mut quantity = 0;
    let mut grams: u64 = 0;
    let order_number = crate::state::parse_gid(&order).map_or(0, |(_, id)| id);
    for (position, line) in draft.lines.iter().enumerate() {
        let id = line_item_gid(order_number, position);
        let discounted = line.unit_price * i64::from(line.quantity) - line.discount;
        let unfulfilled = line.quantity - fulfilled[position];
        subtotal += discounted;
        line_discounts += line.discount;
        quantity += line.quantity;
        grams = line
            .grams
            .saturating_mul(u64::from(line.quantity))
            .saturating_add(grams);
        state.insert(line_item_record(
            &id,
            line,
            &currency,
            unfulfilled,
            draft.financial.captured(),
            &created,
        ));
        line_items.push(id);
    }
    let shipping_original = draft
        .shipping
        .as_ref()
        .map_or(0, |shipping| shipping.original);
    let shipping_discounted = draft
        .shipping
        .as_ref()
        .map_or(0, |shipping| shipping.discounted);
    let subtotal = subtotal - draft.order_discount;
    // The tax is on top, as the storefront's `order.total_price` and the checkout count it.
    let total = subtotal + shipping_discounted + draft.tax;
    let discounts = line_discounts + draft.order_discount + shipping_original - shipping_discounted;
    let paid = match draft.financial {
        FinancialStatus::Paid | FinancialStatus::PartiallyRefunded | FinancialStatus::Refunded => {
            draft.paid.unwrap_or(total)
        }
        FinancialStatus::PartiallyPaid => draft.paid.unwrap_or(0),
        _ => 0,
    };
    let refunded = if draft.financial == FinancialStatus::Refunded {
        paid
    } else {
        0
    };
    let fully_fulfilled = draft.fulfillment == FulfillmentState::Fulfilled;

    // The fulfillment order: every order has one, at the location that fulfills it.
    let location = draft.location.as_ref().and_then(|location| {
        state
            .get(location)
            .map(|record| (location.clone(), record.clone()))
    });
    let assigned =
        location
            .as_ref()
            .map_or(json!({"name": "", "countryCode": "US"}), |(id, record)| {
                let address = record.get("address").cloned().unwrap_or(Value::Null);
                json!({
                    "name": record.get("name"),
                    "location": id,
                    "address1": address.get("address1"),
                    "address2": address.get("address2"),
                    "city": address.get("city"),
                    "province": address.get("provinceCode"),
                    "zip": address.get("zip"),
                    "phone": address.get("phone"),
                    "countryCode": address
                        .get("countryCode")
                        .and_then(Value::as_str)
                        .filter(|code| !code.is_empty())
                        .unwrap_or("US"),
                })
            });
    let fulfillment_order = derived("FulfillmentOrder", "fulfillment_order", &order, "1");
    let fulfillment = derived("Fulfillment", "fulfillment", &order, "1");
    let has_fulfillment = fulfilled.iter().any(|quantity| *quantity > 0);
    let mut order_lines = Vec::new();
    let mut fulfillment_lines = Vec::new();
    for (position, line) in draft.lines.iter().enumerate() {
        let line_item = &line_items[position];
        let id = derived(
            "FulfillmentOrderLineItem",
            "fulfillment_order_line_item",
            &order,
            &position.to_string(),
        );
        state.insert(value::object(json!({
            "__typename": "FulfillmentOrderLineItem",
            "id": id,
            "lineItem": line_item,
            "totalQuantity": line.quantity,
            "remainingQuantity": line.quantity - fulfilled[position],
            "productTitle": line.title,
            "variantTitle": line.variant_title,
            "sku": line.sku,
            "vendor": line.vendor,
            "variant": line.variant,
            "inventoryItemId": line.inventory_item,
            "requiresShipping": line.requires_shipping,
            "image": line.image,
            "weight": line.weight,
            "warnings": [],
        })));
        order_lines.push(id);
        if fulfilled[position] > 0 {
            let id = derived(
                "FulfillmentLineItem",
                "fulfillment_line_item",
                &order,
                &position.to_string(),
            );
            let share = line.unit_price * i64::from(fulfilled[position]);
            let discounted = share
                - model::share(
                    line.discount,
                    i64::from(fulfilled[position]),
                    i64::from(line.quantity.max(1)),
                );
            state.insert(value::object(json!({
                "__typename": "FulfillmentLineItem",
                "id": id,
                "lineItem": line_item,
                "quantity": fulfilled[position],
                "originalTotalSet": bag(share),
                "discountedTotalSet": bag(discounted),
                "createdAt": created,
            })));
            fulfillment_lines.push((position, id));
        }
    }
    let destination = draft.shipping_address.as_ref().map(|address| {
        json!({
            "id": derived("FulfillmentOrderDestination", "fulfillment_order_destination", &order, "1"),
            "firstName": address.get("firstName"),
            "lastName": address.get("lastName"),
            "company": address.get("company"),
            "address1": address.get("address1"),
            "address2": address.get("address2"),
            "city": address.get("city"),
            "province": address.get("province"),
            "zip": address.get("zip"),
            "countryCode": address.get("countryCodeV2"),
            "phone": address.get("phone"),
            "email": draft.email,
        })
    });
    let status = if draft.cancelled {
        "CANCELLED"
    } else if fully_fulfilled {
        "CLOSED"
    } else if has_fulfillment {
        "IN_PROGRESS"
    } else {
        "OPEN"
    };
    let method = match draft.shipping.as_ref() {
        Some(_) => "SHIPPING",
        None if draft.lines.iter().any(|line| line.requires_shipping) => "SHIPPING",
        None => "NONE",
    };
    state.insert(value::object(json!({
        "__typename": "FulfillmentOrder",
        "id": fulfillment_order,
        "order": order,
        "orderId": order,
        "orderName": draft.name,
        "orderProcessedAt": created,
        "status": status,
        "requestStatus": "UNSUBMITTED",
        "assignedLocation": assigned,
        "destination": destination,
        "deliveryMethod": {
            "id": derived("DeliveryMethod", "delivery_method", &order, "1"),
            "methodType": method,
            "presentedName": draft.shipping.as_ref().map(|shipping| shipping.title.clone()),
            "serviceCode": draft.shipping.as_ref().and_then(|shipping| shipping.handle.clone()),
        },
        "lineItems": order_lines,
        "fulfillments": if has_fulfillment { vec![fulfillment.clone()] } else { Vec::new() },
        "fulfillmentHolds": [],
        "fulfillAt": created,
        "createdAt": created,
        "updatedAt": created,
    })));

    if has_fulfillment {
        state.insert(value::object(json!({
            "__typename": "Fulfillment",
            "id": fulfillment,
            "name": format!("{}-F1", draft.name),
            "order": order,
            "status": "SUCCESS",
            "displayStatus": "FULFILLED",
            "location": location.as_ref().map(|(id, _)| id),
            "fulfillmentLineItems": fulfillment_lines.iter().map(|(_, id)| id).collect::<Vec<_>>(),
            "fulfillmentOrders": [fulfillment_order],
            "totalQuantity": fulfilled.iter().sum::<u32>(),
            "requiresShipping": draft.lines.iter().any(|line| line.requires_shipping),
            "trackingInfo": [],
            "deliveredAt": null,
            "inTransitAt": null,
            "createdAt": created,
            "updatedAt": created,
        })));
    }

    // The transactions of the money the order took (or that is waiting to be captured).
    let mut transactions = Vec::new();
    let mut transaction = |state: &mut ShopState, kind: &str, amount: i64, parent: Option<&str>| {
        let id = derived(
            "OrderTransaction",
            "order_transaction",
            &order,
            &(transactions.len() + 1).to_string(),
        );
        state.insert(value::object(json!({
            "__typename": "OrderTransaction",
            "id": id,
            "kind": kind,
            "status": "SUCCESS",
            "amountSet": bag(amount),
            "gateway": draft.gateway,
            "formattedGateway": draft.gateway,
            "order": order,
            "parentTransaction": parent,
            "test": false,
            "manuallyCapturable": kind == "AUTHORIZATION",
            "createdAt": created,
            "processedAt": created,
            "fees": [],
        })));
        transactions.push(id.clone());
        id
    };
    match draft.financial {
        FinancialStatus::Authorized => {
            transaction(state, "AUTHORIZATION", total, None);
        }
        status if status.captured() && paid > 0 => {
            let sale = transaction(state, "SALE", paid, None);
            if status == FinancialStatus::Refunded {
                transaction(state, "REFUND", refunded, Some(&sale));
            }
        }
        _ => {}
    }
    // The tender transactions of the money they moved; the caller emits their events.
    for made in &transactions {
        payments::record_tender(state, made);
    }

    // The returns. A line that was fulfilled comes back as a `ReturnLineItem` of its
    // fulfillment; one that was not, as an `UnverifiedReturnLineItem`.
    let mut returns = Vec::new();
    let mut return_status = "NO_RETURN";
    for (index, (status, lines)) in draft.returns.iter().enumerate() {
        let id = derived("Return", "return", &order, &index.to_string());
        let status = match status.to_ascii_lowercase().as_str() {
            "requested" => "REQUESTED",
            "closed" => "CLOSED",
            "declined" => "DECLINED",
            "canceled" | "cancelled" => "CANCELED",
            _ => "OPEN",
        };
        return_status = match status {
            "REQUESTED" => "RETURN_REQUESTED",
            "OPEN" => "IN_PROGRESS",
            "CLOSED" => "RETURNED",
            _ => return_status,
        };
        let mut return_lines = Vec::new();
        for (rank, (position, quantity)) in lines.iter().enumerate() {
            let Some(line) = draft.lines.get(*position) else {
                continue;
            };
            let processed = if status == "CLOSED" { *quantity } else { 0 };
            let fulfillment_line = fulfillment_lines
                .iter()
                .find(|(fulfilled, _)| fulfilled == position)
                .map(|(_, id)| id.clone());
            let line_id = derived(
                if fulfillment_line.is_some() {
                    "ReturnLineItem"
                } else {
                    "UnverifiedReturnLineItem"
                },
                "return_line_item",
                &id,
                &rank.to_string(),
            );
            let mut record = value::object(json!({
                "__typename": crate::state::gid_type(&line_id),
                "id": line_id,
                "quantity": quantity,
                "processedQuantity": processed,
                "unprocessedQuantity": quantity - processed,
                "processableQuantity": if status == "OPEN" { quantity - processed } else { 0 },
                "refundableQuantity": quantity,
                "refundedQuantity": 0,
                "returnReasonNote": "",
                "customerNote": null,
            }));
            match fulfillment_line {
                Some(fulfillment_line) => {
                    record.insert("fulfillmentLineItem".to_string(), json!(fulfillment_line));
                }
                None => {
                    record.insert(
                        "unitPrice".to_string(),
                        value::money_v2(line.unit_price, &currency),
                    );
                }
            }
            state.insert(record);
            return_lines.push(line_id);
        }
        state.insert(value::object(json!({
            "__typename": "Return",
            "id": id,
            "name": format!("{}-R{}", draft.name, index + 1),
            "status": status,
            "order": order,
            "returnLineItems": return_lines,
            "totalQuantity": lines.iter().map(|(_, quantity)| quantity).sum::<u32>(),
            "createdAt": created,
            "closedAt": if status == "CLOSED" { json!(created) } else { Value::Null },
        })));
        returns.push(id);
    }

    let shipping_line = draft.shipping.as_ref().map(|shipping| {
        json!({
            "id": derived("ShippingLine", "shipping_line", &order, "1"),
            "title": shipping.title,
            "code": shipping.handle.clone().unwrap_or_else(|| shipping.title.clone()),
            "shippingRateHandle": shipping.handle,
            "source": "shopify",
            "custom": false,
            "isRemoved": false,
            "originalPriceSet": bag(shipping.original),
            "discountedPriceSet": bag(shipping.discounted),
            "currentDiscountedPriceSet": bag(shipping.discounted),
            "discountAllocations": [],
            "taxLines": [],
            "createdAt": created,
        })
    });
    let tax_lines: Vec<Value> = (draft.tax > 0)
        .then(|| json!({"title": "Tax", "priceSet": bag(draft.tax)}))
        .into_iter()
        .collect();
    let display_fulfillment = match draft.fulfillment {
        FulfillmentState::Fulfilled => "FULFILLED",
        FulfillmentState::Partial => "PARTIALLY_FULFILLED",
        FulfillmentState::Unfulfilled => "UNFULFILLED",
    };
    let financial = draft.financial;
    let mut record = value::object(json!({
        "__typename": "Order",
        "id": order,
        "name": draft.name,
        "number": draft.number,
        "createdAt": created,
        "processedAt": created,
        "updatedAt": created,
        "cancelledAt": if draft.cancelled { json!(created) } else { Value::Null },
        "cancelReason": if draft.cancelled { json!("OTHER") } else { Value::Null },
        "confirmed": true,
        "test": false,
        "email": draft.email,
        "phone": draft.phone,
        "customer": draft.customer,
        "purchasingEntity": draft.customer,
        "customerAcceptsMarketing": draft.accepts_marketing,
        "currencyCode": currency,
        "presentmentCurrencyCode": currency,
        "taxesIncluded": taxes_included,
        "taxExempt": draft.tax_exempt,
        "estimatedTaxes": false,
        "dutiesIncluded": false,
        "edited": false,
        "note": draft.note.as_deref().map_or(Value::Null, text),
        "tags": [],
        "customAttributes": attributes(&draft.attributes),
        "discountCode": draft.discount_codes.first(),
        "discountCodes": draft.discount_codes,
        "sourceName": draft.source_name,
        "lineItems": line_items,
        "shippingAddress": draft.shipping_address,
        "billingAddress": draft.billing_address,
        "displayAddress": draft.billing_address.as_ref().or(draft.shipping_address.as_ref()),
        "billingAddressMatchesShippingAddress": match (&draft.billing_address, &draft.shipping_address) {
            (Some(billing), Some(shipping)) => same_address(billing, shipping),
            _ => false,
        },
        "shippingLine": shipping_line,
        "shippingLines": shipping_line.iter().collect::<Vec<_>>(),
        "taxLines": tax_lines,
        "currentTaxLines": tax_lines,
    }));
    // The amounts, then the state of the order.
    record.extend(value::object(json!({
        "subtotalLineItemsQuantity": quantity,
        "currentSubtotalLineItemsQuantity": quantity,
        "subtotalPriceSet": bag(subtotal),
        "currentSubtotalPriceSet": bag(subtotal),
        "totalShippingPriceSet": bag(shipping_discounted),
        "currentShippingPriceSet": bag(shipping_discounted),
        "totalTaxSet": bag(draft.tax),
        "currentTotalTaxSet": bag(draft.tax),
        "totalDiscountsSet": bag(discounts),
        "currentTotalDiscountsSet": bag(discounts),
        "cartDiscountAmountSet": bag(draft.order_discount),
        "currentCartDiscountAmountSet": bag(draft.order_discount),
        "totalPriceSet": bag(total),
        "currentTotalPriceSet": bag(total),
        "originalTotalPriceSet": bag(total),
        "totalReceivedSet": bag(paid),
        "netPaymentSet": bag(paid - refunded),
        "totalRefundedSet": bag(refunded),
        "totalRefundedShippingSet": bag(0),
        "totalOutstandingSet": bag(if financial.captured() { (total - paid).max(0) } else { total }),
        "totalCapturableSet": bag(if financial == FinancialStatus::Authorized { total } else { 0 }),
        "totalTipReceivedSet": bag(0),
        "totalWeight": grams.to_string(),
        "currentTotalWeight": grams.to_string(),
    })));
    record.extend(value::object(json!({
        "displayFinancialStatus": financial.graphql(),
        "displayFulfillmentStatus": display_fulfillment,
        "fullyPaid": financial.captured() && paid >= total,
        "unpaid": matches!(financial, FinancialStatus::Pending | FinancialStatus::Authorized | FinancialStatus::Expired | FinancialStatus::Voided),
        "capturable": financial == FinancialStatus::Authorized,
        "canMarkAsPaid": matches!(financial, FinancialStatus::Pending | FinancialStatus::PartiallyPaid) && !draft.cancelled,
        "refundable": financial.captured() && financial != FinancialStatus::Refunded,
        "restockable": !draft.cancelled,
        "fulfillable": !draft.cancelled && !fully_fulfilled,
        "requiresShipping": draft.lines.iter().any(|line| line.requires_shipping),
        "returnStatus": return_status,
        "returns": returns,
        "fulfillments": if has_fulfillment { vec![fulfillment] } else { Vec::new() },
        "fulfillmentOrders": [fulfillment_order],
        "transactions": transactions,
        "refunds": [],
        "discountApplications": [],
        "metafields": [],
        "paymentTerms": null,
        "shopifyProtect": null,
    })));
    state.insert(record);
    if let Some(customer) = &draft.customer {
        customers::refresh_customer(state, customer);
    }
    order
}

/// The `LineItem` record of a line: `unfulfilled` of its units left to ship, refundable when
/// the order's money was taken.
pub fn line_item_record(
    id: &str,
    line: &LineDraft,
    currency: &str,
    unfulfilled: u32,
    captured: bool,
    created: &str,
) -> crate::state::Record {
    let bag = |cents: i64| value::money_bag(cents, currency);
    let original = line.unit_price * i64::from(line.quantity);
    let discounted = original - line.discount;
    let unit_after = if line.quantity == 0 {
        line.unit_price
    } else {
        discounted / i64::from(line.quantity)
    };
    let unfulfilled_share = |total: i64| {
        if line.quantity == 0 {
            0
        } else {
            model::share(total, i64::from(unfulfilled), i64::from(line.quantity))
        }
    };
    let name = match &line.variant_title {
        Some(variant) => format!("{} - {variant}", line.title),
        None => line.title.clone(),
    };
    let attributes: Vec<Value> = line
        .attributes
        .iter()
        .map(|(key, value)| json!({"key": key, "value": value}))
        .collect();
    value::object(json!({
        "__typename": "LineItem",
        "id": id,
        "name": name,
        "title": line.title,
        "variantTitle": line.variant_title,
        "sku": line.sku,
        "vendor": line.vendor,
        "quantity": line.quantity,
        "currentQuantity": line.quantity,
        "unfulfilledQuantity": unfulfilled,
        "nonFulfillableQuantity": 0,
        "refundableQuantity": if captured { line.quantity } else { 0 },
        "originalUnitPriceSet": bag(line.unit_price),
        "discountedUnitPriceSet": bag(unit_after),
        "discountedUnitPriceAfterAllDiscountsSet": bag(unit_after),
        "originalTotalSet": bag(original),
        "discountedTotalSet": bag(discounted),
        "totalDiscountSet": bag(line.discount),
        "unfulfilledOriginalTotalSet": bag(unfulfilled_share(original)),
        "unfulfilledDiscountedTotalSet": bag(unfulfilled_share(discounted)),
        "requiresShipping": line.requires_shipping,
        "taxable": line.taxable,
        "isGiftCard": line.gift_card,
        "restockable": true,
        "product": line.product,
        "variant": line.variant,
        "image": line.image,
        "weight": line.weight,
        "customAttributes": attributes,
        "discountAllocations": [],
        "taxLines": [],
        "duties": [],
        "sellingPlan": null,
        "createdAt": created,
    }))
}

/// Whether two `MailingAddress`es are the same place for the same person (their ids aside).
fn same_address(a: &Value, b: &Value) -> bool {
    [
        "firstName",
        "lastName",
        "company",
        "address1",
        "address2",
        "city",
        "provinceCode",
        "countryCodeV2",
        "zip",
    ]
    .iter()
    .all(|key| a.get(key) == b.get(key))
}

/// An order placed in the local checkout, as the checkout knows it once the buyer paid: what
/// the server's `place_order` has. [`record_placed_order`] makes it an order of the shop.
pub struct PlacedOrder<'a> {
    /// The id the checkout gave the order, kept when no order of the shop has it. `None`: a new
    /// id of the shop's counter, as `orderCreate` gives (`gid://shopify/Order/<n>`, a large `n`,
    /// not the order number).
    pub id: Option<&'a str>,
    /// The number of the order (`1004` for `#1004`), kept when no order of the shop has it.
    /// `None`: the shop's next one ([`model::next_order_number`]), as `orderCreate` and a
    /// completed draft order take, so that the checkout and the Admin API share one numbering.
    pub number: Option<u64>,
    pub processed_at: DateTime<Utc>,
    /// The lines, discounts, delivery and costs.
    pub result: &'a lsf_checkout::CheckoutResult,
    /// The logged-in customer, by their lsf-core id.
    pub customer_id: Option<u64>,
    /// The company location the buyer ordered for (a B2B checkout), by its lsf-core id: the
    /// order's `purchasingEntity` is then a `PurchasingCompany`, and the location's counters
    /// count it.
    pub company_location_id: Option<u64>,
    pub email: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub shipping_address: Option<&'a lsf_checkout::Address>,
    pub note: Option<&'a str>,
    pub attributes: &'a [(String, String)],
    /// The name of the payment method chosen: the gateway of the transaction.
    pub payment: Option<&'a str>,
}

/// What the shop recorded of an order placed in the local checkout: its GID and its number,
/// which the checkout shows on its thank-you page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedOrder {
    pub id: String,
    pub number: u64,
}

/// Records an order placed in the local checkout: the order, its lines, its fulfillment order,
/// the transaction of what was paid now; the customer's counters follow. Emits `orders/create`,
/// `order_transactions/create` for the payment, then `orders/paid` when paid in full, as Shopify
/// does for a paid order. The stock is left as it is: the local checkout keeps the stock of the
/// data, so that test purchases never sell a product out. Returns the GID and the number the
/// order got: no two orders of the shop share a number.
pub fn record_placed_order(state: &mut ShopState, placed: &PlacedOrder) -> RecordedOrder {
    let result = placed.result;
    let number = placed
        .number
        .filter(|number| {
            !state
                .all("Order")
                .any(|order| order.get("number").and_then(Value::as_u64) == Some(*number))
        })
        .unwrap_or_else(|| model::next_order_number(state));
    let id = placed
        .id
        .filter(|id| !state.contains(id) && crate::state::gid_type(id) == Some("Order"))
        .map_or_else(|| state.new_gid("Order"), str::to_string);
    // A guest whose email is a customer's places the order as that customer: Shopify links a
    // guest checkout to the customer account with the same email (Help Center behaviour, which
    // shopify.dev does not document). Emails compare ignoring case.
    let customer = match placed.customer_id {
        Some(id) => Some(gid("Customer", id)).filter(|customer| state.contains(customer)),
        None => placed
            .email
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .and_then(|email| {
                state.keys("Customer").find(|key| {
                    state
                        .get(key)
                        .and_then(|record| record.get("email"))
                        .and_then(Value::as_str)
                        .is_some_and(|known| known.eq_ignore_ascii_case(email))
                })
            })
            .map(str::to_string),
    };
    let customer_record = customer.as_ref().and_then(|customer| state.get(customer));
    let accepts_marketing = customer_record
        .and_then(|record| record.get("emailMarketingConsent"))
        .and_then(|consent| consent.get("marketingState"))
        .and_then(Value::as_str)
        == Some("SUBSCRIBED");
    let tax_exempt = customer_record
        .and_then(|record| record.get("taxExempt"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let lines = result
        .lines
        .iter()
        .map(|line| {
            let variant = gid("ProductVariant", line.variant_id);
            let variant_record = state.get(&variant);
            // The variant's own item: one the Admin API created has an id of the counter, not
            // the one lsf-core would derive.
            let inventory_item = variant_record
                .and_then(|record| record.get("inventoryItem"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let weight = inventory_item
                .as_deref()
                .and_then(|item| state.get(item))
                .and_then(|item| item.get("measurement"))
                .and_then(|measurement| measurement.get("weight"))
                .cloned();
            LineDraft {
                product: Some(gid("Product", line.product_id)),
                variant: Some(variant.clone()),
                inventory_item,
                title: line.title.clone(),
                variant_title: line
                    .variant_title
                    .clone()
                    .filter(|title| title != "Default Title"),
                sku: line.sku.clone().filter(|sku| !sku.is_empty()),
                vendor: Some(line.vendor.clone()).filter(|vendor| !vendor.is_empty()),
                quantity: line.quantity,
                unit_price: line.unit_price,
                discount: line.subtotal - line.total,
                requires_shipping: line.requires_shipping,
                taxable: variant_record
                    .and_then(|record| record.get("taxable"))
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                gift_card: state
                    .get(&gid("Product", line.product_id))
                    .and_then(|record| record.get("isGiftCard"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                image: None,
                weight,
                grams: 0,
                attributes: line
                    .attributes
                    .iter()
                    .map(|attribute| (attribute.key.clone(), attribute.value.clone()))
                    .collect(),
            }
        })
        .collect();
    let selected = result.delivery_groups.iter().find_map(|group| {
        let handle = group.selected.as_ref()?;
        group.options.iter().find(|option| &option.handle == handle)
    });
    let shipping = selected.map(|option| ShippingDraft {
        title: option.title.clone(),
        handle: Some(option.handle.clone()),
        original: result
            .delivery_groups
            .iter()
            .filter_map(|group| {
                let handle = group.selected.as_ref()?;
                group.options.iter().find(|option| &option.handle == handle)
            })
            .map(|option| option.original_cost)
            .sum(),
        discounted: result
            .delivery_groups
            .iter()
            .map(lsf_checkout::DeliveryGroup::selected_cost)
            .sum(),
    });
    let line_discounts: i64 = result
        .lines
        .iter()
        .map(|line| line.subtotal - line.total)
        .sum();
    let address = placed.shipping_address.map(|address| {
        let address = address.clone().normalized();
        let core = lsf_core::store::Address {
            id: stable_id("address", &format!("{id}/shipping")),
            first_name: address.first_name.unwrap_or_default(),
            last_name: address.last_name.unwrap_or_default(),
            company: address.company.unwrap_or_default(),
            address1: address.address1.unwrap_or_default(),
            address2: address.address2.unwrap_or_default(),
            city: address.city.unwrap_or_default(),
            province: String::new(),
            province_code: address.province_code.unwrap_or_default(),
            country: address
                .country_code
                .as_deref()
                .and_then(lsf_core::store::reference::country_name)
                .unwrap_or_default()
                .to_string(),
            country_code: address.country_code.unwrap_or_default(),
            zip: address.zip.unwrap_or_default(),
            phone: address.phone.unwrap_or_default(),
        };
        value::mailing_address(&core, "Address")
    });
    let location = state.keys("Location").next().map(str::to_string);
    let draft = OrderDraft {
        id: id.clone(),
        name: format!("#{number}"),
        number,
        created_at: placed.processed_at,
        customer,
        email: placed.email.map(str::to_string),
        phone: placed.phone.map(str::to_string),
        accepts_marketing,
        tax_exempt,
        lines,
        billing_address: address.clone(),
        shipping_address: address,
        shipping,
        tax: result.cost.tax,
        order_discount: (result.cost.discounts - line_discounts).max(0),
        discount_codes: result
            .discount_codes
            .iter()
            .filter(|status| status.applicable)
            .map(|status| status.code.clone())
            .collect(),
        note: placed.note.map(str::to_string),
        attributes: placed.attributes.to_vec(),
        financial: if result.cost.due_later > 0 {
            FinancialStatus::PartiallyPaid
        } else {
            FinancialStatus::Paid
        },
        fulfillment: FulfillmentState::Unfulfilled,
        cancelled: false,
        paid: Some(result.cost.due_now),
        gateway: placed.payment.map(str::to_string),
        source_name: Some("web".to_string()),
        returns: Vec::new(),
        location,
    };
    let order = insert_order(state, &draft);
    record_discounts(state, &order, result);
    let location = placed
        .company_location_id
        .map(|id| gid("CompanyLocation", id))
        .filter(|location| state.contains(location));
    if let Some(location) = &location {
        let company = state
            .get(location)
            .and_then(|record| record.get("company"))
            .cloned()
            .unwrap_or(Value::Null);
        // The buyer's contact at the company: `CompanyContact/<customer id>`, the seed's.
        let contact = draft
            .customer
            .as_deref()
            .and_then(crate::state::parse_gid)
            .map(|(_, id)| gid("CompanyContact", id))
            .filter(|contact| state.contains(contact));
        if let Some(record) = state.get_mut(&order) {
            record.insert(
                "purchasingEntity".into(),
                json!({"__typename": "PurchasingCompany", "company": company, "location": location, "contact": contact}),
            );
        }
        customers::refresh_company_location(state, location);
    }
    state.emit("orders/create", &order);
    // What Shopify fires for a paid online order: the order, the transaction of the payment,
    // then `orders/paid`.
    for transaction in state
        .get(&order)
        .map(|record| gids(record.get("transactions")))
        .unwrap_or_default()
    {
        state.emit("order_transactions/create", &transaction);
        payments::emit_tender(state, &transaction);
    }
    if draft.financial == FinancialStatus::Paid {
        state.emit("orders/paid", &order);
    }
    // What is left to pay later (a deposit) is charged to the card the buyer paid with.
    if result.cost.due_later > 0 {
        let name = draft
            .customer
            .as_deref()
            .and_then(|customer| state.get(customer))
            .and_then(|customer| customer.get("displayName"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        payments::vault_checkout_card(state, &order, &name);
    }
    RecordedOrder { id: order, number }
}

/// The discounts of a checkout as its order keeps them: a `DiscountApplication` for each, in the
/// checkout's order (an `AutomaticDiscountApplication`, or a `DiscountCodeApplication` for one of
/// a code), and on each line and on the shipping line the `DiscountAllocation`s the discounts
/// made, which name their application. What `Order.discountApplications`, the lines'
/// `discountAllocations` and the `discount_applications` of the order webhooks read.
fn record_discounts(state: &mut ShopState, order: &str, result: &lsf_checkout::CheckoutResult) {
    let Some(currency) = state
        .get(order)
        .and_then(|record| record.get("currencyCode"))
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let applications: Vec<Value> = result
        .discounts
        .iter()
        .enumerate()
        .map(|(index, discount)| {
            let details = &discount.details;
            let value = match details.value {
                lsf_checkout::DiscountValue::Percentage(percentage) => {
                    json!({"__typename": "PricingPercentageValue", "percentage": percentage})
                }
                lsf_checkout::DiscountValue::Fixed(cents) => {
                    let mut money = value::money_v2(cents, &currency);
                    money["__typename"] = json!("MoneyV2");
                    money
                }
            };
            let mut application = json!({
                "index": index,
                "allocationMethod": details.allocation_method.graphql(),
                "targetSelection": details.target_selection.graphql(),
                "targetType": details.target_type.graphql(),
                "value": value,
            });
            match &discount.code {
                Some(code) => {
                    application["__typename"] = json!("DiscountCodeApplication");
                    application["code"] = json!(code);
                }
                None => {
                    application["__typename"] = json!("AutomaticDiscountApplication");
                    application["title"] = json!(discount.title);
                }
            }
            application
        })
        .collect();
    if applications.is_empty() {
        return;
    }
    let allocations = |made: &[lsf_checkout::DiscountAllocation]| -> Value {
        made.iter()
            .filter_map(|allocation| {
                Some(json!({
                    "allocatedAmountSet": value::money_bag(allocation.amount, &currency),
                    "discountApplication": applications.get(allocation.discount)?,
                }))
            })
            .collect()
    };
    for (line, item) in result.lines.iter().zip(line_items(state, order)) {
        let made = allocations(&line.discount_allocations);
        if let Some(record) = state.get_mut(&item) {
            record.insert("discountAllocations".to_string(), made);
        }
    }
    let shipping: Vec<lsf_checkout::DiscountAllocation> = result
        .delivery_groups
        .iter()
        .flat_map(|group| group.discount_allocations.iter().cloned())
        .collect();
    let shipping = allocations(&shipping);
    if let Some(record) = state.get_mut(order) {
        for key in ["shippingLine", "shippingLines"] {
            match record.get_mut(key) {
                Some(Value::Object(line)) => {
                    line.insert("discountAllocations".to_string(), shipping.clone());
                }
                Some(Value::Array(lines)) => {
                    for line in lines.iter_mut().filter_map(Value::as_object_mut) {
                        line.insert("discountAllocations".to_string(), shipping.clone());
                    }
                }
                _ => {}
            }
        }
        record.insert(
            "discountApplications".to_string(),
            Value::Array(applications.clone()),
        );
    }
}

/// The line items of an order record.
pub fn line_items(state: &ShopState, order: &str) -> Vec<String> {
    state
        .get(order)
        .map(|record| gids(record.get("lineItems")))
        .unwrap_or_default()
}

/// What happens when the shop's clock passes a date: the payment schedules that came due fire
/// `payment_schedules/due` ([`payments::tick`]), the store credit that expired leaves its
/// account ([`store_credit::tick`]). Runs before every request of the Admin API,
/// and when the clock is moved (`POST /__lsf/admin/clock`).
pub fn tick(state: &mut ShopState) {
    payments::tick(state);
    store_credit::tick(state);
}

/// The custom resolvers and the mutations of this family: see "Writing a resolver or a
/// mutation" in `crates/admin/docs/design.md`.
pub fn register(registry: &mut crate::resolve::Registry) {
    queries::register(registry);
    lifecycle::register(registry);
    fulfillments::register(registry);
    refunds::register(registry);
    returns::register(registry);
    edits::register(registry);
    drafts::register(registry);
    services::register(registry);
    payments::register(registry);
    risk::register(registry);
    store_credit::register(registry);
}

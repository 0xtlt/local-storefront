//! The storefront's carts and checkouts as the Admin API sees them: the `carts/create`,
//! `carts/update`, `checkouts/create`, `checkouts/update` and `checkouts/delete` webhooks, the
//! abandoned checkouts (`abandonedCheckouts`, `abandonedCheckoutsCount`), the abandonments of
//! customers (`abandonment`, `abandonmentByAbandonedCheckoutId`), and
//! `abandonmentUpdateActivitiesDeliveryStatuses`.
//!
//! The server reports what the buyer does (`lsf`'s cart routes and local checkout), through the
//! default shop (`AdminApi::change_shop`):
//!
//! - **Carts** ([`record_cart`]): a cart that gets its first line, note or attribute is
//!   `carts/create` ("Occurs when a cart is created in the online store"); each later change of
//!   its lines, note or attributes `carts/update`. A cart is no record of the Admin API (its
//!   GraphQL has none): the payload is built from the event, its `id` and `token` the cart's
//!   token, its prices the variants' in the Admin API.
//! - **Checkouts** ([`record_checkout`]): the first time the checkout of a cart is computed it is
//!   `checkouts/create`, then each change of what the buyer sees (lines, contact, address,
//!   delivery, payment, discount codes, totals) `checkouts/update`; placing the order
//!   ([`complete_checkout`]) sets `completedAt` (`checkouts/update`). Each checkout is an
//!   `AbandonedCheckout` record from its creation, numbered by the shop's counter (`name` is
//!   `#<n>`), with its lines, totals, addresses and the customer (the one logged in, or the
//!   customer whose email the buyer typed, as orders link guests). A checkout of the same cart
//!   after an order is another one.
//! - **Abandonment**: "A checkout is considered abandoned if it remains incomplete for more than
//!   ten minutes" (Help Center, "Recovering abandoned checkouts"), and only once "a customer
//!   adds contact information" (shopify.dev, `abandonedCheckouts`). `abandonedCheckouts` lists
//!   those, by the shop's clock, and those recovered (completed after being abandoned: "Includes
//!   both abandoned and recovered checkouts"). Its filters: `status` (`open`, `closed`),
//!   `recovery_state` (`recovered`, `not_recovered`), `email_state` (`sent`, `not_sent`,
//!   `scheduled`), `created_at`, `updated_at`, `id`; its sort keys include `CUSTOMER_NAME` and
//!   `TOTAL_PRICE`. "Abandoned checkouts that are older than three months are automatically
//!   deleted": [`delete_expired`] deletes them (`checkouts/delete`, `{"id": <n>}`).
//! - **Abandonments**: a checkout of a customer (an `Abandonment` needs one: `customer:
//!   Customer!`) has an `Abandonment` of type `CHECKOUT`, readable once the checkout is
//!   abandoned; its other fields are computed when read (whether the customer ordered since,
//!   hours since their last abandoned checkout, whether the products are still available, the
//!   most significant one being the customer's latest). Its `app` is the app's record *(chosen:
//!   lsf has no record of the Online Store app)*; lsf sends no abandoned checkout email, so
//!   `emailState` is `null` until the app reports one with
//!   `abandonmentUpdateActivitiesDeliveryStatuses` (`ABANDONMENT_NOT_FOUND`,
//!   `MARKETING_ACTIVITY_NOT_FOUND`), which sets `emailState` and, when `SENT`, `emailSentAt`.
//!
//! Carts and checkouts are not store data: they do not sync. The payloads follow the examples of
//! shopify.dev's webhook reference, field by field.

use chrono::{DateTime, Duration, Utc};
use lsf_core::util::stable_id;
use serde_json::{Value, json};

use crate::domains::catalog::cents_of;
use crate::domains::orders::model;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::sort::SortValue;
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::SearchValue;
use crate::state::value::{self, money_bag};
use crate::state::{Record, ShopState, gid};

/// How long a checkout stays incomplete before it is abandoned (Help Center: "more than ten
/// minutes").
const ABANDONMENT_DELAY_MINUTES: i64 = 10;

/// How long the admin keeps an abandoned checkout ("older than three months are automatically
/// deleted").
const RETENTION_DAYS: i64 = 90;

/// A line of a cart, as the server reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CartLine {
    pub variant_id: u64,
    pub quantity: u32,
    pub properties: Vec<(String, String)>,
    /// The line's key, as `/cart.js` gives it.
    pub key: String,
}

/// A cart of the storefront, as the server reports it after a change.
#[derive(Clone, Debug)]
pub struct Cart<'a> {
    pub token: &'a str,
    pub note: &'a str,
    pub attributes: &'a [(String, String)],
    pub lines: &'a [CartLine],
}

/// A checkout of the local checkout, as the server computed it.
#[derive(Clone, Debug)]
pub struct Checkout<'a> {
    /// The token of the cart the checkout is of.
    pub cart_token: &'a str,
    /// How many orders the session placed before this checkout: a cart's checkouts after an
    /// order are other checkouts.
    pub generation: u64,
    pub result: &'a lsf_checkout::CheckoutResult,
    pub customer_id: Option<u64>,
    pub email: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub shipping_address: Option<&'a lsf_checkout::Address>,
    pub note: &'a str,
    pub attributes: &'a [(String, String)],
    /// The name of the payment method chosen: the checkout's gateway.
    pub payment: Option<&'a str>,
    /// The language of the checkout (`fr`).
    pub locale: Option<&'a str>,
}

pub fn register(registry: &mut Registry) {
    registry
        .mutation("abandonmentEmailStateUpdate", email_state_update)
        .mutation(
            "abandonmentUpdateActivitiesDeliveryStatuses",
            update_delivery_statuses,
        )
        .query("abandonedCheckouts", abandoned_checkouts)
        .query("abandonedCheckoutsCount", abandoned_checkouts_count)
        .query("abandonment", abandonment)
        .query("abandonmentByAbandonedCheckoutId", abandonment_by_checkout)
        .field(
            "Abandonment",
            "customerHasNoOrderSinceAbandonment",
            |r, _ctx, object| Ok(Output::value(!customer_ordered_since(r, object, "Order"))),
        )
        .field(
            "Abandonment",
            "customerHasNoDraftOrderSinceAbandonment",
            |r, _ctx, object| {
                Ok(Output::value(!customer_ordered_since(
                    r,
                    object,
                    "DraftOrder",
                )))
            },
        )
        .field(
            "Abandonment",
            "daysSinceLastAbandonmentEmail",
            |r, _ctx, object| {
                let sent = r
                    .map_of(object)
                    .and_then(|record| value::date_time_of(record.get("emailSentAt")));
                Ok(Output::value(sent.map_or(0, |sent| {
                    (r.state().now() - sent).num_days().max(0)
                })))
            },
        )
        .field(
            "Abandonment",
            "hoursSinceLastAbandonedCheckout",
            |r, _ctx, object| {
                let customer = r
                    .map_of(object)
                    .and_then(|record| record.get("customer"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Ok(match last_abandonment(r.state(), customer) {
                    Some(at) => {
                        Output::value((r.state().now() - at).num_seconds().max(0) as f64 / 3600.0)
                    }
                    None => Output::Null,
                })
            },
        )
        .field("Abandonment", "inventoryAvailable", |r, _ctx, object| {
            Ok(Output::value(inventory_available(r, object)))
        })
        .field(
            "Abandonment",
            "isMostSignificantAbandonment",
            |r, _ctx, object| {
                let customer = r
                    .map_of(object)
                    .and_then(|record| record.get("customer"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let latest = latest_abandonment_of(r.state(), customer);
                Ok(Output::value(latest.as_deref() == object.key()))
            },
        )
        .field(
            "Abandonment",
            "lastBrowseAbandonmentDate",
            last_abandonment_date,
        )
        .field(
            "Abandonment",
            "lastCartAbandonmentDate",
            last_abandonment_date,
        )
        .field(
            "Abandonment",
            "lastCheckoutAbandonmentDate",
            last_abandonment_date,
        )
        .search_field("AbandonedCheckout", "status", |_r, _item, record| {
            let closed = record.get("completedAt").is_some_and(|at| !at.is_null());
            Some(vec![SearchValue::Keyword(
                if closed { "closed" } else { "open" }.into(),
            )])
        })
        .search_field(
            "AbandonedCheckout",
            "recovery_state",
            |_r, _item, record| {
                let recovered = record.get("lsfRecovered").and_then(Value::as_bool) == Some(true);
                Some(vec![SearchValue::Keyword(
                    if recovered {
                        "recovered"
                    } else {
                        "not_recovered"
                    }
                    .into(),
                )])
            },
        )
        .search_field("AbandonedCheckout", "email_state", |r, _item, record| {
            let state = record
                .get("lsfAbandonment")
                .and_then(Value::as_str)
                .and_then(|abandonment| r.state().get(abandonment))
                .and_then(|abandonment| abandonment.get("emailState"))
                .and_then(Value::as_str)
                .unwrap_or("NOT_SENT")
                .to_ascii_lowercase();
            Some(vec![SearchValue::Keyword(state)])
        })
        .sort_key("AbandonedCheckout", "CHECKOUT_ID", |_r, item| {
            SortValue::of(
                item.as_str()
                    .and_then(crate::state::parse_gid)
                    .map(|(_, id)| json!(id))
                    .as_ref(),
            )
        })
        .sort_key("AbandonedCheckout", "TOTAL_PRICE", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|record| record.get("totalPriceSet"))
                    .and_then(|set| set.pointer("/shopMoney/amount")),
            )
        })
        .sort_key("AbandonedCheckout", "CUSTOMER_NAME", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|record| record.get("customer"))
                    .and_then(Value::as_str)
                    .and_then(|customer| r.state().get(customer))
                    .and_then(|customer| customer.get("displayName")),
            )
        })
        .default_text("AbandonedCheckout", |r, record| {
            let mut texts: Vec<String> = ["name", "lsfEmail"]
                .iter()
                .filter_map(|field| record.get(*field).and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            if let Some(name) = record
                .get("customer")
                .and_then(Value::as_str)
                .and_then(|customer| r.state().get(customer))
                .and_then(|customer| customer.get("displayName"))
                .and_then(Value::as_str)
            {
                texts.push(name.to_string());
            }
            texts
        });
}

// --- Carts ---

/// What the webhooks of a cart report of its lines: the variants' prices and details as the
/// Admin API has them.
fn cart_lines(state: &ShopState, lines: &[CartLine]) -> Vec<Value> {
    lines
        .iter()
        .map(|line| {
            let variant_gid = gid("ProductVariant", line.variant_id);
            let variant = state.get(&variant_gid);
            let product = variant
                .and_then(|variant| variant.get("product"))
                .and_then(Value::as_str)
                .and_then(|product| state.get(product));
            let price = cents_of(variant.and_then(|variant| variant.get("price"))).unwrap_or(0);
            let title = product
                .and_then(|product| product.get("title"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let variant_title = variant
                .and_then(|variant| variant.get("title"))
                .and_then(Value::as_str)
                .filter(|title| *title != "Default Title");
            let weight = variant
                .and_then(|variant| variant.get("inventoryItem"))
                .and_then(Value::as_str)
                .and_then(|item| state.get(item))
                .and_then(|item| item.get("measurement"))
                .and_then(|measurement| measurement.get("weight"))
                .cloned();
            json!({
                "id": line.variant_id,
                "variant": variant_gid,
                "product": variant.and_then(|variant| variant.get("product")),
                "key": line.key,
                "quantity": line.quantity,
                "properties": line.properties.iter().map(|(key, value)| json!({"key": key, "value": value})).collect::<Vec<_>>(),
                "price": value::money(price),
                "linePrice": value::money(price.saturating_mul(i64::from(line.quantity))),
                "title": match variant_title {
                    Some(variant_title) => format!("{title} - {variant_title}"),
                    None => title.to_string(),
                },
                "sku": variant.and_then(|variant| variant.get("sku")),
                "taxable": variant.and_then(|variant| variant.get("taxable")).cloned().unwrap_or(json!(true)),
                "vendor": product.and_then(|product| product.get("vendor")),
                "giftCard": product.and_then(|product| product.get("isGiftCard")).cloned().unwrap_or(json!(false)),
                "weight": weight,
            })
        })
        .collect()
}

/// Records a change of a storefront cart: `carts/create` for a cart that had nothing, else
/// `carts/update`. A cart that is still empty is no cart: nothing happens.
pub fn record_cart(state: &mut ShopState, cart: &Cart<'_>) {
    let empty = cart.lines.is_empty() && cart.note.is_empty() && cart.attributes.is_empty();
    let memo = format!("carts/{}", cart.token);
    let known = state.memo(&memo).cloned();
    if known.is_none() && empty {
        return;
    }
    let now = state.timestamp();
    let created = known
        .as_ref()
        .and_then(|known| known.get("createdAt").cloned())
        .unwrap_or_else(|| json!(now));
    let topic = if known.is_none() {
        "carts/create"
    } else {
        "carts/update"
    };
    state.set_memo(memo, json!({"createdAt": created}));
    let snapshot = value::object(json!({
        "__typename": "Cart",
        "token": cart.token,
        "note": cart.note,
        "attributes": cart.attributes.iter().map(|(key, value)| json!({"key": key, "value": value})).collect::<Vec<_>>(),
        "lineItems": cart_lines(state, cart.lines),
        "currencyCode": model::currency(state),
        "createdAt": created,
        "updatedAt": now,
    }));
    state.emit_with(topic, &format!("cart/{}", cart.token), snapshot);
}

// --- Checkouts ---

fn checkout_key(cart_token: &str, generation: u64) -> String {
    format!("checkouts/{cart_token}/{generation}")
}

/// The `AbandonedCheckout` of a cart's checkout, if it has one.
fn checkout_of(state: &ShopState, cart_token: &str, generation: u64) -> Option<String> {
    state
        .memo(&checkout_key(cart_token, generation))
        .and_then(Value::as_str)
        .filter(|id| state.contains(id))
        .map(str::to_string)
}

/// A customer of the shop by email, ignoring case: the customer a guest's checkout is linked to.
pub(super) fn customer_by_email(state: &ShopState, email: &str) -> Option<String> {
    let email = email.trim();
    if email.is_empty() {
        return None;
    }
    state
        .all("Customer")
        .find(|customer| {
            customer
                .get("email")
                .and_then(Value::as_str)
                .is_some_and(|known| known.eq_ignore_ascii_case(email))
        })
        .and_then(|customer| customer.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

/// A `MailingAddress` of an address of the checkout, its id lsf-core's of `key`.
pub(super) fn mailing_address(address: &lsf_checkout::Address, key: &str) -> Value {
    let address = address.clone().normalized();
    let core = lsf_core::store::Address {
        id: stable_id("address", key),
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
}

/// The fields of a checkout's record as the result gives them.
fn checkout_fields(state: &ShopState, id: &str, checkout: &Checkout<'_>) -> Record {
    let result = checkout.result;
    let currency = if result.cost.currency_code.is_empty() {
        model::currency(state)
    } else {
        result.cost.currency_code.to_ascii_uppercase()
    };
    let bag = |cents: i64| money_bag(cents, &currency);
    let lines: Vec<Value> = result
        .lines
        .iter()
        .enumerate()
        .map(|(position, line)| {
            let variant = gid("ProductVariant", line.variant_id);
            let quantity = i64::from(line.quantity);
            let unit_after = if quantity > 0 { line.total / quantity } else { line.total };
            json!({
                "id": gid("AbandonedCheckoutLineItem", stable_id("abandoned_checkout_line_item", &format!("{id}/{position}"))),
                // The key of the line in the checkout's webhooks: 32 hexadecimal digits, as
                // Shopify's.
                "lsfKey": value::uuid(&format!("{id}/{}/{position}", line.id)).replace('-', ""),
                "title": line.title,
                "variantTitle": line.variant_title.clone().filter(|title| title != "Default Title"),
                "sku": line.sku,
                "quantity": line.quantity,
                "product": gid("Product", line.product_id),
                "variant": variant,
                "image": line.image.as_ref().map(|url| json!({"url": url, "altText": line.image_alt})),
                "originalUnitPriceSet": bag(line.unit_price),
                "originalTotalPriceSet": bag(line.subtotal),
                "discountedUnitPriceSet": bag(unit_after),
                "discountedTotalPriceSet": bag(line.total),
                "discountedUnitPriceWithCodeDiscount": bag(unit_after),
                "discountedTotalPriceWithCodeDiscount": bag(line.total),
                "customAttributes": line.attributes.iter().map(|attribute| json!({"key": attribute.key, "value": attribute.value})).collect::<Vec<_>>(),
                "discountAllocations": [],
                "components": null,
                "parentRelationship": null,
                "requiresShipping": line.requires_shipping,
                "vendor": line.vendor,
            })
        })
        .collect();
    let quantity: u32 = result.lines.iter().map(|line| line.quantity).sum();
    let address = checkout
        .shipping_address
        .filter(|address| address.country_code.is_some() || address.address1.is_some())
        .map(|address| mailing_address(address, &format!("{id}/checkout")));
    let shipping: Vec<Value> = result
        .delivery_groups
        .iter()
        .filter_map(|group| {
            let handle = group.selected.as_ref()?;
            let option = group
                .options
                .iter()
                .find(|option| &option.handle == handle)?;
            Some(json!({
                "title": option.title,
                "handle": option.handle,
                "price": value::money(group.selected_cost()),
            }))
        })
        .collect();
    let rate = super::orders::tax_rate_setting(state);
    let mut fields = value::object(json!({
        "updatedAt": state.timestamp(),
        "lineItems": lines,
        "lineItemsQuantity": quantity,
        "subtotalPriceSet": bag(result.cost.subtotal - result.cost.discounts),
        "totalLineItemsPriceSet": bag(result.cost.subtotal),
        "totalDiscountSet": bag(result.cost.discounts),
        "totalTaxSet": bag(result.cost.tax),
        "totalPriceSet": bag(result.cost.total),
        "totalDutiesSet": null,
        "taxLines": if result.cost.tax > 0 {
            json!([{"title": "Tax", "rate": rate as f64 / 10_000.0, "priceSet": bag(result.cost.tax), "channelLiable": false}])
        } else {
            json!([])
        },
        "taxesIncluded": false,
        "discountCodes": result.discount_codes.iter().filter(|code| code.applicable).map(|code| code.code.clone()).collect::<Vec<_>>(),
        "note": checkout.note,
        "customAttributes": checkout.attributes.iter().map(|(key, value)| json!({"key": key, "value": value})).collect::<Vec<_>>(),
        "shippingAddress": address,
        "billingAddress": address,
        // What the webhooks report that GraphQL's `AbandonedCheckout` does not show.
        "lsfEmail": checkout.email.filter(|email| !email.trim().is_empty()),
        "lsfPhone": checkout.phone.filter(|phone| !phone.trim().is_empty()),
        "lsfGateway": checkout.payment,
        "lsfShippingLines": shipping,
        "lsfLocale": checkout.locale,
        "lsfCurrency": currency,
    }));
    let customer = checkout
        .customer_id
        .map(|id| gid("Customer", id))
        .filter(|customer| state.contains(customer))
        .or_else(|| {
            checkout
                .email
                .and_then(|email| customer_by_email(state, email))
        });
    fields.insert("customer".into(), json!(customer));
    fields
}

/// What a webhook reports of a checkout: the fields that change it, without the dates.
fn fingerprint(fields: &Record) -> Value {
    let mut fields = fields.clone();
    fields.remove("updatedAt");
    Value::Object(fields)
}

/// Records the checkout of a cart as the server just computed it: `checkouts/create` the first
/// time, `checkouts/update` when what it reports changed. Answers the `AbandonedCheckout`.
pub fn record_checkout(state: &mut ShopState, checkout: &Checkout<'_>) -> Option<String> {
    if checkout.result.lines.is_empty() {
        return checkout_of(state, checkout.cart_token, checkout.generation);
    }
    delete_expired(state);
    let now = state.timestamp();
    match checkout_of(state, checkout.cart_token, checkout.generation) {
        Some(id) => {
            if state
                .get(&id)
                .is_some_and(|record| record.get("completedAt").is_some_and(|at| !at.is_null()))
            {
                return Some(id);
            }
            let fields = checkout_fields(state, &id, checkout);
            let before = state.get(&id).map(|record| {
                let mut known = Record::new();
                for key in fields.keys() {
                    if let Some(value) = record.get(key) {
                        known.insert(key.clone(), value.clone());
                    }
                }
                fingerprint(&known)
            });
            if before.as_ref() == Some(&fingerprint(&fields)) {
                return Some(id);
            }
            if let Some(record) = state.get_mut(&id) {
                record.extend(fields);
                record.insert("lsfLastActivityAt".into(), json!(now));
            }
            ensure_abandonment(state, &id);
            state.emit("checkouts/update", &id);
            Some(id)
        }
        None => {
            let id = state.new_gid("AbandonedCheckout");
            let number = model::number(&id);
            let domain = state
                .shop()
                .and_then(|shop| shop.get("primaryDomain"))
                .and_then(Value::as_str)
                .and_then(|domain| state.get(domain))
                .and_then(|domain| domain.get("host"))
                .and_then(Value::as_str)
                .unwrap_or(state.domain())
                .to_string();
            let shop_number = state.shop_gid().map_or(0, model::number);
            let key = value::uuid(&format!("abandoned-checkout-key/{id}")).replace('-', "");
            let mut record = value::object(json!({
                "__typename": "AbandonedCheckout",
                "id": id,
                "name": format!("#{number}"),
                "abandonedCheckoutUrl": format!(
                    "https://{domain}/{shop_number}/checkouts/ac/{}/recover?key={key}",
                    checkout.cart_token
                ),
                "createdAt": now,
                "completedAt": null,
                "lsfToken": value::uuid(&format!("checkout-token/{id}")).replace('-', ""),
                "lsfCartToken": checkout.cart_token,
                "lsfLastActivityAt": now,
                "lsfRecovered": false,
            }));
            record.extend(checkout_fields(state, &id, checkout));
            state.insert_as(id.clone(), record);
            state.set_memo(
                checkout_key(checkout.cart_token, checkout.generation),
                json!(id),
            );
            ensure_abandonment(state, &id);
            state.emit("checkouts/create", &id);
            Some(id)
        }
    }
}

/// Gives a checkout of a customer its `Abandonment`, when it has none yet.
fn ensure_abandonment(state: &mut ShopState, checkout: &str) {
    let Some(record) = state.get(checkout) else {
        return;
    };
    if record
        .get("lsfAbandonment")
        .is_some_and(|known| !known.is_null())
    {
        return;
    }
    let Some(customer) = record
        .get("customer")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let created = record.get("createdAt").cloned().unwrap_or(Value::Null);
    let products: Vec<Value> = record
        .get("lineItems")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|line| {
            json!({
                "product": line.get("product"),
                "variant": line.get("variant"),
                "quantity": line.get("quantity"),
            })
        })
        .collect();
    let app = state.keys("App").next().map(str::to_string);
    let id = state.new_gid("Abandonment");
    state.insert_as(
        id.clone(),
        value::object(json!({
            "__typename": "Abandonment",
            "id": id,
            "abandonmentType": "CHECKOUT",
            "mostRecentStep": "CHECKOUT",
            "abandonedCheckoutPayload": checkout,
            "app": app,
            "cartUrl": null,
            "createdAt": created,
            "customer": customer,
            "emailState": null,
            "emailSentAt": null,
            "isFromOnlineStore": true,
            "isFromShopApp": false,
            "isFromShopPay": false,
            "isFromCustomStorefront": false,
            "visitStartedAt": null,
            "productsAddedToCart": products,
            "productsViewed": [],
            "lsfDeliveries": [],
        })),
    );
    if let Some(record) = state.get_mut(checkout) {
        record.insert("lsfAbandonment".into(), json!(id));
    }
}

/// Records that the checkout of a cart was completed by an order: `completedAt` is set, and a
/// checkout that was abandoned before is recovered.
pub fn complete_checkout(state: &mut ShopState, cart_token: &str, generation: u64) {
    let Some(id) = checkout_of(state, cart_token, generation) else {
        return;
    };
    let now = state.now();
    let Some(record) = state.get(&id) else {
        return;
    };
    if record.get("completedAt").is_some_and(|at| !at.is_null()) {
        return;
    }
    let recovered = is_abandoned_at(record, now);
    if let Some(record) = state.get_mut(&id) {
        record.insert("completedAt".into(), json!(value::date_time(now)));
        record.insert("updatedAt".into(), json!(value::date_time(now)));
        record.insert("lsfRecovered".into(), json!(recovered));
    }
    state.emit("checkouts/update", &id);
}

/// Whether a checkout is abandoned at `now`: it has contact information, and it was left
/// incomplete for more than the delay.
fn is_abandoned_at(record: &Record, now: DateTime<Utc>) -> bool {
    let contact = ["lsfEmail", "lsfPhone"].iter().any(|field| {
        record
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    });
    let last = value::date_time_of(record.get("lsfLastActivityAt"));
    contact && last.is_some_and(|last| now - last > Duration::minutes(ABANDONMENT_DELAY_MINUTES))
}

/// Whether `abandonedCheckouts` lists a checkout: abandoned now, or recovered.
pub fn is_listed(state: &ShopState, record: &Record) -> bool {
    let completed = record.get("completedAt").is_some_and(|at| !at.is_null());
    if completed {
        return record.get("lsfRecovered").and_then(Value::as_bool) == Some(true);
    }
    is_abandoned_at(record, state.now())
}

/// When the customer last abandoned a checkout (its last activity, if it is abandoned now):
/// what `abandoned_checkout_date` reads.
pub fn last_abandonment(state: &ShopState, customer: &str) -> Option<DateTime<Utc>> {
    state
        .all("AbandonedCheckout")
        .filter(|record| record.get("customer").and_then(Value::as_str) == Some(customer))
        .filter(|record| is_listed(state, record))
        .filter_map(|record| value::date_time_of(record.get("lsfLastActivityAt")))
        .max()
}

/// The customer's latest abandonment that is listed.
fn latest_abandonment_of(state: &ShopState, customer: &str) -> Option<String> {
    state
        .all("Abandonment")
        .filter(|record| record.get("customer").and_then(Value::as_str) == Some(customer))
        .filter(|record| abandonment_listed(state, record))
        .max_by_key(|record| value::date_time_of(record.get("createdAt")))
        .and_then(|record| record.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

/// Whether an abandonment is readable: its checkout is abandoned (or recovered).
fn abandonment_listed(state: &ShopState, abandonment: &Record) -> bool {
    abandonment
        .get("abandonedCheckoutPayload")
        .and_then(Value::as_str)
        .and_then(|checkout| state.get(checkout))
        .is_some_and(|checkout| is_listed(state, checkout))
}

/// Deletes the abandoned checkouts older than three months, as the admin does:
/// `checkouts/delete` for each, its abandonment with it.
pub fn delete_expired(state: &mut ShopState) -> usize {
    let now = state.now();
    let expired: Vec<String> = state
        .all("AbandonedCheckout")
        .filter(|record| {
            value::date_time_of(record.get("lsfLastActivityAt"))
                .is_some_and(|last| now - last > Duration::days(RETENTION_DAYS))
        })
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    for id in &expired {
        let abandonment = state
            .get(id)
            .and_then(|record| record.get("lsfAbandonment"))
            .and_then(Value::as_str)
            .map(str::to_string);
        state.delete(id, "checkouts/delete");
        if let Some(abandonment) = abandonment {
            state.remove(&abandonment);
        }
    }
    expired.len()
}

// --- Queries ---

fn listed_checkouts(r: &ShopResolver<'_>) -> Vec<Value> {
    r.state()
        .all("AbandonedCheckout")
        .filter(|record| is_listed(r.state(), record))
        .filter_map(|record| record.get("id").cloned())
        .collect()
}

fn abandoned_checkouts(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, listed_checkouts(r))
}

fn abandoned_checkouts_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = r.search(ctx, "AbandonedCheckout", listed_checkouts(r));
    Ok(r.count(ctx, items.len()))
}

fn abandonment(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let state = r.state();
    Ok(Output::object_or_null(
        state
            .get_of(id, "Abandonment")
            .filter(|record| abandonment_listed(state, record))
            .map(|_| Obj::record(id)),
    ))
}

fn abandonment_by_checkout(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let checkout = ctx.arg_str("abandonedCheckoutId").unwrap_or_default();
    let state = r.state();
    let found = state
        .get(checkout)
        .filter(|record| is_listed(state, record))
        .and_then(|record| record.get("lsfAbandonment"))
        .and_then(Value::as_str)
        .filter(|abandonment| state.contains(abandonment));
    Ok(Output::object_or_null(found.map(Obj::record)))
}

/// Whether the customer of an abandonment placed an order (a draft order) since.
fn customer_ordered_since(r: &ShopResolver<'_>, object: &Obj, type_name: &str) -> bool {
    let Some(record) = r.map_of(object) else {
        return false;
    };
    let customer = record.get("customer").and_then(Value::as_str);
    let since = value::date_time_of(record.get("createdAt"));
    r.state().all(type_name).any(|order| {
        order.get("customer").and_then(Value::as_str) == customer
            && value::date_time_of(order.get("createdAt"))
                .zip(since)
                .is_some_and(|(at, since)| at >= since)
    })
}

/// Whether the variants of an abandonment's checkout are still for sale.
fn inventory_available(r: &ShopResolver<'_>, object: &Obj) -> bool {
    r.map_of(object)
        .and_then(|record| record.get("productsAddedToCart"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|product| product.get("variant").and_then(Value::as_str))
        .all(|variant| {
            r.state()
                .get(variant)
                .and_then(|record| record.get("availableForSale"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
}

/// `lastCheckoutAbandonmentDate` (and the cart's and the browse's: lsf records the checkout
/// only): when the customer last abandoned a checkout, else the abandonment's date.
fn last_abandonment_date(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(record) = r.map_of(object) else {
        return Ok(Output::Null);
    };
    let customer = record
        .get("customer")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let at = last_abandonment(r.state(), customer)
        .map(value::date_time)
        .or_else(|| {
            record
                .get("createdAt")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    Ok(Output::value(at))
}

/// Sets the email state of an abandonment (`SENT` with when it was sent), and keeps the
/// delivery a marketing activity reported.
fn set_email_state(
    state: &mut ShopState,
    abandonment: &str,
    status: &str,
    sent_at: Option<String>,
    delivery: Option<Value>,
) {
    if let Some(record) = state.get_mut(abandonment) {
        if let Some(delivery) = delivery {
            let activity = delivery.get("marketingActivity").cloned();
            let mut deliveries: Vec<Value> = record
                .get("lsfDeliveries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|known| known.get("marketingActivity") != activity.as_ref())
                .cloned()
                .collect();
            deliveries.push(delivery);
            record.insert("lsfDeliveries".into(), json!(deliveries));
        }
        record.insert("emailState".into(), json!(status));
        if status == "SENT" {
            record.insert("emailSentAt".into(), json!(sent_at));
        }
    }
    state.emit(crate::domains::app::CHANGED, abandonment);
}

/// `abandonmentEmailStateUpdate`, deprecated for `abandonmentUpdateActivitiesDeliveryStatuses`:
/// the email state of the abandonment, without a marketing activity.
fn email_state_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let abandonment = ctx.arg_str("id").unwrap_or_default().to_string();
    if m.state().type_of(&abandonment) != Some("Abandonment") {
        // lsf's words for the code `ABANDONMENT_NOT_FOUND`.
        let error = UserError::new(&["id"], "Abandonment not found").code("ABANDONMENT_NOT_FOUND");
        return Ok(payload(json!({"abandonment": null}), &[error]));
    }
    let status = ctx.arg_str("emailState").unwrap_or("NOT_SENT").to_string();
    let sent_at = ctx
        .arg_str("emailSentAt")
        .map(str::to_string)
        .or_else(|| (status == "SENT").then(|| m.timestamp()));
    set_email_state(m.state_mut(), &abandonment, &status, sent_at, None);
    Ok(payload(json!({"abandonment": abandonment}), &[]))
}

/// `abandonmentUpdateActivitiesDeliveryStatuses`.
fn update_delivery_statuses(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let abandonment = ctx.arg_str("abandonmentId").unwrap_or_default().to_string();
    let activity = ctx
        .arg_str("marketingActivityId")
        .unwrap_or_default()
        .to_string();
    let status = ctx
        .arg_str("deliveryStatus")
        .unwrap_or("NOT_SENT")
        .to_string();
    let mut errors = Vec::new();
    if m.state().type_of(&abandonment) != Some("Abandonment") {
        // lsf's words for the codes of `AbandonmentUpdateActivitiesDeliveryStatusesUserErrorCode`.
        errors.push(
            UserError::new(&["abandonmentId"], "Abandonment not found")
                .code("ABANDONMENT_NOT_FOUND"),
        );
    }
    if m.state().type_of(&activity) != Some("MarketingActivity") {
        errors.push(
            UserError::new(&["marketingActivityId"], "Marketing activity not found")
                .code("MARKETING_ACTIVITY_NOT_FOUND"),
        );
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"abandonment": null}), &errors));
    }
    let delivered = ctx
        .arg_str("deliveredAt")
        .map(str::to_string)
        .or_else(|| (status == "SENT").then(|| m.timestamp()));
    let reason = ctx
        .arg_str("deliveryStatusChangeReason")
        .map(str::to_string);
    let delivery = json!({
        "marketingActivity": activity,
        "deliveryStatus": status,
        "deliveredAt": delivered,
        "reason": reason,
    });
    set_email_state(
        m.state_mut(),
        &abandonment,
        &status,
        delivered.clone(),
        Some(delivery),
    );
    Ok(payload(json!({"abandonment": abandonment}), &[]))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Cart, CartLine, Checkout};
    use crate::domains::segments::tests::{customer, data, demo, run, topics};
    use crate::state::{Clock, ShopState, parse_gid};

    /// A line of the result of a checkout, for the first variant of the demo store.
    fn result(shop: &ShopState) -> lsf_checkout::CheckoutResult {
        let variant = shop.keys("ProductVariant").next().unwrap().to_string();
        let record = shop.get(&variant).unwrap();
        let product = record["product"].as_str().unwrap();
        let price = crate::domains::catalog::cents_of(record.get("price")).unwrap();
        lsf_checkout::CheckoutResult {
            lines: vec![lsf_checkout::ResultLine {
                id: "gid://shopify/CartLine/1".into(),
                variant_id: parse_gid(&variant).unwrap().1,
                product_id: parse_gid(product).unwrap().1,
                title: "Demo product".into(),
                quantity: 2,
                unit_price: price,
                subtotal: price * 2,
                total: price * 2,
                ..Default::default()
            }],
            cost: lsf_checkout::Cost {
                subtotal: price * 2,
                total: price * 2,
                due_now: price * 2,
                currency_code: "USD".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn later(shop: &mut ShopState, minutes: i64) {
        let at = shop.now() + chrono::Duration::minutes(minutes);
        shop.set_clock(Clock::Frozen(at));
    }

    #[test]
    fn a_cart_is_created_then_updated() {
        let mut shop = demo();
        let variant = parse_gid(shop.keys("ProductVariant").next().unwrap())
            .unwrap()
            .1;
        let empty = Cart {
            token: "c1",
            note: "",
            attributes: &[],
            lines: &[],
        };
        super::record_cart(&mut shop, &empty);
        assert!(topics(&mut shop).is_empty());
        let lines = [CartLine {
            variant_id: variant,
            quantity: 3,
            properties: vec![("Engraving".into(), "Hi".into())],
            key: format!("{variant}:abc"),
        }];
        let cart = Cart {
            lines: &lines,
            ..empty.clone()
        };
        super::record_cart(&mut shop, &cart);
        let events = shop.take_events();
        assert_eq!(events[0].topic, "carts/create");
        let payload = crate::webhooks::payload::payload(
            "carts/create",
            events[0].snapshot.as_ref().unwrap(),
            &crate::api::samples::ShopLookup(&shop),
        )
        .unwrap();
        assert_eq!(payload["token"], "c1");
        assert_eq!(payload["line_items"][0]["quantity"], 3);
        assert_eq!(
            payload["line_items"][0]["properties"],
            json!({"Engraving": "Hi"})
        );
        let price = &payload["line_items"][0]["price"];
        assert!(price.as_str().unwrap().contains('.'), "{payload}");
        super::record_cart(&mut shop, &empty);
        assert_eq!(topics(&mut shop), ["carts/update"]);
    }

    #[test]
    fn a_checkout_is_abandoned_ten_minutes_after_its_contact() {
        let mut shop = demo();
        let result = result(&shop);
        let jane = customer(&shop, "jane.doe@example.com");
        let mut checkout = Checkout {
            cart_token: "c1",
            generation: 0,
            result: &result,
            customer_id: None,
            email: None,
            phone: None,
            shipping_address: None,
            note: "",
            attributes: &[],
            payment: None,
            locale: None,
        };
        let id = super::record_checkout(&mut shop, &checkout).unwrap();
        assert_eq!(topics(&mut shop), ["checkouts/create"]);
        // The same checkout again: nothing changed, no webhook.
        super::record_checkout(&mut shop, &checkout);
        assert!(topics(&mut shop).is_empty());
        let query = "{ abandonedCheckouts(first: 5) { nodes { id name customer { email } totalPriceSet { shopMoney { amount } }
                lineItems(first: 5) { nodes { quantity title variant { id } } } abandonedCheckoutUrl } }
            abandonedCheckoutsCount { count } }";
        // Without contact information, it is never abandoned.
        later(&mut shop, 30);
        let none = run(&mut shop, query, json!({}));
        assert_eq!(data(&none, "abandonedCheckoutsCount")["count"], 0);

        checkout.email = Some("JANE.DOE@example.com");
        super::record_checkout(&mut shop, &checkout);
        let events = shop.take_events();
        assert_eq!(events[0].topic, "checkouts/update");
        let payload = crate::webhooks::payload::payload(
            "checkouts/update",
            shop.get(&id).unwrap(),
            &crate::api::samples::ShopLookup(&shop),
        )
        .unwrap();
        assert_eq!(payload["email"], "JANE.DOE@example.com");
        assert_eq!(payload["customer"]["admin_graphql_api_id"], json!(jane));
        assert_eq!(payload["cart_token"], "c1");
        // Nine minutes later: not yet. Eleven: abandoned, with its abandonment.
        later(&mut shop, 9);
        assert_eq!(
            data(&run(&mut shop, query, json!({})), "abandonedCheckoutsCount")["count"],
            0
        );
        later(&mut shop, 2);
        let listed = run(&mut shop, query, json!({}));
        assert!(listed["extensions"].get("lsf").is_none(), "{listed}");
        let node = &data(&listed, "abandonedCheckouts")["nodes"][0];
        assert_eq!(node["id"], json!(id));
        assert_eq!(node["customer"]["email"], "jane.doe@example.com");
        assert_eq!(node["lineItems"]["nodes"][0]["quantity"], 2);
        let abandonment = run(
            &mut shop,
            "query($id: ID!) { abandonmentByAbandonedCheckoutId(abandonedCheckoutId: $id) {
                id abandonmentType customer { email } emailState customerHasNoOrderSinceAbandonment
                inventoryAvailable isMostSignificantAbandonment hoursSinceLastAbandonedCheckout
                lastCheckoutAbandonmentDate productsAddedToCart(first: 5) { nodes { quantity } }
                abandonedCheckoutPayload { id } } }",
            json!({"id": id}),
        );
        let found = data(&abandonment, "abandonmentByAbandonedCheckoutId").clone();
        assert_eq!(found["abandonmentType"], "CHECKOUT");
        assert_eq!(found["customer"]["email"], "jane.doe@example.com");
        assert_eq!(found["customerHasNoOrderSinceAbandonment"], true);
        assert_eq!(found["isMostSignificantAbandonment"], true);
        assert_eq!(found["emailState"], Value::Null);
        assert_eq!(found["productsAddedToCart"]["nodes"][0]["quantity"], 2);
        // The segment filter reads it.
        let members = run(
            &mut shop,
            "{ customerSegmentMembers(first: 5, query: \"abandoned_checkout_date = today\") { totalCount } }",
            json!({}),
        );
        assert_eq!(data(&members, "customerSegmentMembers")["totalCount"], 1);

        // An email the app sent for it.
        let activity = run(
            &mut shop,
            "mutation { marketingActivityUpsertExternal(input: {remoteId: \"mail-1\", title: \"Come back\", remoteUrl: \"https://mail.example.com\",
                tactic: ABANDONED_CART, marketingChannelType: EMAIL, status: ACTIVE, urlParameterValue: \"come-back\"}) { marketingActivity { id } } }",
            json!({}),
        );
        let activity =
            data(&activity, "marketingActivityUpsertExternal")["marketingActivity"]["id"].clone();
        let update = "mutation($abandonment: ID!, $activity: ID!) { abandonmentUpdateActivitiesDeliveryStatuses(
                abandonmentId: $abandonment, marketingActivityId: $activity, deliveryStatus: SENT, deliveredAt: \"2026-10-31T12:20:00Z\") {
                abandonment { emailState emailSentAt } userErrors { field code } } }";
        let sent = run(
            &mut shop,
            update,
            json!({"abandonment": found["id"], "activity": activity}),
        );
        assert_eq!(
            data(&sent, "abandonmentUpdateActivitiesDeliveryStatuses")["abandonment"],
            json!({"emailState": "SENT", "emailSentAt": "2026-10-31T12:20:00Z"})
        );
        let before = shop.picture();
        let refused = run(
            &mut shop,
            update,
            json!({"abandonment": "gid://shopify/Abandonment/999", "activity": "gid://shopify/MarketingActivity/999"}),
        );
        let codes: Vec<Value> =
            data(&refused, "abandonmentUpdateActivitiesDeliveryStatuses")["userErrors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|error| error["code"].clone())
                .collect();
        assert_eq!(
            codes,
            [
                json!("ABANDONMENT_NOT_FOUND"),
                json!("MARKETING_ACTIVITY_NOT_FOUND")
            ]
        );
        assert_eq!(shop.picture(), before);

        // Completed after it was abandoned: recovered, still listed, closed.
        super::complete_checkout(&mut shop, "c1", 0);
        assert_eq!(topics(&mut shop), ["checkouts/update"]);
        let recovered = run(
            &mut shop,
            "{ abandonedCheckouts(first: 5, query: \"recovery_state:recovered status:closed\") { nodes { completedAt } } }",
            json!({}),
        );
        assert_eq!(
            data(&recovered, "abandonedCheckouts")["nodes"][0]["completedAt"],
            json!(shop.timestamp())
        );
        // Three months on, the admin deletes it.
        later(&mut shop, 91 * 24 * 60);
        assert_eq!(super::delete_expired(&mut shop), 1);
        let events = shop.take_events();
        assert_eq!(events[0].topic, "checkouts/delete");
        let payload = crate::webhooks::payload::payload(
            "checkouts/delete",
            events[0].snapshot.as_ref().unwrap(),
            &crate::api::samples::ShopLookup(&shop),
        )
        .unwrap();
        assert_eq!(payload, json!({"id": parse_gid(&id).unwrap().1}));
        assert_eq!(shop.count("Abandonment"), 0);
    }

    #[test]
    fn the_deprecated_email_state_update_sets_the_abandonments_email_state() {
        let mut shop = demo();
        let result = result(&shop);
        let checkout = Checkout {
            cart_token: "c1",
            generation: 0,
            result: &result,
            customer_id: None,
            email: Some("jane.doe@example.com"),
            phone: None,
            shipping_address: None,
            note: "",
            attributes: &[],
            payment: None,
            locale: None,
        };
        let checkout_id = super::record_checkout(&mut shop, &checkout).unwrap();
        let abandonment = shop.get(&checkout_id).unwrap()["lsfAbandonment"].clone();
        later(&mut shop, 11);
        let update = "mutation($id: ID!) { abandonmentEmailStateUpdate(id: $id, emailState: SENT) {
            abandonment { emailState emailSentAt } userErrors { field code } } }";
        let sent = run(&mut shop, update, json!({"id": abandonment}));
        assert_eq!(
            data(&sent, "abandonmentEmailStateUpdate")["abandonment"],
            json!({"emailState": "SENT", "emailSentAt": shop.timestamp()})
        );
        let before = shop.picture();
        let refused = run(
            &mut shop,
            update,
            json!({"id": "gid://shopify/Abandonment/999"}),
        );
        assert_eq!(
            data(&refused, "abandonmentEmailStateUpdate")["userErrors"],
            json!([{"field": ["id"], "code": "ABANDONMENT_NOT_FOUND"}])
        );
        assert_eq!(shop.picture(), before);
    }
}

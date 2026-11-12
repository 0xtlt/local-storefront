//! The code discounts of Shopify's own types at the local checkout: a `DiscountCodeBasic` or a
//! `DiscountCodeFreeShipping` of the default shop applies once the buyer enters one of its
//! codes, without a Function, and the orders placed with it count as its uses.
//!
//! - [`for_checkout`] reads them as the checkout applies them ([`CodeDiscountConfig`]): what
//!   they take off (`customerGets`: a `DiscountPercentage` or a `DiscountAmount`, with
//!   `appliesOnEachItem`, on `AllDiscountItems`, `DiscountProducts` or `DiscountCollections`;
//!   free shipping, up to `maximumShippingPrice`, to the countries of `destinationSelection`),
//!   their `minimumRequirement`, and whether the buyer may use them at all: the discount is
//!   `ACTIVE` by its dates and the shop's clock, its `usageLimit` is above its
//!   `asyncUsageCount`, the buyer is one of its `context` (all buyers, the listed customers, the
//!   members of the listed segments), and has not placed an order with it when it
//!   `appliesOncePerCustomer`. The checkout checks the rest against the cart
//!   (`crate::checkout::pipeline`).
//! - [`count_uses`] counts an order placed with a code: the discount's `asyncUsageCount` and its
//!   code's go up by one, as Shopify's do ("The number of times that the discount has been
//!   used", shopify.dev's `DiscountCodeBasic.asyncUsageCount`), at once rather than
//!   asynchronously.
//!
//! Not read: `combinesWith` (one code of these types applies per checkout), a `context` of
//! markets (`DiscountMarkets`: every buyer), `appliesOnOneTimePurchase` and
//! `appliesOnSubscription`, `recurringCycleLimit`. The buy X get Y codes (`DiscountCodeBxgy`) and
//! the automatic discounts of Shopify's types stay records.

use serde_json::Value;

use super::discounts::{inner, status_at};
use crate::admin::state::value::{self, str_field as text};
use crate::admin::state::{ShopState, gid, gid_number, parse_gid};
use crate::checkout::{
    CodeDiscountConfig, CodeDiscountEffect, CodeDiscountItems, CodeDiscountValue,
    MinimumRequirement,
};

/// Who is at the checkout: the logged-in customer (by their lsf-core id), else the email they
/// entered, which names the customer that has it, as the order placed will
/// ([`super::super::orders::record_placed_order`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct Buyer<'a> {
    pub customer_id: Option<u64>,
    pub email: Option<&'a str>,
}

/// The code discounts of Shopify's own types (`DiscountCodeBasic`, `DiscountCodeFreeShipping`) of
/// the shop, in creation order, as the checkout of `buyer` applies them. Amounts are in the
/// shop's currency.
pub fn for_checkout(state: &ShopState, buyer: &Buyer) -> Vec<CodeDiscountConfig> {
    let customer = buyer
        .customer_id
        .map(|id| gid("Customer", id))
        .filter(|customer| state.contains(customer))
        .or_else(|| {
            buyer
                .email
                .and_then(|email| super::super::checkouts::customer_by_email(state, email))
        });
    let now = state.timestamp();
    let mut keys: Vec<&str> = state.keys("DiscountCodeNode").collect();
    keys.sort_by_key(|key| gid_number(key));
    keys.into_iter()
        .filter_map(|key| {
            let discount = state.get(key).and_then(inner)?;
            let effect = match text(discount, "__typename")? {
                "DiscountCodeBasic" => amount_off(state, discount)?,
                "DiscountCodeFreeShipping" => free_shipping(discount),
                _ => return None,
            };
            let codes = codes_of(discount);
            let refused = refusal(
                state,
                discount,
                &codes,
                &now,
                customer.as_deref(),
                buyer.email,
            );
            Some(CodeDiscountConfig {
                id: parse_gid(key)?.1,
                title: text(discount, "title").unwrap_or_default().to_string(),
                codes,
                effect,
                minimum: minimum(discount.get("minimumRequirement")),
                refused,
            })
        })
        .collect()
}

/// The codes of a code discount.
fn codes_of(discount: &crate::admin::state::Record) -> Vec<String> {
    discount
        .get("codes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|code| code.get("code").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// What a `DiscountCodeBasic` takes off, and off what. `None` for a value that is not an
/// amount off (`DiscountOnQuantity` is buy X get Y's).
fn amount_off(
    state: &ShopState,
    discount: &crate::admin::state::Record,
) -> Option<CodeDiscountEffect> {
    let gets = discount.get("customerGets")?;
    let value = gets.get("value")?;
    let value = match value.get("__typename").and_then(Value::as_str)? {
        // A fraction in the Admin API (0.1 for 10 %), a percentage at the checkout.
        "DiscountPercentage" => CodeDiscountValue::Percentage(
            value
                .get("percentage")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                * 100.0,
        ),
        "DiscountAmount" => CodeDiscountValue::Amount {
            cents: value::cents(value.get("amount")),
            each: value
                .get("appliesOnEachItem")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        _ => return None,
    };
    let items = gets.get("items");
    let numbers = |field: &str| -> Vec<u64> {
        items
            .and_then(|items| items.get(field))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|id| parse_gid(id).map(|(_, number)| number))
            .collect()
    };
    let items = match items
        .and_then(|items| items.get("__typename"))
        .and_then(Value::as_str)
    {
        Some("DiscountProducts") => CodeDiscountItems::Listed {
            products: numbers("products"),
            variants: numbers("productVariants"),
        },
        // The products the collections have now.
        Some("DiscountCollections") => {
            let collections: Vec<String> = items
                .and_then(|items| items.get("collections"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            let mut products: Vec<u64> = Vec::new();
            for collection in &collections {
                let members = state
                    .get(collection)
                    .and_then(|record| record.get("products"))
                    .and_then(Value::as_array);
                for number in members
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(|id| parse_gid(id).map(|(_, number)| number))
                {
                    if !products.contains(&number) {
                        products.push(number);
                    }
                }
            }
            CodeDiscountItems::Listed {
                products,
                variants: Vec::new(),
            }
        }
        _ => CodeDiscountItems::All,
    };
    Some(CodeDiscountEffect::AmountOff { value, items })
}

/// What a `DiscountCodeFreeShipping` makes free: the options up to its maximum price, to the
/// countries it lists (all of them with `includeRestOfWorld`, or `DiscountCountryAll`).
fn free_shipping(discount: &crate::admin::state::Record) -> CodeDiscountEffect {
    let maximum_price = discount
        .get("maximumShippingPrice")
        .filter(|price| !price.is_null())
        .map(|price| value::cents(Some(price)));
    let destination = discount.get("destinationSelection");
    let countries = match destination
        .and_then(|destination| destination.get("__typename"))
        .and_then(Value::as_str)
    {
        Some("DiscountCountries")
            if destination
                .and_then(|destination| destination.get("includeRestOfWorld"))
                .and_then(Value::as_bool)
                != Some(true) =>
        {
            Some(
                destination
                    .and_then(|destination| destination.get("countries"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_ascii_uppercase)
                    .collect(),
            )
        }
        _ => None,
    };
    CodeDiscountEffect::FreeShipping {
        maximum_price,
        countries,
    }
}

/// A `DiscountMinimumRequirement`.
fn minimum(requirement: Option<&Value>) -> Option<MinimumRequirement> {
    let requirement = requirement?;
    match requirement.get("__typename").and_then(Value::as_str)? {
        "DiscountMinimumSubtotal" => Some(MinimumRequirement::Subtotal(value::cents(
            requirement.get("greaterThanOrEqualToSubtotal"),
        ))),
        "DiscountMinimumQuantity" => {
            let quantity = match requirement.get("greaterThanOrEqualToQuantity")? {
                Value::String(text) => text.trim().parse().ok()?,
                Value::Number(number) => u32::try_from(number.as_u64()?).ok()?,
                _ => return None,
            };
            Some(MinimumRequirement::Quantity(quantity))
        }
        _ => None,
    }
}

/// Why the buyer cannot use a code discount whatever the cart, in lsf's words: its status by the
/// dates, its usage limit, its buyers, its one use per customer. `customer` is the buyer's
/// `Customer` GID, when they are one.
fn refusal(
    state: &ShopState,
    discount: &crate::admin::state::Record,
    codes: &[String],
    now: &str,
    customer: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    match status_at(discount, now) {
        "SCHEDULED" => {
            return Some(format!(
                "it is scheduled: it starts at {}",
                text(discount, "startsAt").unwrap_or_default()
            ));
        }
        "EXPIRED" => {
            return Some(format!(
                "it expired at {}",
                text(discount, "endsAt").unwrap_or_default()
            ));
        }
        _ => {}
    }
    let used = discount
        .get("asyncUsageCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if let Some(limit) = discount.get("usageLimit").and_then(Value::as_i64)
        && used >= limit
    {
        return Some(format!(
            "it reached its usage limit: used {used} times of {limit}"
        ));
    }
    let context = discount.get("context");
    let listed = |field: &str| -> Vec<&str> {
        context
            .and_then(|context| context.get(field))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    };
    let who = match customer {
        Some(customer) => format!("the buyer ({customer})"),
        None => "a buyer who is not a customer of the shop".to_string(),
    };
    match context
        .and_then(|context| context.get("__typename"))
        .and_then(Value::as_str)
    {
        Some("DiscountCustomers")
            if !customer.is_some_and(|customer| listed("customers").contains(&customer)) =>
        {
            return Some(format!(
                "it is for the customers it lists, and {who} is not one of them"
            ));
        }
        Some("DiscountCustomerSegments")
            if !customer.is_some_and(|customer| {
                listed("segments").iter().any(|segment| {
                    crate::admin::domains::segments::is_member(state, customer, segment)
                })
            }) =>
        {
            return Some(format!(
                "it is for the customers of the segments it lists, and {who} is in none of them"
            ));
        }
        _ => {}
    }
    if discount
        .get("appliesOncePerCustomer")
        .and_then(Value::as_bool)
        == Some(true)
        && used_by(state, codes, customer, email)
    {
        return Some("it applies once per customer, and the buyer has used it".to_string());
    }
    None
}

/// Whether the customer, or the email, has an order placed with one of `codes`.
fn used_by(
    state: &ShopState,
    codes: &[String],
    customer: Option<&str>,
    email: Option<&str>,
) -> bool {
    let email = email.map(str::trim).filter(|email| !email.is_empty());
    if customer.is_none() && email.is_none() {
        return false;
    }
    state.all("Order").any(|order| {
        let theirs = customer.is_some_and(|customer| text(order, "customer") == Some(customer))
            || email.is_some_and(|email| {
                text(order, "email").is_some_and(|known| known.eq_ignore_ascii_case(email))
            });
        theirs
            && order
                .get("discountCodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|used| codes.iter().any(|code| code.eq_ignore_ascii_case(used)))
    })
}

/// Counts the uses of the codes an order was placed with: the `asyncUsageCount` of each code
/// discount that has one of them, and of that code, go up by one. A discount counts one use per
/// order, whatever the number of its codes the order has.
pub fn count_uses(state: &mut ShopState, order: &str) {
    let used: Vec<String> = state
        .get(order)
        .and_then(|record| record.get("discountCodes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if used.is_empty() {
        return;
    }
    let nodes: Vec<String> = state.keys("DiscountCodeNode").map(str::to_string).collect();
    for node in nodes {
        let Some(discount) = state.get_mut(&node).and_then(super::discounts::inner_mut) else {
            continue;
        };
        let mut counted = false;
        if let Some(Value::Array(codes)) = discount.get_mut("codes") {
            for code in codes.iter_mut() {
                let matches = code
                    .get("code")
                    .and_then(Value::as_str)
                    .is_some_and(|known| used.iter().any(|used| used.eq_ignore_ascii_case(known)));
                if matches && let Some(code) = code.as_object_mut() {
                    increment(code);
                    counted = true;
                }
            }
        }
        if counted {
            increment(discount);
        }
    }
}

/// One more use in `asyncUsageCount`.
fn increment(record: &mut crate::admin::state::Record) {
    let count = record
        .get("asyncUsageCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    record.insert("asyncUsageCount".into(), Value::from(count + 1));
}

#[cfg(test)]
mod tests;

//! Money inputs past the bound lsf takes ([`value::MAX_MONEY_CENTS`]): the resolver refuses the
//! mutation with a user error for each, before it runs, so that no mutation adds or multiplies
//! an amount its 64 bits of cents cannot hold.
//!
//! An amount is money when its input field is of the `Money` scalar, or of `Decimal` in an input
//! that holds an amount ([`MONEY_DECIMALS`]: `MoneyInput.amount`, a discount's amount, a selling
//! plan's fixed price, …). A `Decimal` that is a rate, a percentage or a count is not.

use serde_json::{Map, Value};

use crate::graphql::FieldContext;
use crate::resolve::UserError;
use crate::schema::{Kind, Schema, TypeRef};
use crate::state::value;

/// The `Decimal` input fields of the 2026-10 schema that hold an amount of money, by input type.
const MONEY_DECIMALS: [(&str, &str); 17] = [
    ("MoneyInput", "amount"),
    ("AppSubscriptionDiscountValueInput", "amount"),
    ("DiscountAmountInput", "amount"),
    ("DiscountAutomaticFreeShippingInput", "maximumShippingPrice"),
    ("DiscountCodeFreeShippingInput", "maximumShippingPrice"),
    ("DiscountCustomerBuysValueInput", "amount"),
    ("DiscountEffectInput", "amount"),
    (
        "DiscountMinimumSubtotalInput",
        "greaterThanOrEqualToSubtotal",
    ),
    ("GiftCardCreateInput", "initialValue"),
    ("InventoryItemInput", "cost"),
    ("SellingPlanCheckoutChargeValueInput", "fixedValue"),
    ("SellingPlanPricingPolicyValueInput", "fixedValue"),
    ("SubscriptionContractProductChangeInput", "currentPrice"),
    ("SubscriptionDraftInput", "deliveryPrice"),
    ("SubscriptionLineInput", "currentPrice"),
    ("SubscriptionLineUpdateInput", "currentPrice"),
    ("SubscriptionPricingPolicyInput", "basePrice"),
];

/// The money inputs of a field's arguments that are past the bound: one user error each, at the
/// path of the input (`["order", "lineItems", "0", "priceSet", "shopMoney", "amount"]`).
pub fn oversized(ctx: &FieldContext<'_>) -> Vec<UserError> {
    let schema = ctx.schema();
    let mut errors = Vec::new();
    for (name, argument) in &ctx.definition().args {
        if let Some(given) = ctx.arguments().get(name) {
            let mut path = vec![name.clone()];
            walk(schema, &argument.ty, None, given, &mut path, &mut errors);
        }
    }
    errors
}

/// Walks an input value of a type, collecting the amounts past the bound. `parent` is the input
/// object type and the field the value is of.
fn walk(
    schema: &Schema,
    ty: &TypeRef,
    parent: Option<(&str, &str)>,
    given: &Value,
    path: &mut Vec<String>,
    errors: &mut Vec<UserError>,
) {
    if let Value::Array(items) = given {
        if ty.is_list() {
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                walk(schema, ty, parent, item, path, errors);
                path.pop();
            }
        }
        return;
    }
    let Some(named) = schema.get(ty.named()) else {
        return;
    };
    match named.kind {
        Kind::InputObject => {
            let Value::Object(fields) = given else {
                return;
            };
            for (key, value) in fields {
                if let Some(field) = named.input_field(key) {
                    path.push(key.clone());
                    walk(
                        schema,
                        &field.ty,
                        Some((&named.name, key)),
                        value,
                        path,
                        errors,
                    );
                    path.pop();
                }
            }
        }
        Kind::Scalar => {
            let money = named.name == "Money"
                || (named.name == "Decimal"
                    && parent.is_some_and(|key| MONEY_DECIMALS.contains(&key)));
            if money && too_large(given) {
                let name = path.last().map_or("Amount".to_string(), |key| words(key));
                let field: Vec<&str> = path.iter().map(String::as_str).collect();
                errors.push(UserError::new(&field, value::money_too_large(&name)));
            }
        }
        _ => {}
    }
}

/// An input field's name as the words of a message: `originalUnitPrice` → `Original unit price`.
fn words(key: &str) -> String {
    let mut words = String::with_capacity(key.len() + 4);
    for (index, c) in key.chars().enumerate() {
        if index == 0 {
            words.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            words.push(' ');
            words.extend(c.to_lowercase());
        } else {
            words.push(c);
        }
    }
    words
}

/// Whether an amount, as the arguments give it (a string, or a number for `Money` given as one),
/// is past the bound. What is not a number is left to the mutation, which says it is invalid.
pub fn too_large(amount: &Value) -> bool {
    match amount {
        Value::String(text) => {
            let digits = text.trim();
            let digits = digits.strip_prefix('-').unwrap_or(digits);
            let (units, fraction) = digits.split_once('.').unwrap_or((digits, ""));
            let number = !(units.is_empty() && fraction.is_empty())
                && units
                    .bytes()
                    .chain(fraction.bytes())
                    .all(|b| b.is_ascii_digit());
            // Ten trillion has 14 digits: more significant digits than that are past it.
            number
                && (units.trim_start_matches('0').len() > value::MAX_MONEY_TEXT.len()
                    || units.parse::<i64>().is_ok_and(|units| {
                        !units
                            .checked_mul(100)
                            .is_some_and(|cents| cents < value::MAX_MONEY_CENTS)
                    }))
        }
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|amount| (amount * 100.0).abs() >= value::MAX_MONEY_CENTS as f64),
        _ => false,
    }
}

/// The codes of an invalid input, in the order they are looked for in a payload's code enum.
const INVALID_CODES: [&str; 3] = ["INVALID", "INVALID_INPUT", "INVALID_VALUE"];

/// The payload of a mutation refused for its amounts: the errors in each list of user errors
/// the payload has (`userErrors`, `…UserErrors`), with the code of an invalid input where the
/// list's code enum has one ([`INVALID_CODES`]); the rest is left `null`.
pub fn refused(schema: &Schema, payload_type: &str, errors: &[UserError]) -> Map<String, Value> {
    let mut payload = Map::new();
    let Some(payload_type) = schema.get(payload_type) else {
        return payload;
    };
    for (name, field) in &payload_type.fields {
        if name != "userErrors" && !name.ends_with("UserErrors") {
            continue;
        }
        let code = schema
            .get(field.ty.named())
            .and_then(|error| error.field("code"))
            .and_then(|code| schema.get(code.ty.named()))
            .and_then(|codes| {
                INVALID_CODES
                    .into_iter()
                    .find(|code| codes.enum_values.contains_key(*code))
            })
            .map(str::to_string);
        let listed = errors
            .iter()
            .map(|error| {
                let mut error = error.clone();
                error.code = code.clone();
                error.to_json()
            })
            .collect();
        payload.insert(name.clone(), Value::Array(listed));
    }
    payload
}

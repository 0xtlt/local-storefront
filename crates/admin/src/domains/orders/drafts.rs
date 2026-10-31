//! Draft orders: `draftOrderCreate`, `draftOrderUpdate`, `draftOrderDelete`,
//! `draftOrderCalculate`, `draftOrderComplete`, `draftOrderDuplicate`, `draftOrderInvoiceSend`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A draft order (`#D1`, `#D2`, … *(chosen: the admin's names)*) has line items of a variant
//!   (its title, SKU and price, unless `priceOverride` or `originalUnitPriceWithCurrency` sets
//!   one) or custom ones (a `title` and a price), each with an applied discount (a percentage, or
//!   a fixed amount per unit), an applied discount of its own, a shipping line, a customer (or
//!   `useCustomerDefaultAddress`), addresses, tags, a note, custom attributes.
//! - Totals: the line items after their discounts, the draft's discount, the shipping, and the
//!   tax at the rate of the store data (`checkout.tax_rate`, on the taxable lines, inside the
//!   prices when the shop's prices include taxes), none for a tax-exempt draft.
//!   `draftOrderCalculate` answers the same totals without making a draft.
//! - `draftOrderComplete` makes the order (`sourceName` `shopify_draft_order` unless given),
//!   paid through a manual sale unless `paymentPending`, its units committed in stock; the draft
//!   is `COMPLETED` with its `order`. Events: `orders/create`, `order_transactions/create`,
//!   `orders/paid`, `draft_orders/update`.
//! - `draftOrderInvoiceSend` records the invoice as sent (`INVOICE_SENT`, `invoiceSentAt`) and
//!   puts the email in the shop's outbox (`crate::outbox`): lsf sends nothing.
//!   `draftOrderDuplicate` copies a draft as a new open one.
//! - B2B drafts (`purchasingEntity.purchasingCompany`) are the contact's customer's, for the
//!   company location, with its addresses and payment terms unless the input gives its own;
//!   their order is the location's (`purchasingEntity`), pending until paid when it has payment
//!   terms. `paymentTerms` makes or changes the draft's terms (`payments`), which the order
//!   takes when the draft is completed.
//! - Events: `draft_orders/create`, `draft_orders/update`, `draft_orders/delete`.
//!
//! Messages: "Add at least 1 product", "Draft order not found", "Invalid payment gateway" and
//! "To is invalid" are the reference's examples; the others are Rails' wording or lsf's. Not
//! simulated (said in `extensions.lsf.notes`): deposits, discount codes and automatic discounts, metafields, the shipping rates of
//! `availableShippingRates`.

use lsf_core::util::stable_id;
use serde_json::{Map, Value, json};

use super::TAX_RATE_SETTING;
use super::model::{self, NewTransaction, array, field_cents, int, text};
use super::{FinancialStatus, FulfillmentState, LineDraft, OrderDraft, ShippingDraft};
use crate::domains::catalog::{gids, tags_of};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("draftOrderCreate", create)
        .mutation("draftOrderUpdate", update)
        .mutation("draftOrderDelete", delete)
        .mutation("draftOrderCalculate", calculate)
        .mutation("draftOrderComplete", complete)
        .mutation("draftOrderDuplicate", duplicate)
        .mutation("draftOrderInvoiceSend", invoice_send);
}

/// The inputs of `DraftOrderInput` lsf does not simulate yet.
const NOT_SIMULATED: &[(&str, &str)] = &[
    ("deposit", "deposits"),
    ("metafields", "metafields"),
    ("discountCodes", "discount codes"),
    ("acceptAutomaticDiscounts", "automatic discounts"),
    ("localizedFields", "localized fields"),
    ("localizationExtensions", "localization extensions"),
    ("sessionToken", "session tokens"),
    ("transformerFingerprint", "bundles"),
];

/// A line of a draft as its record (or a calculated draft) holds it, before its id.
fn line_value(
    state: &ShopState,
    input: &Value,
    currency: &str,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Option<Map<String, Value>> {
    let bag = |cents: i64| value::money_bag(cents, currency);
    let get = |name: &str| input.get(name).and_then(Value::as_str);
    let quantity = input.get("quantity").and_then(Value::as_i64).unwrap_or(0);
    if quantity < 1 {
        errors.push(UserError::new(
            &[path, &["quantity"]].concat(),
            "Quantity must be greater than or equal to 1",
        ));
        return None;
    }
    let price_input = input
        .get("priceOverride")
        .filter(|price| !price.is_null())
        .or_else(|| {
            input
                .get("originalUnitPriceWithCurrency")
                .filter(|price| !price.is_null())
        })
        .or_else(|| {
            input
                .get("originalUnitPrice")
                .filter(|price| !price.is_null())
        })
        .map(|price| model::cents(Some(price)));
    let mut line = Map::new();
    let unit;
    match get("variantId") {
        Some(variant) => {
            let Some(variant_record) = state
                .get(variant)
                .filter(|_| state.type_of(variant) == Some("ProductVariant"))
            else {
                errors.push(UserError::new(
                    &[path, &["variantId"]].concat(),
                    "Product variant does not exist",
                ));
                return None;
            };
            let product = text(variant_record, "product").and_then(|product| state.get(product));
            let item = text(variant_record, "inventoryItem").and_then(|item| state.get(item));
            let variant_title =
                text(variant_record, "title").filter(|title| *title != "Default Title");
            let title = product
                .and_then(|product| text(product, "title"))
                .unwrap_or_default();
            unit = price_input.unwrap_or_else(|| field_cents(variant_record, "price"));
            line.extend(value::object(json!({
                "title": title,
                "variantTitle": variant_title,
                "name": match variant_title {
                    Some(variant) => format!("{title} - {variant}"),
                    None => title.to_string(),
                },
                "sku": variant_record.get("sku"),
                "vendor": product.and_then(|product| product.get("vendor")),
                "product": text(variant_record, "product"),
                "variant": variant,
                "custom": false,
                "requiresShipping": input.get("requiresShipping").and_then(Value::as_bool)
                    .or_else(|| item.and_then(|item| item.get("requiresShipping")).and_then(Value::as_bool))
                    .unwrap_or(true),
                "taxable": input.get("taxable").and_then(Value::as_bool)
                    .or_else(|| variant_record.get("taxable").and_then(Value::as_bool))
                    .unwrap_or(true),
                "isGiftCard": product.and_then(|product| product.get("isGiftCard")).and_then(Value::as_bool).unwrap_or(false),
                "weight": item.and_then(|item| item.get("measurement")).and_then(|m| m.get("weight")),
                "image": variant_record.get("image"),
            })));
        }
        None => {
            let title = get("title").unwrap_or_default().trim();
            if title.is_empty() {
                errors.push(UserError::new(
                    &[path, &["title"]].concat(),
                    "Title can't be blank",
                ));
                return None;
            }
            unit = price_input.unwrap_or(0);
            let weight = input.get("weight").cloned().or_else(|| {
                input
                    .get("grams")
                    .and_then(Value::as_f64)
                    .map(|grams| json!({"unit": "GRAMS", "value": grams}))
            });
            line.extend(value::object(json!({
                "title": title,
                "variantTitle": null,
                "name": title,
                "sku": get("sku"),
                "vendor": null,
                "product": null,
                "variant": null,
                "custom": true,
                "requiresShipping": input.get("requiresShipping").and_then(Value::as_bool).unwrap_or(true),
                "taxable": input.get("taxable").and_then(Value::as_bool).unwrap_or(true),
                "isGiftCard": false,
                "weight": weight,
                "image": null,
            })));
        }
    }
    // Amounts are bounded (`value::MAX_MONEY_CENTS`), so that the draft's sums of them hold.
    let Some(original) = unit
        .checked_mul(quantity)
        .filter(|total| value::money_in_bounds(*total))
    else {
        errors.push(UserError::new(
            &[path, &["quantity"]].concat(),
            value::money_too_large("Total price"),
        ));
        return None;
    };
    let discount_input = input
        .get("appliedDiscount")
        .filter(|discount| !discount.is_null());
    let (discount, applied) = match discount_input {
        Some(discount) => {
            let (amount, applied) = applied_discount(discount, original, Some(quantity), currency);
            (amount, applied)
        }
        None => (0, Value::Null),
    };
    let discounted = original - discount;
    let price_override = input
        .get("priceOverride")
        .filter(|price| !price.is_null())
        .map(|_| value::money_v2(unit, currency));
    line.extend(value::object(json!({
        "uuid": get("uuid"),
        "quantity": quantity,
        "customAttributes": model::attributes(input.get("customAttributes")),
        "appliedDiscount": applied,
        "originalUnitPriceSet": bag(unit),
        "originalUnitPriceWithCurrency": value::money_v2(unit, currency),
        "priceOverride": price_override,
        "originalTotalSet": bag(original),
        "discountedTotalSet": bag(discounted),
        "totalDiscountSet": bag(discount),
        "approximateDiscountedUnitPriceSet": bag(discounted / quantity),
        "discountedUnitPriceSet": bag(discounted / quantity),
        "taxLines": [],
        "components": [],
        "bundleComponents": [],
        "fulfillmentService": null,
        "customAttributesV2": [],
    })));
    Some(line)
}

/// A `DraftOrderAppliedDiscount` of an input, and what it takes off `base` (per unit when
/// `units` is given and it is a fixed amount: a line's discount).
fn applied_discount(input: &Value, base: i64, units: Option<i64>, currency: &str) -> (i64, Value) {
    let value = input.get("value").and_then(Value::as_f64).unwrap_or(0.0);
    let kind = input
        .get("valueType")
        .and_then(Value::as_str)
        .unwrap_or("FIXED_AMOUNT");
    let amount = match kind {
        "PERCENTAGE" => ((base as f64) * value.clamp(0.0, 100.0) / 100.0).round() as i64,
        _ => {
            let fixed = input
                .get("amountWithCurrency")
                .or_else(|| input.get("amount"))
                .filter(|amount| !amount.is_null())
                .map_or_else(
                    || (value * 100.0).round() as i64,
                    |amount| model::cents(Some(amount)),
                );
            fixed.saturating_mul(units.unwrap_or(1)).min(base)
        }
    }
    .max(0);
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    (
        amount,
        json!({
            "title": input.get("title"),
            "description": description,
            "value": value,
            "valueType": kind,
            "amountSet": value::money_bag(amount, currency),
            "amountV2": value::money_v2(amount, currency),
            "amount": value::money(amount),
        }),
    )
}

/// The totals of a draft's lines, discount, shipping and tax, as the fields of a `DraftOrder`
/// (and a `CalculatedDraftOrder`).
fn totals(
    state: &ShopState,
    lines: &[Map<String, Value>],
    applied: Option<&Value>,
    shipping: Option<&Value>,
    tax_exempt: bool,
    currency: &str,
) -> Map<String, Value> {
    let bag = |cents: i64| value::money_bag(cents, currency);
    let original: i64 = lines
        .iter()
        .map(|line| field_cents(line, "originalTotalSet"))
        .sum();
    let subtotal_lines: i64 = lines
        .iter()
        .map(|line| field_cents(line, "discountedTotalSet"))
        .sum();
    let line_discounts = original - subtotal_lines;
    let taxable_lines: i64 = lines
        .iter()
        .filter(|line| line.get("taxable").and_then(Value::as_bool) != Some(false))
        .map(|line| field_cents(line, "discountedTotalSet"))
        .sum();
    let (order_discount, applied_value) = match applied {
        Some(discount) if !discount.is_null() => {
            applied_discount(discount, subtotal_lines, None, currency)
        }
        _ => (0, Value::Null),
    };
    let subtotal = subtotal_lines - order_discount;
    let taxable = if subtotal_lines == 0 {
        0
    } else {
        taxable_lines - order_discount * taxable_lines / subtotal_lines
    };
    let shipping_price = shipping.map_or(0, |line| {
        field_cents(line.as_object().unwrap_or(&Map::new()), "originalPriceSet")
    });
    let rate = state
        .setting(TAX_RATE_SETTING)
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let included = state
        .shop()
        .and_then(|shop| shop.get("taxesIncluded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tax = if tax_exempt || rate <= 0 {
        0
    } else if included {
        ((taxable as f64) * rate as f64 / (10_000.0 + rate as f64)).round() as i64
    } else {
        ((taxable as f64) * rate as f64 / 10_000.0).round() as i64
    };
    let total = subtotal + shipping_price + if included { 0 } else { tax };
    let quantity: i64 = lines.iter().map(|line| int(line, "quantity")).sum();
    let grams: i64 = lines
        .iter()
        .map(|line| {
            let grams = line
                .get("weight")
                .and_then(|weight| {
                    let value = weight.get("value")?.as_f64()?;
                    Some(match weight.get("unit").and_then(Value::as_str) {
                        Some("KILOGRAMS") => value * 1000.0,
                        Some("POUNDS") => value * 453.592_37,
                        Some("OUNCES") => value * 28.349_523_125,
                        _ => value,
                    })
                })
                .unwrap_or(0.0);
            (grams.round() as i64).saturating_mul(int(line, "quantity"))
        })
        .fold(0i64, i64::saturating_add);
    let overridden: Vec<bool> = lines
        .iter()
        .map(|line| {
            line.get("priceOverride")
                .is_some_and(|price| !price.is_null())
        })
        .collect();
    let tax_lines: Vec<Value> = (tax > 0)
        .then(|| json!({"title": "Tax", "rate": rate as f64 / 10_000.0, "ratePercentage": rate as f64 / 100.0, "priceSet": bag(tax), "price": value::money(tax), "channelLiable": false, "source": "Shopify"}))
        .into_iter()
        .collect();
    value::object(json!({
        "appliedDiscount": applied_value,
        "lineItemsSubtotalPrice": bag(subtotal_lines),
        "totalLineItemsPriceSet": bag(original),
        "subtotalPriceSet": bag(subtotal),
        "subtotalPrice": value::money(subtotal),
        "totalDiscountsSet": bag(line_discounts + order_discount),
        "totalShippingPriceSet": bag(shipping_price),
        "totalShippingPrice": value::money(shipping_price),
        "totalTaxSet": bag(tax),
        "totalTax": value::money(tax),
        "taxLines": tax_lines,
        "taxesIncluded": included,
        "totalPriceSet": bag(total),
        "totalPrice": value::money(total),
        "amountDueNowSet": bag(total),
        "amountDueLaterSet": bag(0),
        "totalQuantityOfLineItems": quantity,
        "totalWeight": grams.max(0).to_string(),
        "allVariantPricesOverridden": !overridden.is_empty() && overridden.iter().all(|flag| *flag),
        "anyVariantPricesOverridden": overridden.iter().any(|flag| *flag),
    }))
}

/// A `ShippingLine` of a `ShippingLineInput`.
fn shipping_line(input: &Value, currency: &str, now: &str) -> Option<Value> {
    let input = input.as_object()?;
    let price = input
        .get("priceWithCurrency")
        .filter(|price| !price.is_null())
        .or_else(|| input.get("price"))
        .map_or(0, |price| model::cents(Some(price)));
    let handle = input.get("shippingRateHandle").and_then(Value::as_str);
    Some(json!({
        "title": input.get("title").and_then(Value::as_str).unwrap_or("Shipping"),
        "code": input.get("title"),
        "shippingRateHandle": handle,
        "custom": handle.is_none(),
        "source": "shopify",
        "isRemoved": false,
        "originalPriceSet": value::money_bag(price, currency),
        "discountedPriceSet": value::money_bag(price, currency),
        "currentDiscountedPriceSet": value::money_bag(price, currency),
        "originalPrice": value::money_v2(price, currency),
        "discountedPrice": value::money_v2(price, currency),
        "price": value::money(price),
        "discountAllocations": [],
        "taxLines": [],
        "createdAt": now,
    }))
}

/// What a `DraftOrderInput` gives, checked: the lines and the fields that are not totals.
struct Draft {
    lines: Vec<Map<String, Value>>,
    fields: Map<String, Value>,
}

/// Reads a `DraftOrderInput` against `base` (a draft's record for an update, empty for a
/// creation): its line items when given, the fields given.
fn read_input(
    mutation: &mut Mutation<'_>,
    name: &str,
    input: &Map<String, Value>,
    base: Option<&Record>,
    errors: &mut Vec<UserError>,
) -> Option<Draft> {
    for (field, what) in NOT_SIMULATED {
        if input
            .get(*field)
            .is_some_and(|value| !value.is_null() && value != &json!([]))
        {
            mutation.note(format!(
                "{name}: lsf does not simulate {what} yet: `{field}` was ignored"
            ));
        }
    }
    let state = mutation.state();
    let currency = model::currency(state);
    let now = state.timestamp();
    let lines: Option<Vec<Map<String, Value>>> = input
        .get("lineItems")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let index = index.to_string();
                    line_value(state, item, &currency, &["lineItems", &index], errors)
                })
                .collect()
        });
    if let Some(lines) = &lines {
        let total: i128 = lines
            .iter()
            .map(|line| i128::from(field_cents(line, "originalTotalSet")))
            .sum();
        if !i64::try_from(total).is_ok_and(value::money_in_bounds) {
            errors.push(UserError::new(
                &["lineItems"],
                value::money_too_large("Total price"),
            ));
        }
    }
    let lines = match (lines, base) {
        (Some(lines), _) => lines,
        (None, Some(base)) => gids(base.get("lineItems"))
            .iter()
            .filter_map(|line| state.get(line).cloned())
            .collect(),
        (None, None) => Vec::new(),
    };
    let mut fields = Map::new();
    let purchasing = model::input_of(input.get("purchasingEntity"));
    let state = mutation.state();
    let company = model::input_of(purchasing.get("purchasingCompany"));
    if !company.is_empty() {
        purchasing_company(state, &company, input, &mut fields, errors);
    }
    if let Some(terms) = input.get("paymentTerms").filter(|terms| !terms.is_null()) {
        let terms = model::input_of(Some(terms));
        let base = base
            .and_then(|base| text(base, "paymentTerms"))
            .and_then(|terms| state.get(terms));
        match super::payments::read_terms(state, &terms, &["paymentTerms"], "INVALID", base) {
            Ok(_) => {
                fields.insert(TERMS_INPUT.into(), Value::Object(terms));
            }
            Err(error) => errors.push(error),
        }
    }
    let customer = purchasing
        .get("customerId")
        .or_else(|| input.get("customerId"))
        .filter(|id| !id.is_null());
    if let Some(customer) = customer.filter(|_| company.is_empty()) {
        let id = customer.as_str().unwrap_or_default();
        if state.type_of(id) == Some("Customer") {
            fields.insert("customer".into(), json!(id));
            fields.insert("purchasingEntity".into(), json!(id));
        } else {
            errors.push(UserError::new(&["customerId"], "Customer does not exist"));
        }
    }
    if let Some(email) = input.get("email") {
        let email = email
            .as_str()
            .map(str::trim)
            .filter(|email| !email.is_empty());
        if email.is_some_and(|email| !email.contains('@')) {
            errors.push(UserError::new(&["email"], "Email is invalid"));
        }
        fields.insert("email".into(), json!(email));
    }
    for field in ["phone", "poNumber"] {
        if let Some(value) = input.get(field) {
            fields.insert(
                field.into(),
                json!(value.as_str().filter(|text| !text.is_empty())),
            );
        }
    }
    if let Some(note) = input.get("note") {
        fields.insert(
            "note2".into(),
            json!(note.as_str().filter(|text| !text.is_empty())),
        );
    }
    if input.contains_key("tags") {
        fields.insert("tags".into(), json!(tags_of(input.get("tags"))));
    }
    if input.contains_key("customAttributes") {
        fields.insert(
            "customAttributes".into(),
            model::attributes(input.get("customAttributes")),
        );
    }
    for field in [
        "taxExempt",
        "visibleToCustomer",
        "allowDiscountCodesInCheckout",
    ] {
        if let Some(flag) = input.get(field).and_then(Value::as_bool) {
            fields.insert(field.into(), json!(flag));
        }
    }
    if let Some(until) = input.get("reserveInventoryUntil") {
        fields.insert("reserveInventoryUntil".into(), until.clone());
    }
    if let Some(discount) = input.get("appliedDiscount") {
        if let Some(value) = discount.get("value").and_then(Value::as_f64)
            && discount.get("valueType").and_then(Value::as_str) == Some("PERCENTAGE")
            && !(0.0..=100.0).contains(&value)
        {
            errors.push(UserError::new(
                &["appliedDiscount", "value"],
                "Value must be between 0 and 100",
            ));
        }
        fields.insert("lsfAppliedDiscount".into(), discount.clone());
    }
    if let Some(line) = input.get("shippingLine") {
        fields.insert(
            "shippingLine".into(),
            shipping_line(line, &currency, &now).unwrap_or(Value::Null),
        );
    }
    let id_of = |kind: &str| {
        format!(
            "gid://shopify/MailingAddress/{}?model_name=Address",
            stable_id(
                "draft_address",
                &format!("{now}/{kind}/{}", state.count("DraftOrder"))
            )
        )
    };
    for (field, kind) in [
        ("shippingAddress", "shipping"),
        ("billingAddress", "billing"),
    ] {
        if let Some(address) = input.get(field) {
            fields.insert(
                field.into(),
                model::mailing_address(address, &id_of(kind)).unwrap_or(Value::Null),
            );
        }
    }
    if input
        .get("useCustomerDefaultAddress")
        .and_then(Value::as_bool)
        == Some(true)
        && let Some(address) = fields
            .get("customer")
            .and_then(Value::as_str)
            .and_then(|customer| state.get(customer))
            .and_then(|customer| customer.get("defaultAddress"))
            .filter(|address| !address.is_null())
    {
        for field in ["shippingAddress", "billingAddress"] {
            fields.entry(field).or_insert_with(|| address.clone());
        }
    }
    if let Some(email) = fields
        .get("customer")
        .and_then(Value::as_str)
        .and_then(|customer| state.get(customer))
        .and_then(|customer| customer.get("email"))
        .filter(|_| {
            !input.contains_key("email")
                && base.is_none_or(|base| base.get("email").is_none_or(Value::is_null))
        })
    {
        fields.insert("email".into(), email.clone());
    }
    Some(Draft { lines, fields })
}

/// The field of a draft's fields that holds its `paymentTerms` input until the draft is
/// written: [`apply_terms`] makes or changes its payment terms from it.
const TERMS_INPUT: &str = "lsfPaymentTermsInput";

/// Reads a `PurchasingCompanyInput`: the company, one of its locations and one of its contacts.
/// The draft is the contact's customer's, for the location, with the location's addresses and
/// its payment terms unless the input gives its own (Shopify's B2B drafts take the location's
/// buyer experience).
fn purchasing_company(
    state: &ShopState,
    company: &Map<String, Value>,
    input: &Map<String, Value>,
    fields: &mut Map<String, Value>,
    errors: &mut Vec<UserError>,
) {
    let path = |field: &'static str| ["purchasingEntity", "purchasingCompany", field];
    let id = |field: &str| {
        company
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (company_id, location, contact) = (
        id("companyId"),
        id("companyLocationId"),
        id("companyContactId"),
    );
    if state.type_of(&company_id) != Some("Company") {
        // lsf's wording, as every message of the B2B inputs: shopify.dev shows none.
        errors.push(UserError::new(&path("companyId"), "Company does not exist"));
        return;
    }
    let of_company = |key: &str, kind: &str| {
        state
            .get(key)
            .filter(|_| state.type_of(key) == Some(kind))
            .filter(|record| text(record, "company") == Some(company_id.as_str()))
    };
    let Some(location_record) = of_company(&location, "CompanyLocation") else {
        errors.push(UserError::new(
            &path("companyLocationId"),
            "Company location does not exist for this company",
        ));
        return;
    };
    let Some(contact_record) = of_company(&contact, "CompanyContact") else {
        errors.push(UserError::new(
            &path("companyContactId"),
            "Company contact does not exist for this company",
        ));
        return;
    };
    fields.insert(
        "purchasingEntity".into(),
        json!({"__typename": "PurchasingCompany", "company": company_id, "location": location, "contact": contact}),
    );
    if let Some(customer) = text(contact_record, "customer") {
        fields.insert("customer".into(), json!(customer));
    }
    for (field, from) in [
        ("shippingAddress", "shippingAddress"),
        ("billingAddress", "billingAddress"),
    ] {
        if input.contains_key(field) {
            continue;
        }
        let address = location_record
            .get(from)
            .filter(|address| !address.is_null())
            .or_else(|| location_record.get("shippingAddress"))
            .and_then(Value::as_object);
        if let Some(address) = address {
            let address_input = json!({
                "firstName": address.get("firstName"),
                "lastName": address.get("lastName"),
                "company": address.get("companyName"),
                "address1": address.get("address1"),
                "address2": address.get("address2"),
                "city": address.get("city"),
                "province": address.get("province"),
                "provinceCode": address.get("zoneCode"),
                "countryCode": address.get("countryCode"),
                "zip": address.get("zip"),
                "phone": address.get("phone"),
            });
            let address_id = address
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(address) = model::mailing_address(&address_input, &address_id) {
                fields.insert(field.into(), address);
            }
        }
    }
    if !input.contains_key("paymentTerms")
        && let Some(template) = location_record
            .get("buyerExperienceConfiguration")
            .and_then(|configuration| configuration.get("paymentTermsTemplate"))
            .and_then(Value::as_str)
    {
        fields.insert(
            TERMS_INPUT.into(),
            json!({"paymentTermsTemplateId": template}),
        );
    }
}

/// Makes or changes a draft's payment terms from the `paymentTerms` input its fields kept
/// (`payment_terms/create` or `…/update`), and keeps their amounts with the draft's total.
fn apply_terms(state: &mut ShopState, id: &str, input: Option<&Value>) {
    let existing = state
        .get(id)
        .and_then(|record| text(record, "paymentTerms"))
        .map(str::to_string);
    if let Some(input) = input.and_then(Value::as_object) {
        let base = existing.as_deref().and_then(|terms| state.get(terms));
        let Ok(spec) =
            super::payments::read_terms(state, input, &["paymentTerms"], "INVALID", base)
        else {
            return;
        };
        match &existing {
            Some(terms) => {
                super::payments::update_terms(state, terms, &spec);
                state.emit("payment_terms/update", terms);
            }
            None => {
                let terms = super::payments::create_terms(state, "draftOrder", id, &spec);
                state.emit("payment_terms/create", &terms);
            }
        }
    } else if let Some(terms) = &existing {
        super::payments::refresh_terms(state, terms);
    }
}

/// Writes a draft's line items (replacing its old ones) and its fields and totals into its
/// record. Returns the `paymentTerms` input the draft was given, which the caller applies with
/// [`apply_terms`] once it emitted the draft's event.
fn write_draft(
    state: &mut ShopState,
    id: &str,
    draft: &Draft,
    replace_lines: bool,
) -> Option<Value> {
    let currency = model::currency(state);
    let mut line_ids = Vec::new();
    if replace_lines {
        for old in state
            .get(id)
            .map(|record| gids(record.get("lineItems")))
            .unwrap_or_default()
        {
            state.remove(&old);
        }
        for line in &draft.lines {
            let line_id = state.new_gid("DraftOrderLineItem");
            let mut record = line.clone();
            record.insert("__typename".into(), json!("DraftOrderLineItem"));
            record.insert("id".into(), json!(line_id));
            if record.get("uuid").is_none_or(Value::is_null) {
                record.insert(
                    "uuid".into(),
                    json!(value::uuid(&format!("{}/{line_id}", state.domain()))),
                );
            }
            state.insert(record);
            line_ids.push(line_id);
        }
    }
    let mut record = state.get(id).cloned()?;
    let mut fields = draft.fields.clone();
    let terms = fields.remove(TERMS_INPUT);
    record.extend(fields);
    if replace_lines {
        record.insert("lineItems".into(), json!(line_ids));
    }
    let applied = record.remove("lsfAppliedDiscount").or_else(|| {
        record.get("appliedDiscount").filter(|discount| !discount.is_null()).map(|discount| {
            json!({"value": discount.get("value"), "valueType": discount.get("valueType"), "title": discount.get("title"), "description": discount.get("description")})
        })
    });
    let lines: Vec<Map<String, Value>> = gids(record.get("lineItems"))
        .iter()
        .filter_map(|line| state.get(line).cloned())
        .collect();
    let tax_exempt = record.get("taxExempt").and_then(Value::as_bool) == Some(true);
    record.extend(totals(
        state,
        &lines,
        applied.as_ref(),
        record.get("shippingLine"),
        tax_exempt,
        &currency,
    ));
    let matches = match (record.get("billingAddress"), record.get("shippingAddress")) {
        (Some(billing), Some(shipping)) if !billing.is_null() && !shipping.is_null() => [
            "address1",
            "address2",
            "city",
            "zip",
            "countryCodeV2",
            "firstName",
            "lastName",
        ]
        .iter()
        .all(|key| billing.get(key) == shipping.get(key)),
        _ => false,
    };
    record.insert(
        "billingAddressMatchesShippingAddress".into(),
        json!(matches),
    );
    state.insert(record);
    state.touch(id);
    terms
}

/// The numeric part of draft names: one above the highest.
fn next_draft_number(state: &ShopState) -> u64 {
    state
        .all("DraftOrder")
        .filter_map(|draft| {
            text(draft, "name")?
                .trim_start_matches("#D")
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// A new draft's record, before its lines and totals.
fn new_draft(state: &mut ShopState) -> String {
    let id = state.new_gid("DraftOrder");
    let now = state.timestamp();
    let currency = model::currency(state);
    let shop = state.shop().cloned().unwrap_or_default();
    let origin = text(&shop, "url").unwrap_or_default().to_string();
    let shop_number = state
        .shop_gid()
        .and_then(parse_gid)
        .map_or(0, |(_, number)| number);
    let key = format!("{}/{id}", state.domain());
    let token = format!(
        "{:016x}{:016x}",
        stable_id("draft_order_invoice", &key),
        stable_id("draft_order_invoice/2", &key)
    );
    let number = next_draft_number(state);
    state.insert(value::object(json!({
        "__typename": "DraftOrder",
        "id": id,
        "name": format!("#D{number}"),
        "status": "OPEN",
        "createdAt": now,
        "updatedAt": now,
        "completedAt": null,
        "invoiceSentAt": null,
        "invoiceUrl": format!("{origin}/{shop_number}/invoices/{token}"),
        "invoiceEmailTemplateSubject": "Complete your purchase",
        "currencyCode": currency,
        "presentmentCurrencyCode": currency,
        "email": null,
        "phone": null,
        "note2": null,
        "poNumber": null,
        "customer": null,
        "purchasingEntity": null,
        "tags": [],
        "customAttributes": [],
        "taxExempt": false,
        "visibleToCustomer": false,
        "allowDiscountCodesInCheckout": false,
        "acceptAutomaticDiscounts": null,
        "reserveInventoryUntil": null,
        "discountCodes": [],
        "platformDiscounts": [],
        "warnings": [],
        "lineItems": [],
        "appliedDiscount": null,
        "shippingLine": null,
        "shippingAddress": null,
        "billingAddress": null,
        "order": null,
        "ready": true,
        "hasTimelineComment": false,
        "paymentTerms": null,
        "metafields": [],
    })));
    id
}

/// `draftOrderCreate`.
fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    Ok(match create_from(mutation, ctx.field_name(), &input) {
        Ok(id) => payload(json!({"draftOrder": id}), &[]),
        Err(errors) => payload(json!({"draftOrder": null}), &errors),
    })
}

/// Makes a draft order of a `DraftOrderInput`, as `draftOrderCreate` does (`name` is the
/// mutation, for its notes): `draft_orders/create`. Its GID, or the input's errors.
pub(super) fn create_from(
    mutation: &mut Mutation<'_>,
    name: &str,
    input: &Map<String, Value>,
) -> Result<String, Vec<UserError>> {
    let mut errors = Vec::new();
    let Some(draft) = read_input(mutation, name, input, None, &mut errors) else {
        return Err(errors);
    };
    if array(input, "lineItems").is_empty() {
        errors.insert(0, UserError::new(&[], "Add at least 1 product"));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let state = mutation.state_mut();
    let id = new_draft(state);
    let terms = write_draft(state, &id, &draft, true);
    state.emit("draft_orders/create", &id);
    apply_terms(state, &id, terms.as_ref());
    Ok(id)
}

/// The draft order of a GID, or Shopify's "Draft order not found".
pub(super) fn find_draft(state: &ShopState, id: &str) -> Result<Record, UserError> {
    state
        .get(id)
        .filter(|_| state.type_of(id) == Some("DraftOrder"))
        .cloned()
        .ok_or_else(|| UserError::new(&[], "Draft order not found"))
}

/// lsf's wording for a change to a completed draft.
fn completed_error() -> UserError {
    UserError::new(
        &[],
        "This draft order has been completed and can no longer be changed",
    )
}

/// `draftOrderUpdate`.
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let record = match find_draft(mutation.state(), &id) {
        Ok(record) => record,
        Err(error) => return Ok(payload(json!({"draftOrder": null}), &[error])),
    };
    if text(&record, "status") == Some("COMPLETED") {
        return Ok(payload(json!({"draftOrder": null}), &[completed_error()]));
    }
    let input = model::input_of(ctx.arg("input"));
    let mut errors = Vec::new();
    let Some(draft) = read_input(
        mutation,
        ctx.field_name(),
        &input,
        Some(&record),
        &mut errors,
    ) else {
        return Ok(payload(json!({"draftOrder": null}), &errors));
    };
    if input.contains_key("lineItems") && draft.lines.is_empty() && errors.is_empty() {
        errors.push(UserError::new(&[], "Add at least 1 product"));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"draftOrder": null}), &errors));
    }
    let state = mutation.state_mut();
    let terms = write_draft(state, &id, &draft, input.contains_key("lineItems"));
    state.emit("draft_orders/update", &id);
    apply_terms(state, &id, terms.as_ref());
    Ok(payload(json!({"draftOrder": id}), &[]))
}

/// `draftOrderDelete`.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx
        .arg("input")
        .and_then(|input| input.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let record = match find_draft(mutation.state(), &id) {
        Ok(record) => record,
        Err(error) => return Ok(payload(json!({"deletedId": null}), &[error])),
    };
    let state = mutation.state_mut();
    remove_draft(state, &id, &record);
    Ok(payload(json!({"deletedId": id}), &[]))
}

/// Deletes a draft order with its line items and its payment terms: `draft_orders/delete`, and
/// `payment_terms/delete` for terms the draft still owned.
pub(super) fn remove_draft(state: &mut ShopState, id: &str, record: &Record) {
    if let Some(terms) = text(record, "paymentTerms")
        && state
            .get(terms)
            .is_some_and(|terms| text(terms, "order").is_none())
    {
        super::payments::delete_terms(state, terms);
    }
    state.delete(id, "draft_orders/delete");
    for line in gids(record.get("lineItems")) {
        state.remove(&line);
    }
}

/// The subject and the text of a draft order's invoice email: the email's own when given, else
/// lsf's wording of Shopify's template ("Invoice #D1 from <shop>", as `draftOrderInvoicePreview`'s
/// example shows it).
pub(super) fn invoice_text(
    state: &ShopState,
    record: &Record,
    email: &Map<String, Value>,
) -> (String, String) {
    let name = text(record, "name").unwrap_or_default();
    let shop = state
        .shop()
        .and_then(|shop| text(shop, "name"))
        .unwrap_or_default();
    let subject = email
        .get("subject")
        .and_then(Value::as_str)
        .map_or_else(|| format!("Draft Order {name}"), str::to_string);
    let mut body = format!("Invoice {name} from {shop}");
    if let Some(message) = email.get("customMessage").and_then(Value::as_str) {
        body.push('\n');
        body.push_str(message);
    }
    if let Some(url) = text(record, "invoiceUrl") {
        body.push('\n');
        body.push_str(url);
    }
    (subject, body)
}

/// `draftOrderCalculate`: the totals of a draft order input, without making a draft.
fn calculate(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let mut errors = Vec::new();
    let Some(draft) = read_input(mutation, ctx.field_name(), &input, None, &mut errors) else {
        return Ok(payload(json!({"calculatedDraftOrder": null}), &errors));
    };
    if array(&input, "lineItems").is_empty() {
        errors.insert(0, UserError::new(&[], "Add at least 1 product"));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"calculatedDraftOrder": null}), &errors));
    }
    let state = mutation.state();
    let currency = model::currency(state);
    let tax_exempt = draft.fields.get("taxExempt").and_then(Value::as_bool) == Some(true);
    let mut calculated = draft.fields.clone();
    let applied = calculated.remove("lsfAppliedDiscount");
    calculated.remove(TERMS_INPUT);
    calculated.extend(totals(
        state,
        &draft.lines,
        applied.as_ref(),
        draft.fields.get("shippingLine"),
        tax_exempt,
        &currency,
    ));
    for field in [
        "email",
        "note2",
        "poNumber",
        "tags",
        "customAttributes",
        "shippingAddress",
        "billingAddress",
        "visibleToCustomer",
        "reserveInventoryUntil",
        "allowDiscountCodesInCheckout",
    ] {
        calculated.remove(field);
    }
    calculated.insert("lineItems".into(), json!(draft.lines));
    calculated.insert("currencyCode".into(), json!(currency));
    calculated.insert("presentmentCurrencyCode".into(), json!(currency));
    calculated.insert("discountCodes".into(), json!([]));
    calculated.insert("platformDiscounts".into(), json!([]));
    calculated.insert("warnings".into(), json!([]));
    calculated.insert("alerts".into(), json!([]));
    calculated.insert("availableShippingRates".into(), json!([]));
    calculated.insert("billingAddressMatchesShippingAddress".into(), json!(false));
    calculated.entry("customer").or_insert(Value::Null);
    calculated.entry("shippingLine").or_insert(Value::Null);
    Ok(payload(json!({"calculatedDraftOrder": calculated}), &[]))
}

/// `draftOrderComplete`.
fn complete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let record = match find_draft(mutation.state(), &id) {
        Ok(record) => record,
        Err(error) => return Ok(payload(json!({"draftOrder": null}), &[error])),
    };
    if text(&record, "status") == Some("COMPLETED") {
        return Ok(payload(
            json!({"draftOrder": null}),
            &[UserError::new(
                &[],
                "This draft order has already been completed",
            )],
        ));
    }
    if ctx.arg("paymentGatewayId").is_some_and(|id| !id.is_null()) {
        // lsf has no payment gateways: any id is Shopify's "Invalid payment gateway".
        return Ok(payload(
            json!({"draftOrder": null}),
            &[UserError::new(&[], "Invalid payment gateway")],
        ));
    }
    let lines: Vec<Record> = gids(record.get("lineItems"))
        .iter()
        .filter_map(|line| mutation.state().get(line).cloned())
        .collect();
    if lines.is_empty() {
        return Ok(payload(
            json!({"draftOrder": null}),
            &[UserError::new(&[], "Add at least 1 product")],
        ));
    }
    // An order of payment terms is paid later: it is pending, as `paymentPending` makes it.
    let terms = text(&record, "paymentTerms").map(str::to_string);
    let pending = ctx.arg_bool("paymentPending") == Some(true) || terms.is_some();
    let source = ctx
        .arg_str("sourceName")
        .unwrap_or("shopify_draft_order")
        .to_string();
    let state = mutation.state_mut();
    let order_id = state.new_gid("Order");
    let number = model::next_order_number(state);
    let line_drafts: Vec<LineDraft> = lines
        .iter()
        .map(|line| LineDraft {
            product: text(line, "product").map(str::to_string),
            variant: text(line, "variant").map(str::to_string),
            inventory_item: text(line, "variant")
                .and_then(|variant| state.get(variant))
                .and_then(|variant| text(variant, "inventoryItem"))
                .map(str::to_string),
            title: text(line, "title").unwrap_or_default().to_string(),
            variant_title: text(line, "variantTitle").map(str::to_string),
            sku: text(line, "sku").map(str::to_string),
            vendor: text(line, "vendor").map(str::to_string),
            quantity: int(line, "quantity").max(0) as u32,
            unit_price: field_cents(line, "originalUnitPriceSet"),
            discount: field_cents(line, "totalDiscountSet"),
            requires_shipping: line
                .get("requiresShipping")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            taxable: line.get("taxable").and_then(Value::as_bool).unwrap_or(true),
            gift_card: line
                .get("isGiftCard")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            image: line.get("image").filter(|image| !image.is_null()).cloned(),
            weight: line
                .get("weight")
                .filter(|weight| !weight.is_null())
                .cloned(),
            grams: 0,
            attributes: line
                .get("customAttributes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|attribute| {
                    Some((
                        attribute.get("key")?.as_str()?.to_string(),
                        attribute
                            .get("value")?
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    ))
                })
                .collect(),
        })
        .collect();
    let line_subtotal = field_cents(&record, "lineItemsSubtotalPrice");
    let subtotal = field_cents(&record, "subtotalPriceSet");
    let shipping = record
        .get("shippingLine")
        .filter(|line| !line.is_null())
        .map(|line| {
            let price = field_cents(line.as_object().unwrap_or(&Map::new()), "originalPriceSet");
            ShippingDraft {
                title: line
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Shipping")
                    .to_string(),
                handle: line
                    .get("shippingRateHandle")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                original: price,
                discounted: price,
            }
        });
    let included = record.get("taxesIncluded").and_then(Value::as_bool) == Some(true);
    let draft = OrderDraft {
        id: order_id.clone(),
        name: format!("#{number}"),
        number,
        created_at: state.now(),
        customer: text(&record, "customer").map(str::to_string),
        email: text(&record, "email").map(str::to_string),
        phone: text(&record, "phone").map(str::to_string),
        accepts_marketing: false,
        tax_exempt: record.get("taxExempt").and_then(Value::as_bool) == Some(true),
        lines: line_drafts,
        shipping_address: record
            .get("shippingAddress")
            .filter(|a| !a.is_null())
            .cloned(),
        billing_address: record
            .get("billingAddress")
            .filter(|a| !a.is_null())
            .cloned(),
        shipping,
        tax: if included {
            0
        } else {
            field_cents(&record, "totalTaxSet")
        },
        order_discount: line_subtotal - subtotal,
        discount_codes: Vec::new(),
        note: text(&record, "note2").map(str::to_string),
        attributes: record
            .get("customAttributes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|attribute| {
                Some((
                    attribute.get("key")?.as_str()?.to_string(),
                    attribute
                        .get("value")?
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ))
            })
            .collect(),
        financial: FinancialStatus::Pending,
        fulfillment: FulfillmentState::Unfulfilled,
        cancelled: false,
        paid: None,
        gateway: None,
        source_name: Some(source),
        returns: Vec::new(),
        location: state.keys("Location").next().map(str::to_string),
    };
    let order = super::insert_order(state, &draft);
    if let Some(order_record) = state.get_mut(&order) {
        order_record.insert(
            "tags".into(),
            record.get("tags").cloned().unwrap_or(json!([])),
        );
        order_record.insert(
            "poNumber".into(),
            record.get("poNumber").cloned().unwrap_or(Value::Null),
        );
        order_record.insert("closed".into(), json!(false));
        if let Some(entity) = record
            .get("purchasingEntity")
            .filter(|entity| entity.is_object())
        {
            order_record.insert("purchasingEntity".into(), entity.clone());
        }
        if included {
            order_record.insert(
                "totalTaxSet".into(),
                record.get("totalTaxSet").cloned().unwrap_or(Value::Null),
            );
            order_record.insert("taxesIncluded".into(), json!(true));
        }
    }
    if let Some(location) = record
        .get("purchasingEntity")
        .and_then(|entity| entity.get("location"))
        .and_then(Value::as_str)
    {
        crate::domains::customers::refresh_company_location(state, location);
    }
    state.emit("orders/create", &order);
    if let Some(terms) = &terms {
        super::payments::attach_to_order(state, terms, &order);
    }
    if !pending {
        let total = state
            .get(&order)
            .map_or(0, |order| field_cents(order, "totalPriceSet"));
        model::add_transaction(
            state,
            &order,
            &NewTransaction {
                kind: "SALE",
                amount: total,
                gateway: Some("manual"),
                parent: None,
                manual: true,
                test: false,
            },
        );
    }
    model::settle_financial_status(state, &order);
    // The units are committed, as an order of the admin commits them.
    for line in gids(state.get(&order).and_then(|record| record.get("lineItems"))) {
        let Some(item_record) = state.get(&line).cloned() else {
            continue;
        };
        let location = model::line_location(state, &order, &line);
        model::commit_line(
            state,
            &line,
            location.as_deref(),
            int(&item_record, "quantity"),
        );
    }
    if state
        .get(&order)
        .and_then(|record| text(record, "displayFinancialStatus"))
        == Some("PAID")
    {
        state.emit("orders/paid", &order);
    }
    let now = state.timestamp();
    if let Some(draft) = state.get_mut(&id) {
        draft.insert("status".into(), json!("COMPLETED"));
        draft.insert("completedAt".into(), json!(now));
        draft.insert("order".into(), json!(order));
    }
    state.touch(&id);
    state.emit("draft_orders/update", &id);
    Ok(payload(json!({"draftOrder": id}), &[]))
}

/// `draftOrderDuplicate`.
fn duplicate(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx
        .arg_str("id")
        .or_else(|| ctx.arg_str("draftOrderId"))
        .unwrap_or_default()
        .to_string();
    let record = match find_draft(mutation.state(), &id) {
        Ok(record) => record,
        Err(error) => return Ok(payload(json!({"draftOrder": null}), &[error])),
    };
    let lines: Vec<Map<String, Value>> = gids(record.get("lineItems"))
        .iter()
        .filter_map(|line| mutation.state().get(line).cloned())
        .collect();
    let state = mutation.state_mut();
    let copy = new_draft(state);
    let mut fields = Map::new();
    for field in [
        "email",
        "phone",
        "note2",
        "poNumber",
        "customer",
        "purchasingEntity",
        "tags",
        "customAttributes",
        "taxExempt",
        "shippingLine",
        "shippingAddress",
        "billingAddress",
        "visibleToCustomer",
        "allowDiscountCodesInCheckout",
    ] {
        if let Some(value) = record.get(field) {
            fields.insert(field.into(), value.clone());
        }
    }
    if let Some(discount) = record
        .get("appliedDiscount")
        .filter(|discount| !discount.is_null())
    {
        fields.insert(
            "lsfAppliedDiscount".into(),
            json!({"value": discount.get("value"), "valueType": discount.get("valueType"), "title": discount.get("title"), "description": discount.get("description")}),
        );
    }
    let lines = lines
        .into_iter()
        .map(|mut line| {
            line.remove("id");
            line.remove("uuid");
            line
        })
        .collect();
    let terms = write_draft(state, &copy, &Draft { lines, fields }, true);
    apply_terms(state, &copy, terms.as_ref());
    state.emit("draft_orders/create", &copy);
    Ok(payload(json!({"draftOrder": copy}), &[]))
}

/// `draftOrderInvoiceSend`: the invoice is recorded as sent; lsf sends no email.
fn invoice_send(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let record = match find_draft(mutation.state(), &id) {
        Ok(record) => record,
        Err(error) => return Ok(payload(json!({"draftOrder": null}), &[error])),
    };
    if text(&record, "status") == Some("COMPLETED") {
        return Ok(payload(json!({"draftOrder": id}), &[completed_error()]));
    }
    let email = model::input_of(ctx.arg("email"));
    let to = email
        .get("to")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| text(&record, "email").map(str::to_string));
    if to.as_ref().is_none_or(|to| !to.contains('@')) {
        // The reference's example, which answers the draft order with the error.
        return Ok(payload(
            json!({"draftOrder": id}),
            &[UserError::new(&[], "To is invalid")],
        ));
    }
    let (subject, body) = invoice_text(mutation.state(), &record, &email);
    let now = mutation.timestamp();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("INVOICE_SENT"));
        record.insert("invoiceSentAt".into(), json!(now));
    }
    state.touch(&id);
    // Net terms are issued with the invoice: their due date counts from it.
    if let Some(terms) = text(&record, "paymentTerms") {
        super::payments::issue_terms(state, terms);
        super::payments::refresh_terms(state, terms);
    }
    let to = to.unwrap_or_default();
    crate::outbox::send(
        state,
        &crate::outbox::Email {
            template: "draft_order_invoice",
            to: &to,
            subject,
            body,
            resource: Some(&id),
        },
    );
    state.emit("draft_orders/update", &id);
    Ok(payload(json!({"draftOrder": id}), &[]))
}

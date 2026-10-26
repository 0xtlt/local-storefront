//! STUB, replaced by the input builder (see "Contract between `pipeline` and `input`" in
//! `.design/checkout.md`). It keeps the exact signature the pipeline calls and, instead of
//! executing the query, answers every Function with the same generic input: the common fields
//! of the Function input graph, built from the context.

use serde_json::{Value, json};

use crate::{DeliveryGroup, DiscountClass, Metafield, PaymentMethod, ResultLine};

pub struct InputContext<'a> {
    pub store: &'a lsf_core::Store,
    /// customer_id, country, cart note/attributes.
    pub session: &'a lsf_core::Session,
    /// email, phone, address, codes, metafields.
    pub state: &'a crate::CheckoutState,
    /// The lines as they are at this step.
    pub lines: &'a [ResultLine],
    pub delivery_groups: &'a [DeliveryGroup],
    pub payment_methods: &'a [PaymentMethod],
    /// The object that owns the Function.
    pub owner: Owner<'a>,
    pub entered_codes: &'a [String],
    pub triggering_code: Option<&'a str>,
    pub currency: &'a str,
    pub now: chrono::DateTime<chrono::FixedOffset>,
}

pub struct Owner<'a> {
    /// Which root field holds it.
    pub kind: OwnerKind,
    /// A gid, e.g. `gid://shopify/DiscountAutomaticNode/1`.
    pub id: String,
    pub metafields: &'a [Metafield],
    pub discount_classes: &'a [DiscountClass],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerKind {
    Discount,
    DiscountNode,
    CartTransform,
    DeliveryCustomization,
    PaymentCustomization,
    Validation,
}

/// Runs `query` (the Function's input query) against the checkout, with `variables` from
/// `[extensions.input.variables]` (the JSON value of that owner metafield, or none).
pub fn build(
    _query: &str,
    _variables: Option<&Value>,
    ctx: &InputContext,
) -> Result<Value, String> {
    let money = |cents: i64| json!({ "amount": format!("{}.{:02}", cents / 100, (cents % 100).abs()), "currencyCode": ctx.currency });
    let lines: Vec<Value> = ctx
        .lines
        .iter()
        .map(|line| {
            json!({
                "id": line.id,
                "quantity": line.quantity,
                "cost": {
                    "amountPerQuantity": money(line.unit_price),
                    "subtotalAmount": money(line.subtotal),
                    "totalAmount": money(line.total),
                },
                "merchandise": {
                    "__typename": "ProductVariant",
                    "id": format!("gid://shopify/ProductVariant/{}", line.variant_id),
                    "sku": line.sku,
                    "title": line.variant_title,
                    "product": {
                        "id": format!("gid://shopify/Product/{}", line.product_id),
                        "title": line.title,
                    },
                },
            })
        })
        .collect();
    let subtotal: i64 = ctx.lines.iter().map(|line| line.subtotal).sum();
    let metafield = ctx.owner.metafields.first().map(|metafield| {
        json!({ "namespace": metafield.namespace, "key": metafield.key, "type": metafield.kind, "value": metafield.value })
    });
    let delivery_groups: Vec<Value> = ctx
        .delivery_groups
        .iter()
        .map(|group| {
            json!({
                "id": group.handle,
                "selectedDeliveryOption": group.selected.as_ref().map(|handle| json!({ "handle": handle })),
                "deliveryOptions": group.options.iter().map(|option| json!({
                    "handle": option.handle,
                    "title": option.title,
                    "cost": money(option.cost),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let owner = json!({ "id": ctx.owner.id, "metafield": metafield });

    let mut input = json!({
        "cart": {
            "lines": lines,
            "cost": { "subtotalAmount": money(subtotal) },
            "buyerIdentity": { "email": ctx.state.email, "phone": ctx.state.phone },
            "deliveryGroups": delivery_groups,
        },
        "paymentMethods": ctx.payment_methods.iter().map(|method| json!({ "id": method.handle, "name": method.name })).collect::<Vec<_>>(),
        "enteredDiscountCodes": ctx.entered_codes.iter().map(|code| json!({ "code": code, "rejectable": true })).collect::<Vec<_>>(),
        "triggeringDiscountCode": ctx.triggering_code,
        "localization": { "country": { "isoCode": ctx.state.shipping_address.as_ref().and_then(|a| a.country_code.clone()) } },
        "presentmentCurrencyRate": "1.0",
    });
    let key = match ctx.owner.kind {
        OwnerKind::Discount => "discount",
        OwnerKind::DiscountNode => "discountNode",
        OwnerKind::CartTransform => "cartTransform",
        OwnerKind::DeliveryCustomization => "deliveryCustomization",
        OwnerKind::PaymentCustomization => "paymentCustomization",
        OwnerKind::Validation => "validation",
    };
    let mut owner = owner;
    if matches!(
        ctx.owner.kind,
        OwnerKind::Discount | OwnerKind::DiscountNode
    ) {
        owner["discountClasses"] = json!(ctx.owner.discount_classes);
    }
    input[key] = owner;
    Ok(input)
}

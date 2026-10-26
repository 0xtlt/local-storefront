//! The input of a Shopify Function: its input query (GraphQL) is executed against the local
//! checkout, and the result is the JSON Shopify would send. See `crates/checkout/docs/design.md`.
//!
//! - [`exec`] is a generic GraphQL executor (aliases, fragments, variables, `__typename`).
//! - [`crate::schema`] checks the query against the schema of the Function's API version: a field
//!   that release does not have is an error, whatever the graph could answer.
//! - [`graph`] is the Function input graph: `cart`, `localization`, `shop`, the owners, ...,
//!   read from the store, the session and the state of the checkout.
//!
//! Money is a decimal string (`"19.99"`, as Shopify writes decimals: `"10.0"` for ten),
//! enums are their name (`"SHIPPING"`) and ids are GIDs.

mod exec;
mod graph;
#[cfg(test)]
mod tests;

use serde_json::Value as Json;

use crate::version::{ApiVersion, FunctionApi};
use crate::{CheckoutState, DeliveryGroup, DiscountClass, Metafield, PaymentMethod, ResultLine};

/// Everything a Function input can show about the checkout at one step of the pipeline.
pub struct InputContext<'a> {
    /// The API version of the Function: its schema says which fields the input query can ask.
    pub version: ApiVersion,
    /// The Function API of the target the query is for.
    pub api: FunctionApi,
    pub store: &'a lsf_core::Store,
    /// `customer_id`, `country`, cart note and attributes.
    pub session: &'a lsf_core::Session,
    /// Email, phone, address, codes, metafields.
    pub state: &'a CheckoutState,
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

/// The object that owns a Function (a discount, a cart transform, ...): it holds the
/// metafields the Function reads its configuration from.
pub struct Owner<'a> {
    /// Which root field of the input holds it.
    pub kind: OwnerKind,
    /// `gid://shopify/DiscountAutomaticNode/1`, ...
    pub id: String,
    pub metafields: &'a [Metafield],
    pub discount_classes: &'a [DiscountClass],
}

/// The root field of the input that holds the owner of the Function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerKind {
    /// `discount`: the discount Function API.
    Discount,
    /// `discountNode`: the legacy product, order and shipping discount APIs.
    DiscountNode,
    CartTransform,
    DeliveryCustomization,
    PaymentCustomization,
    Validation,
}

impl OwnerKind {
    /// The name of the root field.
    pub fn field(self) -> &'static str {
        match self {
            OwnerKind::Discount => "discount",
            OwnerKind::DiscountNode => "discountNode",
            OwnerKind::CartTransform => "cartTransform",
            OwnerKind::DeliveryCustomization => "deliveryCustomization",
            OwnerKind::PaymentCustomization => "paymentCustomization",
            OwnerKind::Validation => "validation",
        }
    }
}

/// The id a Function sees for a payment method, and gives back in its operations.
pub fn payment_method_id(handle: &str) -> String {
    format!("{PAYMENT_METHOD_PREFIX}{handle}")
}

/// The handle of the payment method an id of [`payment_method_id`] names.
pub fn payment_method_handle(id: &str) -> Option<&str> {
    id.strip_prefix(PAYMENT_METHOD_PREFIX)
}

const PAYMENT_METHOD_PREFIX: &str = "gid://shopify/PaymentCustomizationPaymentMethod/";

/// The id a Function sees for a delivery group, and gives back in its operations.
pub fn delivery_group_id(handle: &str) -> String {
    format!("{DELIVERY_GROUP_PREFIX}{handle}")
}

/// The handle of the delivery group an id of [`delivery_group_id`] names.
pub fn delivery_group_handle(id: &str) -> Option<&str> {
    id.strip_prefix(DELIVERY_GROUP_PREFIX)
}

const DELIVERY_GROUP_PREFIX: &str = "gid://shopify/CartDeliveryGroup/";

/// Runs `query` (the Function's input query) against the checkout, with `variables` from
/// `[extensions.input.variables]` (the JSON value of that owner metafield, or none): its
/// top-level keys are the values of the query's variables.
///
/// The error names the path of the field that does not exist in the API version, or that
/// cannot be answered.
pub fn build(query: &str, variables: Option<&Json>, ctx: &InputContext) -> Result<Json, String> {
    let document = graphql_parser::parse_query::<String>(query)
        .map_err(|error| format!("input query: invalid GraphQL: {error}"))?;
    ctx.version
        .schema(ctx.api)
        .validate_query(&document)
        .map_err(|message| {
            format!(
                "input query: not valid in API version {} ({} Function API): {message}",
                ctx.version, ctx.api
            )
        })?;
    execute(&document, variables, ctx)
}

/// [`build`] without the check against the schema: what the executor says about a query.
#[cfg(test)]
fn build_unchecked(
    query: &str,
    variables: Option<&Json>,
    ctx: &InputContext,
) -> Result<Json, String> {
    let document = graphql_parser::parse_query::<String>(query)
        .map_err(|error| format!("input query: invalid GraphQL: {error}"))?;
    execute(&document, variables, ctx)
}

fn execute<'q>(
    document: &'q graphql_parser::query::Document<'q, String>,
    variables: Option<&Json>,
    ctx: &InputContext,
) -> Result<Json, String> {
    let empty = serde_json::Map::new();
    let variables = match variables {
        None | Some(Json::Null) => &empty,
        Some(Json::Object(map)) => map,
        Some(other) => {
            return Err(format!(
                "input query: the variables must be a JSON object, got {other}"
            ));
        }
    };
    exec::execute(&graph::FunctionInput::new(ctx), document, variables)
        .map_err(|message| format!("input query: {message}"))
}

/// Cents as the decimal string Shopify sends: `1999` is `"19.99"`, `1000` is `"10.0"`.
pub(crate) fn decimal(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let (whole, fraction) = (cents.unsigned_abs() / 100, cents.unsigned_abs() % 100);
    match fraction {
        0 => format!("{sign}{whole}.0"),
        f if f % 10 == 0 => format!("{sign}{whole}.{}", f / 10),
        f => format!("{sign}{whole}.{f:02}"),
    }
}

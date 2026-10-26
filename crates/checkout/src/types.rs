//! The types every part of the checkout shares: what the buyer chose, how the checkout is
//! configured, and what it computes. Amounts are in minor units of the shop's currency.
//!
//! Result types serialize to the JSON the checkout page reads (camelCase, money as
//! `{"amount": 19.99, "currencyCode": "EUR"}`): see `.design/checkout.md`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// An amount in minor units of the shop's currency (cents).
pub type Cents = i64;

/// What the buyer entered and chose so far. Kept in the session, changed by the page.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutState {
    pub email: Option<String>,
    pub phone: Option<String>,
    pub shipping_address: Option<Address>,
    /// The option chosen in each delivery group: group handle → option handle.
    #[serde(default)]
    pub selected_delivery: IndexMap<String, String>,
    /// The handle of the chosen payment method.
    pub selected_payment: Option<String>,
    /// The codes the buyer entered, in the order they were entered.
    #[serde(default)]
    pub discount_codes: Vec<String>,
    /// Metafields set by checkout UI extensions (`applyMetafieldChange`).
    #[serde(default)]
    pub metafields: Vec<Metafield>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub company: Option<String>,
    pub address1: Option<String>,
    pub address2: Option<String>,
    pub city: Option<String>,
    /// ISO 3166-1 alpha-2.
    pub country_code: Option<String>,
    pub province_code: Option<String>,
    pub zip: Option<String>,
    pub phone: Option<String>,
}

/// A metafield as Shopify stores it: `value` is the string form (JSON for `json` and list types).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Metafield {
    pub namespace: String,
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
}

/// How the checkout is set up: what the store data says outside the app.
#[derive(Clone, Debug, Default)]
pub struct CheckoutConfig {
    /// The delivery options offered to every address. One group holds every line that needs
    /// shipping.
    pub shipping_rates: Vec<ShippingRate>,
    pub payment_methods: Vec<PaymentMethodConfig>,
    /// Discounts backed by a discount Function of the app.
    pub discounts: Vec<DiscountConfig>,
    /// The metafields of the object that owns each non-discount Function (cart transform,
    /// delivery/payment customization, validation): Function handle → metafields.
    pub function_metafields: IndexMap<String, Vec<Metafield>>,
    /// Tax rate in basis points (2000 = 20 %), added on top of the discounted subtotal. 0 by
    /// default.
    pub tax_rate_bps: i64,
}

#[derive(Clone, Debug)]
pub struct ShippingRate {
    pub handle: String,
    pub title: String,
    pub price: Cents,
    pub description: Option<String>,
    pub kind: DeliveryKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeliveryKind {
    #[default]
    Shipping,
    PickUp,
    Local,
}

#[derive(Clone, Debug)]
pub struct PaymentMethodConfig {
    pub handle: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct DiscountConfig {
    /// The handle of the discount Function in the app.
    pub function: String,
    pub title: String,
    /// `None` for an automatic discount; otherwise it applies once the buyer enters it.
    pub code: Option<String>,
    /// What the discount may reduce (`PRODUCT`, `ORDER`, `SHIPPING`). Empty means all three.
    pub classes: Vec<DiscountClass>,
    /// The metafields of the discount node, read by the Function's input query.
    pub metafields: Vec<Metafield>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiscountClass {
    Product,
    Order,
    Shipping,
}

/// What the checkout looks like once every Function ran.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutResult {
    pub lines: Vec<ResultLine>,
    pub discounts: Vec<AppliedDiscount>,
    pub discount_codes: Vec<DiscountCodeStatus>,
    pub delivery_groups: Vec<DeliveryGroup>,
    pub payment_methods: Vec<PaymentMethod>,
    /// Errors that block completing the checkout (validation Functions, Function failures).
    pub errors: Vec<CheckoutError>,
    pub cost: Cost,
    /// Every Function run, in order, for the debug panel and `lsf function run`.
    pub runs: Vec<FunctionRun>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultLine {
    /// `gid://shopify/CartLine/<n>`, n from 1 in cart order. Lines a cart transform creates
    /// get new ids after the last one.
    pub id: String,
    pub variant_id: u64,
    pub product_id: u64,
    pub title: String,
    pub variant_title: Option<String>,
    pub sku: Option<String>,
    pub image: Option<String>,
    pub quantity: u32,
    #[serde(serialize_with = "money")]
    pub unit_price: Cents,
    /// Before line discounts.
    #[serde(serialize_with = "money")]
    pub subtotal: Cents,
    /// After line discounts.
    #[serde(serialize_with = "money")]
    pub total: Cents,
    pub discount_allocations: Vec<DiscountAllocation>,
    pub attributes: Vec<KeyValue>,
    /// For a line a cart transform made from others (merge) or split (expand).
    pub components: Vec<ResultLine>,
    /// Which cart transform operation changed the line: `expand`, `merge`, `update`.
    pub transformed_by: Option<String>,
    pub requires_shipping: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscountAllocation {
    pub title: String,
    pub code: Option<String>,
    #[serde(serialize_with = "money")]
    pub amount: Cents,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedDiscount {
    pub title: String,
    pub code: Option<String>,
    pub class: DiscountClass,
    #[serde(serialize_with = "money")]
    pub amount: Cents,
    /// The Function that produced it.
    pub function: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscountCodeStatus {
    pub code: String,
    /// Whether a discount accepted it and applied something.
    pub applicable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryGroup {
    pub handle: String,
    /// The ids of the lines the group delivers.
    pub line_ids: Vec<String>,
    /// In the order the buyer sees them, after delivery customizations.
    pub options: Vec<DeliveryOption>,
    pub selected: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryOption {
    pub handle: String,
    pub title: String,
    pub description: Option<String>,
    pub kind: DeliveryKind,
    #[serde(serialize_with = "money")]
    pub original_cost: Cents,
    /// After shipping discounts.
    #[serde(serialize_with = "money")]
    pub cost: Cents,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentMethod {
    pub handle: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutError {
    pub message: String,
    /// The target the Function gave (`$.cart`, `$.cart.buyerIdentity.email`, ...), if any.
    pub target: Option<String>,
    /// The handle of the Function that reported it, if any.
    pub function: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(serialize_with = "money")]
    pub subtotal: Cents,
    /// Product and order discounts.
    #[serde(serialize_with = "money")]
    pub discounts: Cents,
    #[serde(serialize_with = "money")]
    pub shipping: Cents,
    #[serde(serialize_with = "money")]
    pub tax: Cents,
    #[serde(serialize_with = "money")]
    pub total: Cents,
    pub currency_code: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionRun {
    pub handle: String,
    pub target: String,
    /// The export of the Wasm module that ran (`run`, `cart_lines_discounts_generate_run`, ...).
    pub export: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    /// What the Function logged (stderr), as Shopify shows it.
    pub logs: String,
    /// Why the run failed: no module, a trap, an output that does not parse.
    pub error: Option<String>,
    /// Wasm instructions executed. Shopify stops a Function past 11 million.
    pub instructions: u64,
}

/// Money as checkout UI extensions see it: `{"amount": 19.99, "currencyCode": "EUR"}`. The
/// currency is filled in by the page from `cost.currencyCode`, so only the amount is written
/// here.
fn money<S: serde::Serializer>(cents: &Cents, serializer: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(1))?;
    // Two decimals for every currency the demo uses; zero-decimal currencies are rare in tests.
    map.serialize_entry("amount", &(*cents as f64 / 100.0))?;
    map.end()
}

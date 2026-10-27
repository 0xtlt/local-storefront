//! The types every part of the checkout shares: what the buyer chose, how the checkout is
//! configured, and what it computes. Amounts are in minor units of the shop's currency.
//!
//! Result types serialize to the JSON the checkout page reads (camelCase, money as
//! `{"amount": 19.99, "currencyCode": "EUR"}`): see `crates/checkout/docs/design.md`.

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
    /// The language the buyer reads the checkout in (`fr`, `pt-BR`), which the Functions see as
    /// `localization.language`; the primary language of the store when `None`. It comes from
    /// the request, so it is neither kept nor read from the session.
    #[serde(skip)]
    pub language: Option<String>,
    /// The step of the buyer journey the Functions run for (`buyerJourney.step`). The request
    /// decides, so it is neither kept nor read from the session.
    #[serde(skip)]
    pub step: BuyerJourneyStep,
}

/// Where the buyer is when the Functions run: `buyerJourney.step` of the validation input.
/// A validation Function branches on it, to check a rule only when the order is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BuyerJourneyStep {
    /// The buyer is looking at or changing the checkout: loading the page, entering an address,
    /// changing a line. The errors are shown, nothing is refused.
    Interaction,
    /// The buyer places the order: the errors of the Functions refuse it.
    #[default]
    Completion,
}

impl BuyerJourneyStep {
    /// The value of the `BuyerJourneyStep` enum of the Functions API.
    pub fn graphql(self) -> &'static str {
        match self {
            Self::Interaction => "CHECKOUT_INTERACTION",
            Self::Completion => "CHECKOUT_COMPLETION",
        }
    }
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
    /// Where the method is offered. Empty means `[PaymentMethod]`.
    pub placements: Vec<PaymentPlacement>,
}

/// `PaymentCustomizationPaymentMethodPlacement` of the Functions API: where a payment method
/// is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PaymentPlacement {
    /// The list of payment methods of the checkout.
    PaymentMethod,
    /// The express buttons (Shop Pay, Apple Pay, ...).
    AcceleratedCheckout,
}

impl PaymentPlacement {
    /// The value of the enum in the Functions API.
    pub fn graphql(self) -> &'static str {
        match self {
            Self::PaymentMethod => "PAYMENT_METHOD",
            Self::AcceleratedCheckout => "ACCELERATED_CHECKOUT",
        }
    }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiscountClass {
    #[default]
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
    /// The alt text of the image, when the store data has one.
    pub image_alt: Option<String>,
    /// The vendor and type of the product, and the option values of the variant, as the
    /// `merchandise` of a cart line of a checkout UI extension reads them.
    pub vendor: String,
    pub product_type: String,
    pub selected_options: Vec<SelectedOption>,
    /// The selling plan the line is sold with, when the store knows it.
    pub selling_plan: Option<LineSellingPlan>,
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
    /// The lines of the cart the line is made of, with the quantity of each it holds: one for a
    /// line of the cart, expanded or updated, several for a merge. Lets a change of the line be
    /// made on the cart.
    #[serde(skip)]
    pub sources: Vec<SourceLine>,
    pub requires_shipping: bool,
}

/// An option of a product and the value a variant has for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedOption {
    pub name: String,
    pub value: String,
}

/// The selling plan of a line, as `merchandise.sellingPlan` of the Cart Lines API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSellingPlan {
    /// `gid://shopify/SellingPlan/<n>`.
    pub id: String,
    pub recurring_deliveries: bool,
}

/// A part of a line of the cart: its index in the cart, and the quantity taken from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLine {
    pub index: usize,
    pub quantity: u32,
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
    /// The class of the discount that made it: the cart cost of a Function input leaves out
    /// the order discounts from its subtotal, and the cost of a line of a UI extension leaves
    /// them out of its total.
    pub class: DiscountClass,
    /// Which discount made it: its index in [`CheckoutResult::discounts`], where a Function
    /// input finds the application the allocation belongs to.
    #[serde(skip)]
    pub discount: usize,
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
    /// What a Function input shows of the discount: its `DiscountApplication`.
    #[serde(skip)]
    pub details: DiscountDetails,
}

/// `DiscountApplication` of the Functions API: how a discount is spread and what it targets.
#[derive(Clone, Debug, Default)]
pub struct DiscountDetails {
    pub allocation_method: AllocationMethod,
    pub target_selection: TargetSelection,
    pub target_type: DiscountTarget,
    pub value: DiscountValue,
    /// The metafields of the discount, which `DiscountApplication.metafield` reads.
    pub metafields: Vec<Metafield>,
}

/// `DiscountApplicationAllocationMethod`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AllocationMethod {
    /// The value is spread across the lines it targets.
    #[default]
    Across,
    /// The value applies to each line it targets.
    Each,
}

impl AllocationMethod {
    pub fn graphql(self) -> &'static str {
        match self {
            Self::Across => "ACROSS",
            Self::Each => "EACH",
        }
    }
}

/// `DiscountApplicationTargetSelection`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetSelection {
    /// Every line.
    #[default]
    All,
    /// The lines the discount is entitled to (an order discount that leaves some out).
    Entitled,
    /// The lines or options the Function named.
    Explicit,
}

impl TargetSelection {
    pub fn graphql(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Entitled => "ENTITLED",
            Self::Explicit => "EXPLICIT",
        }
    }
}

/// `DiscountApplicationTarget`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiscountTarget {
    #[default]
    LineItem,
    ShippingLine,
}

impl DiscountTarget {
    pub fn graphql(self) -> &'static str {
        match self {
            Self::LineItem => "LINE_ITEM",
            Self::ShippingLine => "SHIPPING_LINE",
        }
    }
}

/// `PricingValue`: what the discount takes off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DiscountValue {
    /// A percentage: `10.0` is 10 %.
    Percentage(f64),
    Fixed(Cents),
}

impl Default for DiscountValue {
    fn default() -> Self {
        Self::Fixed(0)
    }
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
    /// What the shipping discounts took off the selected option.
    #[serde(skip)]
    pub discount_allocations: Vec<DiscountAllocation>,
}

impl DeliveryGroup {
    /// What the selected option costs, after shipping discounts (0 when none is selected).
    pub fn selected_cost(&self) -> Cents {
        self.selected
            .as_ref()
            .and_then(|selected| {
                self.options
                    .iter()
                    .find(|option| &option.handle == selected)
            })
            .map_or(0, |option| option.cost)
    }
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
    /// Where the method is still offered. A method a Function hid from the list of the checkout
    /// only (`PAYMENT_METHOD`) is not in [`CheckoutResult::payment_methods`], and a Function
    /// that runs after sees it with the placements that are left.
    #[serde(skip)]
    pub placements: Vec<PaymentPlacement>,
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

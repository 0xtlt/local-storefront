//! The Subscriptions API: subscription contracts, their drafts and calculations, their billing
//! cycles and billing attempts, and the customer payment methods they charge.
//!
//! The family's records, in the shapes of their GraphQL types:
//!
//! - **`SubscriptionContract`** (`gid://shopify/SubscriptionContract/<n>`): the customer, the
//!   payment method, the currency, the billing and delivery policies, the delivery method and
//!   price, the lines (inline `SubscriptionLine`s, `gid://shopify/SubscriptionLine/<uuid>`), the
//!   discounts (inline `SubscriptionManualDiscount`s), the note and custom attributes, the
//!   status, `nextBillingDate` (the app's), `revisionId`, the origin order, the orders and the
//!   billing attempts. What the billing cycles need is kept under `lsf…` keys GraphQL never
//!   reads: the schedule (`lsfSchedule`, see [`cycles`]) and the edits of each cycle
//!   (`lsfCycleEdits`).
//! - **`SubscriptionDraft`**: a contract being made or changed by the draft mutations (deprecated
//!   in 2026-10, still served), with the contract it changes (`originalContract`), the billing
//!   cycle it edits, and what it started from (`lsfOriginal`, for `linesAdded` and the others).
//! - **`SubscriptionContractCalculationSuccess`** (`gid://shopify/SubscriptionContractCalculation
//!   /<n>`): what a calculate mutation computed, which `subscriptionContractCalculationCommit`
//!   applies.
//! - **`SubscriptionBillingAttempt`**: one charge of a cycle, its state, its order.
//! - **`CustomerPaymentMethod`** (`gid://shopify/CustomerPaymentMethod/<32 hex digits>`, the
//!   token of Shopify's ids): a card, a PayPal billing agreement, or a method imported from a
//!   gateway, with what lsf decides a charge does with it (`lsfOutcome`, see
//!   [`payment_methods`]).
//!
//! Billing cycles are not records: [`cycles`] computes them from the contract's schedule and the
//! shop's clock.
//!
//! None of these reaches the storefront or the checkout: the store data has no subscriptions
//! nor saved payment methods (see "Store sync" in `crates/app/docs/admin/design.md`). The orders the
//! billing attempts create are orders of the Admin API, as `orderCreate`'s.

pub mod billing;
pub mod calculation;
pub mod checkout;
pub mod contracts;
pub mod cycles;
pub mod drafts;
pub mod model;
pub mod payment_methods;

#[cfg(test)]
mod tests;

use lsf_core::store::model::CheckoutDeliveryKind;
use serde_json::{Value, json};

use crate::admin::resolve::Registry;
use crate::admin::state::seed::Seed;

/// The setting of the shop that holds the shipping rates of the store data's checkout: what a
/// draft's `deliveryOptions` and a calculation's `fetchAvailableDeliveryOptions` offer.
pub const DELIVERY_RATES_SETTING: &str = "subscriptionDeliveryRates";

/// Keeps what the family reads of the store data that no record holds: the checkout's shipping
/// rates.
pub fn seed(seed: &mut Seed) {
    let rates: Vec<Value> = seed
        .store
        .checkout
        .shipping_rates
        .iter()
        .map(|rate| {
            json!({
                "handle": rate.handle,
                "title": rate.title,
                "price": rate.price,
                "description": rate.description,
                "kind": match rate.kind {
                    CheckoutDeliveryKind::Shipping => "shipping",
                    CheckoutDeliveryKind::PickUp => "pickup",
                    CheckoutDeliveryKind::Local => "local",
                },
            })
        })
        .collect();
    seed.state
        .set_setting(DELIVERY_RATES_SETTING, Value::Array(rates));
}

pub fn register(registry: &mut Registry) {
    billing::register(registry);
    calculation::register(registry);
    contracts::register(registry);
    cycles::register(registry);
    drafts::register(registry);
    payment_methods::register(registry);
}

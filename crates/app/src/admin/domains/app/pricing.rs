//! Managed pricing, which Shopify now calls Shopify App Pricing: the app's plans, which Shopify
//! keeps in the Partner Dashboard and lsf takes from `--pricing-plans` or the control API
//! (`PUT /__lsf/admin/pricing-plans`), and the merchant's choice of one on the plan selection page
//! of the admin (`/admin/charges/<app handle>/pricing_plans`, served by `api::pricing`).
//!
//! As shopify.dev's "Shopify App Pricing" describes it (read on 2026-11-08):
//!
//! - The page is `https://admin.shopify.com/store/<store>/charges/<app handle>/pricing_plans`.
//!   Shopify creates the subscription when the merchant approves a plan, then redirects to the
//!   plan's welcome link: a path of the app in the admin gets `plan_handle` appended, a full URL
//!   `plan_handle` and the shop's domain (`shop`).
//! - "Once you opt in to Shopify App Pricing, you can't create new recurring application charges
//!   using the Billing API": `appSubscriptionCreate` and `appPurchaseOneTimeCreate` answer
//!   [`BILLING_API_REFUSED`], the user error developers report on community.shopify.dev (Shopify's
//!   staff there: "the restriction applies to all Billing API charge creation, including one-time
//!   charges").
//! - A merchant has one subscription at a time (staff: "A merchant can only have one active
//!   subscription at a time"): a new plan replaces the current one at once, the old one
//!   `CANCELLED`, the new one `ACTIVE`, as developers saw it on test stores. A downgrade to a free
//!   plan keeps the paid plan's `currentPeriodEnd` (staff: "a downgrade doesn't start a new
//!   billing cycle").
//! - The features Shopify keeps for the apps enrolled before April 2026 ([`Pricing::legacy`],
//!   lsf's default): `charge_id` appended to the welcome link, `app_subscriptions/update` (with
//!   `plan_handle`) on every change, and test subscriptions on development stores, which lsf's
//!   shops are. Without them (`legacy = false`: the apps enrolled since, and every app after
//!   April 28, 2026 as the page words it): `plan_handle` alone, no webhook, and no test flag. Those
//!   apps read their subscription from the Partner API's `activeSubscription`, which lsf does not
//!   simulate: lsf still records the subscription in the Admin API.
//!
//! The subscription is the Billing API's record (`AppSubscription`, one `AppRecurringPricing` line
//! with the plan's `planHandle`): `currentAppInstallation.activeSubscriptions`,
//! `appSubscriptionCancel` and uninstalling the app work on it as on the others (`billing`). Without
//! the legacy features they send no webhook either ([`Billing::subscription_topic`]): "Shopify App
//! Pricing doesn't use webhooks to notify your app of subscription changes", and for "status
//! changes that happen outside of a redirect (such as cancellations or freezes)" the page sends
//! the app to the Partner API. Uninstalling the app sends none in either mode: its webhook
//! subscriptions are deleted with it (`billing::cancel_on_uninstall`).

use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::CHANGED;
use super::billing;
use crate::admin::state::{ShopState, gid, parse_gid, value};

/// The user error of the Billing API's charge creation for an app on managed pricing.
pub const BILLING_API_REFUSED: &str =
    "Managed Pricing Apps cannot use the Billing API (to create charges).";

/// How the app bills, as the mutations of the Billing API see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Billing {
    /// No plans declared: the Billing API makes the charges, and sends its webhooks.
    #[default]
    BillingApi,
    /// Managed pricing with the features Shopify keeps for the apps enrolled before April 2026
    /// ([`Pricing::legacy`], lsf's default): `app_subscriptions/update` on every change.
    Legacy,
    /// Managed pricing as Shopify App Pricing is for the apps enrolled since April 2026, and for
    /// every app after April 28, 2026 (`legacy = false`): no subscription webhook at all.
    AppPricing,
}

impl Billing {
    /// How an app with `pricing` bills: by the Billing API without plans.
    pub fn of(pricing: Option<&Pricing>) -> Billing {
        match pricing {
            Some(pricing) if !pricing.plans.is_empty() && pricing.legacy => Billing::Legacy,
            Some(pricing) if !pricing.plans.is_empty() => Billing::AppPricing,
            _ => Billing::BillingApi,
        }
    }

    /// Whether the app is on managed pricing: the Billing API creates no charge.
    pub fn managed(self) -> bool {
        self != Billing::BillingApi
    }

    /// The topic of a change of a subscription: `app_subscriptions/update`, or [`CHANGED`] (no
    /// webhook) under Shopify App Pricing, which "no longer sends webhooks for subscription
    /// changes" after April 28, 2026, whether the app cancels it or the merchant changes plan.
    pub fn subscription_topic(self) -> &'static str {
        match self {
            Billing::AppPricing => CHANGED,
            Billing::BillingApi | Billing::Legacy => "app_subscriptions/update",
        }
    }
}

/// How often a plan is billed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interval {
    Monthly,
    Annual,
}

impl Interval {
    fn parse(text: &str) -> Option<Interval> {
        match text.trim().to_ascii_lowercase().as_str() {
            "monthly" | "every_30_days" => Some(Interval::Monthly),
            "annual" | "yearly" => Some(Interval::Annual),
            _ => None,
        }
    }

    /// As `AppPricingInterval` names it: `EVERY_30_DAYS`, `ANNUAL`.
    pub fn graphql(self) -> &'static str {
        match self {
            Interval::Monthly => "EVERY_30_DAYS",
            Interval::Annual => "ANNUAL",
        }
    }

    /// As a declaration writes it: `monthly`, `annual`.
    pub fn name(self) -> &'static str {
        match self {
            Interval::Monthly => "monthly",
            Interval::Annual => "annual",
        }
    }

    /// How long a period lasts, as the page says it.
    fn every(self) -> &'static str {
        match self {
            Interval::Monthly => "every 30 days",
            Interval::Annual => "every year",
        }
    }
}

/// A plan of the app.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// What the subscription's `planHandle`, the webhook's `plan_handle` and the welcome link's
    /// `plan_handle` say.
    pub handle: String,
    /// The plan's display name: the subscription's `name`.
    pub name: String,
    /// The price of a period, `0` for a free plan.
    pub price: f64,
    /// `USD` unless said otherwise (Shopify takes USD only).
    pub currency: String,
    pub interval: Interval,
    pub trial_days: i64,
    pub free: bool,
    /// The plan's top features, which the page lists.
    pub features: Vec<String>,
    /// Where the merchant goes once the plan is chosen: a path of the app (`/welcome`) or a URL.
    /// The app's home in the admin without one.
    pub welcome_link: Option<String>,
}

impl Plan {
    pub fn to_json(&self) -> Value {
        json!({
            "handle": self.handle,
            "name": self.name,
            "price": format!("{:.2}", self.price),
            "currency": self.currency,
            "interval": self.interval.name(),
            "trial_days": self.trial_days,
            "free": self.free,
            "features": self.features,
            "welcome_link": self.welcome_link,
        })
    }

    /// What the plan costs, as the page says it: `Free`, `9.99 USD every 30 days`.
    pub fn price_text(&self) -> String {
        if self.free {
            return "Free".to_string();
        }
        format!(
            "{:.2} {} {}",
            self.price,
            self.currency,
            self.interval.every()
        )
    }
}

/// The plans of an app on managed pricing.
#[derive(Clone, Debug, PartialEq)]
pub struct Pricing {
    pub plans: Vec<Plan>,
    /// The features Shopify keeps for the apps enrolled before April 2026: `charge_id`,
    /// `app_subscriptions/update`, test subscriptions. `true` unless the declaration says
    /// `legacy = false`.
    pub legacy: bool,
}

impl Default for Pricing {
    fn default() -> Pricing {
        Pricing {
            plans: Vec::new(),
            legacy: true,
        }
    }
}

/// A declaration as written, before it is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declared {
    #[serde(default)]
    plans: Vec<DeclaredPlan>,
    legacy: Option<bool>,
    /// The JSON Schema an editor checks a JSON declaration with.
    #[serde(rename = "$schema")]
    _schema: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredPlan {
    handle: Option<String>,
    name: Option<String>,
    /// A number or a string (`"9.99"`).
    price: Option<Value>,
    currency: Option<String>,
    interval: Option<String>,
    #[serde(alias = "trialDays")]
    trial_days: Option<i64>,
    free: Option<bool>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(alias = "welcomeLink")]
    welcome_link: Option<String>,
}

impl Pricing {
    /// The plans of a JSON declaration: `{"plans": [...], "legacy"?: bool}`.
    pub fn from_json(value: &Value) -> Result<Pricing, String> {
        let declared: Declared =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        Pricing::checked(declared)
    }

    /// The plans of a TOML declaration: `legacy = …` and `[[plans]]` tables.
    pub fn from_toml(text: &str) -> Result<Pricing, String> {
        let declared: Declared = toml::from_str(text).map_err(|error| error.to_string())?;
        Pricing::checked(declared)
    }

    /// The plans of a file: JSON for a `.json` file, TOML otherwise.
    pub fn read(path: &Path) -> Result<Pricing, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let json = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        let parsed = if json {
            serde_json::from_str::<Value>(&text)
                .map_err(|error| error.to_string())
                .and_then(|value| Pricing::from_json(&value))
        } else {
            Pricing::from_toml(&text)
        };
        parsed.map_err(|error| format!("{}: {error}", path.display()))
    }

    fn checked(declared: Declared) -> Result<Pricing, String> {
        let mut errors = Vec::new();
        let mut plans: Vec<Plan> = Vec::new();
        for (index, plan) in declared.plans.into_iter().enumerate() {
            if let Some(plan) = checked_plan(index, plan, &mut errors) {
                if plans.iter().any(|other| other.handle == plan.handle) {
                    errors.push(format!(
                        "plans[{index}]: two plans have the handle {}",
                        plan.handle
                    ));
                }
                plans.push(plan);
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(Pricing {
            plans,
            legacy: declared.legacy.unwrap_or(true),
        })
    }

    pub fn plan(&self, handle: &str) -> Option<&Plan> {
        self.plans.iter().find(|plan| plan.handle == handle)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "legacy": self.legacy,
            "plans": self.plans.iter().map(Plan::to_json).collect::<Vec<_>>(),
        })
    }
}

/// A plan as declared, checked: `None`, and why in `errors`, when it is wrong.
fn checked_plan(index: usize, declared: DeclaredPlan, errors: &mut Vec<String>) -> Option<Plan> {
    let before = errors.len();
    let handle = declared
        .handle
        .map(|handle| handle.trim().to_string())
        .unwrap_or_default();
    let at = if handle.is_empty() {
        format!("plans[{index}]")
    } else {
        format!("plans[{index}] ({handle})")
    };
    if handle.is_empty() {
        errors.push(format!("{at}: handle is required"));
    } else if !handle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        errors.push(format!(
            "{at}: a handle is letters, digits, - and _, as it goes in URLs"
        ));
    }
    let name = declared
        .name
        .map(|name| name.trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        errors.push(format!("{at}: name is required"));
    }
    let price = match &declared.price {
        None => None,
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        Some(_) => None,
    };
    if declared.price.is_some() && price.is_none_or(|price| !price.is_finite()) {
        errors.push(format!("{at}: price is a number (9.99, or \"9.99\")"));
    }
    let price = price.filter(|price| price.is_finite());
    if price.is_some_and(|price| price < 0.0) {
        errors.push(format!("{at}: price is 0 or more"));
    }
    let price = price.map(|price| (price * 100.0).round() / 100.0);
    let free = match (declared.free, price) {
        (Some(true), Some(price)) if price > 0.0 => {
            errors.push(format!(
                "{at}: a free plan costs nothing: leave price out, or make it 0"
            ));
            true
        }
        (Some(free), _) => free,
        (None, Some(price)) => price == 0.0,
        (None, None) => {
            if declared.price.is_none() {
                errors.push(format!(
                    "{at}: price is required (0, or free = true, for a free plan)"
                ));
            }
            false
        }
    };
    if !free && price == Some(0.0) && declared.free == Some(false) {
        errors.push(format!("{at}: a plan that is not free has a price above 0"));
    }
    let currency = declared
        .currency
        .map(|currency| currency.trim().to_ascii_uppercase())
        .unwrap_or_else(|| "USD".to_string());
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_alphabetic()) {
        errors.push(format!(
            "{at}: currency is a code of three letters (USD), not {currency:?}"
        ));
    }
    let interval = match declared.interval.as_deref() {
        None => Some(Interval::Monthly),
        Some(text) => {
            let interval = Interval::parse(text);
            if interval.is_none() {
                errors.push(format!("{at}: interval is monthly or annual, not {text:?}"));
            }
            interval
        }
    };
    let trial_days = declared.trial_days.unwrap_or(0);
    if trial_days < 0 {
        errors.push(format!("{at}: trial_days is 0 or more"));
    }
    if free && trial_days > 0 {
        errors.push(format!("{at}: a free plan has no trial"));
    }
    let welcome_link = declared
        .welcome_link
        .map(|link| link.trim().to_string())
        .filter(|link| !link.is_empty());
    if let Some(link) = &welcome_link
        && !(link.starts_with('/') || link.starts_with("http://") || link.starts_with("https://"))
    {
        errors.push(format!(
            "{at}: welcome_link is a path of the app (/welcome) or a URL (https://…), not {link:?}"
        ));
    }
    let features: Vec<String> = declared
        .features
        .into_iter()
        .map(|feature| feature.trim().to_string())
        .filter(|feature| !feature.is_empty())
        .collect();
    if errors.len() > before {
        return None;
    }
    Some(Plan {
        handle,
        name,
        price: price.unwrap_or(0.0),
        currency,
        interval: interval.unwrap_or(Interval::Monthly),
        trial_days,
        free,
        features,
        welcome_link,
    })
}

// ---- the shop's subscription ---------------------------------------------------------------

/// The shop's active subscription.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Current {
    pub id: String,
    pub name: String,
    /// The handle of its plan, when it is one of managed pricing.
    pub plan: Option<String>,
}

/// The `planHandle` of a subscription's recurring line, when it has one.
fn plan_handle(state: &ShopState, subscription: &str) -> Option<String> {
    state
        .get(subscription)?
        .get("lineItems")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find_map(|line| {
            state
                .get(line)?
                .get("plan")?
                .get("pricingDetails")?
                .get("planHandle")?
                .as_str()
                .map(str::to_string)
        })
}

/// The shop's active subscription (the latest, should the Billing API have left two).
pub fn current(state: &ShopState) -> Option<Current> {
    let id = state
        .keys("AppSubscription")
        .filter(|key| billing::status(state, key) == Some("ACTIVE"))
        .max_by_key(|key| parse_gid(key).map_or(0, |(_, number)| number))?
        .to_string();
    let name = state
        .get(&id)
        .and_then(|record| record.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let plan = plan_handle(state, &id);
    Some(Current { id, name, plan })
}

/// Whether the shop is a development store (`plan.partnerDevelopment`), as lsf's shops are.
fn development_store(state: &ShopState) -> bool {
    state
        .shop()
        .and_then(|shop| shop.get("plan"))
        .and_then(|plan| plan.get("partnerDevelopment"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// What choosing a plan did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    /// The new subscription.
    pub subscription: String,
    /// Its number: the `charge_id` of the welcome link.
    pub number: u64,
    /// The subscriptions it replaced, now `CANCELLED`.
    pub replaced: Vec<String>,
    /// Where the merchant goes: the welcome link, with its parameters.
    pub redirect: String,
}

/// Why a plan cannot be chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectError {
    /// No plan has this handle.
    UnknownPlan,
    /// The shop is on this plan already.
    Current,
}

/// The merchant chooses the plan `handle`: a new `ACTIVE` subscription replaces the current one
/// (`CANCELLED`), with `app_subscriptions/update` for each when [`Pricing::legacy`]. `redirect`
/// makes the welcome link of the plan and the subscription's number ([`welcome_url`]), which is
/// also the subscription's `returnUrl`.
pub fn select(
    state: &mut ShopState,
    pricing: &Pricing,
    handle: &str,
    redirect: impl FnOnce(&Plan, u64) -> String,
) -> Result<Selected, SelectError> {
    let plan = pricing.plan(handle).ok_or(SelectError::UnknownPlan)?;
    billing::expire_stale(state);
    if current(state).and_then(|current| current.plan).as_deref() == Some(handle) {
        return Err(SelectError::Current);
    }
    let test = pricing.legacy && development_store(state);
    let now = state.timestamp();
    let number = billing::next_charge(state);
    let id = gid("AppSubscription", number);
    let return_url = redirect(plan, number);
    let line = format!("gid://shopify/AppSubscriptionLineItem/{number}?v=1&index=0");
    state.insert(value::object(json!({
        "__typename": "AppSubscriptionLineItem",
        "id": line,
        "plan": {"pricingDetails": {
            "__typename": "AppRecurringPricing",
            "price": billing::money_v2(plan.price, &plan.currency),
            "interval": plan.interval.graphql(),
            "planHandle": plan.handle,
            "discount": null,
        }},
        "usageRecords": [],
    })));
    state.insert(value::object(json!({
        "__typename": "AppSubscription",
        "id": id,
        "name": plan.name,
        "status": "PENDING",
        "test": test,
        "trialDays": plan.trial_days,
        "returnUrl": return_url,
        "currentPeriodEnd": null,
        "lineItems": [line],
        "replacementBehavior": "STANDARD",
        "createdAt": now,
        "updatedAt": now,
    })));
    let replaced: Vec<String> = state
        .keys("AppSubscription")
        .filter(|key| *key != id && billing::status(state, key) == Some("ACTIVE"))
        .map(str::to_string)
        .collect();
    // A downgrade to a free plan does not start a new billing cycle: the paid plan's period.
    let kept_period = if plan.free {
        replaced
            .iter()
            .filter_map(|old| state.get(old)?.get("currentPeriodEnd").cloned())
            .filter(|end| value::date_time_of(Some(end)).is_some_and(|end| end > state.now()))
            .max_by_key(|end| value::date_time_of(Some(end)))
    } else {
        None
    };
    let topic = Billing::of(Some(pricing)).subscription_topic();
    for old in &replaced {
        billing::set_status(state, old, "CANCELLED");
        state.emit(topic, old);
    }
    billing::activate(state, &id);
    if let Some(end) = kept_period
        && let Some(record) = state.get_mut(&id)
    {
        record.insert("currentPeriodEnd".into(), end);
    }
    state.emit(topic, &id);
    Ok(Selected {
        subscription: id,
        number,
        replaced,
        redirect: return_url,
    })
}

/// Where the merchant goes once the plan is chosen: the plan's welcome link, with
/// `charge_id` (legacy) and `plan_handle`. A path (and no link: the app's home) is the app's page
/// in lsf's admin, `admin_app` (`http://127.0.0.1:9292/admin/apps/<handle>`), with the admin's
/// `shop` first; a URL gets the shop's domain too, as `shop`.
pub fn welcome_url(plan: &Plan, admin_app: &str, shop: &str, legacy: bool, number: u64) -> String {
    let mut params: Vec<(&str, String)> = Vec::new();
    let link = plan.welcome_link.as_deref().unwrap_or("/");
    let absolute = link.starts_with("http://") || link.starts_with("https://");
    let base = if absolute {
        link.to_string()
    } else {
        params.push(("shop", shop.to_string()));
        let path = link.trim_start_matches('/');
        if path.is_empty() || path.starts_with('?') || path.starts_with('#') {
            format!("{admin_app}{path}")
        } else {
            format!("{admin_app}/{path}")
        }
    };
    if legacy {
        params.push(("charge_id", number.to_string()));
    }
    params.push(("plan_handle", plan.handle.clone()));
    if absolute {
        params.push(("shop", shop.to_string()));
    }
    with_params(&base, &params)
}

/// `url` with `params` added to its query, before its fragment.
fn with_params(url: &str, params: &[(&str, String)]) -> String {
    let (base, fragment) = url
        .split_once('#')
        .map_or((url, None), |(base, fragment)| (base, Some(fragment)));
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in params {
        serializer.append_pair(name, value);
    }
    let query = serializer.finish();
    let separator = match base.contains('?') {
        true if base.ends_with('?') || base.ends_with('&') => "",
        true => "&",
        false => "?",
    };
    let mut target = format!("{base}{separator}{query}");
    if let Some(fragment) = fragment {
        target.push('#');
        target.push_str(fragment);
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = r#"
# The plans of the app, as the Partner Dashboard has them.
[[plans]]
handle = "free"
name = "Free"
price = 0
features = ["100 orders a month"]

[[plans]]
handle = "pro"
name = "Pro"
price = 9.99
trial_days = 7
welcome_link = "/welcome"

[[plans]]
handle = "pro-annual"
name = "Pro (annual)"
price = "99"
currency = "usd"
interval = "annual"
welcome_link = "https://app.example.com/thanks?from=plans#top"
"#;

    #[test]
    fn a_toml_declaration_gives_its_plans_with_their_defaults() {
        let pricing = Pricing::from_toml(TOML).unwrap();
        assert!(pricing.legacy);
        let [free, pro, annual] = pricing.plans.as_slice() else {
            panic!("{pricing:?}");
        };
        assert!(free.free && free.price == 0.0 && free.trial_days == 0);
        assert_eq!(free.features, ["100 orders a month"]);
        assert_eq!(
            (
                pro.price,
                pro.currency.as_str(),
                pro.interval,
                pro.trial_days,
                pro.free
            ),
            (9.99, "USD", Interval::Monthly, 7, false)
        );
        assert_eq!((annual.price, annual.currency.as_str()), (99.0, "USD"));
        assert_eq!(annual.interval, Interval::Annual);
        assert_eq!(pro.price_text(), "9.99 USD every 30 days");
        assert_eq!(annual.price_text(), "99.00 USD every year");
        assert_eq!(free.price_text(), "Free");
    }

    #[test]
    fn a_json_declaration_takes_the_same_fields_and_camel_case_aliases() {
        let pricing = Pricing::from_json(&json!({
            "legacy": false,
            "plans": [{"handle": "pro", "name": "Pro", "price": 5, "trialDays": 3, "welcomeLink": "/hi"}]
        }))
        .unwrap();
        assert!(!pricing.legacy);
        assert_eq!(pricing.plans[0].trial_days, 3);
        assert_eq!(pricing.plans[0].welcome_link.as_deref(), Some("/hi"));
        assert_eq!(
            pricing.to_json()["plans"][0]["price"],
            json!("5.00"),
            "{}",
            pricing.to_json()
        );
    }

    #[test]
    fn a_wrong_declaration_says_every_fault_of_every_plan() {
        let error = Pricing::from_json(&json!({"plans": [
            {"name": "No handle", "price": 1},
            {"handle": "pro", "name": "Pro"},
            {"handle": "gift", "name": "Gift", "free": true, "price": 3, "trial_days": 2},
            {"handle": "x y", "name": "X", "price": -1, "interval": "weekly", "currency": "dollars"},
            {"handle": "pro", "name": "Pro again", "price": 2, "welcome_link": "welcome"},
            {"handle": "dup", "name": "D", "price": 1},
            {"handle": "dup", "name": "D2", "price": 1}
        ]}))
        .unwrap_err();
        for part in [
            "plans[0]: handle is required",
            "plans[1] (pro): price is required (0, or free = true, for a free plan)",
            "plans[2] (gift): a free plan costs nothing",
            "plans[2] (gift): a free plan has no trial",
            "plans[3] (x y): a handle is letters, digits, - and _",
            "plans[3] (x y): price is 0 or more",
            "plans[3] (x y): interval is monthly or annual, not \"weekly\"",
            "plans[3] (x y): currency is a code of three letters (USD), not \"DOLLARS\"",
            "plans[4] (pro): welcome_link is a path of the app (/welcome) or a URL",
            "plans[6]: two plans have the handle dup",
        ] {
            assert!(error.contains(part), "no {part:?} in {error}");
        }
        let unknown =
            Pricing::from_toml("[[plans]]\nhandle = \"a\"\nname = \"A\"\nprice = 1\ntrial = 3\n")
                .unwrap_err();
        assert!(unknown.contains("unknown field `trial`"), "{unknown}");
    }

    #[test]
    fn the_welcome_link_gets_the_charge_the_plan_and_the_shop() {
        let pricing = Pricing::from_toml(TOML).unwrap();
        let app = "http://127.0.0.1:9292/admin/apps/my-app";
        let shop = "a.myshopify.com";
        assert_eq!(
            welcome_url(&pricing.plans[0], app, shop, true, 3),
            "http://127.0.0.1:9292/admin/apps/my-app?shop=a.myshopify.com&charge_id=3&plan_handle=free"
        );
        assert_eq!(
            welcome_url(&pricing.plans[1], app, shop, true, 4),
            "http://127.0.0.1:9292/admin/apps/my-app/welcome?shop=a.myshopify.com&charge_id=4&plan_handle=pro"
        );
        assert_eq!(
            welcome_url(&pricing.plans[2], app, shop, true, 5),
            "https://app.example.com/thanks?from=plans&charge_id=5&plan_handle=pro-annual&shop=a.myshopify.com#top"
        );
        assert_eq!(
            welcome_url(&pricing.plans[1], app, shop, false, 4),
            "http://127.0.0.1:9292/admin/apps/my-app/welcome?shop=a.myshopify.com&plan_handle=pro"
        );
    }
}

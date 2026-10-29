//! The pages of the checkout that are not a checkout: the home page, which lists what is wrong
//! with the app and its data, the checkout scenarios, the customers and the extensions of the
//! app, and the notices that say why a scenario did not open. They are HTML in the look of the
//! checkout (`checkout.css`) that works offline and without a script: `home.js` only adds the
//! filter of the scenarios and asks for the check of the Functions (`home_check`).
//!
//! The home page is built as JSON first (`GET …?format=json` answers it), and the HTML is drawn
//! from that JSON, so that the two cannot say different things.

use std::collections::HashMap;

use lsf_checkout::app::{App, FunctionExtension, Selection, Severity, TargetKind, UiExtension};
use lsf_checkout::placement::placement;
use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::model::ScenarioLook;
use lsf_core::store::{CheckoutScenario, CheckoutSettings};
use lsf_core::{Session, Store};
use lsf_liquid::filters::escape_html;
use serde_json::{Value as Json, json};

use super::home_check::Report;
use super::reply::{Reply, cache};
use super::{Incoming, ServerState};
use crate::checkout::{CheckoutOptions, display_money, encoded_handle};
use crate::watch::Failure;

/// Where the files of the checkout page are served from.
pub const ASSETS: &str = "/__lsf/checkout/assets";

/// The Function targets of Shopify that the local checkout knows of and does not run (delivery
/// generators, fulfillment constraints, order routing). Any other target it does not run is one
/// it does not know: a typo, or a target of no Function API.
const LISTED_TARGETS: &[&str] = &[
    "cart.fulfillment-constraints.generate.",
    "cart.fulfillment-groups.location-rankings.generate.",
    "cart.local-pickup-delivery-option-generator.",
    "cart.pickup-point-delivery-option-generator.",
    "purchase.fulfillment-constraint-rule.",
    "purchase.order-routing-location-rule.",
    "purchase.local-pickup-delivery-option-generator.",
    "purchase.pickup-point-delivery-option-generator.",
];

fn is_listed_target(target: &str) -> bool {
    LISTED_TARGETS
        .iter()
        .any(|prefix| target.starts_with(prefix))
}

/// How many problems a group shows before the others are folded.
const SHOWN_PROBLEMS: usize = 8;

fn esc(text: &str) -> String {
    escape_html(text)
}

/// A page of the checkout's look around `body`.
pub fn document(title: &str, body: &str) -> String {
    document_with(title, body, "")
}

fn document_with(title: &str, body: &str, head: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light\"><title>{}</title>\
         <link rel=\"stylesheet\" href=\"{ASSETS}/checkout.css\">{head}</head>\
         <body><div class=\"lsf-app lsf-home\">{body}</div></body></html>",
        esc(title)
    )
}

/// A page that says why a request for a checkout cannot be answered: a scenario that does not
/// exist or does not validate. `home` is where the list of the scenarios is.
pub fn notice(
    status: u16,
    title: &str,
    paragraphs: &[String],
    details: &[String],
    home: &str,
) -> Reply {
    let mut body = format!(
        "<header class=\"lsf-header\"><a class=\"lsf-brand\" href=\"{}\">Checkout</a></header>\
         <main class=\"lsf-home-main\"><section class=\"lsf-section\"><h2>{}</h2>",
        esc(home),
        esc(title)
    );
    for paragraph in paragraphs {
        body.push_str(&format!("<p>{}</p>", esc(paragraph)));
    }
    for detail in details {
        body.push_str(&format!(
            "<pre class=\"lsf-home-problem\">{}</pre>",
            esc(detail)
        ));
    }
    body.push_str(&format!(
        "<p><a class=\"lsf-link\" href=\"{}\">All checkouts and extensions</a></p></section></main>",
        esc(home)
    ));
    Reply::html(status, document(title, &body)).cached(cache::PRIVATE)
}

// --- What the page says ------------------------------------------------------------------

/// How bad a problem is. Errors come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Error,
    Warning,
}

impl Level {
    fn of(severity: Severity) -> Level {
        match severity {
            Severity::Error => Level::Error,
            Severity::Warning => Level::Warning,
        }
    }

    fn of_json(problem: &Json) -> Level {
        if problem["severity"] == "error" {
            Level::Error
        } else {
            Level::Warning
        }
    }

    fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

/// The status of something the page lists, from its problems.
fn status_of(levels: impl IntoIterator<Item = Level>) -> &'static str {
    match levels.into_iter().min() {
        Some(Level::Error) => "error",
        Some(Level::Warning) => "warning",
        None => "ok",
    }
}

fn problem(level: Level, message: impl Into<String>) -> (Level, String) {
    (level, message.into())
}

/// The id of the row of a scenario or an extension on the page, for the links to it. Two names
/// never share one ([`encoded_handle`]).
fn anchor(prefix: &str, name: &str) -> String {
    format!("{prefix}-{}", encoded_handle(name))
}

/// What the loader says of an extension.
fn diagnostics_of<'a>(
    app: &'a App,
    handle: &'a str,
) -> impl Iterator<Item = &'a lsf_checkout::app::Diagnostic> {
    app.diagnostics
        .iter()
        .filter(move |diagnostic| diagnostic.extension.as_deref() == Some(handle))
}

/// The problems of a Function: what the loader says, where what keeps it from running is an
/// error (no Wasm module, a target the local checkout does not know).
fn function_problems(app: &App, function: &FunctionExtension) -> Vec<(Level, String)> {
    let unknown: Vec<&str> = function
        .targets
        .iter()
        .filter(|target| {
            target.kind == TargetKind::Unsupported && !is_listed_target(&target.target)
        })
        .map(|target| target.target.as_str())
        .collect();
    diagnostics_of(app, &function.handle)
        .map(|diagnostic| {
            if !function.built() && diagnostic.message.starts_with("no Wasm module") {
                return problem(Level::Error, &diagnostic.message);
            }
            match unknown.iter().find(|target| {
                diagnostic.message == format!("target `{target}` is not run by the local checkout")
            }) {
                Some(target) => problem(
                    Level::Error,
                    format!("unknown target `{target}`: no Function API has it, it never runs"),
                ),
                None => problem(Level::of(diagnostic.severity), &diagnostic.message),
            }
        })
        .collect()
}

/// The problems of a checkout UI extension: what the loader says (among them an `owner_type`
/// the page refuses, the targets the page has no place for); one that is not built is an error.
fn ui_problems(app: &App, extension: &UiExtension) -> Vec<(Level, String)> {
    diagnostics_of(app, &extension.handle)
        .map(|diagnostic| {
            if !extension.built() && diagnostic.message.starts_with("not built") {
                problem(Level::Error, &diagnostic.message)
            } else {
                problem(Level::of(diagnostic.severity), &diagnostic.message)
            }
        })
        .collect()
}

fn problems_json(problems: &[(Level, String)]) -> Vec<Json> {
    problems
        .iter()
        .map(|(level, message)| json!({"severity": level.name(), "message": message}))
        .collect()
}

/// The runs of a Function that fail, as the check found them.
fn failures_json(report: Option<&Report>, handle: &str) -> Vec<Json> {
    report
        .into_iter()
        .flat_map(|report| &report.failures)
        .filter(|failure| failure.handle == handle)
        .map(|failure| {
            json!({
                "target": failure.target,
                "error": failure.error,
                "scenarios": failure.scenarios,
            })
        })
        .collect()
}

/// The status of an extension: its problems, and the runs that fail. One that `--extension`
/// leaves out and that has no problem is `off`.
fn extension_status(problems: &[(Level, String)], failures: &[Json], selected: bool) -> String {
    let status = status_of(
        problems
            .iter()
            .map(|(level, _)| *level)
            .chain((!failures.is_empty()).then_some(Level::Error)),
    );
    if status == "ok" && !selected {
        "off".to_string()
    } else {
        status.to_string()
    }
}

fn function_json(
    app: &App,
    selection: &Selection,
    function: &FunctionExtension,
    report: Option<&Report>,
    build: Option<&Failure>,
    used_by: usize,
    discounted: &std::collections::HashSet<&str>,
) -> Json {
    let targets: Vec<Json> = function
        .targets
        .iter()
        .map(|target| {
            let spec = target.spec();
            json!({
                "target": target.target,
                "api": spec.map(|spec| spec.api.to_string()),
                "runs": target.kind != TargetKind::Unsupported,
                // Whether a checkout has a run of it: a target it runs, on an API version it
                // runs (a Function that is not built has a run, that fails).
                "hasRun": target.kind != TargetKind::Unsupported && function.version.is_some(),
                "fetch": matches!(
                    target.kind,
                    TargetKind::LinesDiscountsFetch
                        | TargetKind::DeliveryDiscountsFetch
                        | TargetKind::ValidationFetch
                ),
                "legacy": spec.is_some_and(|spec| spec.legacy),
                "unknown": target.kind == TargetKind::Unsupported && !is_listed_target(&target.target),
            })
        })
        .collect();
    let mut apis: Vec<String> = Vec::new();
    for target in &function.targets {
        if let Some(spec) = target.spec() {
            let api = spec.api.to_string();
            if !apis.contains(&api) {
                apis.push(api);
            }
        }
    }
    let mut problems = function_problems(app, function);
    // A discount Function runs for a discount of the store: one that none names never runs.
    let discount = function.targets.iter().any(|target| {
        matches!(
            target.kind,
            TargetKind::LinesDiscounts
                | TargetKind::ProductDiscount
                | TargetKind::OrderDiscount
                | TargetKind::DeliveryDiscounts
                | TargetKind::ShippingDiscount
        )
    });
    let store_note = discount && !discounted.contains(function.handle.as_str());
    if store_note {
        problems.push(problem(
            Level::Warning,
            format!(
                "no discount of the store data runs it, so it never runs: a discount Function runs for a discount, \
                 which `checkout.discounts` of `store.json` (or of a scenario) declares, e.g. \
                 `{}`, with `\"code\"` for a discount code",
                crate::checkout::discount_example(function)
            ),
        ));
    }
    let failures = failures_json(report, &function.handle);
    let selected = selection.contains(&function.handle);
    json!({
        "handle": function.handle,
        "anchor": anchor("function", &function.handle),
        "name": function.name,
        "apiVersion": function.api_version,
        "supported": function.version.is_some(),
        "apis": apis,
        "targets": targets,
        // Whether the Function can make an HTTP request (it has a `.fetch` target).
        "network": targets.iter().any(|target| target["fetch"] == true),
        "built": function.built(),
        "buildCommand": function.build_hint(),
        "selected": selected,
        // A build of `--watch` that failed is an error, even if the last module runs.
        "status": if build.is_some() {
            "error".to_string()
        } else {
            extension_status(&problems, &failures, selected)
        },
        // The note that no discount names it is the store's, not of the scenarios that run it.
        "problems": problems_json(&problems)
            .into_iter()
            .enumerate()
            .map(|(index, mut problem)| {
                if store_note && index == problems.len() - 1 {
                    problem["store"] = json!(true);
                }
                problem
            })
            .collect::<Vec<_>>(),
        // Its runs that fail when the scenarios run, once the check ran.
        "failures": failures,
        // Its last build of `--watch`, when it failed.
        "build": build.map(|failure| json!({"reason": failure.reason, "output": failure.output})),
        // How many scenarios that open run it.
        "scenarios": used_by,
    })
}

fn ui_extension_json(
    app: &App,
    selection: &Selection,
    extension: &UiExtension,
    used_by: usize,
) -> Json {
    let targets: Vec<Json> = extension
        .targets
        .iter()
        .map(|target| json!({"target": target.target, "place": placement(&target.target)}))
        .collect();
    // Where it renders: `checkout` (the checkout and the thank-you page), `customer-account`.
    let mut surfaces: Vec<&str> = Vec::new();
    for target in &extension.targets {
        if let Some(surface) = lsf_checkout::placement::surface(&target.target)
            && !surfaces.contains(&surface)
        {
            surfaces.push(surface);
        }
    }
    let capabilities: Vec<&str> = extension
        .capabilities
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| value.as_bool() == Some(true))
        .map(|(name, _)| name.as_str())
        .collect();
    let problems = ui_problems(app, extension);
    let selected = selection.contains(&extension.handle);
    // Whether the checkout loads it: every error keeps it out (not built, an API version it
    // does not run, an `owner_type` that is wrong or missing); a warning does not.
    let loads = !problems.iter().any(|(level, _)| *level == Level::Error);
    // The bundle the checkout loads, for the page to load it on its own and see what it
    // registers (`home.js`): only for an extension the checkout loads.
    let script = (selected && extension.built() && extension.version.is_some()).then(|| {
        let file = extension
            .bundle
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("/__lsf/extensions/{}/{file}", extension.handle)
    });
    json!({
        "handle": extension.handle,
        "anchor": anchor("ui", &extension.handle),
        "name": extension.name,
        "apiVersion": extension.api_version,
        "supported": extension.version.is_some(),
        "targets": targets,
        "surfaces": surfaces,
        "capabilities": capabilities,
        "built": extension.built(),
        "selected": selected,
        "status": extension_status(&problems, &[], selected),
        "problems": problems_json(&problems),
        "scenarios": used_by,
        "loads": loads,
        "script": script,
    })
}

fn names(settings: &CheckoutSettings, store: &Store) -> (String, String) {
    let currency = store.shop.currency.as_str();
    let rates = settings
        .shipping_rates
        .iter()
        .map(|rate| format!("{} {}", rate.title, display_money(rate.price, currency)))
        .collect::<Vec<_>>()
        .join(", ");
    let methods = settings
        .payment_methods
        .iter()
        .map(|method| method.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    (rates, methods)
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What a scenario is: a few `(label, text)` lines (the cart, the buyer, the country and
/// language, what was entered, the discounts, the settings it overrides, the look), and the
/// same in a few words for its row (`3 lines · guest · FR · fr · 1 code`).
fn describe(
    store: &Store,
    scenario: &CheckoutScenario,
) -> (Vec<(&'static str, String)>, Vec<String>) {
    let mut session = Session::initial(store);
    crate::server::apply_session_input(
        store,
        &scenario.session,
        &mut session,
        &mut Diagnostics::new(),
        &scenario.file,
    );
    let mut lines = Vec::new();
    let mut facts = Vec::new();

    let mut plans = false;
    let cart: Vec<String> = session
        .cart_lines
        .iter()
        .map(|line| match store.variant(line.variant_id) {
            Some((product, variant)) => {
                let mut text = format!("{} × {}", line.quantity, product.title);
                if !variant.title.is_empty() && variant.title != "Default Title" {
                    text.push_str(&format!(" ({})", variant.title));
                }
                if let Some((group, plan)) = line
                    .selling_plan
                    .and_then(|id| store.selling_plan_location(id))
                {
                    plans = true;
                    let plan = &store.selling_plan_groups[group].plans[plan];
                    text.push_str(&format!(" · {}", plan.name));
                    if plan.checkout_charge != lsf_core::store::CheckoutCharge::Percentage(100) {
                        text.push_str(" (deferred payment)");
                    }
                }
                text
            }
            None => format!("{} × a variant that is not in the data", line.quantity),
        })
        .collect();
    facts.push(if cart.is_empty() {
        "empty cart".to_string()
    } else {
        count(cart.len(), "line", "lines")
    });
    if plans {
        facts.push("selling plan".to_string());
    }
    let mut cart_text = if cart.is_empty() {
        "empty".to_string()
    } else {
        cart.join("; ")
    };
    if !session.cart_note.is_empty() {
        cart_text.push_str(&format!(" · note “{}”", session.cart_note));
    }
    if !session.cart_attributes.is_empty() {
        let attributes: Vec<String> = session
            .cart_attributes
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        cart_text.push_str(&format!(" · attributes {}", attributes.join(", ")));
    }
    lines.push(("Cart", cart_text));

    let customer = session.customer_id.and_then(|id| store.customer_by_id(id));
    let buyer = match customer {
        None => {
            facts.push("guest".to_string());
            "guest".to_string()
        }
        Some(customer) => {
            let mut text = format!("customer {}", customer.email);
            match customer.company {
                Some(company) => {
                    let company = &store.companies[company];
                    facts.push(format!("B2B {}", company.name));
                    text.push_str(&format!(" · B2B, {}", company.name));
                    if let Some(location) =
                        customer.current_location(session.company_location, store)
                    {
                        text.push_str(&format!(", {}", company.locations[location].name));
                    }
                }
                None => facts.push(if customer.first_name.is_empty() {
                    customer.email.clone()
                } else {
                    customer.first_name.clone()
                }),
            }
            text
        }
    };
    lines.push(("Buyer", buyer));

    let entered = &scenario.entered;
    let country = entered
        .shipping_address
        .as_ref()
        .and_then(|address| address.country_code.clone())
        .or(session.country.clone())
        .map(|code| code.to_ascii_uppercase());
    let language = scenario
        .language
        .as_deref()
        .and_then(|iso| store.language(iso))
        .unwrap_or_else(|| store.primary_language());
    if let Some(country) = &country {
        facts.push(country.clone());
    }
    facts.push(language.iso_code.clone());
    lines.push((
        "Country, language",
        format!(
            "{}, {}",
            country.as_deref().unwrap_or("the store's first country"),
            language.name
        ),
    ));

    let mut typed = Vec::new();
    if let Some(email) = &entered.email {
        typed.push(format!("email {email}"));
    }
    if let Some(address) = &entered.shipping_address {
        let country = address.country_code.as_deref().map(str::to_ascii_uppercase);
        let place: Vec<&str> = [
            address.address1.as_deref(),
            address.city.as_deref(),
            country.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        typed.push(format!("address {}", place.join(", ")));
    }
    if let Some(delivery) = &entered.delivery {
        typed.push(format!("delivery {delivery}"));
    }
    if let Some(payment) = &entered.payment {
        typed.push(format!("payment {payment}"));
    }
    if !typed.is_empty() {
        lines.push(("Entered", typed.join(" · ")));
    }

    // The discounts are listed when the scenario has its own: otherwise they are the store's,
    // the same for every scenario.
    let mut discounts = Vec::new();
    if scenario.overrides.contains(&"discounts") {
        if scenario.checkout.discounts.is_empty() {
            discounts.push("none".to_string());
        }
        for discount in &scenario.checkout.discounts {
            discounts.push(match &discount.code {
                Some(code) => format!("{} (code {code})", discount.title),
                None => format!("{} (automatic)", discount.title),
            });
        }
        facts.push(count(
            scenario.checkout.discounts.len(),
            "own discount",
            "own discounts",
        ));
    } else {
        discounts.push(format!(
            "the store's {}",
            count(scenario.checkout.discounts.len(), "discount", "discounts")
        ));
    }
    if !entered.discount_codes.is_empty() {
        discounts.push(format!(
            "codes entered: {}",
            entered.discount_codes.join(", ")
        ));
        facts.push(count(entered.discount_codes.len(), "code", "codes"));
    }
    lines.push(("Discounts", discounts.join(" · ")));

    let (rates, methods) = names(&scenario.checkout, store);
    let mut set_up = Vec::new();
    if scenario.overrides.contains(&"shipping_rates") {
        set_up.push(format!("shipping {rates}"));
    }
    if scenario.overrides.contains(&"payment_methods") {
        set_up.push(format!("payment {methods}"));
    }
    if scenario.overrides.contains(&"tax_rate") {
        set_up.push(format!(
            "tax {}%",
            scenario.checkout.tax_rate_bps as f64 / 100.0
        ));
    }
    if scenario.overrides.contains(&"functions") {
        set_up.push("Function settings".to_string());
    }
    if !set_up.is_empty() {
        lines.push(("Overrides", set_up.join(" · ")));
    }

    if let Some(look) = scenario.look {
        lines.push((
            "Look",
            match look {
                ScenarioLook::Standard => "standard",
                ScenarioLook::Shop => "shop",
            }
            .to_string(),
        ));
        if look == ScenarioLook::Shop {
            facts.push("Shop look".to_string());
        }
    }
    (lines, facts)
}

/// A query component, `form_urlencoded`'s way (spaces as `+`).
pub(super) fn query_escape(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

/// What the scenarios have in common: the app, what `--extension` selects, the statuses of the
/// extensions, and what the check of the Functions found.
struct Context<'a> {
    store: &'a Store,
    app: &'a App,
    options: &'a CheckoutOptions,
    selection: &'a Selection,
    root: &'a str,
    report: Option<&'a Report>,
    /// The worst level of the problems of each extension, by handle, from its own problems
    /// only (`None`: it has none).
    statuses: HashMap<String, Option<Level>>,
}

/// The extensions a scenario runs: the ones it names, or every one that is selected.
fn uses<'a>(context: &'a Context, scenario: &'a CheckoutScenario) -> Vec<&'a str> {
    match &scenario.extensions {
        Some(handles) => handles.iter().map(String::as_str).collect(),
        None => context
            .app
            .handles()
            .into_iter()
            .filter(|handle| context.selection.contains(handle))
            .collect(),
    }
}

/// The problems of a scenario of its own: it does not validate, or it names an extension the
/// app does not have.
fn own_problems(context: &Context, scenario: &CheckoutScenario) -> Vec<String> {
    let mut problems = scenario.problems.clone();
    if problems.is_empty()
        && let Err(problem) =
            crate::checkout::selection_for(context.app, &context.options.extensions, Some(scenario))
    {
        problems.push(format!("{}: {problem}", scenario.file));
    }
    problems
}

fn scenario_json(context: &Context, scenario: &CheckoutScenario) -> Json {
    let problems = own_problems(context, scenario);
    let root = context.root;
    let open = format!("{root}/checkout?scenario={}", query_escape(&scenario.name));
    let (summary, facts): (Vec<Json>, Vec<String>) = if scenario.problems.is_empty() {
        let (lines, facts) = describe(context.store, scenario);
        (
            lines
                .into_iter()
                .map(|(label, text)| json!({"label": label, "text": text}))
                .collect(),
            facts,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let handles = uses(context, scenario);
    let functions: Vec<&str> = handles
        .iter()
        .copied()
        .filter(|handle| context.app.function(handle).is_some())
        .collect();
    let ui: Vec<&str> = handles
        .iter()
        .copied()
        .filter(|handle| context.app.ui_extension(handle).is_some())
        .collect();
    let failing: Vec<&str> = match (context.report, problems.is_empty()) {
        (Some(report), true) => report.failing_in(&scenario.name),
        _ => Vec::new(),
    };
    // The worst of its own problems and the Functions that fail in it (errors), and of the
    // problems of the extensions it runs (each at its own level).
    let own = (!problems.is_empty() || !failing.is_empty()).then_some(Level::Error);
    let status = status_of(
        own.into_iter().chain(
            handles
                .iter()
                .filter_map(|handle| context.statuses.get(*handle).copied().flatten()),
        ),
    );
    json!({
        "name": scenario.name,
        "anchor": anchor("scenario", &scenario.name),
        "title": scenario.title,
        "description": scenario.description,
        "file": scenario.file,
        "status": status,
        // Its row in a few words: `3 lines · guest · FR · fr`.
        "facts": facts,
        "summary": summary,
        "problems": problems,
        // The extensions it runs: the ones it names, or every one (`allExtensions`).
        "allExtensions": scenario.extensions.is_none(),
        "functions": functions,
        "uiExtensions": ui,
        // The Functions that fail in it, once the check ran.
        "failing": failing,
        "open": problems.is_empty().then_some(open.clone()),
        "openAllTargets": problems.is_empty().then(|| format!("{open}&targets=all")),
    })
}

/// One line of the list of problems.
struct Problem {
    level: Level,
    /// What it is about: `store`, `app`, `build` (a build of `--watch` that failed), `crash` (a
    /// Function that fails when a scenario runs), `function`, `uiExtension` or `scenario`.
    /// Within a level, in this order.
    kind: &'static str,
    /// The handle, the scenario or the file it is about.
    subject: Option<String>,
    message: String,
    /// The target of a run that fails.
    target: Option<String>,
    /// Where to look: the row of the scenario or of the extension, and what to call it.
    link: Option<(String, String)>,
    /// The scenarios a Function fails in.
    scenarios: Vec<String>,
    /// The checkout where it shows (see [`Open`]).
    open: Option<Open>,
    /// What else to read: the end of the output of a build that failed.
    detail: Option<String>,
}

/// A fresh checkout where a problem shows: the scenario that runs the extension (or the
/// store's cart, when no scenario does), with the dialog of the Functions open on `functions`
/// (`?functions=<handle>` or `<handle>:<target>`) when it is set.
struct Open {
    scenario: Option<String>,
    functions: Option<String>,
    /// Whether the dialog opens on a run (otherwise the page, or the overview of the dialog).
    run: bool,
    /// The customer account rather than a checkout: a problem of an extension that only renders
    /// there.
    account: bool,
}

impl Open {
    fn href(&self, root: &str, scenario: Option<&str>) -> String {
        if self.account {
            return format!("{root}/account");
        }
        checkout_href(
            root,
            scenario.or(self.scenario.as_deref()),
            self.functions.as_deref(),
        )
    }
}

/// `/checkout`, with `?scenario=` and `?functions=` when they are given.
fn checkout_href(root: &str, scenario: Option<&str>, functions: Option<&str>) -> String {
    let mut pairs = Vec::new();
    if let Some(name) = scenario {
        pairs.push(format!("scenario={}", query_escape(name)));
    }
    if let Some(functions) = functions {
        pairs.push(format!("functions={}", query_escape(functions)));
    }
    if pairs.is_empty() {
        format!("{root}/checkout")
    } else {
        format!("{root}/checkout?{}", pairs.join("&"))
    }
}

impl Problem {
    fn rank(&self) -> (Level, u8) {
        let kind = match self.kind {
            "store" => 0,
            "app" => 1,
            "build" => 2,
            "crash" => 3,
            "function" => 4,
            "uiExtension" => 5,
            _ => 6,
        };
        (self.level, kind)
    }

    fn to_json(&self, root: &str) -> Json {
        let open = self.open.as_ref().map(|open| {
            let mut json = json!({
                "href": open.href(root, None),
                "scenario": open.scenario,
                "functions": open.functions,
                "run": open.run,
            });
            // Only said of a problem shown in the customer account.
            if open.account {
                json["account"] = json!(true);
            }
            json
        });
        // Each scenario a Function fails in, with the checkout of that scenario on the run.
        let runs: Vec<Json> = match &self.open {
            Some(open) if self.kind == "crash" => self
                .scenarios
                .iter()
                .map(|name| json!({"scenario": name, "href": open.href(root, Some(name))}))
                .collect(),
            _ => Vec::new(),
        };
        json!({
            "severity": self.level.name(),
            "kind": self.kind,
            "subject": self.subject,
            "message": self.message,
            "target": self.target,
            "href": self.link.as_ref().map(|(href, _)| href),
            "where": self.link.as_ref().map(|(_, label)| label),
            "scenarios": self.scenarios,
            // The checkout where it shows, and for a run that fails, each scenario's.
            "open": open,
            "runs": runs,
            "detail": self.detail,
        })
    }
}

/// The targets of an extension, each once.
fn target_names(extension: &Json) -> Vec<&str> {
    let mut names = Vec::new();
    for target in extension["targets"].as_array().into_iter().flatten() {
        let name = text_of(&target["target"]);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Where a problem of an extension shows: the scenario that runs it (`runsIn`, see `build`), or
/// the store's cart.
/// - A Function: the dialog of the Functions on it, on the target the message names when it
///   has several. The checkout shows that run, or the overview when it has none.
/// - A checkout UI extension that the page loads shows on the page: no dialog. One it does not
///   load (not built, an API version it does not run, an `owner_type` it refuses) is in the
///   overview of the dialog.
fn open_for(kind: &str, extension: &Json, message: &str) -> Open {
    let handle = text_of(&extension["handle"]);
    let scenario = extension["runsIn"].as_str().map(str::to_string);
    if kind == "uiExtension" {
        let account = only_in_account(extension);
        return Open {
            scenario: scenario.filter(|_| !account),
            functions: (extension["loads"] != true && !account).then(|| handle.to_string()),
            run: false,
            account,
        };
    }
    let targets: Vec<&Json> = extension["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let named = targets
        .iter()
        .find(|target| message.contains(&format!("`{}`", text_of(&target["target"]))));
    let functions = match named {
        Some(target) if target_names(extension).len() > 1 => {
            format!("{handle}:{}", text_of(&target["target"]))
        }
        _ => handle.to_string(),
    };
    let run = match named {
        Some(target) => target["hasRun"] == true,
        None => targets.iter().any(|target| target["hasRun"] == true),
    };
    Open {
        scenario,
        functions: Some(functions),
        run,
        account: false,
    }
}

/// Whether every target of a UI extension that has a place is one of the customer account.
fn only_in_account(extension: &Json) -> bool {
    let surfaces = strings(&extension["surfaces"]);
    !surfaces.is_empty()
        && surfaces
            .iter()
            .all(|surface| *surface == "customer-account")
}

/// Everything that is wrong, errors first: the store data, the app, the Functions that fail
/// when the scenarios run, the extensions, the scenarios.
fn problem_list(
    state: &ServerState,
    app: &App,
    functions: &[Json],
    ui_extensions: &[Json],
    scenarios: &[Json],
    report: Option<&Report>,
) -> Vec<Problem> {
    let mut list = Vec::new();
    let add = |list: &mut Vec<Problem>, level, kind, subject: Option<String>, message, link| {
        list.push(Problem {
            level,
            kind,
            subject,
            message,
            target: None,
            link,
            scenarios: Vec::new(),
            open: None,
            detail: None,
        });
    };
    for item in &state.loaded().diagnostics.items {
        // The scenarios have lines of their own.
        if item.severity == lsf_core::diagnostics::Severity::Error
            && !item.file.starts_with("checkouts/")
        {
            add(
                &mut list,
                Level::Error,
                "store",
                Some(item.file.clone()),
                item.to_string(),
                None,
            );
        }
    }
    for diagnostic in app
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.extension.is_none())
    {
        add(
            &mut list,
            Level::of(diagnostic.severity),
            "app",
            None,
            diagnostic.message.clone(),
            None,
        );
    }
    for (kind, extensions, label) in [
        ("function", functions, "Functions"),
        ("uiExtension", ui_extensions, "UI extensions"),
    ] {
        for extension in extensions {
            for problem in extension["problems"].as_array().into_iter().flatten() {
                let message = text_of(&problem["message"]);
                list.push(Problem {
                    level: Level::of_json(problem),
                    kind,
                    subject: extension["handle"].as_str().map(str::to_string),
                    message: message.to_string(),
                    target: None,
                    link: Some((
                        format!("#{}", text_of(&extension["anchor"])),
                        label.to_string(),
                    )),
                    scenarios: Vec::new(),
                    open: Some(open_for(kind, extension, message)),
                    detail: None,
                });
            }
        }
    }
    // The builds of `--watch` that failed: the Function runs its last module meanwhile.
    for function in functions {
        let build = &function["build"];
        if !build.is_object() {
            continue;
        }
        let message = format!(
            "{}: {}",
            text_of(&build["reason"]),
            if function["built"] == true {
                "the checkout runs the last module that built"
            } else {
                "no module of it built yet"
            }
        );
        list.push(Problem {
            level: Level::Error,
            kind: "build",
            subject: function["handle"].as_str().map(str::to_string),
            open: Some(open_for("function", function, &message)),
            message,
            target: None,
            link: Some((
                format!("#{}", text_of(&function["anchor"])),
                "Functions".to_string(),
            )),
            scenarios: Vec::new(),
            detail: build["output"]
                .as_str()
                .filter(|output| !output.is_empty())
                .map(str::to_string),
        });
    }
    for scenario in scenarios {
        let problems = scenario["problems"].as_array().cloned().unwrap_or_default();
        let Some(first) = problems.first().and_then(Json::as_str) else {
            continue;
        };
        let message = if problems.len() > 1 {
            format!("{first} (and {} more)", problems.len() - 1)
        } else {
            first.to_string()
        };
        add(
            &mut list,
            Level::Error,
            "scenario",
            scenario["name"].as_str().map(str::to_string),
            message,
            Some((
                format!("#{}", text_of(&scenario["anchor"])),
                text_of(&scenario["file"]).to_string(),
            )),
        );
    }
    // A Function that cannot run fails in every scenario that runs it: its own error says why.
    let broken = |handle: &str| {
        functions.iter().any(|function| {
            function["handle"] == handle
                && function["problems"]
                    .as_array()
                    .is_some_and(|problems| problems.iter().any(|p| p["severity"] == "error"))
        })
    };
    for failure in report.into_iter().flat_map(|report| &report.failures) {
        if broken(&failure.handle) {
            continue;
        }
        let first = &failure.scenarios[0];
        // The run itself: the target too when the Function has several.
        let several = functions
            .iter()
            .find(|function| function["handle"] == failure.handle.as_str())
            .is_some_and(|function| target_names(function).len() > 1);
        let functions = if several {
            format!("{}:{}", failure.handle, failure.target)
        } else {
            failure.handle.clone()
        };
        list.push(Problem {
            level: Level::Error,
            kind: "crash",
            subject: Some(failure.handle.clone()),
            message: failure.error.clone(),
            target: Some(failure.target.clone()),
            link: Some((format!("#{}", anchor("scenario", first)), first.clone())),
            scenarios: failure.scenarios.clone(),
            open: Some(Open {
                scenario: Some(first.clone()),
                functions: Some(functions),
                run: true,
                account: false,
            }),
            detail: None,
        });
    }
    list.sort_by_key(Problem::rank);
    list
}

/// Where the page asks for the check of the Functions: `/__lsf/checkouts/check`, with the
/// language of the page when it is not the store's own.
pub fn check_url(locale: Option<&str>) -> String {
    match locale {
        Some(iso) => format!("/__lsf/checkouts/check?locale={}", query_escape(iso)),
        None => "/__lsf/checkouts/check".to_string(),
    }
}

/// Everything the home page says about `app` and `store` (the request's one snapshot of each),
/// in the language whose locale root is `root`. `report` is what the check of the Functions
/// found, when it ran; otherwise the page asks `check` for it.
fn build(
    state: &ServerState,
    options: &CheckoutOptions,
    store: &Store,
    app: &App,
    root: &str,
    check: &str,
    report: Option<&Report>,
) -> Json {
    let selection =
        Selection::resolve(app, &options.extensions).unwrap_or_else(|_| Selection::all());
    // The discount Functions a discount of the store data, or of a scenario, runs.
    let discounted: std::collections::HashSet<&str> = store
        .checkout
        .discounts
        .iter()
        .chain(
            store
                .scenarios
                .iter()
                .flat_map(|scenario| &scenario.checkout.discounts),
        )
        .map(|discount| discount.function.as_str())
        .collect();
    let mut context = Context {
        store,
        app,
        options,
        selection: &selection,
        root,
        report,
        statuses: HashMap::new(),
    };

    // How many scenarios that open run each extension, and the one its problems open: the one
    // that names it among the fewest extensions (the first of those), otherwise the first that
    // runs every extension.
    let opens: Vec<&CheckoutScenario> = store
        .scenarios
        .iter()
        .filter(|scenario| own_problems(&context, scenario).is_empty())
        .collect();
    let mut used_by: HashMap<String, usize> = HashMap::new();
    let mut runs_in: HashMap<String, (usize, String)> = HashMap::new();
    for scenario in &opens {
        let rank = scenario.extensions.as_ref().map_or(usize::MAX, Vec::len);
        for handle in uses(&context, scenario) {
            *used_by.entry(handle.to_string()).or_default() += 1;
            let best = runs_in
                .entry(handle.to_string())
                .or_insert_with(|| (rank, scenario.name.clone()));
            if rank < best.0 {
                *best = (rank, scenario.name.clone());
            }
        }
    }
    let used = |handle: &str| used_by.get(handle).copied().unwrap_or(0);
    let runs_in = |mut extension: Json| {
        extension["runsIn"] = json!(
            runs_in
                .get(text_of(&extension["handle"]))
                .map(|(_, name)| name)
        );
        extension
    };
    let functions: Vec<Json> = app
        .functions
        .iter()
        .map(|function| {
            let build = state.builds.failure(&function.handle);
            function_json(
                app,
                &selection,
                function,
                report,
                build.as_ref(),
                used(&function.handle),
                &discounted,
            )
        })
        .map(runs_in)
        .collect();
    let ui_extensions: Vec<Json> = app
        .ui_extensions
        .iter()
        .map(|extension| ui_extension_json(app, &selection, extension, used(&extension.handle)))
        .map(runs_in)
        .map(|mut extension| {
            // Where a problem the page finds when it loads the bundle shows (`home.js`): the
            // customer account for an extension that only renders there.
            extension["open"] = if only_in_account(&extension) {
                extension["runsIn"] = Json::Null;
                json!(format!("{root}/account"))
            } else {
                json!(checkout_href(root, extension["runsIn"].as_str(), None))
            };
            extension
        })
        .collect();
    // What a scenario reads of the extensions it uses: their own problems, not the runs that
    // fail (those are the scenario's own), nor what is the store's rather than the scenario's (a
    // discount Function no discount names).
    for extension in functions.iter().chain(&ui_extensions) {
        let levels = extension["problems"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|problem| problem["store"] != true)
            .map(Level::of_json);
        context
            .statuses
            .insert(text_of(&extension["handle"]).to_string(), levels.min());
    }
    let scenarios: Vec<Json> = store
        .scenarios
        .iter()
        .map(|scenario| scenario_json(&context, scenario))
        .collect();
    let problems = problem_list(state, app, &functions, &ui_extensions, &scenarios, report);
    let errors = problems
        .iter()
        .filter(|problem| problem.level == Level::Error)
        .count();
    let runnable = scenarios
        .iter()
        .filter(|scenario| scenario["open"].is_string())
        .count();
    let check = match report {
        Some(report) => {
            let mut json = report.to_json();
            json["url"] = json!(check);
            json
        }
        None if runnable > 0 => json!({"state": "pending", "url": check, "total": runnable}),
        None => json!({"state": "none"}),
    };

    json!({
        "root": root,
        "app": {
            "name": app.name,
            "handle": app.handle,
            "clientId": app.client_id,
            "directory": app.root.display().to_string(),
        },
        "data": state.app.describe_source(),
        "dataFolder": state.app.data_dir().map(|dir| dir.join("checkouts").display().to_string()),
        "storeDataProblems": state.loaded().diagnostics.error_count(),
        // Everything that is wrong, errors first.
        "problems": problems.iter().map(|problem| problem.to_json(root)).collect::<Vec<_>>(),
        "counts": {"errors": errors, "warnings": problems.len() - errors},
        // The Functions that fail when the scenarios run: `pending` until the check ran.
        "check": check,
        "functions": functions,
        "uiExtensions": ui_extensions,
        "otherExtensions": app.other.iter()
            .map(|other| json!({"handle": other.handle, "type": other.kind}))
            .collect::<Vec<_>>(),
        "scenarios": scenarios,
        // The login profiles: the customers of the store data, each with a checkout logged in as
        // them. A scenario gives it a cart, the session's own may well be empty: the first that
        // opens in the store's own language and country, the first that opens otherwise.
        "customers": store.customers.iter()
            .map(|customer| {
                let mut profile = super::checkout_account::profile_json(customer, store, false);
                // Opens: no problem of its own, and the extensions it names are the app's.
                let opens = || {
                    store.scenarios.iter().filter(|scenario| {
                        scenario.problems.is_empty()
                            && crate::checkout::selection_for(app, &options.extensions, Some(scenario)).is_ok()
                    })
                };
                let scenario = opens()
                    .find(|scenario| scenario.language.is_none() && scenario.session.country.is_none())
                    .or_else(|| opens().next())
                    .map(|scenario| format!("scenario={}&", query_escape(&scenario.name)))
                    .unwrap_or_default();
                profile["open"] = json!(format!("{root}/checkout?{scenario}customer={}", query_escape(&customer.email)));
                if let Some(profile) = profile.as_object_mut() {
                    profile.remove("current");
                }
                profile
            })
            .collect::<Vec<_>>(),
        "links": {
            "checkout": format!("{root}/checkout"),
            "allTargets": format!("{root}/checkout?targets=all"),
            "shop": format!("{root}/checkout?variant=shop"),
            // Where the customer account UI extensions render.
            "account": format!("{root}/account"),
        },
    })
}

/// `GET /` of a checkout without a theme and `GET /__lsf/checkouts`: the page, or with
/// `?format=json` what it says. The check of the Functions is in it when it already ran on the
/// app and the data as they are (or with `?check=1`, which runs it first); otherwise the page
/// asks `check` for it once it is shown.
pub fn page(
    state: &ServerState,
    incoming: &Incoming,
    options: &CheckoutOptions,
    root: &str,
    check: &str,
) -> Reply {
    let store = state.store_of(incoming);
    let home = match state.load_app(&options.app_dir) {
        Ok(app) => {
            let report = if incoming.query_param("check").is_some() {
                Some(state.home_check.get_or_run(state, &app, &store))
            } else {
                state.home_check.get(&app, &store)
            };
            build(state, options, &store, &app, root, check, report.as_deref())
        }
        Err(problem) => json!({"error": problem, "root": root}),
    };
    if incoming.query_param("format") == Some("json") {
        return Reply::json(200, &home).cached(cache::PRIVATE);
    }
    let mut html = render(&home, &check_now_href(incoming));
    if state.options.live_reload {
        super::live_reload::inject(&mut html);
    }
    Reply::html(200, html).cached(cache::PRIVATE)
}

/// The page itself with `check=1`: what a browser without the script follows to run the check.
fn check_now_href(incoming: &Incoming) -> String {
    let mut pairs: Vec<String> = incoming
        .query
        .iter()
        .filter(|(key, _)| key != "check")
        .map(|(key, value)| format!("{}={}", query_escape(key), query_escape(value)))
        .collect();
    pairs.push("check=1".to_string());
    format!("{}?{}", incoming.path, pairs.join("&"))
}

/// `GET /__lsf/checkouts/check`: runs the Functions of the scenarios (or takes the last run on
/// the same app and data), and answers what the page shows of it: the list of problems, the
/// rows of the scenarios and of the Functions that fail, in HTML, and the order of the
/// scenarios, problems first.
pub fn check(
    state: &ServerState,
    incoming: &Incoming,
    options: &CheckoutOptions,
    root: &str,
) -> Reply {
    let store = state.store_of(incoming);
    let app = match state.load_app(&options.app_dir) {
        Ok(app) => app,
        Err(problem) => {
            return Reply::json(200, &json!({"state": "error", "error": problem}))
                .cached(cache::PRIVATE);
        }
    };
    let report = state.home_check.get_or_run(state, &app, &store);
    let url = check_url(incoming.query_param("locale"));
    let home = build(state, options, &store, &app, root, &url, Some(&report));
    let mut answer = report.to_json();
    answer["problems"] = json!(problems_html(&home, ""));
    let mut rows = serde_json::Map::new();
    let failing = |list: &Json| list.as_array().is_some_and(|list| !list.is_empty());
    // A row is drawn again as the page drew it: in its group.
    let groups = scenario_groups(&sorted_scenarios(&home));
    let grouped = groups.len() > 1;
    for (group, scenarios) in &groups {
        for scenario in scenarios
            .iter()
            .filter(|scenario| failing(&scenario["failing"]))
        {
            rows.insert(
                text_of(&scenario["anchor"]).to_string(),
                json!(scenario_html(
                    scenario,
                    app_dir(&home),
                    grouped.then_some(group.as_str())
                )),
            );
        }
    }
    for function in home["functions"].as_array().into_iter().flatten() {
        if failing(&function["failures"]) {
            rows.insert(
                text_of(&function["anchor"]).to_string(),
                json!(function_html(function, app_dir(&home))),
            );
        }
    }
    answer["rows"] = Json::Object(rows);
    answer["order"] = json!(
        sorted_scenarios(&home)
            .iter()
            .map(|scenario| scenario["anchor"].clone())
            .collect::<Vec<_>>()
    );
    Reply::json(200, &answer).cached(cache::PRIVATE)
}

// --- The HTML ----------------------------------------------------------------------------

fn code(text: &str) -> String {
    format!("<code>{}</code>", esc(text))
}

/// The folder of the app, whose paths the page shows relative to it.
fn app_dir(home: &Json) -> &str {
    text_of(&home["app"]["directory"])
}

/// `text` with the paths inside the app folder `dir` relative to it.
fn shorten(text: &str, dir: &str) -> String {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        return text.to_string();
    }
    text.replace(&format!("{dir}/"), "")
}

/// A message as the page shows it: the paths inside the app relative to its folder, and what is
/// between backticks as code. Escaped first: the message cannot add HTML of its own. (The full
/// message is the `title` of what shows it.)
fn message_html(text: &str, dir: &str) -> String {
    let short = shorten(text, dir);
    let parts: Vec<&str> = short.split('`').collect();
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index % 2 == 0 {
            out.push_str(&esc(part));
        } else if index + 1 < parts.len() {
            out.push_str(&code(part));
        } else {
            // A backtick with no pair.
            out.push('`');
            out.push_str(&esc(part));
        }
    }
    out
}

fn text_of(value: &Json) -> &str {
    value.as_str().unwrap_or_default()
}

fn strings(value: &Json) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect()
}

fn banner(tone: &str, text: &str) -> String {
    format!(
        "<div class=\"lsf-banner is-{tone}\" role=\"status\"><div><p>{}</p></div></div>",
        esc(text)
    )
}

/// A status dot, with what it means for a screen reader.
fn dot(status: &str) -> String {
    let label = match status {
        "error" => "Error",
        "warning" => "Warning",
        "off" => "Left out",
        _ => "OK",
    };
    format!("<span class=\"lsf-dot is-{status}\" role=\"img\" aria-label=\"{label}\"></span>")
}

/// Text for screen readers only.
fn hidden(text: &str) -> String {
    format!("<span class=\"lsf-sr\">{}</span>", esc(text))
}

fn section_head(id: &str, title: &str, aside: &str) -> String {
    format!(
        "<div class=\"lsf-h-head\"><h2 id=\"{id}\">{title}</h2>{}</div>",
        if aside.is_empty() {
            String::new()
        } else {
            format!("<p class=\"lsf-h-count\">{aside}</p>")
        }
    )
}

fn problem_html(problem: &Json, dir: &str) -> String {
    let severity = text_of(&problem["severity"]);
    let kind = text_of(&problem["kind"]);
    let label = match kind {
        "crash" => "fails",
        "build" => "build",
        "function" => "Function",
        "uiExtension" => "UI extension",
        "scenario" => "scenario",
        "store" => "store data",
        _ => "app",
    };
    let message = text_of(&problem["message"]);
    let subject = problem["subject"].as_str().map(code).unwrap_or_default();
    let scenarios = strings(&problem["scenarios"]);
    let more = if scenarios.len() > 1 {
        format!(" <span class=\"lsf-muted\">+{}</span>", scenarios.len() - 1)
    } else {
        String::new()
    };
    let open = &problem["open"];
    let place = match (open["href"].as_str(), problem["href"].as_str()) {
        (Some(href), _) if open["account"] == true => format!(
            "<a class=\"lsf-hp-where lsf-hp-open\" href=\"{}\" title=\"Opens the customer account, where it renders\">\
             Open the customer account{more}</a>",
            esc(href)
        ),
        (Some(href), _) => {
            let run = open["run"] == true;
            let label = if run {
                "Open the run"
            } else {
                "Open in checkout"
            };
            let place = match open["scenario"].as_str() {
                Some(name) => format!("the scenario {name}"),
                None => "the store's cart".to_string(),
            };
            let shows = match open["functions"].as_str() {
                Some(_) if run => ", the Functions dialog on its run",
                Some(_) => ", the Functions dialog on its overview",
                None => "",
            };
            format!(
                "<a class=\"lsf-hp-where lsf-hp-open\" href=\"{}\" title=\"{}\">{label} \
                 <span class=\"lsf-hp-in\">{}</span>{more}</a>",
                esc(href),
                esc(&format!("Opens a fresh checkout of {place}{shows}")),
                esc(open["scenario"].as_str().unwrap_or("store's cart")),
            )
        }
        (None, Some(href)) => format!(
            "<a class=\"lsf-hp-where\" href=\"{}\">{}{}</a>",
            esc(href),
            esc(text_of(&problem["where"])),
            more
        ),
        (None, None) if kind == "store" => {
            "<span class=\"lsf-hp-where lsf-muted\"><code>lsf validate</code></span>".to_string()
        }
        (None, None) => String::new(),
    };
    let mut full = String::new();
    if let Some(target) = problem["target"].as_str() {
        full.push_str(&format!("<p>Target {}</p>", code(target)));
    }
    full.push_str(&format!(
        "<pre title=\"{}\">{}</pre>",
        esc(message),
        message_html(message, dir)
    ));
    if let Some(detail) = problem["detail"].as_str() {
        full.push_str(&format!(
            "<p>The end of its output:</p><pre class=\"lsf-hp-output\">{}</pre>",
            esc(&shorten(detail, dir))
        ));
    }
    if scenarios.len() > 1 {
        // Each with the checkout of that scenario on the run.
        let links: Vec<String> = problem["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|run| {
                format!(
                    "<a href=\"{}\">{}</a>",
                    esc(text_of(&run["href"])),
                    esc(text_of(&run["scenario"]))
                )
            })
            .collect();
        full.push_str(&format!(
            "<p>Fails in {} scenarios, open the run in each: {}</p>",
            scenarios.len(),
            links.join(", ")
        ));
    }
    // The rows of this page it is about, when the link on the right opens a checkout.
    if open.is_object() {
        let mut rows = Vec::new();
        if let Some(subject) = problem["subject"].as_str() {
            let (prefix, list) = match kind {
                "uiExtension" => ("ui", "UI extensions"),
                _ => ("function", "Functions"),
            };
            rows.push(format!(
                "<a href=\"#{}\">{}</a> in the {list}",
                esc(&anchor(prefix, subject)),
                esc(subject)
            ));
        }
        if kind == "crash" {
            for name in &scenarios {
                rows.push(format!(
                    "<a href=\"#{}\">{}</a> in the scenarios",
                    esc(&anchor("scenario", name)),
                    esc(name)
                ));
            }
        }
        if !rows.is_empty() {
            full.push_str(&format!(
                "<p class=\"lsf-hp-rowlinks\">On this page: {}</p>",
                rows.join(" · ")
            ));
        }
    }
    format!(
        "<li class=\"lsf-hp-row is-{severity}\" data-problem=\"{kind}\"{}>\
         <details><summary>{}<span class=\"lsf-hp-kind\">{label}</span>{subject}\
         <span class=\"lsf-hp-msg\" title=\"{}\">{}</span></summary>\
         <div class=\"lsf-hp-full\">{full}</div></details>{place}</li>",
        problem["subject"]
            .as_str()
            .map(|subject| format!(" data-subject=\"{}\"", esc(subject)))
            .unwrap_or_default(),
        dot(severity),
        esc(message),
        message_html(message, dir),
    )
}

/// What the check of the Functions says, under the title of the problems.
fn check_line(home: &Json, check_now: &str) -> String {
    let check = &home["check"];
    let total = check["total"].as_u64().unwrap_or(0) as usize;
    let text = match text_of(&check["state"]) {
        "pending" => format!(
            "<span class=\"lsf-h-spin\" aria-hidden=\"true\"></span>Running the Functions of {} to find the ones that fail…{}",
            count(total, "scenario", "scenarios"),
            if check_now.is_empty() {
                String::new()
            } else {
                format!(
                    "<noscript> <a class=\"lsf-link\" href=\"{}\">Run them now</a></noscript>",
                    esc(check_now)
                )
            }
        ),
        "done" | "partial" => {
            let failures = check["failures"].as_array().map_or(0, Vec::len);
            let checked = check["checked"].as_u64().unwrap_or(0) as usize;
            let seconds = check["elapsedMs"].as_u64().unwrap_or(0) as f64 / 1000.0;
            let mut text = if checked < total {
                format!(
                    "Checked {checked} of {total} scenarios ({seconds:.1} s): out of time for the others"
                )
            } else {
                format!(
                    "Checked {} ({seconds:.1} s, without sending their HTTP requests)",
                    count(total, "scenario", "scenarios")
                )
            };
            text.push_str(&match failures {
                0 => ": no run fails.".to_string(),
                1 => ": 1 run fails.".to_string(),
                n => format!(": {n} runs fail."),
            });
            esc(&text)
        }
        _ => return String::new(),
    };
    format!(
        "<p class=\"lsf-hp-check\" role=\"status\" aria-live=\"polite\" data-check-line>{text}</p>"
    )
}

/// The list of problems, errors first, each on one line: the first thing on the page.
fn problems_html(home: &Json, check_now: &str) -> String {
    let problems = home["problems"].as_array().cloned().unwrap_or_default();
    let errors = home["counts"]["errors"].as_u64().unwrap_or(0) as usize;
    let warnings = home["counts"]["warnings"].as_u64().unwrap_or(0) as usize;
    let check = &home["check"];
    let mut counts = Vec::new();
    if errors > 0 {
        counts.push(count(errors, "error", "errors"));
    }
    if warnings > 0 {
        counts.push(count(warnings, "warning", "warnings"));
    }
    let mut out = format!(
        "<section class=\"lsf-hp{}\" id=\"problems\" data-section=\"problems\" aria-labelledby=\"lsf-h-problems\" \
         data-check=\"{}\" data-check-url=\"{}\">{}{}",
        if errors > 0 {
            " has-errors"
        } else if warnings > 0 {
            " has-warnings"
        } else {
            " is-clear"
        },
        esc(text_of(&check["state"])),
        esc(text_of(&check["url"])),
        section_head("lsf-h-problems", "Problems", &counts.join(" · ")),
        check_line(home, check_now),
    );
    if problems.is_empty() {
        out.push_str(&format!(
            "<p class=\"lsf-hp-clear\">{}{}</p>",
            dot("ok"),
            if check["state"] == "pending" {
                "No problems so far."
            } else {
                "No problems."
            }
        ));
    } else {
        out.push_str(
            "<p class=\"lsf-muted lsf-h-note\">What the app, the store data and the scenarios get wrong. \
             Each one opens the checkout where it shows.</p>",
        );
    }
    out.push_str("<div class=\"lsf-hp-groups\" data-problem-groups>");
    for (severity, title) in [("error", "Errors"), ("warning", "Warnings")] {
        let rows: Vec<String> = problems
            .iter()
            .filter(|problem| problem["severity"] == severity)
            .map(|problem| problem_html(problem, app_dir(home)))
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "<div class=\"lsf-hp-group is-{severity}\"><h3>{title} <span class=\"lsf-h-n\">{}</span></h3>\
             <ul class=\"lsf-hp-rows\">{}</ul>",
            rows.len(),
            rows[..rows.len().min(SHOWN_PROBLEMS)].concat()
        ));
        if rows.len() > SHOWN_PROBLEMS {
            out.push_str(&format!(
                "<details class=\"lsf-hp-more\"><summary>Show {} more</summary><ul class=\"lsf-hp-rows\">{}</ul></details>",
                rows.len() - SHOWN_PROBLEMS,
                rows[SHOWN_PROBLEMS..].concat()
            ));
        }
        out.push_str("</div>");
    }
    out.push_str("</div></section>");
    out
}

/// What the bar at the top of the home page says of the problems, and leads to them: updated by
/// the page when the check answers or a bundle does not load (`home.js`).
fn problem_pill(home: &Json) -> String {
    let errors = home["counts"]["errors"].as_u64().unwrap_or(0) as usize;
    let warnings = home["counts"]["warnings"].as_u64().unwrap_or(0) as usize;
    let mut counts = Vec::new();
    if errors > 0 {
        counts.push(count(errors, "error", "errors"));
    }
    if warnings > 0 {
        counts.push(count(warnings, "warning", "warnings"));
    }
    let (status, text) = match (errors, warnings) {
        (0, 0) if home["check"]["state"] == "pending" => ("ok", "No problems so far".to_string()),
        (0, 0) => ("ok", "No problems".to_string()),
        (0, _) => ("warning", counts.join(" · ")),
        _ => ("error", counts.join(" · ")),
    };
    format!(
        "<a class=\"lsf-h-pill is-{status}\" href=\"#problems\" data-tab-link=\"problems\" data-problem-pill>{}<span>{}</span></a>",
        dot(status),
        esc(&text)
    )
}

/// The scenarios with problems first, then by name.
fn sorted_scenarios(home: &Json) -> Vec<&Json> {
    let mut scenarios: Vec<&Json> = home["scenarios"].as_array().into_iter().flatten().collect();
    let rank = |scenario: &Json| match text_of(&scenario["status"]) {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    };
    scenarios.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| text_of(&a["name"]).cmp(text_of(&b["name"])))
    });
    scenarios
}

/// Links to the rows of extensions, as small chips.
fn extension_chips(handles: &[&str], prefix: &str) -> String {
    handles
        .iter()
        .map(|handle| {
            format!(
                "<a class=\"lsf-h-chip\" href=\"#{}\">{}</a>",
                esc(&anchor(prefix, handle)),
                esc(handle)
            )
        })
        .collect()
}

/// The scenarios by the part of their title before `: ` ("Discount: buy 2, get 1"), when two or
/// more share it, in the order of the names of the groups; the others first, under
/// "Checkouts". Every scenario in one group when none shares a part.
fn scenario_groups<'a>(scenarios: &[&'a Json]) -> Vec<(String, Vec<&'a Json>)> {
    let prefix = |scenario: &Json| {
        text_of(&scenario["title"])
            .split_once(": ")
            .map(|(prefix, _)| prefix.trim().to_string())
            .filter(|prefix| !prefix.is_empty())
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    for scenario in scenarios {
        if let Some(prefix) = prefix(scenario) {
            *counts.entry(prefix).or_default() += 1;
        }
    }
    let mut groups: Vec<(String, Vec<&Json>)> = Vec::new();
    let mut others = Vec::new();
    for scenario in scenarios {
        match prefix(scenario).filter(|prefix| counts[prefix] > 1) {
            Some(prefix) => match groups.iter_mut().find(|(name, _)| *name == prefix) {
                Some((_, list)) => list.push(scenario),
                None => groups.push((prefix, vec![scenario])),
            },
            None => others.push(*scenario),
        }
    }
    groups.sort_by_key(|(name, _)| name.to_lowercase());
    if !others.is_empty() {
        groups.insert(0, ("Checkouts".to_string(), others));
    }
    groups
}

/// `text` with its first letter in capitals: "legacy API" → "Legacy API".
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What the menu of the groups and the heading of a group say of its scenarios: the worst of
/// their dots, and how many have problems.
fn group_status(scenarios: &[&Json]) -> (&'static str, usize) {
    let rank = |status: &str| match status {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    };
    let worst = scenarios
        .iter()
        .map(|scenario| rank(text_of(&scenario["status"])))
        .min()
        .unwrap_or(2);
    let troubled = scenarios
        .iter()
        .filter(|scenario| rank(text_of(&scenario["status"])) < 2)
        .count();
    (["error", "warning", "ok"][worst], troubled)
}

/// A group of scenarios: its heading (its name, how many, how many have problems) and its rows.
fn scenario_group_html(name: &str, scenarios: &[&Json], dir: &str) -> String {
    let (worst, troubled) = group_status(scenarios);
    let rows: String = scenarios
        .iter()
        .map(|scenario| scenario_html(scenario, dir, Some(name)))
        .collect();
    format!(
        "<section class=\"lsf-h-group\" data-group=\"{}\" data-worst=\"{worst}\">\
         <h3 class=\"lsf-h-group-head\"><span data-group-dot>{}</span><span class=\"lsf-h-group-name\">{}</span>\
         <span class=\"lsf-h-n\">{}</span><span class=\"lsf-h-group-problems\" data-group-problems>{}</span></h3>\
         <ul class=\"lsf-h-list\" data-list=\"scenarios\">{rows}</ul></section>",
        esc(name),
        dot(worst),
        esc(name),
        count(scenarios.len(), "scenario", "scenarios"),
        match troubled {
            0 => String::new(),
            n => format!("{n} with problems"),
        },
    )
}

/// `scenario` as a row: its title (without the part its group says), what it is about, and the
/// way to open it; the rest under it, when it opens.
fn scenario_html(scenario: &Json, dir: &str, group: Option<&str>) -> String {
    let name = text_of(&scenario["name"]);
    let title = text_of(&scenario["title"]);
    let short = group
        .and_then(|group| title.strip_prefix(group))
        .and_then(|rest| rest.strip_prefix(": "))
        .map(capitalized)
        .unwrap_or_else(|| title.to_string());
    let status = text_of(&scenario["status"]);
    let functions = strings(&scenario["functions"]);
    let ui = strings(&scenario["uiExtensions"]);
    let problems = strings(&scenario["problems"]);
    let failing = strings(&scenario["failing"]);
    let description = scenario["description"].as_str().unwrap_or_default();

    let uses = if scenario["allExtensions"] == true {
        "every extension of the app".to_string()
    } else if functions.is_empty() && ui.is_empty() {
        "no extension".to_string()
    } else {
        let mut parts = Vec::new();
        if !functions.is_empty() {
            parts.push(count(functions.len(), "Function", "Functions"));
        }
        if !ui.is_empty() {
            parts.push(count(ui.len(), "UI extension", "UI extensions"));
        }
        parts.join(" and ")
    };
    let mut kinds = Vec::new();
    if !functions.is_empty() {
        kinds.push("functions");
    }
    if !ui.is_empty() {
        kinds.push("ui");
    }
    let search = [name, title, text_of(&scenario["file"]), description]
        .into_iter()
        .chain(functions.iter().copied())
        .chain(ui.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    // The second line: what is wrong, or the facts and the description.
    let meta = match (problems.first(), failing.is_empty()) {
        (Some(problem), _) => format!(
            "<span class=\"lsf-h-meta is-error\" title=\"{}\">{}</span>",
            esc(problem),
            message_html(problem, dir)
        ),
        (None, false) => format!(
            "<span class=\"lsf-h-meta is-error\">Fails: {}</span>",
            esc(&failing.join(", "))
        ),
        // What it is about: its description, else the cart and the buyer in a few words.
        (None, true) => {
            let text = if description.is_empty() {
                strings(&scenario["facts"]).join(" · ")
            } else {
                description.to_string()
            };
            format!(
                "<span class=\"lsf-h-meta\" title=\"{}\">{}</span>",
                esc(&text),
                esc(&text)
            )
        }
    };

    let mut body = String::new();
    if !description.is_empty() {
        body.push_str(&format!("<p class=\"lsf-h-text\">{}</p>", esc(description)));
    }
    body.push_str(&format!(
        "<p class=\"lsf-muted lsf-h-file\">Runs {} · {} · {}</p>",
        esc(&uses),
        code(name),
        esc(text_of(&scenario["file"]))
    ));
    for problem in &problems {
        body.push_str(&format!(
            "<pre class=\"lsf-home-problem\" title=\"{}\">{}</pre>",
            esc(problem),
            message_html(problem, dir)
        ));
    }
    if !failing.is_empty() {
        body.push_str(&format!(
            "<p class=\"lsf-h-fails\">Fails when it opens: {}</p>",
            extension_chips(&failing, "function")
        ));
    }
    let facts: String = scenario["summary"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "<div><dt>{}</dt><dd>{}</dd></div>",
                esc(text_of(&row["label"])),
                esc(text_of(&row["text"]))
            )
        })
        .collect();
    if !facts.is_empty() {
        body.push_str(&format!("<dl class=\"lsf-h-facts\">{facts}</dl>"));
    }
    if scenario["allExtensions"] == true {
        body.push_str("<p class=\"lsf-muted\">Runs every extension of the app.</p>");
    } else {
        for (label, handles, prefix) in [("Functions", &functions, "function"), ("UI", &ui, "ui")] {
            if !handles.is_empty() {
                body.push_str(&format!(
                    "<div class=\"lsf-h-uses-list\"><span class=\"lsf-h-label\">{label}</span>{}</div>",
                    extension_chips(handles, prefix)
                ));
            }
        }
    }
    if let Some(all) = scenario["openAllTargets"].as_str() {
        body.push_str(&format!(
            "<p><a class=\"lsf-link\" href=\"{}\">Open it with every extension at every target it declares</a></p>",
            esc(all)
        ));
    }

    let actions = match scenario["open"].as_str() {
        Some(open) => format!(
            "<a class=\"lsf-h-btn\" href=\"{}\">Open{}</a>",
            esc(open),
            hidden(&format!(" {title}")),
        ),
        None => "<span class=\"lsf-chip is-error\">has errors</span>".to_string(),
    };
    format!(
        "<li class=\"lsf-h-row lsf-h-scenario\" id=\"{}\" data-scenario=\"{}\" data-status=\"{status}\" \
         data-uses=\"{}\" data-ui-extensions=\"{}\" data-search=\"{}\"><details class=\"lsf-h-details\"><summary>{}\
         <span class=\"lsf-h-main\"><span class=\"lsf-h-title\"><strong title=\"{}\">{}</strong></span>{meta}</span>\
         </summary><div class=\"lsf-h-body\">{body}</div></details>\
         <div class=\"lsf-h-actions\">{actions}</div></li>",
        esc(text_of(&scenario["anchor"])),
        esc(name),
        kinds.join(" "),
        esc(&ui.join(" ")),
        esc(&search),
        dot(status),
        esc(title),
        esc(&short),
    )
}

/// The targets of an extension as chips: the first two, and how many more.
fn target_chips(targets: &[&str]) -> String {
    let mut chips: String = targets
        .iter()
        .take(2)
        .map(|target| {
            format!(
                "<span class=\"lsf-h-tchip\" title=\"{0}\">{0}</span>",
                esc(target)
            )
        })
        .collect();
    if targets.len() > 2 {
        chips.push_str(&format!(
            "<span class=\"lsf-h-tchip is-more\">+{}</span>",
            targets.len() - 2
        ));
    }
    chips
}

fn used_by(extension: &Json) -> String {
    match extension["scenarios"].as_u64().unwrap_or(0) {
        0 => "in no scenario".to_string(),
        n => format!("in {}", count(n as usize, "scenario", "scenarios")),
    }
}

fn api_version(extension: &Json) -> String {
    match extension["apiVersion"].as_str() {
        Some(version) if extension["supported"] == true => format!("API {version}"),
        Some(version) => format!("API {version} is not supported"),
        None => "no api_version".to_string(),
    }
}

/// The first problem of an extension, on one line under its name.
fn issue_line(extension: &Json, dir: &str) -> String {
    let failures = extension["failures"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let problems = extension["problems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let first_error = problems
        .iter()
        .find(|problem| problem["severity"] == "error");
    let (severity, text) = match (first_error, failures.first(), problems.first()) {
        (Some(problem), _, _) => ("error", text_of(&problem["message"]).to_string()),
        (None, Some(failure), _) => (
            "error",
            format!(
                "fails in {}: {}",
                count(
                    failure["scenarios"].as_array().map_or(0, Vec::len),
                    "scenario",
                    "scenarios"
                ),
                text_of(&failure["error"])
            ),
        ),
        (None, None, Some(problem)) => ("warning", text_of(&problem["message"]).to_string()),
        // A UI extension whose bundle the page loads keeps the line of what loading it finds:
        // the row does not grow when it is found.
        (None, None, None) if extension["script"].is_string() => {
            return "<span class=\"lsf-h-meta lsf-h-issue\" data-issue></span>".to_string();
        }
        (None, None, None) => return String::new(),
    };
    let others = problems.len() + failures.len() - 1;
    format!(
        "<span class=\"lsf-h-meta is-{severity}\" title=\"{}\" data-issue>{}{}</span>",
        esc(&text),
        message_html(&text, dir),
        if others > 0 {
            format!(" (+{others})")
        } else {
            String::new()
        }
    )
}

fn problem_lines(extension: &Json, dir: &str) -> String {
    let mut out = String::new();
    for problem in extension["problems"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "<li class=\"is-{}\" title=\"{}\">{}</li>",
            esc(text_of(&problem["severity"])),
            esc(text_of(&problem["message"])),
            message_html(text_of(&problem["message"]), dir)
        ));
    }
    for failure in extension["failures"].as_array().into_iter().flatten() {
        let scenarios = strings(&failure["scenarios"]);
        let links: Vec<String> = scenarios
            .iter()
            .map(|name| {
                format!(
                    "<a href=\"#{}\">{}</a>",
                    esc(&anchor("scenario", name)),
                    esc(name)
                )
            })
            .collect();
        out.push_str(&format!(
            "<li class=\"is-error\">{} fails in {}: {}</li>",
            code(text_of(&failure["target"])),
            links.join(", "),
            message_html(text_of(&failure["error"]), dir)
        ));
    }
    if out.is_empty() {
        out
    } else {
        format!("<ul class=\"lsf-h-issues\">{out}</ul>")
    }
}

fn extension_row(
    extension: &Json,
    dir: &str,
    data: &str,
    meta: &[String],
    targets: &[&str],
    body: &str,
) -> String {
    // What the page needs to load the bundle of a checkout UI extension on its own.
    let probe = match extension["script"].as_str() {
        Some(script) => format!(
            " data-probe=\"{}\" data-api=\"{}\" data-targets=\"{}\"",
            esc(script),
            esc(text_of(&extension["apiVersion"])),
            esc(&targets.join(" "))
        ),
        None => String::new(),
    };
    // The checkout where a problem found that way shows.
    let open = match extension["open"].as_str() {
        Some(href) if !probe.is_empty() => format!(
            " data-open=\"{}\" data-open-in=\"{}\"{}",
            esc(href),
            esc(extension["runsIn"].as_str().unwrap_or_default()),
            if only_in_account(extension) {
                " data-open-account"
            } else {
                ""
            }
        ),
        _ => String::new(),
    };
    let handle = text_of(&extension["handle"]);
    let status = text_of(&extension["status"]);
    let name = extension["name"]
        .as_str()
        .map(|name| format!("<span class=\"lsf-h-name\">{}</span>", esc(name)))
        .unwrap_or_default();
    format!(
        "<li class=\"lsf-h-row lsf-h-ext\" id=\"{}\" {data}=\"{}\" data-status=\"{status}\"{probe}{open}>\
         <details class=\"lsf-h-details\"><summary>{}<span class=\"lsf-h-main\">\
         <span class=\"lsf-h-title\"><code class=\"lsf-h-handle\">{}</code>{name}</span>\
         <span class=\"lsf-h-meta\">{}</span>{}</span><span class=\"lsf-h-targets\">{}</span></summary>\
         <div class=\"lsf-h-body\">{body}{}</div></details></li>",
        esc(text_of(&extension["anchor"])),
        esc(handle),
        dot(status),
        esc(handle),
        esc(&meta.join(" · ")),
        issue_line(extension, dir),
        target_chips(targets),
        problem_lines(extension, dir),
    )
}

fn function_html(function: &Json, dir: &str) -> String {
    let mut meta: Vec<String> = strings(&function["apis"])
        .iter()
        .map(|api| format!("{api} API"))
        .collect();
    meta.push(api_version(function));
    if function["built"] != true {
        meta.push("not built".to_string());
    }
    if function["network"] == true {
        meta.push("network access (.fetch)".to_string());
    }
    if function["selected"] != true {
        meta.push("left out by --extension".to_string());
    }
    meta.push(used_by(function));
    let targets_list = function["targets"].as_array().cloned().unwrap_or_default();
    let targets: Vec<&str> = targets_list
        .iter()
        .map(|target| text_of(&target["target"]))
        .collect();
    let rows: String = targets_list
        .iter()
        .map(|target| {
            let mut notes = Vec::new();
            if let Some(api) = target["api"].as_str() {
                notes.push(format!("{api} API"));
            }
            if target["fetch"] == true {
                notes.push("makes an HTTP request".to_string());
            }
            if target["legacy"] == true {
                notes.push("legacy target".to_string());
            }
            if target["unknown"] == true {
                notes.push("unknown target".to_string());
            } else if target["runs"] == false {
                notes.push("not run by the local checkout".to_string());
            }
            format!(
                "<li>{} <span class=\"lsf-muted\">{}</span></li>",
                code(text_of(&target["target"])),
                esc(&notes.join(" · "))
            )
        })
        .collect();
    let mut body = format!("<ul class=\"lsf-h-tlist\">{rows}</ul>");
    if function["built"] != true {
        body.push_str(&match function["buildCommand"].as_str() {
            Some(command) => format!(
                "<p class=\"lsf-muted\">Build it with {} (or <code>shopify app build</code>), or start with <code>--build</code>.</p>",
                code(command)
            ),
            None => "<p class=\"lsf-muted\">Build it with <code>shopify app build</code>.</p>"
                .to_string(),
        });
    }
    extension_row(function, dir, "data-function", &meta, &targets, &body)
}

fn ui_extension_html(extension: &Json, dir: &str) -> String {
    let mut meta = vec![api_version(extension)];
    if extension["built"] != true {
        meta.push("not built".to_string());
    }
    for capability in strings(&extension["capabilities"]) {
        meta.push(capability.replace('_', " "));
    }
    if extension["selected"] != true {
        meta.push("left out by --extension".to_string());
    }
    if only_in_account(extension) {
        meta.push("in the customer account".to_string());
    } else {
        meta.push(used_by(extension));
    }
    let targets_list = extension["targets"].as_array().cloned().unwrap_or_default();
    let targets: Vec<&str> = targets_list
        .iter()
        .map(|target| text_of(&target["target"]))
        .collect();
    let rows: String = targets_list
        .iter()
        .map(|target| {
            let place = match target["place"].as_str() {
                Some(place) => format!("<span class=\"lsf-muted\">{}</span>", esc(place)),
                None => "<span class=\"lsf-chip is-error\">no place on the page</span>".to_string(),
            };
            format!(
                "<li data-target=\"{}\">{} {place}</li>",
                esc(text_of(&target["target"])),
                code(text_of(&target["target"]))
            )
        })
        .collect();
    extension_row(
        extension,
        dir,
        "data-ui-extension",
        &meta,
        &targets,
        &format!("<ul class=\"lsf-h-tlist\">{rows}</ul>"),
    )
}

/// The short facts about a customer: their company, tags, orders, addresses and country.
fn customer_facts(customer: &Json) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    if let Some(company) = customer["company"].as_str() {
        facts.push(("b2b", format!("B2B · {company}")));
        let locations = customer["locationsCount"].as_u64().unwrap_or(0) as usize;
        if locations > 1 {
            facts.push(("", count(locations, "location", "locations")));
        }
    }
    for tag in customer["tags"].as_array().into_iter().flatten() {
        facts.push(("tag", text_of(tag).to_string()));
    }
    let orders = customer["ordersCount"].as_u64().unwrap_or(0) as usize;
    facts.push((
        "",
        if orders == 0 {
            "No orders".to_string()
        } else {
            count(orders, "order", "orders")
        },
    ));
    let addresses = customer["addressesCount"].as_u64().unwrap_or(0) as usize;
    if addresses > 0 {
        facts.push(("", count(addresses, "address", "addresses")));
    }
    if let Some(country) = customer["countryCode"].as_str() {
        facts.push(("", country.to_string()));
    }
    facts
}

fn customer_html(customer: &Json, account: &str) -> String {
    let badges: String = customer_facts(customer)
        .iter()
        .map(|(kind, text)| {
            let class = if kind.is_empty() {
                "lsf-badge".to_string()
            } else {
                format!("lsf-badge is-{kind}")
            };
            format!("<span class=\"{class}\">{}</span>", esc(text))
        })
        .collect();
    let name = text_of(&customer["name"]);
    let email = text_of(&customer["email"]);
    let initials: String = name
        .split(|c: char| c.is_whitespace() || c == '@' || c == '.')
        .filter(|word| !word.is_empty())
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    // Their customer account, logged in as them.
    let account = format!(
        "<a class=\"lsf-h-btn\" href=\"{}\" data-account-of=\"{}\">Account</a>",
        esc(&super::account::login_link(email, account)),
        esc(email)
    );
    format!(
        "<li class=\"lsf-h-person\" data-customer=\"{}\"><div class=\"lsf-h-person-head\">\
         <span class=\"lsf-avatar\" aria-hidden=\"true\">{}</span><div class=\"lsf-h-person-who\">\
         <strong>{}</strong><span class=\"lsf-muted\">{}</span></div></div>\
         <span class=\"lsf-badges\">{badges}</span>\
         <div class=\"lsf-h-actions\"><a class=\"lsf-h-btn is-primary\" href=\"{}\">Checkout as {}</a>{account}</div></li>",
        esc(email),
        esc(if initials.is_empty() { "?" } else { &initials }),
        esc(name),
        esc(email),
        esc(text_of(&customer["open"])),
        esc(name.split_whitespace().next().unwrap_or(name)),
    )
}

/// A list of extensions of the Extensions tab: its rows, and what it says of them.
struct ExtensionList {
    extensions: Vec<Json>,
    /// Its tab: `functions`, `checkout-ui-extensions`, `customer-account-ui-extensions`.
    tab: &'static str,
    /// Its name in the page: `functions`, `ui-extensions`, `account-extensions`.
    section: &'static str,
    heading: &'static str,
    row: fn(&Json, &str) -> String,
    empty: &'static str,
    about: String,
}

/// A tab of the home page: its id, its label, and its count.
fn tab_html(id: &str, label: &str, count: String, selected: bool) -> String {
    format!(
        "<a class=\"lsf-h-tab\" role=\"tab\" id=\"tab-{id}\" href=\"#{id}\" data-tab=\"{id}\" aria-controls=\"panel-{id}\" \
         aria-selected=\"{selected}\">{label}{count}</a>"
    )
}

/// The home page, drawn from what [`build`] says: a bar with the ways in and the problems, then
/// four tabs (the scenarios, the extensions, the customers, the problems). Without the script,
/// the first tab shows. `check_now` is the page that runs the check of the Functions, for a
/// browser without the script.
pub fn render(home: &Json, check_now: &str) -> String {
    if let Some(problem) = home["error"].as_str() {
        let body = format!(
            "<header class=\"lsf-header\"><span class=\"lsf-brand\">Checkout</span></header>\
             <main class=\"lsf-home-main\">{}</main>",
            banner("critical", problem)
        );
        return document("Checkout", &body);
    }
    let app = &home["app"];
    let title = app["name"].as_str().unwrap_or("Shopify app");
    let links = &home["links"];
    let dir = app_dir(home);
    let scenarios = sorted_scenarios(home);
    let folder = home["dataFolder"]
        .as_str()
        .unwrap_or("the `checkouts` folder of the store data");
    let customers = home["customers"].as_array().cloned().unwrap_or_default();
    let account_link = text_of(&links["account"]);

    // The extensions, in three lists: an extension with targets in both the checkout and the
    // customer account is in both; its second row is a copy that does not load the bundle again.
    let all_ui = home["uiExtensions"].as_array().cloned().unwrap_or_default();
    let checkout_ui: Vec<Json> = all_ui
        .iter()
        .filter(|extension| !only_in_account(extension))
        .cloned()
        .collect();
    let account_ui: Vec<Json> = all_ui
        .iter()
        .filter(|extension| strings(&extension["surfaces"]).contains(&"customer-account"))
        .map(|extension| {
            if only_in_account(extension) {
                return extension.clone();
            }
            let mut copy = extension.clone();
            copy["anchor"] = json!(anchor("account-ui", text_of(&extension["handle"])));
            copy["script"] = Json::Null;
            copy
        })
        .collect();
    let lists = [
        ExtensionList {
            extensions: home["functions"].as_array().cloned().unwrap_or_default(),
            tab: "functions",
            section: "functions",
            heading: "Functions",
            row: function_html,
            empty: "The app has no Function.",
            about: "What each Function is, its targets, and the scenarios that run it.".to_string(),
        },
        ExtensionList {
            extensions: checkout_ui,
            tab: "checkout-ui-extensions",
            section: "ui-extensions",
            heading: "Checkout UI extensions",
            row: ui_extension_html,
            empty: "The app has no checkout UI extension.",
            about: format!(
                "Where each one renders in the checkout and the thank-you page. \
                 <a class=\"lsf-link\" href=\"{}\">Open a checkout where each renders at every target it declares</a>.",
                esc(text_of(&links["allTargets"]))
            ),
        },
        ExtensionList {
            extensions: account_ui,
            tab: "customer-account-ui-extensions",
            section: "account-extensions",
            heading: "Customer account UI extensions",
            row: ui_extension_html,
            empty: "The app has no customer account UI extension.",
            about: format!(
                "Where each one renders in the customer account. <a class=\"lsf-link\" href=\"{}\">Open the customer account</a> \
                 as one of the customers, and add <code>?targets=all</code> to list where each target is.",
                esc(account_link)
            ),
        },
    ];
    let n = |count: usize| format!("<span class=\"lsf-h-n\">{count}</span>");

    // The bar: the app, the problems, the ways in.
    let mut body = format!(
        "<header class=\"lsf-h-top\"><div class=\"lsf-h-top-in\"><div class=\"lsf-h-app\">\
         <span class=\"lsf-brand\">{}</span><span class=\"lsf-muted\" title=\"{}\">{}</span></div>\
         <div class=\"lsf-h-top-actions\" data-section=\"open\">{}\
         <details class=\"lsf-h-more\"><summary class=\"lsf-h-btn\" aria-label=\"More ways to open a checkout\">More</summary>\
         <div class=\"lsf-h-menu\"><a href=\"{}\">Every extension target</a><a href=\"{}\">Shop look</a></div></details>\
         <a class=\"lsf-h-btn\" href=\"{}\" data-link=\"account\">Customer account</a>\
         <a class=\"lsf-h-btn is-primary\" href=\"{}\">Open the checkout</a></div></div>\
         <nav class=\"lsf-h-tabs\" role=\"tablist\" aria-label=\"Home\">{}{}{}{}</nav></header>",
        esc(title),
        esc(text_of(&home["data"])),
        esc(text_of(&home["data"])),
        problem_pill(home),
        esc(text_of(&links["allTargets"])),
        esc(text_of(&links["shop"])),
        esc(account_link),
        esc(text_of(&links["checkout"])),
        tab_html("scenarios", "Scenarios", n(scenarios.len()), true),
        lists
            .iter()
            .map(|list| tab_html(list.tab, list.heading, n(list.extensions.len()), false))
            .collect::<String>(),
        tab_html("customers", "Customers", n(customers.len()), false),
        tab_html(
            "problems",
            "Problems",
            format!(
                "<span class=\"lsf-h-badge\" data-problem-tab-count{}>{}</span>",
                if home["problems"]
                    .as_array()
                    .is_some_and(|list| !list.is_empty())
                {
                    ""
                } else {
                    " hidden"
                },
                home["problems"].as_array().map_or(0, Vec::len)
            ),
            false
        ),
    );
    body.push_str("<main class=\"lsf-home-main\">");

    // Scenarios: the groups on the left, the scenarios on the right.
    body.push_str(
        "<section class=\"lsf-h-panel\" id=\"panel-scenarios\" role=\"tabpanel\" aria-labelledby=\"tab-scenarios\" data-panel=\"scenarios\">",
    );
    body.push_str(
        "<span id=\"scenarios\" class=\"lsf-h-anchor\"></span><div data-section=\"scenarios\">",
    );
    if scenarios.is_empty() {
        body.push_str(&format!(
            "<div class=\"lsf-h-card\"><p class=\"lsf-muted lsf-h-empty\">None yet. A scenario is a file in {}: a cart, a buyer, what they entered. \
             <code>lsf checkout init --app {}</code> writes a set to start from.</p></div>",
            code(folder),
            esc(text_of(&app["directory"])),
        ));
    } else {
        let groups = scenario_groups(&scenarios);
        let grouped = groups.len() > 1;
        // The menu: every scenario, then each group.
        body.push_str("<div class=\"lsf-h-split\"><aside class=\"lsf-h-side\">\
             <input class=\"lsf-h-search\" type=\"search\" placeholder=\"Search scenarios\" aria-label=\"Search the scenarios\" \
             data-filter-query disabled>");
        let (worst, _) = group_status(&scenarios);
        body.push_str(&format!(
            "<nav class=\"lsf-h-groupnav\" aria-label=\"Groups of scenarios\">\
             <button type=\"button\" data-group-nav=\"\" aria-current=\"true\" disabled><span data-group-dot>{}</span>\
             <span>All</span><span class=\"lsf-h-n\">{}</span></button>",
            dot(worst),
            scenarios.len()
        ));
        if grouped {
            body.push_str("<hr>");
            for (name, list) in &groups {
                let (worst, _) = group_status(list);
                body.push_str(&format!(
                    "<button type=\"button\" data-group-nav=\"{}\" disabled><span data-group-dot>{}</span><span>{}</span>\
                     <span class=\"lsf-h-n\">{}</span></button>",
                    esc(name),
                    dot(worst),
                    esc(name),
                    list.len()
                ));
            }
        }
        body.push_str("</nav></aside>");
        // The list: its heading, the filter, the groups.
        let troubled = scenarios
            .iter()
            .filter(|scenario| matches!(text_of(&scenario["status"]), "error" | "warning"))
            .count();
        body.push_str(&format!(
            "<div class=\"lsf-h-card lsf-h-scenarios\"><div class=\"lsf-h-cardhead\"><div>\
             <h2 data-list-title>All scenarios</h2><p class=\"lsf-muted\" data-list-meta>{}{} · in <code title=\"{}\">{}</code></p></div>\
             <div class=\"lsf-h-filter\" data-filter><div class=\"lsf-h-seg\" role=\"group\" aria-label=\"Show\">\
             <button type=\"button\" data-filter-chip=\"all\" aria-pressed=\"true\" disabled>All</button>\
             <button type=\"button\" data-filter-chip=\"problems\" aria-pressed=\"false\" disabled>With problems</button>\
             <button type=\"button\" data-filter-chip=\"functions\" aria-pressed=\"false\" disabled>Functions</button>\
             <button type=\"button\" data-filter-chip=\"ui\" aria-pressed=\"false\" disabled>UI</button></div>\
             <p class=\"lsf-h-shown\"><span class=\"lsf-h-shown-size\" aria-hidden=\"true\">{total} of {total} scenarios shown</span>\
             <span data-filter-count aria-live=\"polite\">{}</span></p></div></div>",
            count(scenarios.len(), "scenario", "scenarios"),
            match troubled {
                0 => String::new(),
                n => format!(" · {n} with problems"),
            },
            esc(folder),
            esc(&shorten(folder, dir)),
            count(scenarios.len(), "scenario", "scenarios"),
            total = scenarios.len(),
        ));
        if grouped {
            for (name, list) in &groups {
                body.push_str(&scenario_group_html(name, list, dir));
            }
        } else {
            body.push_str("<ul class=\"lsf-h-list\" data-list=\"scenarios\">");
            for scenario in &scenarios {
                body.push_str(&scenario_html(scenario, dir, None));
            }
            body.push_str("</ul>");
        }
        body.push_str(
            "<p class=\"lsf-muted lsf-h-empty\" data-filter-empty hidden>No scenario matches.</p></div></div>",
        );
    }
    body.push_str("</div></section>");

    // The extensions: a tab for the Functions, one for the checkout UI extensions, one for the
    // customer account UI extensions. The others of the app close the last one.
    let others = home["otherExtensions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (index, list) in lists.iter().enumerate() {
        let with_problems = list
            .extensions
            .iter()
            .filter(|extension| matches!(text_of(&extension["status"]), "error" | "warning"))
            .count();
        body.push_str(&format!(
            "<section class=\"lsf-h-panel\" id=\"panel-{tab}\" role=\"tabpanel\" aria-labelledby=\"tab-{tab}\" data-panel=\"{tab}\" hidden>\
             <span id=\"{tab}\" class=\"lsf-h-anchor\"></span><div class=\"lsf-h-card\" data-section=\"{}\">\
             <div class=\"lsf-h-cardhead\"><div><h2>{}</h2><p class=\"lsf-muted\">{}{}</p></div></div>",
            list.section,
            list.heading,
            list.about,
            match with_problems {
                0 => String::new(),
                n => format!(" <span class=\"lsf-h-group-problems\">{n} with problems</span>"),
            },
            tab = list.tab,
        ));
        if list.extensions.is_empty() {
            body.push_str(&format!(
                "<p class=\"lsf-muted lsf-h-empty\">{}</p>",
                list.empty
            ));
        } else {
            body.push_str("<ul class=\"lsf-h-list lsf-h-extlist\">");
            for extension in &list.extensions {
                body.push_str(&(list.row)(extension, dir));
            }
            body.push_str("</ul>");
        }
        if index == lists.len() - 1 && !others.is_empty() {
            let items: String = others
                .iter()
                .map(|other| {
                    format!(
                        "<li>{} <span class=\"lsf-muted\">{}</span></li>",
                        code(text_of(&other["handle"])),
                        esc(text_of(&other["type"]))
                    )
                })
                .collect();
            body.push_str(&format!(
                "<div class=\"lsf-h-others\" data-section=\"other\"><h3>Other extensions <span class=\"lsf-h-n\">{}</span></h3>\
                 <p class=\"lsf-muted lsf-h-note\">Not part of the checkout or the customer account, so not used here.</p>\
                 <ul class=\"lsf-h-plain\">{items}</ul></div>",
                others.len()
            ));
        }
        body.push_str("</div></section>");
    }

    // Customers.
    body.push_str(
        "<section class=\"lsf-h-panel\" id=\"panel-customers\" role=\"tabpanel\" aria-labelledby=\"tab-customers\" data-panel=\"customers\" hidden>\
         <span id=\"customers\" class=\"lsf-h-anchor\"></span><div class=\"lsf-h-card\" data-section=\"customers\"><div class=\"lsf-h-cardhead\"><div><h2>Customers</h2>",
    );
    if customers.is_empty() {
        body.push_str(
            "</div></div><p class=\"lsf-muted lsf-h-empty\">The store data has no customers: add some to <code>customers.json</code> to log the checkout in as them.</p>",
        );
    } else {
        body.push_str(
            "<p class=\"lsf-muted\">The customers of the store data: open a checkout logged in as one of them, or their customer account.</p></div></div>\
             <ul class=\"lsf-h-people\">",
        );
        for customer in &customers {
            body.push_str(&customer_html(customer, account_link));
        }
        body.push_str("</ul>");
    }
    body.push_str("</div></section>");

    // Problems.
    body.push_str(&format!(
        "<section class=\"lsf-h-panel\" id=\"panel-problems\" role=\"tabpanel\" aria-labelledby=\"tab-problems\" data-panel=\"problems\" hidden>{}</section>",
        problems_html(home, check_now)
    ));
    body.push_str("</main>");
    document_with(
        &format!("{title} · checkout"),
        &body,
        &format!("<script type=\"module\" src=\"{ASSETS}/home.js\"></script>"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_targets_the_checkout_does_not_know_are_unknown() {
        assert!(is_listed_target(
            "cart.pickup-point-delivery-option-generator.fetch"
        ));
        assert!(is_listed_target(
            "cart.fulfillment-constraints.generate.run"
        ));
        assert!(!is_listed_target("cart.validations.generate.later"));
    }

    #[test]
    fn scenarios_are_grouped_by_what_their_titles_start_with() {
        let scenario = |name: &str, title: &str| json!({"name": name, "title": title});
        let list = [
            scenario("a", "Discount: buy 2, get 1"),
            scenario("b", "Discount: free shipping"),
            scenario("c", "A logged-in customer"),
            scenario("d", "Network: alone"),
            scenario("e", "Cart transform: expand"),
            scenario("f", "Cart transform: merge"),
        ];
        let refs: Vec<&Json> = list.iter().collect();
        let groups: Vec<(String, Vec<&str>)> = scenario_groups(&refs)
            .into_iter()
            .map(|(name, scenarios)| {
                (
                    name,
                    scenarios.iter().map(|s| text_of(&s["name"])).collect(),
                )
            })
            .collect();
        assert_eq!(
            groups,
            [
                ("Checkouts".to_string(), vec!["c", "d"]),
                ("Cart transform".to_string(), vec!["e", "f"]),
                ("Discount".to_string(), vec!["a", "b"]),
            ]
        );
        // A row in its group drops what the group says.
        let row = scenario_html(
            &json!({"name": "a", "title": "Discount: buy 2, get 1", "status": "ok", "anchor": "scenario-a"}),
            "",
            Some("Discount"),
        );
        assert!(row.contains(">Buy 2, get 1</strong>"), "{row}");
        assert!(row.contains("title=\"Discount: buy 2, get 1\""), "{row}");
        // Without a part in common, one group.
        let alone = [scenario("x", "Tees"), scenario("y", "Abroad")];
        assert_eq!(scenario_groups(&alone.iter().collect::<Vec<_>>()).len(), 1);
    }

    #[test]
    fn the_anchors_are_ids() {
        assert_eq!(
            anchor("scenario", "fn-broken-panic"),
            "scenario-fn-broken-panic"
        );
        assert_eq!(
            anchor("scenario", "fn-broken panic.é"),
            "scenario-fn-broken_20panic_2e_c3_a9"
        );
        assert_eq!(anchor("ui", "Gift"), "ui-_47ift");
    }

    #[test]
    fn two_names_never_share_an_anchor() {
        let names = ["a b", "a-b", "a_b", "a_20b", "a.b", "A-b", "a--b"];
        let ids: std::collections::HashSet<String> =
            names.iter().map(|name| anchor("scenario", name)).collect();
        assert_eq!(ids.len(), names.len(), "{ids:?}");
        // An id is only made of what an id and a URL fragment take as they are.
        for id in &ids {
            assert!(
                id.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b"-_".contains(&byte)),
                "{id}"
            );
        }
    }

    #[test]
    fn a_message_shows_its_code_and_paths_inside_the_app_relative_to_it() {
        let dir = "/Users/someone/app";
        assert_eq!(
            message_html("`purchase.cart-transform.run` is a legacy target", dir),
            "<code>purchase.cart-transform.run</code> is a legacy target"
        );
        assert_eq!(
            message_html(
                "no Wasm module at /Users/someone/app/extensions/x/dist/function.wasm: build the Function (`shopify app build`)",
                dir
            ),
            "no Wasm module at extensions/x/dist/function.wasm: build the Function (<code>shopify app build</code>)"
        );
        // Escaped before the code is drawn: nothing the message says becomes HTML.
        assert_eq!(
            message_html("`<script>` & <b>`x`", dir),
            "<code>&lt;script&gt;</code> &amp; &lt;b&gt;<code>x</code>"
        );
        // A backtick without its pair stays a backtick.
        assert_eq!(message_html("a `b` c `d", dir), "a <code>b</code> c `d");
        // A path outside the app stays whole.
        assert_eq!(
            message_html("/Users/someone/other/x", dir),
            "/Users/someone/other/x"
        );
    }
}

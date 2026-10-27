//! The pages of the checkout that are not a checkout: the home page, which lists the extensions
//! of the app and the checkout scenarios, and the notices that say why a scenario did not open.
//! They are plain HTML in the look of the checkout (`checkout.css`), without a script, so that
//! they work offline.
//!
//! The home page is built as JSON first (`GET …?format=json` answers it), and the HTML is drawn
//! from that JSON, so that the two cannot say different things.

use lsf_checkout::app::{App, FunctionExtension, Selection, Severity, TargetKind, UiExtension};
use lsf_checkout::placement::placement;
use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::model::ScenarioLook;
use lsf_core::store::{CheckoutScenario, CheckoutSettings};
use lsf_core::{Session, Store};
use lsf_liquid::filters::escape_html;
use serde_json::{Value as Json, json};

use super::reply::{Reply, cache};
use super::{Incoming, ServerState};
use crate::checkout::CheckoutOptions;

/// Where the files of the checkout page are served from.
pub const ASSETS: &str = "/__lsf/checkout/assets";

fn esc(text: &str) -> String {
    escape_html(text)
}

/// A page of the checkout's look around `body`.
pub fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light\"><title>{}</title>\
         <link rel=\"stylesheet\" href=\"{ASSETS}/checkout.css\"></head>\
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

/// The problems the loader found in an extension, as `{severity, message}`.
fn problems_of(app: &App, handle: &str) -> Vec<Json> {
    app.diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.extension.as_deref() == Some(handle))
        .map(|diagnostic| {
            json!({
                "severity": match diagnostic.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                },
                "message": diagnostic.message,
            })
        })
        .collect()
}

fn function_json(app: &App, selection: &Selection, function: &FunctionExtension) -> Json {
    let targets: Vec<Json> = function
        .targets
        .iter()
        .map(|target| {
            let spec = target.spec();
            json!({
                "target": target.target,
                "api": spec.map(|spec| spec.api.to_string()),
                "runs": target.kind != TargetKind::Unsupported,
                "fetch": matches!(
                    target.kind,
                    TargetKind::LinesDiscountsFetch
                        | TargetKind::DeliveryDiscountsFetch
                        | TargetKind::ValidationFetch
                ),
                "legacy": spec.is_some_and(|spec| spec.legacy),
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
    json!({
        "handle": function.handle,
        "name": function.name,
        "apiVersion": function.api_version,
        "supported": function.version.is_some(),
        "apis": apis,
        "targets": targets,
        // Whether the Function can make an HTTP request (it has a `.fetch` target).
        "network": targets.iter().any(|target| target["fetch"] == true),
        "built": function.built(),
        "buildCommand": function.build_command,
        "selected": selection.contains(&function.handle),
        "problems": problems_of(app, &function.handle),
    })
}

fn ui_extension_json(app: &App, selection: &Selection, extension: &UiExtension) -> Json {
    let targets: Vec<Json> = extension
        .targets
        .iter()
        .map(|target| json!({"target": target.target, "place": placement(&target.target)}))
        .collect();
    let capabilities: Vec<&str> = extension
        .capabilities
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| value.as_bool() == Some(true))
        .map(|(name, _)| name.as_str())
        .collect();
    json!({
        "handle": extension.handle,
        "name": extension.name,
        "apiVersion": extension.api_version,
        "supported": extension.version.is_some(),
        "targets": targets,
        "capabilities": capabilities,
        "built": extension.built(),
        "selected": selection.contains(&extension.handle),
        "problems": problems_of(app, &extension.handle),
    })
}

fn money(cents: i64, currency: &str) -> String {
    format!(
        "{}{}.{:02} {currency}",
        if cents < 0 { "-" } else { "" },
        cents.abs() / 100,
        cents.abs() % 100
    )
}

fn names(settings: &CheckoutSettings, store: &Store) -> (String, String) {
    let currency = store.shop.currency.as_str();
    let rates = settings
        .shipping_rates
        .iter()
        .map(|rate| format!("{} {}", rate.title, money(rate.price, currency)))
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

/// What a scenario is, in a few `(label, text)` lines: the cart, the buyer, the country and
/// language, what was entered, the discounts, the settings it overrides, the look and the
/// extensions.
fn summarize(store: &Store, scenario: &CheckoutScenario) -> Vec<(&'static str, String)> {
    let mut session = Session::initial(store);
    crate::server::apply_session_input(
        store,
        &scenario.session,
        &mut session,
        &mut Diagnostics::new(),
        &scenario.file,
    );
    let mut lines = Vec::new();

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
        None => "guest".to_string(),
        Some(customer) => {
            let mut text = format!("customer {}", customer.email);
            if let Some(company) = customer.company {
                let company = &store.companies[company];
                text.push_str(&format!(" · B2B, {}", company.name));
                if let Some(location) = customer.current_location(session.company_location, store) {
                    text.push_str(&format!(", {}", company.locations[location].name));
                }
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

    let mut discounts = Vec::new();
    for discount in &scenario.checkout.discounts {
        discounts.push(match &discount.code {
            Some(code) => format!("{} (code {code})", discount.title),
            None => format!("{} (automatic)", discount.title),
        });
    }
    if !entered.discount_codes.is_empty() {
        discounts.push(format!(
            "codes entered: {}",
            entered.discount_codes.join(", ")
        ));
    }
    if !discounts.is_empty() {
        lines.push(("Discounts", discounts.join(" · ")));
    }

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
    }
    if let Some(extensions) = &scenario.extensions {
        lines.push(("Extensions", extensions.join(", ")));
    }
    lines
}

fn query_escape(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn scenario_json(
    store: &Store,
    app: &App,
    options: &CheckoutOptions,
    scenario: &CheckoutScenario,
    root: &str,
) -> Json {
    let mut problems = scenario.problems.clone();
    if problems.is_empty() {
        // The extensions it names must be the app's.
        if let Err(problem) =
            crate::checkout::selection_for(app, &options.extensions, Some(scenario))
        {
            problems.push(format!("{}: {problem}", scenario.file));
        }
    }
    let open = format!("{root}/checkout?scenario={}", query_escape(&scenario.name));
    let summary: Vec<Json> = if scenario.problems.is_empty() {
        summarize(store, scenario)
            .into_iter()
            .map(|(label, text)| json!({"label": label, "text": text}))
            .collect()
    } else {
        Vec::new()
    };
    json!({
        "name": scenario.name,
        "title": scenario.title,
        "description": scenario.description,
        "file": scenario.file,
        "summary": summary,
        "problems": problems,
        "open": problems.is_empty().then_some(open.clone()),
        "openAllTargets": problems.is_empty().then(|| format!("{open}&targets=all")),
    })
}

/// Everything the home page says, for the request in the language whose locale root is `root`.
pub fn build(
    state: &ServerState,
    incoming: &Incoming,
    options: &CheckoutOptions,
    root: &str,
) -> Json {
    let store = state.store_of(incoming);
    let app = match lsf_checkout::app::load(&options.app_dir) {
        Ok(app) => app,
        Err(problem) => {
            return json!({"error": problem, "root": root});
        }
    };
    let selection =
        Selection::resolve(&app, &options.extensions).unwrap_or_else(|_| Selection::all());
    let general: Vec<Json> = app
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.extension.is_none())
        .map(|diagnostic| {
            json!({
                "severity": match diagnostic.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                },
                "message": diagnostic.message,
            })
        })
        .collect();
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
        "problems": general,
        "functions": app.functions.iter()
            .map(|function| function_json(&app, &selection, function))
            .collect::<Vec<_>>(),
        "uiExtensions": app.ui_extensions.iter()
            .map(|extension| ui_extension_json(&app, &selection, extension))
            .collect::<Vec<_>>(),
        "otherExtensions": app.other.iter()
            .map(|other| json!({"handle": other.handle, "type": other.kind}))
            .collect::<Vec<_>>(),
        "scenarios": store.scenarios.iter()
            .map(|scenario| scenario_json(&store, &app, options, scenario, root))
            .collect::<Vec<_>>(),
        "links": {
            "checkout": format!("{root}/checkout"),
            "allTargets": format!("{root}/checkout?targets=all"),
            "shop": format!("{root}/checkout?variant=shop"),
        },
    })
}

/// `GET /` of a checkout without a theme and `GET /__lsf/checkouts`: the page, or with
/// `?format=json` what it says.
pub fn page(
    state: &ServerState,
    incoming: &Incoming,
    options: &CheckoutOptions,
    root: &str,
) -> Reply {
    let home = build(state, incoming, options, root);
    if incoming.query_param("format") == Some("json") {
        return Reply::json(200, &home).cached(cache::PRIVATE);
    }
    Reply::html(200, render(&home)).cached(cache::PRIVATE)
}

// --- The HTML ----------------------------------------------------------------------------

fn chip(tone: &str, text: &str) -> String {
    format!("<span class=\"lsf-chip is-{tone}\">{}</span>", esc(text))
}

fn code(text: &str) -> String {
    format!("<code>{}</code>", esc(text))
}

fn text_of(value: &Json) -> &str {
    value.as_str().unwrap_or_default()
}

fn banner(tone: &str, text: &str) -> String {
    format!(
        "<div class=\"lsf-banner is-{tone}\" role=\"status\"><div><p>{}</p></div></div>",
        esc(text)
    )
}

fn problem_banners(problems: &Json) -> String {
    problems
        .as_array()
        .into_iter()
        .flatten()
        .map(|problem| {
            let tone = if problem["severity"] == "error" {
                "critical"
            } else {
                "warning"
            };
            banner(tone, text_of(&problem["message"]))
        })
        .collect()
}

fn function_html(function: &Json) -> String {
    let mut chips = Vec::new();
    let mut head = format!("<h3>{}</h3>", esc(text_of(&function["handle"])));
    if let Some(name) = function["name"].as_str() {
        head.push_str(&format!("<span class=\"lsf-muted\">{}</span>", esc(name)));
    }
    chips.push(match function["apiVersion"].as_str() {
        Some(version) if function["supported"] == true => chip("ok", &format!("API {version}")),
        Some(version) => chip("error", &format!("API {version} is not supported")),
        None => chip("error", "no api_version"),
    });
    chips.push(if function["built"] == true {
        chip("ok", "built")
    } else {
        chip("error", "not built")
    });
    if function["network"] == true {
        chips.push(chip("ok", "network access (.fetch)"));
    }
    if function["selected"] != true {
        chips.push(chip("error", "left out by --extension"));
    }
    let targets: String = function["targets"]
        .as_array()
        .into_iter()
        .flatten()
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
            if target["runs"] == false {
                notes.push("not run by the local checkout".to_string());
            }
            format!(
                "<li>{} <span class=\"lsf-muted\">{}</span></li>",
                code(text_of(&target["target"])),
                esc(&notes.join(" · "))
            )
        })
        .collect();
    let build = if function["built"] != true {
        match function["buildCommand"].as_str() {
            Some(command) => format!(
                "<p class=\"lsf-muted\">Build it with {} (or <code>shopify app build</code>), or start with <code>--build</code>.</p>",
                code(command)
            ),
            None => "<p class=\"lsf-muted\">Build it with <code>shopify app build</code>.</p>"
                .to_string(),
        }
    } else {
        String::new()
    };
    format!(
        "<article class=\"lsf-card\" data-function=\"{}\"><div class=\"lsf-card-head\">{head}<div class=\"lsf-chips\">{}</div></div>\
         <ul class=\"lsf-targets\">{targets}</ul>{build}{}</article>",
        esc(text_of(&function["handle"])),
        chips.join(""),
        problem_banners(&function["problems"]),
    )
}

fn ui_extension_html(extension: &Json) -> String {
    let mut head = format!("<h3>{}</h3>", esc(text_of(&extension["handle"])));
    if let Some(name) = extension["name"].as_str() {
        head.push_str(&format!("<span class=\"lsf-muted\">{}</span>", esc(name)));
    }
    let mut chips = vec![match extension["apiVersion"].as_str() {
        Some(version) if extension["supported"] == true => chip("ok", &format!("API {version}")),
        Some(version) => chip("error", &format!("API {version} is not supported")),
        None => chip("error", "no api_version"),
    }];
    chips.push(if extension["built"] == true {
        chip("ok", "built")
    } else {
        chip("error", "not built")
    });
    for capability in extension["capabilities"].as_array().into_iter().flatten() {
        chips.push(chip("ok", text_of(capability)));
    }
    if extension["selected"] != true {
        chips.push(chip("error", "left out by --extension"));
    }
    let targets = extension["targets"].as_array().cloned().unwrap_or_default();
    let rows: String = targets
        .iter()
        .map(|target| {
            let place = match target["place"].as_str() {
                Some(place) => format!("<span class=\"lsf-muted\">{}</span>", esc(place)),
                None => chip("error", "no place on the page"),
            };
            format!(
                "<li data-target=\"{}\">{} {place}</li>",
                esc(text_of(&target["target"])),
                code(text_of(&target["target"]))
            )
        })
        .collect();
    let unplaced: Vec<&str> = targets
        .iter()
        .filter(|target| target["place"].is_null())
        .map(|target| text_of(&target["target"]))
        .collect();
    let note = if unplaced.is_empty() {
        String::new()
    } else {
        banner(
            "warning",
            &format!(
                "The local checkout has no place for {}: the extension is not rendered there.",
                unplaced.join(", ")
            ),
        )
    };
    format!(
        "<article class=\"lsf-card\" data-ui-extension=\"{}\"><div class=\"lsf-card-head\">{head}<div class=\"lsf-chips\">{}</div></div>\
         <ul class=\"lsf-targets\">{rows}</ul>{note}{}</article>",
        esc(text_of(&extension["handle"])),
        chips.join(""),
        problem_banners(&extension["problems"]),
    )
}

fn scenario_html(scenario: &Json) -> String {
    let name = text_of(&scenario["name"]);
    let mut head = format!("<h3>{}</h3>", esc(text_of(&scenario["title"])));
    if scenario["title"] != scenario["name"] {
        head.push_str(&code(name));
    }
    let description = scenario["description"]
        .as_str()
        .map(|text| format!("<p>{}</p>", esc(text)))
        .unwrap_or_default();
    let summary: String = scenario["summary"]
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
    let problems: String = scenario["problems"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|problem| {
            format!(
                "<pre class=\"lsf-home-problem\">{}</pre>",
                esc(text_of(problem))
            )
        })
        .collect();
    let (state, actions) = match scenario["open"].as_str() {
        Some(open) => (
            String::new(),
            format!(
                "<a class=\"lsf-pay\" href=\"{}\">Open the checkout</a>\
                 <a class=\"lsf-apply\" href=\"{}\">With every extension target</a>",
                esc(open),
                esc(text_of(&scenario["openAllTargets"])),
            ),
        ),
        None => (chip("error", "has errors"), String::new()),
    };
    format!(
        "<article class=\"lsf-card\" data-scenario=\"{}\"><div class=\"lsf-card-head\">{head}{state}</div>\
         {description}<dl class=\"lsf-facts\">{summary}</dl>{problems}\
         <div class=\"lsf-home-actions\">{actions}</div>\
         <p class=\"lsf-muted lsf-file\">{}</p></article>",
        esc(name),
        esc(text_of(&scenario["file"])),
    )
}

/// The home page, drawn from what [`build`] says.
pub fn render(home: &Json) -> String {
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

    let mut main = String::new();
    main.push_str(&problem_banners(&home["problems"]));
    if home["storeDataProblems"].as_u64().unwrap_or(0) > 0 {
        main.push_str(&banner(
            "critical",
            "The store data has errors: run `lsf validate` to see them.",
        ));
    }

    // Opening a checkout.
    main.push_str(&format!(
        "<section class=\"lsf-section\" data-section=\"open\"><h2>Open a checkout</h2>\
         <p class=\"lsf-muted\">With the cart of the store data, or with a scenario below.</p>\
         <div class=\"lsf-home-actions\">\
         <a class=\"lsf-pay\" href=\"{}\">Open the checkout</a>\
         <a class=\"lsf-apply\" href=\"{}\">With every extension target</a>\
         <a class=\"lsf-apply\" href=\"{}\">In the Shop look</a></div></section>",
        esc(text_of(&links["checkout"])),
        esc(text_of(&links["allTargets"])),
        esc(text_of(&links["shop"])),
    ));

    // The scenarios.
    let scenarios = home["scenarios"].as_array().cloned().unwrap_or_default();
    main.push_str(
        "<section class=\"lsf-section\" data-section=\"scenarios\"><h2>Checkout scenarios</h2>",
    );
    if scenarios.is_empty() {
        let folder = home["dataFolder"]
            .as_str()
            .unwrap_or("the `checkouts` folder of the store data");
        main.push_str(&format!(
            "<p class=\"lsf-muted\">None yet. A scenario is a file in {}. \
             <code>lsf checkout init --app {}</code> writes a set to start from.</p>",
            code(folder),
            esc(text_of(&app["directory"])),
        ));
    } else {
        main.push_str(&format!(
            "<p class=\"lsf-muted\">{} in {}. Each one opens a fresh checkout.</p><div class=\"lsf-grid\">",
            scenarios.len(),
            code(text_of(&home["dataFolder"])),
        ));
        for scenario in &scenarios {
            main.push_str(&scenario_html(scenario));
        }
        main.push_str("</div>");
    }
    main.push_str("</section>");

    // The Functions.
    let functions = home["functions"].as_array().cloned().unwrap_or_default();
    main.push_str(&format!(
        "<section class=\"lsf-section\" data-section=\"functions\"><h2>Functions <span class=\"lsf-muted\">{}</span></h2>",
        functions.len()
    ));
    if functions.is_empty() {
        main.push_str("<p class=\"lsf-muted\">The app has no Function.</p>");
    } else {
        main.push_str("<div class=\"lsf-grid\">");
        for function in &functions {
            main.push_str(&function_html(function));
        }
        main.push_str("</div>");
    }
    main.push_str("</section>");

    // The checkout UI extensions.
    let extensions = home["uiExtensions"].as_array().cloned().unwrap_or_default();
    main.push_str(&format!(
        "<section class=\"lsf-section\" data-section=\"ui-extensions\"><h2>Checkout UI extensions <span class=\"lsf-muted\">{}</span></h2>",
        extensions.len()
    ));
    if extensions.is_empty() {
        main.push_str("<p class=\"lsf-muted\">The app has no checkout UI extension.</p>");
    } else {
        main.push_str(&format!(
            "<p class=\"lsf-muted\"><a class=\"lsf-link\" href=\"{}\">Open a checkout where each of them renders at every target it declares.</a></p><div class=\"lsf-grid\">",
            esc(text_of(&links["allTargets"]))
        ));
        for extension in &extensions {
            main.push_str(&ui_extension_html(extension));
        }
        main.push_str("</div>");
    }
    main.push_str("</section>");

    // The others.
    let others = home["otherExtensions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !others.is_empty() {
        let list: String = others
            .iter()
            .map(|other| {
                format!(
                    "<li>{} <span class=\"lsf-muted\">{}</span></li>",
                    code(text_of(&other["handle"])),
                    esc(text_of(&other["type"]))
                )
            })
            .collect();
        main.push_str(&format!(
            "<section class=\"lsf-section\" data-section=\"other\"><h2>Other extensions</h2>\
             <p class=\"lsf-muted\">Not part of the checkout, so not used here.</p><ul class=\"lsf-targets\">{list}</ul></section>"
        ));
    }

    let body = format!(
        "<header class=\"lsf-header\"><span class=\"lsf-brand\">{}</span>\
         <span class=\"lsf-brand-sub\">checkouts and extensions</span>\
         <span class=\"lsf-secure\">{}</span></header><main class=\"lsf-home-main\">{main}</main>",
        esc(title),
        esc(text_of(&home["data"])),
    );
    document(&format!("{title} · checkout"), &body)
}

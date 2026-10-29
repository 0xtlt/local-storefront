//! `lsf checkout init`: writes the `checkouts/` folder of the store data, a set of checkout
//! scenarios to try an app with. The scenarios are made for the extensions of the app (a cart
//! that a cart transform can merge, a discount code for a discount Function, several shipping
//! rates for a delivery customization...) and for the store data they are written for: every
//! product, customer, country and language a scenario names is one the data has.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lsf_checkout::app::{App as ShopifyApp, TargetKind};
use lsf_core::store::load::{DataSource, LoadOptions, load};
use lsf_core::store::{CheckoutCharge, CheckoutMetafield, Store};
use serde_json::{Value as Json, json};

use crate::commands::{init, schema};

#[derive(clap::Args)]
pub struct InitArgs {
    /// The directory of a Shopify app (the one with shopify.app.toml). The scenarios are made
    /// for its Functions and checkout UI extensions. Defaults to the app the current folder is
    /// in; outside an app, a generic set is written.
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    app: Option<PathBuf>,

    /// Overwrite the scenarios that already exist.
    #[arg(long)]
    force: bool,
}

impl InitArgs {
    /// `init --app <dir>`, for the tests of the server.
    #[cfg(test)]
    pub fn for_app(app: PathBuf) -> InitArgs {
        InitArgs {
            app: Some(app),
            force: false,
        }
    }
}

/// A data file of the demo store that gives it a selling plan with a deposit, which the
/// pre-order scenario needs. Written with the demo store only.
const PRE_ORDER: &str = r#"{
  "$schema": "./schema/store.schema.json",
  "selling_plan_groups": [
    {
      "name": "Pre-order, 40% now",
      "options": ["Payment"],
      "selling_plans": [
        {
          "name": "Pre-order: 40% today, the rest when it ships",
          "options": ["40% deposit"],
          "recurring_deliveries": false,
          "checkout_charge": { "value_type": "percentage", "value": 40 }
        }
      ],
      "products": ["wool-beanie"]
    }
  ]
}
"#;

/// The handles of the extensions of an app by what they do.
#[derive(Default)]
struct Kinds {
    cart_transform: Vec<String>,
    discount: Vec<String>,
    delivery_customization: Vec<String>,
    payment_customization: Vec<String>,
    validation: Vec<String>,
    /// The checkout UI extensions.
    ui: Vec<String>,
}

impl Kinds {
    fn of(app: &ShopifyApp) -> Kinds {
        let mut kinds = Kinds::default();
        for function in &app.functions {
            let has = |wanted: &[TargetKind]| {
                function
                    .targets
                    .iter()
                    .any(|target| wanted.contains(&target.kind))
            };
            let handle = function.handle.clone();
            if has(&[TargetKind::CartTransform]) {
                kinds.cart_transform.push(handle.clone());
            }
            if has(&[
                TargetKind::LinesDiscounts,
                TargetKind::ProductDiscount,
                TargetKind::OrderDiscount,
                TargetKind::DeliveryDiscounts,
                TargetKind::ShippingDiscount,
            ]) {
                kinds.discount.push(handle.clone());
            }
            if has(&[TargetKind::DeliveryCustomization]) {
                kinds.delivery_customization.push(handle.clone());
            }
            if has(&[TargetKind::PaymentCustomization]) {
                kinds.payment_customization.push(handle.clone());
            }
            if has(&[TargetKind::Validation]) {
                kinds.validation.push(handle);
            }
        }
        kinds.ui = app
            .ui_extensions
            .iter()
            .map(|extension| extension.handle.clone())
            .collect();
        kinds
    }

    /// `extensions` for a scenario about `functions`: only those Functions run, and the checkout
    /// UI extensions, so that the page is the whole page and one Function shows alone.
    fn only(&self, functions: &[String]) -> Json {
        json!(functions.iter().chain(&self.ui).collect::<Vec<_>>())
    }
}

/// A variant a cart can hold: how the data names it, what it is called, and how many the store
/// can sell.
struct Pick {
    reference: Json,
    title: String,
}

/// What the store can sell, one variant of each product that can be bought, in the order of
/// the data.
fn catalog(store: &Store) -> Vec<Pick> {
    let capacity = |variant: &lsf_core::store::Variant| -> i64 {
        use lsf_core::store::model::InventoryPolicy;
        if !variant.inventory_tracked || variant.inventory_policy == InventoryPolicy::Continue {
            i64::MAX
        } else {
            variant.inventory_quantity
        }
    };
    let mut picks = Vec::new();
    for product in &store.products {
        if product.gift_card || product.requires_selling_plan {
            continue;
        }
        let buyable = |variant: &&lsf_core::store::Variant| {
            variant.available && variant.requires_shipping && variant.price > 0
        };
        // A variant with stock for a large cart, otherwise the first one that can be bought.
        let variant = product
            .variants
            .iter()
            .filter(buyable)
            .find(|variant| capacity(variant) >= 12)
            .or_else(|| product.variants.iter().find(buyable));
        if let Some(variant) = variant {
            picks.push(Pick {
                reference: if variant.sku.is_empty() {
                    json!(variant.id)
                } else {
                    json!(variant.sku)
                },
                title: product.title.clone(),
            });
        }
    }
    picks
}

/// A cart of the first `lines.len()` variants of the catalog, in these quantities.
fn cart(picks: &[Pick], quantities: &[u32]) -> Json {
    let items: Vec<Json> = picks
        .iter()
        .zip(quantities)
        .map(|(pick, quantity)| json!({"variant": pick.reference, "quantity": quantity}))
        .collect();
    json!({ "items": items })
}

fn address(country: &str) -> Json {
    json!({
        "first_name": "Ada",
        "last_name": "Lovelace",
        "address1": "1 Test Street",
        "city": "Testville",
        "zip": "12345",
        "country_code": country,
    })
}

/// The scenarios of a plan, and what could not be made.
struct Plan {
    scenarios: Vec<(&'static str, Json)>,
    skipped: Vec<(&'static str, String)>,
}

impl Plan {
    fn add(&mut self, name: &'static str, mut scenario: Json) {
        scenario["$schema"] = json!("../schema/checkout-scenario.schema.json");
        // The schema first.
        let mut ordered = serde_json::Map::new();
        ordered.insert("$schema".into(), scenario["$schema"].take());
        if let Json::Object(rest) = scenario {
            for (key, value) in rest {
                if key != "$schema" {
                    ordered.insert(key, value);
                }
            }
        }
        self.scenarios.push((name, Json::Object(ordered)));
    }

    fn skip(&mut self, name: &'static str, reason: impl Into<String>) {
        self.skipped.push((name, reason.into()));
    }
}

/// A metafield of the checkout settings, as the store data writes it: a JSON metafield reads
/// better as JSON than as the text it is stored as.
fn metafield_json(field: &CheckoutMetafield) -> (String, Json) {
    let value = if field.kind == "json" {
        serde_json::from_str(&field.value).unwrap_or_else(|_| json!(field.value))
    } else {
        json!(field.value)
    };
    (
        format!("{}.{}", field.namespace, field.key),
        json!({"type": field.kind, "value": value}),
    )
}

/// Where the settings of a Function come from when the store data has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// The store data of the app (`<app>/shopify-local`).
    App,
    /// Empty values of the type each variable of the input query has.
    Empty,
}

/// The settings `[extensions.input.variables]` of a Function reads when `given` lacks them:
/// empty values of the type its input query asks for. `None` for a Function whose queries have
/// no variables, or when `given` has them.
fn missing_settings(
    app: &ShopifyApp,
    handle: &str,
    given: &[CheckoutMetafield],
) -> Option<(String, Json, Source)> {
    let function = app.function(handle)?;
    let variables = function.input_variables.as_ref()?;
    if given
        .iter()
        .any(|field| field.namespace == variables.namespace && field.key == variables.key)
    {
        return None;
    }
    let mut values = serde_json::Map::new();
    for target in &function.targets {
        for (name, value) in lsf_checkout::input::empty_variables(&target.input_query) {
            values.entry(name).or_insert(value);
        }
    }
    // A query without variables needs none: the Function reads the metafield, or nothing.
    if values.is_empty() {
        return None;
    }
    Some((
        format!("{}.{}", variables.namespace, variables.key),
        json!({"type": "json", "value": values}),
        Source::Empty,
    ))
}

/// The settings of the Functions of the app that are not discounts, for those the store data
/// gives none: `checkout.functions` to write, by handle.
fn function_settings(
    store: &Store,
    app_store: Option<&Store>,
    app: &ShopifyApp,
    discounts: &[String],
) -> Vec<(String, Json, Source)> {
    app.functions
        .iter()
        .filter(|function| !discounts.contains(&function.handle))
        .filter_map(|function| {
            let given = store.checkout.functions.get(&function.handle);
            // Every setting the app's own store data gives it, for a Function the store data
            // gives none.
            if given.is_none()
                && let Some(own) = app_store
                    .and_then(|store| store.checkout.functions.get(&function.handle))
                    .filter(|own| !own.is_empty())
            {
                let metafields: serde_json::Map<String, Json> =
                    own.iter().map(metafield_json).collect();
                return Some((
                    function.handle.clone(),
                    json!({"metafields": metafields}),
                    Source::App,
                ));
            }
            let given = given.map_or(&[][..], Vec::as_slice);
            let (name, value, source) = missing_settings(app, &function.handle, given)?;
            Some((
                function.handle.clone(),
                json!({"metafields": {name: value}}),
                source,
            ))
        })
        .collect()
}

/// The discounts a scenario sets up for the discount Functions of the app. The ones the store
/// data (or else the store data of the app) configures keep their settings; the others get the
/// settings their input query reads, with empty values.
fn discounts(
    store: &Store,
    app_store: Option<&Store>,
    app: &ShopifyApp,
    handles: &[String],
    code: Option<&str>,
) -> Json {
    let entries: Vec<Json> = handles
        .iter()
        .map(|handle| {
            let configured = |store: &Store| {
                store
                    .checkout
                    .discounts
                    .iter()
                    .find(|discount| discount.function == *handle)
                    .cloned()
            };
            let configured = configured(store).or_else(|| app_store.and_then(configured));
            let configured = configured.as_ref();
            let title = app
                .function(handle)
                .and_then(|function| function.name.clone())
                .unwrap_or_else(|| handle.clone());
            let mut entry =
                json!({"function": handle, "title": configured.map_or(&title, |d| &d.title)});
            if let Some(configured) = configured {
                if !configured.classes.is_empty() {
                    entry["classes"] = json!(configured.classes);
                }
                if !configured.metafields.is_empty() {
                    let metafields: serde_json::Map<String, Json> =
                        configured.metafields.iter().map(metafield_json).collect();
                    entry["metafields"] = Json::Object(metafields);
                }
            }
            let given = configured.map_or(&[][..], |discount| discount.metafields.as_slice());
            if let Some((name, value, _)) = missing_settings(app, handle, given) {
                if !entry["metafields"].is_object() {
                    entry["metafields"] = json!({});
                }
                entry["metafields"][name] = value;
            }
            // The code the buyer enters, or none for a discount that applies by itself.
            entry["code"] = match code {
                Some(code) => json!(configured.and_then(|d| d.code.as_deref()).unwrap_or(code)),
                None => Json::Null,
            };
            entry
        })
        .collect();
    json!(entries)
}

/// The codes a set of discounts is entered with.
fn codes_of(discounts: &Json) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for discount in discounts.as_array().into_iter().flatten() {
        if let Some(code) = discount["code"].as_str()
            && !codes.iter().any(|known| known == code)
        {
            codes.push(code.to_string());
        }
    }
    codes
}

/// The scenarios to write for `store` and, when there is one, the app (and its own store data,
/// where the settings of its Functions are taken from when `store` has none).
fn plan(store: &Store, app: Option<&ShopifyApp>, app_store: Option<&Store>) -> Plan {
    let mut plan = Plan {
        scenarios: Vec::new(),
        skipped: Vec::new(),
    };
    let picks = catalog(store);
    if picks.is_empty() {
        plan.skip(
            "every scenario",
            "the store data has no product that can be bought (available, with a price, that ships)",
        );
        return plan;
    }
    let home = store
        .countries
        .first()
        .map_or("US", |country| country.iso_code.as_str())
        .to_string();
    let kinds = app.map(Kinds::of).unwrap_or_default();

    plan.add(
        "guest",
        json!({
            "title": "A guest, one line",
            "description": "The plainest checkout: nobody is logged in, one line, nothing entered yet.",
            "session": {"customer": "none", "cart": cart(&picks, &[1])},
        }),
    );
    plan.add(
        "customer",
        json!({
            "title": "A logged-in customer",
            "description": "The first customer of the store data buys two products, with a note and a cart attribute.",
            "session": {
                "customer": "default",
                "cart": {
                    "items": cart(&picks, &[2, 1])["items"],
                    "note": "Please leave at the door",
                    "attributes": {"gift": "no"},
                },
            },
        }),
    );

    // A buyer for a company.
    match store.customers.iter().find(|customer| customer.company.is_some()) {
        Some(customer) => {
            let company = &store.companies[customer.company.expect("a company")];
            let mut session = json!({
                "customer": customer.email,
                "country": home,
                "cart": cart(&picks, &[6, 4]),
            });
            // The last location they can buy for, when they have several: not the default one.
            if customer.company_locations.len() > 1
                && let Some(location) = customer.company_locations.last()
            {
                session["company_location"] = json!(company.locations[*location].name);
            }
            plan.add(
                "b2b",
                json!({
                    "title": format!("A B2B buyer, {}", company.name),
                    "description": "A customer who buys for a company, at one of its locations, in larger quantities: what purchasingCompany gives the Functions.",
                    "session": session,
                    "entered": {"shipping_address": address(&home)},
                }),
            );
        }
        None => plan.skip(
            "b2b",
            "no customer buys for a company: add `companies` and a customer with a `company` to the store data",
        ),
    }

    // Another country and another language.
    let other_language = store.languages.iter().find(|language| !language.primary);
    let other_country = other_language
        .and_then(|language| store.country(&language.iso_code))
        .or_else(|| {
            store
                .countries
                .iter()
                .find(|country| country.iso_code != home)
        });
    if other_language.is_none() && other_country.is_none() {
        plan.skip(
            "abroad",
            "the store sells in one country and one language: add some to `localization`",
        );
    } else {
        let mut scenario = json!({
            "title": "Another country and language",
            "description": "A buyer in another country, reading the checkout in another language: localization.country, language and market for the Functions and the extensions.",
            "session": {"cart": cart(&picks, &[2, 1])},
        });
        if let Some(country) = other_country {
            scenario["session"]["country"] = json!(country.iso_code);
            scenario["entered"] = json!({"shipping_address": address(&country.iso_code)});
        }
        if let Some(language) = other_language {
            scenario["language"] = json!(language.iso_code);
        }
        plan.add("abroad", scenario);
    }

    // Selling plans: a subscription, and a pre-order that is paid in two times.
    let plan_line = |deferred: bool| -> Option<Json> {
        store.products.iter().find_map(|product| {
            let pick = picks.iter().find(|pick| {
                product.variants.iter().any(|variant| {
                    pick.reference == json!(variant.sku) || pick.reference == json!(variant.id)
                })
            })?;
            let (_, selling_plan) = product
                .selling_plan_groups
                .iter()
                .flat_map(|group| &store.selling_plan_groups[*group].plans)
                .map(|selling_plan| ((), selling_plan))
                .find(|(_, selling_plan)| {
                    (selling_plan.checkout_charge != CheckoutCharge::Percentage(100)) == deferred
                })?;
            Some(json!({
                "variant": pick.reference,
                "quantity": 2,
                "selling_plan": selling_plan.name,
                "product": pick.title,
            }))
        })
    };
    for (name, deferred, title, description, missing) in [
        (
            "subscription",
            false,
            "A subscription",
            "A line bought with a selling plan that repeats: what the Functions read of a selling plan.",
            "no product is sold with a selling plan: add `selling_plan_groups` to the store data",
        ),
        (
            "pre-order",
            true,
            "A pre-order with a deposit",
            "A line bought with a selling plan that charges a deposit today and the rest later: the checkout shows what is due today and what is due later.",
            "no selling plan takes a deposit: give a plan of `selling_plan_groups` a `checkout_charge`",
        ),
    ] {
        match plan_line(deferred) {
            Some(mut line) => {
                line.as_object_mut().expect("a line").remove("product");
                plan.add(
                    name,
                    json!({
                        "title": title,
                        "description": description,
                        "session": {"customer": "default", "cart": {"items": [line]}},
                    }),
                );
            }
            None => plan.skip(name, missing),
        }
    }

    plan.add(
        "shop-look",
        json!({
            "title": "The Shop look",
            "description": "The checkout in the look of the Shop app and Shop Pay.",
            "session": {"cart": cart(&picks, &[1, 2])},
            "variant": "shop",
        }),
    );
    plan.add(
        "large-cart",
        json!({
            "title": "A large cart",
            "description": "Many units on several lines: quantity limits, amount thresholds, free shipping, volume discounts.",
            "session": {"cart": cart(&picks, &[12, 8, 5, 3])},
        }),
    );

    let Some(app) = app else {
        return plan;
    };

    // What the Functions of the app do, one kind at a time.
    if kinds.cart_transform.is_empty() {
        plan.skip("cart-transform", "the app has no cart transform Function");
    } else {
        plan.add(
            "cart-transform",
            json!({
                "title": "A cart for the cart transform",
                "description": format!(
                    "Several lines of more than one unit, and one line of three: what a merge, an expand or an update of {} can work on. Its settings are the `checkout.functions.<handle>.metafields` of the store data.",
                    kinds.cart_transform.join(", ")
                ),
                "session": {"cart": cart(&picks, &[2, 2, 3])},
                "extensions": kinds.only(&kinds.cart_transform),
            }),
        );
    }
    if kinds.discount.is_empty() {
        plan.skip("discount", "the app has no discount Function");
    } else {
        let automatic = discounts(store, app_store, app, &kinds.discount, None);
        plan.add(
            "discount-automatic",
            json!({
                "title": "An automatic discount",
                "description": format!(
                    "A cart of six, three and two units, for {} with no code. Their settings are the `metafields` of `checkout.discounts` (those of the store data are kept).",
                    kinds.discount.join(", ")
                ),
                "session": {"cart": cart(&picks, &[6, 3, 2])},
                "checkout": {"discounts": automatic},
                "extensions": kinds.only(&kinds.discount),
            }),
        );
        let with_code = discounts(store, app_store, app, &kinds.discount, Some("SAVE10"));
        plan.add(
            "discount-code",
            json!({
                "title": "A discount code",
                "description": format!(
                    "The same cart, with the code {} entered: {} apply as code discounts.",
                    codes_of(&with_code).join(", "),
                    kinds.discount.join(", ")
                ),
                "session": {"cart": cart(&picks, &[6, 3, 2])},
                "entered": {"discount_codes": codes_of(&with_code)},
                "checkout": {"discounts": with_code},
                "extensions": kinds.only(&kinds.discount),
            }),
        );
    }
    if kinds.delivery_customization.is_empty() {
        plan.skip(
            "delivery-options",
            "the app has no delivery customization Function",
        );
    } else {
        plan.add(
            "delivery-options",
            json!({
                "title": "Several delivery options",
                "description": format!(
                    "Standard, express, local delivery and store pickup, for {} to hide, rename or reorder.",
                    kinds.delivery_customization.join(", ")
                ),
                "session": {"cart": cart(&picks, &[2, 1])},
                "entered": {"shipping_address": address(&home), "delivery": "standard"},
                "checkout": {"shipping_rates": [
                    {"handle": "standard", "title": "Standard", "price": "4.90", "description": "3 to 5 days"},
                    {"handle": "express", "title": "Express", "price": "12.90", "description": "Next day"},
                    {"handle": "local", "title": "Local delivery", "price": "6.00", "kind": "local"},
                    {"handle": "pickup", "title": "Pick up in store", "price": "0.00", "kind": "pick_up"},
                ]},
                "extensions": kinds.only(&kinds.delivery_customization),
            }),
        );
    }
    if kinds.payment_customization.is_empty() {
        plan.skip(
            "payment-methods",
            "the app has no payment customization Function",
        );
    } else {
        plan.add(
            "payment-methods",
            json!({
                "title": "Several payment methods",
                "description": format!(
                    "Card, PayPal, cash on delivery, bank deposit and two express buttons, for {} to hide, rename or reorder.",
                    kinds.payment_customization.join(", ")
                ),
                "session": {"cart": cart(&picks, &[3, 2])},
                "entered": {"email": "buyer@example.com", "shipping_address": address(&home)},
                "checkout": {"payment_methods": [
                    {"handle": "card", "name": "Credit card"},
                    {"handle": "paypal", "name": "PayPal"},
                    {"handle": "cod", "name": "Cash on Delivery"},
                    {"handle": "bank", "name": "Bank Deposit"},
                    {"handle": "shop_pay", "name": "Shop Pay", "placements": ["ACCELERATED_CHECKOUT"]},
                    {"handle": "apple_pay", "name": "Apple Pay", "placements": ["ACCELERATED_CHECKOUT"]},
                ]},
                "extensions": kinds.only(&kinds.payment_customization),
            }),
        );
    }
    if kinds.validation.is_empty() {
        plan.skip("validation", "the app has no validation Function");
    } else {
        plan.add(
            "validation",
            json!({
                "title": "A cart that fails validation",
                "description": format!(
                    "A big cart, a note and no email, for {} to refuse. The errors of the page show as you change the checkout, and refuse the order when you pay.",
                    kinds.validation.join(", ")
                ),
                "session": {"cart": {
                    "items": cart(&picks, &[15, 9, 4])["items"],
                    "note": "Ship in one parcel",
                }},
                "extensions": kinds.only(&kinds.validation),
            }),
        );
    }
    if kinds.ui.is_empty() {
        plan.skip("all-extensions", "the app has no checkout UI extension");
    } else {
        let mut lines = cart(&picks, &[2, 1, 1])["items"].clone();
        if let Some(line) = plan_line(false)
            && let Some(items) = lines.as_array_mut()
        {
            let mut line = line;
            line.as_object_mut().expect("a line").remove("product");
            items.push(line);
        }
        plan.add(
            "all-extensions",
            json!({
                "title": "Everything at once",
                "description": "A logged-in customer, several lines, shipping options, a code and a note, with every Function and every checkout UI extension running. Open it \"With every extension target\" to see each extension at every target it declares.",
                "session": {"customer": "default", "cart": {"items": lines, "note": "For the extensions to read"}},
                "entered": {"shipping_address": address(&home), "discount_codes": ["WELCOME"]},
                "checkout": {"shipping_rates": [
                    {"handle": "standard", "title": "Standard", "price": "4.90"},
                    {"handle": "express", "title": "Express", "price": "12.90"},
                ]},
            }),
        );
    }
    plan
}

pub fn run(theme: &Path, data: Option<&Path>, args: InitArgs) -> Result<ExitCode, String> {
    // The app: `--app`, or the one the current folder is in.
    let app_dir = args.app.clone().or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|dir| crate::checkout::app_around(&dir))
    });
    let shopify_app = match &app_dir {
        Some(dir) => {
            let app = lsf_checkout::app::load(dir)?;
            Some(app)
        }
        None => None,
    };
    let directory = init::data_directory(theme, data);

    // A data directory that is not there yet gets the demo store the scenarios are made for.
    let created = !directory.exists();
    if created {
        init::write_demo_store(&directory, false)?;
        init::write_file(&directory, "pre-order.json", PRE_ORDER, false)?;
        eprintln!("{}: created, with the demo store", directory.display());
    }
    schema::write_all(&directory.join("schema"))?;

    let (mut store, mut diagnostics) = load(
        &DataSource::Directory(directory.clone()),
        &LoadOptions::default(),
    );
    // The demo store `lsf init` wrote has no plan with a deposit: it gets the one the pre-order
    // scenario needs, as a store `checkout init` creates does.
    let deposit = |store: &lsf_core::Store| {
        store
            .selling_plan_groups
            .iter()
            .flat_map(|group| &group.plans)
            .any(|plan| plan.checkout_charge != CheckoutCharge::Percentage(100))
    };
    if !created
        && !deposit(&store)
        && store.product_by_handle("wool-beanie").is_some()
        && init::write_file(&directory, "pre-order.json", PRE_ORDER, false)?
    {
        eprintln!(
            "{}: added pre-order.json, a selling plan with a deposit for the pre-order scenario",
            directory.display()
        );
        (store, diagnostics) = load(
            &DataSource::Directory(directory.clone()),
            &LoadOptions::default(),
        );
    }
    // Scenarios that already exist are not the store's problem.
    let store_errors = diagnostics
        .items
        .iter()
        .filter(|item| item.severity == lsf_core::diagnostics::Severity::Error)
        .filter(|item| !item.file.starts_with("checkouts/"))
        .count();
    if store_errors > 0 {
        eprintln!(
            "warning: the store data has {store_errors} error(s): run `lsf validate`. The scenarios are written for what could be read."
        );
    }

    // The store data of the app, where the settings of its Functions are taken from when the
    // store data has none.
    let app_store = app_dir
        .as_ref()
        .map(|dir| dir.join("shopify-local"))
        .filter(|own| own.is_dir() && !same_directory(own, &directory))
        .map(|own| load(&DataSource::Directory(own), &LoadOptions::default()).0);

    let plan = plan(&store, shopify_app.as_ref(), app_store.as_ref());
    let mut written = 0;
    let mut kept = 0;
    for (name, scenario) in &plan.scenarios {
        let relative = format!("checkouts/{name}.json");
        let mut text = serde_json::to_string_pretty(scenario).map_err(|error| error.to_string())?;
        text.push('\n');
        if init::write_file(&directory, &relative, &text, args.force)? {
            eprintln!("  wrote   {relative}");
            written += 1;
        } else {
            eprintln!("  kept    {relative} (it exists; --force overwrites it)");
            kept += 1;
        }
    }
    for (name, reason) in &plan.skipped {
        eprintln!("  skipped {name}: {reason}");
    }
    if let Some(app) = &shopify_app {
        let discounts = Kinds::of(app).discount;
        let settings = function_settings(&store, app_store.as_ref(), app, &discounts);
        write_settings(&directory, &settings)?;
    }
    let target = match &shopify_app {
        Some(app) => format!(
            "{} Function(s) and {} checkout UI extension(s) of {}",
            app.functions.len(),
            app.ui_extensions.len(),
            app.name.as_deref().unwrap_or("the app")
        ),
        None => "no app: the generic set".to_string(),
    };
    eprintln!(
        "{}: wrote {written} scenario(s), kept {kept} existing one(s), for {target}",
        directory.join("checkouts").display()
    );
    match &app_dir {
        Some(app) => eprintln!(
            "next: `lsf checkout --app {}` lists them on its home page",
            app.display()
        ),
        None => eprintln!(
            "next: `lsf checkout --app <dir>` lists them on its home page. Run `lsf checkout init --app <dir>` to make them for an app"
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// Whether two paths are the same directory.
fn same_directory(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The data file `checkout init` writes the settings of the Functions in.
const SETTINGS_FILE: &str = "app-settings.json";

/// The data file of `directory` (other than [`SETTINGS_FILE`]) that has a `checkout` section:
/// only one file may have it.
fn checkout_file(directory: &Path) -> Option<String> {
    let entries = std::fs::read_dir(directory).ok()?;
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json") && name != SETTINGS_FILE)
        .collect();
    names.sort();
    names.into_iter().find(|name| {
        std::fs::read_to_string(directory.join(name))
            .ok()
            .and_then(|text| {
                serde_json::from_str::<Json>(&lsf_core::json::strip_comments(&text)).ok()
            })
            .is_some_and(|json| json.get("checkout").is_some())
    })
}

/// Writes the settings of the Functions the store data has none for in [`SETTINGS_FILE`], or
/// says what to add when another file has the `checkout` section.
fn write_settings(directory: &Path, settings: &[(String, Json, Source)]) -> Result<(), String> {
    if settings.is_empty() {
        return Ok(());
    }
    let functions: serde_json::Map<String, Json> = settings
        .iter()
        .map(|(handle, value, _)| (handle.clone(), value.clone()))
        .collect();
    let handles = |source: Source| {
        settings
            .iter()
            .filter(|(_, _, from)| *from == source)
            .map(|(handle, _, _)| handle.as_str())
            .collect::<Vec<_>>()
    };
    let (copied, empty) = (handles(Source::App), handles(Source::Empty));
    let mut said = Vec::new();
    if !copied.is_empty() {
        said.push(format!(
            "{} from the store data of the app",
            copied.join(", ")
        ));
    }
    if !empty.is_empty() {
        said.push(format!(
            "{} with empty values of the type their input query asks for: fill them in",
            empty.join(", ")
        ));
    }
    let said = said.join("; ");
    if let Some(file) = checkout_file(directory) {
        let snippet = serde_json::to_string_pretty(&json!(functions)).unwrap_or_default();
        eprintln!(
            "  settings: the Functions {said}. {file} has the `checkout` section: add them to its `checkout.functions`:\n{snippet}"
        );
        return Ok(());
    }
    let mut text = serde_json::to_string_pretty(&json!({
        "$schema": "./schema/store.schema.json",
        "checkout": {"functions": functions},
    }))
    .map_err(|error| error.to_string())?;
    text.push('\n');
    // Written again only when it is not there: what the developer filled in stays.
    if init::write_file(directory, SETTINGS_FILE, &text, false)? {
        eprintln!("  wrote   {SETTINGS_FILE}: the settings of {said}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_app() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app")
    }

    fn root(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lsf-checkout-init-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn init_in(root: &Path, app: Option<PathBuf>, force: bool) -> Result<ExitCode, String> {
        run(root, Some(&root.join("data")), InitArgs { app, force })
    }

    fn names_in(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root.join("data/checkouts"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .trim_end_matches(".json")
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn read(root: &Path, name: &str) -> Json {
        let text =
            std::fs::read_to_string(root.join(format!("data/checkouts/{name}.json"))).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn the_set_is_made_for_the_extensions_of_the_app() {
        let root = root("app");
        init_in(&root, Some(fixture_app()), false).unwrap();
        assert_eq!(
            names_in(&root),
            [
                "abroad",
                "all-extensions",
                "b2b",
                "cart-transform",
                "customer",
                "delivery-options",
                "discount-automatic",
                "discount-code",
                "guest",
                "large-cart",
                "payment-methods",
                "pre-order",
                "shop-look",
                "subscription",
                "validation",
            ]
        );
        // The store data was created with them, and everything validates.
        let (store, diagnostics) = load(
            &DataSource::Directory(root.join("data")),
            &LoadOptions::default(),
        );
        assert!(diagnostics.is_empty(), "{diagnostics}");
        for scenario in &store.scenarios {
            assert!(
                scenario.problems.is_empty(),
                "{}: {:?}",
                scenario.name,
                scenario.problems
            );
        }

        // A Function shows alone, with the checkout UI extensions around it.
        assert_eq!(
            read(&root, "cart-transform")["extensions"],
            json!(["bundle-transform", "gift-message", "order-notice"])
        );
        assert_eq!(
            read(&root, "discount-code")["extensions"],
            json!(["volume-discount", "gift-message", "order-notice"])
        );
        assert_eq!(read(&root, "validation")["extensions"][0], "quantity-limit");
        assert_eq!(
            read(&root, "delivery-options")["extensions"][0],
            "delivery-rules"
        );
        assert_eq!(
            read(&root, "payment-methods")["extensions"][0],
            "payment-rules"
        );
        // The discount code is entered, and the discount has it.
        let code = read(&root, "discount-code");
        assert_eq!(code["entered"]["discount_codes"], json!(["SAVE10"]));
        assert_eq!(
            code["checkout"]["discounts"][0]["function"],
            "volume-discount"
        );
        assert_eq!(code["checkout"]["discounts"][0]["code"], "SAVE10");
        assert_eq!(
            read(&root, "discount-automatic")["checkout"]["discounts"][0]["code"],
            Json::Null
        );
        // One scenario for each thing to look at: several rates and methods, a company, a
        // language, a plan with a deposit, a look, and the one with everything.
        assert_eq!(
            read(&root, "delivery-options")["checkout"]["shipping_rates"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            read(&root, "payment-methods")["checkout"]["payment_methods"]
                .as_array()
                .unwrap()
                .len(),
            6
        );
        assert_eq!(
            read(&root, "b2b")["session"]["customer"],
            "alex.morgan@example.com"
        );
        assert_eq!(
            read(&root, "b2b")["session"]["company_location"],
            "Northwind Seattle"
        );
        assert_eq!(read(&root, "abroad")["language"], "fr");
        assert_eq!(read(&root, "abroad")["session"]["country"], "FR");
        assert_eq!(read(&root, "shop-look")["variant"], "shop");
        assert!(
            read(&root, "pre-order")["session"]["cart"]["items"][0]["selling_plan"]
                .as_str()
                .unwrap()
                .starts_with("Pre-order")
        );
        assert!(
            read(&root, "subscription")["session"]["cart"]["items"][0]["selling_plan"]
                .as_str()
                .unwrap()
                .starts_with("Deliver every")
        );
        // The editor knows what the files are.
        assert_eq!(
            read(&root, "guest")["$schema"],
            "../schema/checkout-scenario.schema.json"
        );
        assert!(
            root.join("data/schema/checkout-scenario.schema.json")
                .is_file()
        );
        assert!(root.join("data/products").is_dir());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn without_an_app_the_set_is_generic() {
        let root = root("generic");
        init_in(&root, None, false).unwrap();
        assert_eq!(
            names_in(&root),
            [
                "abroad",
                "b2b",
                "customer",
                "guest",
                "large-cart",
                "pre-order",
                "shop-look",
                "subscription"
            ]
        );
        let (store, diagnostics) = load(
            &DataSource::Directory(root.join("data")),
            &LoadOptions::default(),
        );
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert!(
            store
                .scenarios
                .iter()
                .all(|scenario| scenario.problems.is_empty())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_app_with_nothing_that_matches_gets_the_generic_set() {
        let root = root("nothing");
        let app = root.join("app");
        std::fs::create_dir_all(app.join("extensions/theme")).unwrap();
        std::fs::write(app.join("shopify.app.toml"), "name = \"Nothing\"\n").unwrap();
        std::fs::write(
            app.join("extensions/theme/shopify.extension.toml"),
            "type = \"theme\"\nhandle = \"theme\"\n",
        )
        .unwrap();
        init_in(&root, Some(app), false).unwrap();
        assert_eq!(names_in(&root).len(), 8);
        assert!(!names_in(&root).contains(&"validation".to_string()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nothing_is_overwritten_without_force() {
        let root = root("force");
        init_in(&root, None, false).unwrap();
        let file = root.join("data/checkouts/guest.json");
        std::fs::write(&file, "{\"title\": \"mine\"}").unwrap();
        let store_file = root.join("data/store.json");
        std::fs::write(
            &store_file,
            std::fs::read_to_string(&store_file).unwrap() + "\n",
        )
        .unwrap();
        let store_before = std::fs::read_to_string(&store_file).unwrap();

        init_in(&root, None, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\"title\": \"mine\"}"
        );
        assert_eq!(std::fs::read_to_string(&store_file).unwrap(), store_before);

        init_in(&root, None, true).unwrap();
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("A guest, one line")
        );
        // The store data that exists is not the scenarios', whatever --force says.
        assert_eq!(std::fs::read_to_string(&store_file).unwrap(), store_before);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_scenarios_are_made_for_the_store_data_that_exists() {
        let root = root("own-store");
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // A store of its own: one product without a SKU, one country, one language, no
        // customer, no company, no selling plan.
        std::fs::write(
            data.join("store.json"),
            r#"{"products": [{"title": "Teapot", "price": "30.00"}]}"#,
        )
        .unwrap();
        init_in(&root, Some(fixture_app()), false).unwrap();
        let names = names_in(&root);
        for expected in [
            "guest",
            "customer",
            "shop-look",
            "large-cart",
            "cart-transform",
            "validation",
        ] {
            assert!(names.contains(&expected.to_string()), "{names:?}");
        }
        for skipped in ["b2b", "abroad", "subscription", "pre-order"] {
            assert!(
                !names.contains(&skipped.to_string()),
                "{skipped} in {names:?}"
            );
        }
        // The cart names the product by the id the data gave it, and the demo store was not added.
        let guest = read(&root, "guest");
        assert!(guest["session"]["cart"]["items"][0]["variant"].is_number());
        assert!(!data.join("products").exists());
        let (store, diagnostics) = load(&DataSource::Directory(data), &LoadOptions::default());
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert!(
            store
                .scenarios
                .iter()
                .all(|scenario| scenario.problems.is_empty())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// An app with a validation Function whose input query reads `$tags`, and its own store
    /// data `own` (`shopify-local/store.json`).
    fn app_with_settings(label: &str, own: Option<&str>) -> PathBuf {
        let app = root(&format!("app-{label}"));
        let extension = app.join("extensions/tagged");
        std::fs::create_dir_all(extension.join("src")).unwrap();
        std::fs::write(app.join("shopify.app.toml"), "name = \"Tagged\"\n").unwrap();
        std::fs::write(
            extension.join("shopify.extension.toml"),
            r#"api_version = "2026-10"

[[extensions]]
name = "Tagged"
handle = "tagged"
type = "function"

  [[extensions.targeting]]
  target = "cart.validations.generate.run"
  input_query = "src/run.graphql"
  export = "run"

  [extensions.input.variables]
  namespace = "$app:config"
  key = "settings"
"#,
        )
        .unwrap();
        std::fs::write(
            extension.join("src/run.graphql"),
            "query Input($tags: [String!]!) { cart { buyerIdentity { customer { hasAnyTag(tags: $tags) } } } }",
        )
        .unwrap();
        if let Some(own) = own {
            std::fs::create_dir_all(app.join("shopify-local")).unwrap();
            std::fs::write(app.join("shopify-local/store.json"), own).unwrap();
        }
        app
    }

    fn settings_of(root: &Path) -> Option<Json> {
        let text = std::fs::read_to_string(root.join("data/app-settings.json")).ok()?;
        Some(serde_json::from_str::<Json>(&text).unwrap()["checkout"]["functions"].clone())
    }

    #[test]
    fn the_settings_of_the_functions_come_from_the_store_data_of_the_app() {
        let app = app_with_settings(
            "own",
            Some(
                r#"{"checkout": {"functions": {"tagged": {"metafields": {
                    "$app:config.settings": {"type": "json", "value": {"tags": ["vip"]}}
                }}}}}"#,
            ),
        );
        let root = root("own-settings");
        init_in(&root, Some(app.clone()), false).unwrap();
        assert_eq!(
            settings_of(&root).unwrap()["tagged"]["metafields"]["$app:config.settings"]["value"],
            json!({"tags": ["vip"]})
        );
        let (_, diagnostics) = load(
            &DataSource::Directory(root.join("data")),
            &LoadOptions::default(),
        );
        assert!(diagnostics.is_empty(), "{diagnostics}");
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&app).ok();
    }

    #[test]
    fn a_store_data_with_a_checkout_section_is_told_what_to_add() {
        let app = app_with_settings("section", None);
        let root = root("section");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(
            root.join("data/store.json"),
            r#"{"products": [{"title": "Teapot", "price": "30.00"}], "checkout": {"tax_rate": 20}}"#,
        )
        .unwrap();
        init_in(&root, Some(app.clone()), false).unwrap();
        // Only one file may have the `checkout` section: no second one.
        assert_eq!(settings_of(&root), None);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&app).ok();
    }

    #[test]
    fn the_discounts_of_the_store_data_keep_their_settings() {
        let root = root("discounts");
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("store.json"),
            r#"{
                "products": [{"title": "Teapot", "price": "30.00"}],
                "checkout": {"discounts": [{
                    "function": "volume-discount", "title": "Volume", "code": "BULK",
                    "classes": ["PRODUCT"],
                    "metafields": {"$app:config.settings": {"type": "json", "value": {"minimumQuantity": 3, "percentage": 10}}}
                }]}
            }"#,
        )
        .unwrap();
        init_in(&root, Some(fixture_app()), false).unwrap();
        let code = read(&root, "discount-code");
        let discount = &code["checkout"]["discounts"][0];
        assert_eq!(discount["title"], "Volume");
        assert_eq!(discount["classes"], json!(["PRODUCT"]));
        assert_eq!(
            discount["metafields"]["$app:config.settings"]["value"]["percentage"],
            10
        );
        // It has a code of its own: that is the one to enter.
        assert_eq!(discount["code"], "BULK");
        assert_eq!(code["entered"]["discount_codes"], json!(["BULK"]));
        let automatic = read(&root, "discount-automatic");
        assert_eq!(automatic["checkout"]["discounts"][0]["code"], Json::Null);
        let (store, diagnostics) = load(&DataSource::Directory(data), &LoadOptions::default());
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert!(
            store
                .scenarios
                .iter()
                .all(|scenario| scenario.problems.is_empty())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_wrong_app_is_an_error() {
        let root = root("wrong-app");
        let error = init_in(&root, Some(root.join("nope")), false).unwrap_err();
        assert!(error.contains("is not a directory"), "{error}");
        // Nothing was created.
        assert!(!root.join("data").exists());
    }
}

//! What the commands and the server share to run the checkout of a Shopify app: the options
//! that select the app, the store data mapped to the checkout's configuration, and building the
//! Functions.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;
use lsf_checkout::app::{self, App, Selection};
use lsf_checkout::pipeline::DELIVERY_GROUP_HANDLE;
use lsf_checkout::{
    Address, CheckoutConfig, CheckoutState, DeliveryKind, DiscountClass, DiscountConfig, Metafield,
    PaymentMethodConfig, PaymentPlacement, ShippingRate,
};
use lsf_core::Store;
use lsf_core::store::model::{
    CheckoutDeliveryKind, CheckoutDiscountClass, CheckoutPaymentPlacement,
};
use lsf_core::store::{CheckoutScenario, CheckoutSettings};
use serde_json::Value as Json;

/// Which look the checkout page takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Variant {
    /// Shopify's one-page checkout.
    #[default]
    Standard,
    /// The look of the Shop app and Shop Pay.
    Shop,
}

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Variant::Standard => "standard",
            Variant::Shop => "shop",
        }
    }

    pub fn from_name(name: &str) -> Option<Variant> {
        Variant::from_str(name, true).ok()
    }
}

/// The options that select the app, shared by `serve`, `checkout` and `function run`.
#[derive(clap::Args, Clone, Debug)]
pub struct AppArgs {
    /// The directory of a Shopify app (the one with shopify.app.toml). Its Functions run and its
    /// checkout UI extensions show on the local checkout.
    #[arg(long, env = "LSF_APP", value_name = "DIR")]
    pub app: Option<PathBuf>,

    /// Use only this extension of the app (a Function or a checkout UI extension), by its
    /// handle. Repeat it for several. Without it, every extension of the app is used.
    #[arg(long = "extension", value_name = "HANDLE")]
    pub extensions: Vec<String>,

    /// Run the build command of each Function (`[extensions.build] command`) first.
    #[arg(long)]
    pub build: bool,
}

/// How a server shows the checkout of an app.
#[derive(Clone, Debug)]
pub struct CheckoutOptions {
    pub app_dir: PathBuf,
    /// The extensions named with `--extension`, checked to exist. All when empty.
    pub extensions: Vec<String>,
    /// The look a checkout takes unless the URL says another.
    pub variant: Variant,
    /// The server has no theme: it serves the checkout alone.
    pub standalone: bool,
}

/// Loads the app and the extensions the options select, and builds the Functions on request.
/// Problems of the app itself are in `App::diagnostics`; what stops the command is returned.
pub fn open(args: &AppArgs, app_dir: &Path) -> Result<(App, Selection), String> {
    let app = app::load(app_dir)?;
    let selection = Selection::resolve(&app, &args.extensions)?;
    if args.build {
        build(&app, &selection)?;
    }
    Ok((app, selection))
}

/// Prints what is wrong with the app, as the checkout would show it.
pub fn print_app_diagnostics(app: &App, selection: &Selection) {
    for diagnostic in &app.diagnostics {
        if diagnostic
            .extension
            .as_deref()
            .is_none_or(|handle| selection.contains(handle))
        {
            eprintln!("app: {diagnostic}");
        }
    }
}

/// Runs the build command of each selected Function, in its directory, as `shopify app build`
/// does.
pub fn build(app: &App, selection: &Selection) -> Result<(), String> {
    for function in selection.functions(app) {
        let Some(command) = &function.build_command else {
            eprintln!("build:  {} has no build command", function.handle);
            continue;
        };
        eprintln!("build:  {}: {command}", function.handle);
        let status = if cfg!(windows) {
            Command::new("cmd")
                .args(["/C", command])
                .current_dir(&function.dir)
                .status()
        } else {
            Command::new("sh")
                .args(["-c", command])
                .current_dir(&function.dir)
                .status()
        }
        .map_err(|error| format!("cannot run the build of {}: {error}", function.handle))?;
        if !status.success() {
            return Err(format!(
                "the build of {} failed ({status})",
                function.handle
            ));
        }
    }
    Ok(())
}

/// The checkout settings of the store data, as the checkout library takes them.
pub fn config_from_store(store: &Store) -> CheckoutConfig {
    config_from_settings(&store.checkout)
}

/// The settings of a checkout, as the checkout library takes them: those of the store data, or
/// those of a scenario.
pub fn config_from_settings(settings: &CheckoutSettings) -> CheckoutConfig {
    let metafields = |fields: &[lsf_core::store::CheckoutMetafield]| -> Vec<Metafield> {
        fields
            .iter()
            .map(|field| Metafield {
                namespace: field.namespace.clone(),
                key: field.key.clone(),
                kind: field.kind.clone(),
                value: field.value.clone(),
            })
            .collect()
    };
    CheckoutConfig {
        shipping_rates: settings
            .shipping_rates
            .iter()
            .map(|rate| ShippingRate {
                handle: rate.handle.clone(),
                title: rate.title.clone(),
                price: rate.price,
                description: rate.description.clone(),
                kind: match rate.kind {
                    CheckoutDeliveryKind::Shipping => DeliveryKind::Shipping,
                    CheckoutDeliveryKind::PickUp => DeliveryKind::PickUp,
                    CheckoutDeliveryKind::Local => DeliveryKind::Local,
                },
            })
            .collect(),
        payment_methods: settings
            .payment_methods
            .iter()
            .map(|method| PaymentMethodConfig {
                handle: method.handle.clone(),
                name: method.name.clone(),
                placements: method
                    .placements
                    .iter()
                    .map(|placement| match placement {
                        CheckoutPaymentPlacement::PaymentMethod => PaymentPlacement::PaymentMethod,
                        CheckoutPaymentPlacement::AcceleratedCheckout => {
                            PaymentPlacement::AcceleratedCheckout
                        }
                    })
                    .collect(),
            })
            .collect(),
        discounts: settings
            .discounts
            .iter()
            .map(|discount| DiscountConfig {
                function: discount.function.clone(),
                title: discount.title.clone(),
                code: discount.code.clone(),
                classes: discount
                    .classes
                    .iter()
                    .map(|class| match class {
                        CheckoutDiscountClass::Product => DiscountClass::Product,
                        CheckoutDiscountClass::Order => DiscountClass::Order,
                        CheckoutDiscountClass::Shipping => DiscountClass::Shipping,
                    })
                    .collect(),
                metafields: metafields(&discount.metafields),
            })
            .collect(),
        function_metafields: settings
            .functions
            .iter()
            .map(|(handle, fields)| (handle.clone(), metafields(fields)))
            .collect(),
        tax_rate_bps: settings.tax_rate_bps,
    }
}

/// The scenario a checkout was opened from, when it still exists and can run.
pub fn scenario_named<'a>(store: &'a Store, name: Option<&str>) -> Option<&'a CheckoutScenario> {
    store
        .scenario(name?)
        .filter(|scenario| scenario.problems.is_empty())
}

/// The settings a checkout runs with: those of its scenario, otherwise those of the store.
pub fn config_for(store: &Store, scenario: Option<&str>) -> CheckoutConfig {
    match scenario_named(store, scenario) {
        Some(scenario) => config_from_settings(&scenario.checkout),
        None => config_from_store(store),
    }
}

/// The extensions a checkout uses: those `--extension` selects, narrowed to the ones its
/// scenario names. The scenario must name extensions the app has.
pub fn selection_for(
    app: &App,
    requested: &[String],
    scenario: Option<&CheckoutScenario>,
) -> Result<Selection, String> {
    let selection = Selection::resolve(app, requested)?;
    match scenario.and_then(|scenario| scenario.extensions.as_ref()) {
        Some(handles) => {
            Selection::resolve(app, handles).map_err(|problem| {
                format!("the scenario names an extension the app lacks: {problem}")
            })?;
            Ok(selection.narrowed_to(handles))
        }
        None => Ok(selection),
    }
}

/// What the buyer has entered when a scenario opens: its `entered` section as the checkout
/// keeps it. The language is the request's business, not part of it.
pub fn entered_by(scenario: &CheckoutScenario) -> CheckoutState {
    let entered = &scenario.entered;
    let text = |value: &Option<String>| value.clone().filter(|text| !text.trim().is_empty());
    let address = entered.shipping_address.as_ref().map(|address| Address {
        first_name: text(&address.first_name),
        last_name: text(&address.last_name),
        company: text(&address.company),
        address1: text(&address.address1),
        address2: text(&address.address2),
        city: text(&address.city),
        country_code: text(&address.country_code).map(|code| code.to_ascii_uppercase()),
        province_code: text(&address.province_code),
        zip: text(&address.zip),
        phone: text(&address.phone),
    });
    CheckoutState {
        email: text(&entered.email),
        phone: text(&entered.phone),
        shipping_address: address.filter(|address| *address != Address::default()),
        selected_delivery: entered
            .delivery
            .iter()
            .map(|handle| (DELIVERY_GROUP_HANDLE.to_string(), handle.clone()))
            .collect(),
        selected_payment: entered.payment.clone(),
        discount_codes: entered.discount_codes.clone(),
        scenario: Some(scenario.name.clone()),
        ..CheckoutState::default()
    }
}

/// The translations of a checkout UI extension: `locales/<language>.json`, or
/// `locales/<language>.default.json` for the default one.
pub struct Translations {
    /// Language → the file's content.
    pub strings: Json,
    /// The language of the `.default.json` file, if the extension declares one.
    pub default_language: Option<String>,
}

pub fn translations(extension_dir: &Path) -> Translations {
    let mut strings = serde_json::Map::new();
    let mut default_language = None;
    let Ok(entries) = std::fs::read_dir(extension_dir.join("locales")) else {
        return Translations {
            strings: Json::Object(strings),
            default_language,
        };
    };
    let mut files: Vec<_> = entries.filter_map(Result::ok).collect();
    files.sort_by_key(|entry| entry.file_name());
    for entry in files {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        let language = stem.strip_suffix(".default");
        let language = language.unwrap_or(stem);
        if let Some(content) = std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
        {
            if stem.ends_with(".default") {
                default_language = Some(language.to_string());
            }
            strings.insert(language.to_string(), content);
        }
    }
    Translations {
        strings: Json::Object(strings),
        default_language,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_data_maps_to_the_checkout_configuration() {
        let overlay = serde_json::json!({"checkout": {
            "shipping_rates": [{"handle": "pickup", "title": "Pick up", "price": 0, "kind": "pick_up"}],
            "discounts": [{
                "function": "volume-discount",
                "code": "SAVE",
                "classes": ["ORDER", "SHIPPING"],
                "metafields": {"$app:config.settings": {"type": "json", "value": {"percentage": 5}}}
            }],
            "functions": {"delivery-rules": {"metafields": {"$app:config.mode": "strict"}}},
            "tax_rate": 20
        }});
        let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
            &lsf_core::store::load::DataSource::Demo,
            &lsf_core::store::load::LoadOptions::default(),
            Some(&overlay),
        );
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let config = config_from_store(&store);
        assert_eq!(config.shipping_rates[0].handle, "pickup");
        assert_eq!(config.shipping_rates[0].kind, DeliveryKind::PickUp);
        // Not mentioned: the defaults.
        assert_eq!(config.payment_methods.len(), 3);
        assert_eq!(
            config.payment_methods[0].placements,
            [PaymentPlacement::PaymentMethod]
        );
        let discount = &config.discounts[0];
        assert_eq!(discount.code.as_deref(), Some("SAVE"));
        assert_eq!(
            discount.classes,
            [DiscountClass::Order, DiscountClass::Shipping]
        );
        assert_eq!(discount.metafields[0].namespace, "$app:config");
        assert_eq!(discount.metafields[0].value, r#"{"percentage":5}"#);
        assert_eq!(
            config.function_metafields["delivery-rules"][0].value,
            "strict"
        );
        assert_eq!(config.tax_rate_bps, 2000);
    }

    #[test]
    fn the_default_locale_of_an_extension_is_kept() {
        let dir = std::env::temp_dir().join(format!("lsf-translations-{}", std::process::id()));
        let locales = dir.join("locales");
        std::fs::create_dir_all(&locales).unwrap();
        std::fs::write(locales.join("en.json"), r#"{"hello": "Hello"}"#).unwrap();
        std::fs::write(locales.join("fr.default.json"), r#"{"hello": "Bonjour"}"#).unwrap();
        std::fs::write(locales.join("broken.json"), "{").unwrap();
        let found = translations(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found.default_language.as_deref(), Some("fr"));
        assert_eq!(
            found.strings,
            serde_json::json!({"en": {"hello": "Hello"}, "fr": {"hello": "Bonjour"}})
        );
    }

    #[test]
    fn variants_have_names() {
        assert_eq!(Variant::from_name("shop"), Some(Variant::Shop));
        assert_eq!(Variant::from_name("Standard"), Some(Variant::Standard));
        assert_eq!(Variant::from_name("other"), None);
        assert_eq!(Variant::Shop.name(), "shop");
    }
}

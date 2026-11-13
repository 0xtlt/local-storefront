//! What the commands and the server share to run the checkout of a Shopify app: the options
//! that select the app, the store data mapped to the checkout's configuration, and building the
//! Functions.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;
use lsf_app::checkout::pipeline::DELIVERY_GROUP_HANDLE;
use lsf_app::checkout::{
    Address, CheckoutConfig, CheckoutState, DeliveryKind, DiscountClass, DiscountConfig, Metafield,
    PaymentMethodConfig, PaymentPlacement, ShippingRate,
};
use lsf_app::config::{self, App, Build, Selection};
use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::model::{
    CheckoutDeliveryKind, CheckoutDiscountClass, CheckoutPaymentPlacement,
};
use lsf_core::store::{CheckoutScenario, CheckoutSettings};
use lsf_core::{Session, Store};
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

/// The options that select the app, shared by `serve`, `app dev` and `app function run`.
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

    /// Build the extensions first, as `lsf app build` does: each Function (its
    /// `[extensions.build] command`, or lsf's own build of a JavaScript one), then each UI
    /// extension that names its sources. With `--watch`, build every Function at start, not only
    /// those that changed.
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
    /// `--no-functions`: the app's Functions are left out, so none runs.
    pub no_functions: bool,
}

impl CheckoutOptions {
    /// The extensions a checkout of `scenario` uses ([`selection_for`]). With `--no-functions`,
    /// its UI extensions alone: the app has no Function then (`ServerState::load_app`), and a
    /// discount of the data whose Function is not run is left out, not an error.
    pub fn selection(
        &self,
        app: &App,
        scenario: Option<&CheckoutScenario>,
    ) -> Result<Selection, String> {
        if !self.no_functions {
            return selection_for(app, &self.extensions, scenario);
        }
        let mut handles: Vec<String> = Selection::resolve(app, &self.extensions)?
            .ui_extensions(app)
            .map(|extension| extension.handle.clone())
            .collect();
        if let Some(named) = scenario.and_then(|scenario| scenario.extensions.as_ref()) {
            handles.retain(|handle| named.contains(handle));
        }
        Ok(Selection::only(handles))
    }
}

/// Loads the app and the extensions the options select, and builds the Functions on request.
/// Problems of the app itself are in `App::diagnostics`, read after the build: a module it wrote
/// is not missing. What stops the command is returned.
pub fn open(args: &AppArgs, app_dir: &Path) -> Result<(App, Selection), String> {
    let app = config::load(app_dir)?;
    let selection = Selection::resolve(&app, &args.extensions)?;
    if !args.build {
        return Ok((app, selection));
    }
    build(&app, &selection)?;
    let app = config::load(app_dir)?;
    let selection = Selection::resolve(&app, &args.extensions)?;
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

/// Builds the selected extensions, as `shopify app build` does: the Functions
/// ([`build_functions`]), then the checkout UI extensions ([`build_ui_extensions`]).
pub fn build(app: &App, selection: &Selection) -> Result<(), String> {
    build_functions(app, selection)?;
    build_ui_extensions(app, selection)
}

/// Builds each selected Function: its build command, run in its directory, or for a JavaScript
/// Function without one, lsf's build of it.
pub fn build_functions(app: &App, selection: &Selection) -> Result<(), String> {
    for function in selection.functions(app) {
        // What it is built from, read before the build: a file saved meanwhile is not taken to
        // be in the module ([`crate::freshness`]).
        let sources = crate::freshness::sources(&app.root, function);
        match function.build() {
            None => {
                // Nothing was built: what the module was built from is not known any better.
                eprintln!("build:  {} has no build command", function.handle);
                continue;
            }
            Some(Build::Command(command)) => {
                eprintln!("build:  {}: {command}", function.handle);
                for (query, root) in refresh_typegen(&function.dir, &function.wasm_path) {
                    eprintln!(
                        "build:  {}: {} changed: {} touched, for cargo to compile the crate again",
                        function.handle,
                        query.display(),
                        root.display()
                    );
                }
                let status = build_command(command, &function.dir)
                    .status()
                    .map_err(|error| {
                        format!("cannot run the build of {}: {error}", function.handle)
                    })?;
                if !status.success() {
                    return Err(crate::output::fixable(format!(
                        "the build of {} failed ({status})",
                        function.handle
                    )));
                }
                link(function)?;
            }
            Some(Build::JavaScript) => {
                eprintln!("build:  {}: JavaScript, built by lsf", function.handle);
                lsf_app::checkout::javascript::build(function, &app.root).map_err(|error| {
                    crate::output::fixable(format!(
                        "the build of {} failed: {error}",
                        function.handle
                    ))
                })?;
            }
        }
        if function.built() {
            let sources = sources.after_build(crate::freshness::sources(&app.root, function));
            crate::freshness::record(&app.root, &function.handle, &function.wasm_path, &sources);
            if let Some(problem) = lsf_app::checkout::runner::oversized_module(&function.wasm_path)
            {
                eprintln!("warning: {}: {problem}", function.handle);
            }
        }
    }
    Ok(())
}

/// Builds the Functions of `selection` whose module is older than their files, as
/// `shopify app function run` builds a Function before it runs it, and answers the app as it
/// is then. One lsf cannot build (no build command) runs its old module, with a warning.
pub fn freshen(app_dir: &Path, app: App, selection: &Selection) -> Result<App, String> {
    let mut stale = Vec::new();
    for function in selection.functions(&app) {
        let Some(file) = crate::freshness::stale(&app.root, function) else {
            continue;
        };
        let file = crate::freshness::in_app(&app.root, function, &file);
        if function.build().is_some() {
            eprintln!(
                "build:  {}: {file} changed after its module was built: building it first",
                function.handle
            );
            stale.push(function.handle.clone());
        } else {
            eprintln!(
                "warning: {}: {file} changed after its module was built, and it has no build \
                 command lsf can run: this runs the old module",
                function.handle
            );
        }
    }
    if stale.is_empty() {
        return Ok(app);
    }
    build_functions(&app, &Selection::only(stale))?;
    config::load(app_dir)
}

/// The Functions of `selection` whose module is older than their files: each handle, with the
/// file that changed by its path in the app.
pub fn stale_functions(app: &App, selection: &Selection) -> Vec<(String, String)> {
    selection
        .functions(app)
        .filter_map(|function| {
            let file = crate::freshness::stale(&app.root, function)?;
            Some((
                function.handle.clone(),
                crate::freshness::in_app(&app.root, function, &file),
            ))
        })
        .collect()
}

/// A Rust Function's types come from its `.graphql` files (the input queries, `schema.graphql`),
/// which the `typegen` macro of the `shopify_function` crate reads without telling cargo: after
/// a change to one of them alone, `cargo build` compiles nothing and the old module stays. So
/// when one is newer than the module `module`, the roots of the crate in `dir` (`src/main.rs`,
/// `src/lib.rs`) are touched (their time only), as one does by hand, and cargo compiles the
/// crate again. Returns the newest of those files and each root touched, from `dir`.
pub fn refresh_typegen(dir: &Path, module: &Path) -> Vec<(PathBuf, PathBuf)> {
    if !dir.join("Cargo.toml").is_file() {
        return Vec::new();
    }
    let built = std::fs::metadata(module)
        .and_then(|meta| meta.modified())
        .ok();
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir.join(&relative)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || ["target", "node_modules", "dist"].contains(&&*name) {
                continue;
            }
            let path = relative.join(&*name);
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
            } else if name.ends_with(".graphql")
                && let Ok(modified) = entry.metadata().and_then(|meta| meta.modified())
                && newest.as_ref().is_none_or(|(time, _)| modified > *time)
            {
                newest = Some((modified, path));
            }
        }
    }
    // No module yet: cargo has not compiled the crate, and does.
    let (Some(built), Some((modified, query))) = (built, newest) else {
        return Vec::new();
    };
    if modified <= built {
        return Vec::new();
    }
    let now = std::time::SystemTime::now();
    ["src/main.rs", "src/lib.rs"]
        .into_iter()
        .filter(|root| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(dir.join(root))
                .and_then(|file| file.set_modified(now))
                .is_ok()
        })
        .map(|root| (query.clone(), PathBuf::from(root)))
        .collect()
}

/// What `shopify app build` does after a Function's build command: a module that calls the
/// Function Wasm API by name (a Rust Function's, as `cargo build` writes it) is linked to the
/// host's provider with Shopify's trampoline. A module the command did not write is left to the
/// diagnostics that say so.
pub fn link(function: &config::FunctionExtension) -> Result<(), String> {
    if !function.wasm_path.is_file() {
        return Ok(());
    }
    if lsf_app::checkout::trampoline::apply(&function.wasm_path).map_err(|error| {
        crate::output::fixable(format!("the build of {} failed: {error}", function.handle))
    })? {
        eprintln!(
            "build:  {}: linked to the Function Wasm API (shopify-function-trampoline)",
            function.handle
        );
    }
    Ok(())
}

/// Bundles each selected checkout UI extension that names its sources (`module`), as
/// `shopify app build` does, without Node. One that does not is left as it is: its bundle is
/// the one `dist/` holds.
pub fn build_ui_extensions(app: &App, selection: &Selection) -> Result<(), String> {
    for extension in selection.ui_extensions(app) {
        if !extension.buildable() {
            continue;
        }
        let started = std::time::Instant::now();
        lsf_app::checkout::javascript::build_extension(extension, &app.root).map_err(|error| {
            crate::output::fixable(format!("the build of {} failed: {error}", extension.handle))
        })?;
        eprintln!(
            "build:  {}: built by lsf in {}ms",
            extension.handle,
            started.elapsed().as_millis()
        );
    }
    Ok(())
}

/// A setting of `checkout.extensions` that does not fit the app: an extension it does not have,
/// a key its `[extensions.settings]` does not declare, a value of the wrong type.
pub struct SettingsProblem {
    pub handle: String,
    /// The scenario's file (`checkouts/banner.json`), or `None` for the store data's.
    pub file: Option<String>,
    /// Where it is, as a JSON pointer: `/checkout/extensions/banner/settings/title`.
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for SettingsProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} ({} at {})",
            self.message,
            self.file
                .as_deref()
                .unwrap_or("the checkout section of the store data"),
            self.path
        )
    }
}

/// What is wrong with the settings the store data and its scenarios give the UI extensions of
/// `app` (`checkout.extensions.<handle>.settings`). A scenario is checked for what it changes.
pub fn settings_problems(app: &App, store: &Store) -> Vec<SettingsProblem> {
    let mut problems = Vec::new();
    let mut check =
        |file: Option<&str>, handle: &str, values: &indexmap::IndexMap<String, Json>| {
            let at = format!("/checkout/extensions/{handle}");
            match app.ui_extension(handle) {
                None => {
                    let handles = app
                        .ui_extensions
                        .iter()
                        .map(|extension| extension.handle.as_str());
                    let close = lsf_core::util::closest_match(handle, handles)
                        .map(|close| format!(": did you mean `{close}`?"))
                        .unwrap_or_default();
                    problems.push(SettingsProblem {
                        handle: handle.to_string(),
                        file: file.map(str::to_string),
                        path: at,
                        message: format!("the app has no UI extension `{handle}`{close}"),
                    });
                }
                Some(extension) => {
                    for (key, message) in extension.check_settings(values) {
                        problems.push(SettingsProblem {
                            handle: handle.to_string(),
                            file: file.map(str::to_string),
                            path: format!("{at}/settings/{key}"),
                            message,
                        });
                    }
                }
            }
        };
    for (handle, values) in &store.checkout.extensions {
        check(None, handle, values);
    }
    for scenario in &store.scenarios {
        for (handle, values) in &scenario.checkout.extensions {
            if store.checkout.extensions.get(handle) != Some(values) {
                check(Some(&scenario.file), handle, values);
            }
        }
    }
    problems
}

/// A discount of `checkout.discounts` that runs `function`, for a hint: with its settings, the
/// metafield its `[extensions.input.variables]` names, or the one of the discount its input
/// query reads.
pub fn discount_example(function: &config::FunctionExtension) -> String {
    let metafield = function
        .input_variables
        .as_ref()
        .map(|variables| (variables.namespace.clone(), variables.key.clone()))
        .or_else(|| {
            function.targets.iter().find_map(|target| {
                lsf_app::checkout::input::owner_metafields(&target.input_query)
                    .into_iter()
                    .next()
            })
        });
    let settings = metafield.map_or_else(String::new, |(namespace, key)| {
        format!(
            ", \"metafields\": {{\"{namespace}.{key}\": {{\"type\": \"json\", \"value\": {{…}}}}}}"
        )
    });
    format!(
        "{{\"function\": \"{}\", \"title\": \"10% off\"{settings}}}",
        function.handle
    )
}

/// The app `dir` is in: the closest folder, from `dir` up, that has a `shopify.app.toml`.
pub fn app_around(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|dir| dir.join("shopify.app.toml").is_file())
        .map(Path::to_path_buf)
}

/// The app of a command (`lsf app check`): `--app`, or the one the current folder is in.
pub fn app_of(app: Option<&Path>, command: &str) -> Result<PathBuf, String> {
    match app {
        Some(app) => Ok(app.to_path_buf()),
        None => std::env::current_dir()
            .ok()
            .and_then(|dir| app_around(&dir))
            .ok_or_else(|| {
                format!(
                    "`{}` needs the app: run it in the app's folder, or give `--app DIR` \
                     (or LSF_APP)",
                    as_typed(command)
                )
            }),
    }
}

/// A command of lsf (`lsf app check`) as the user types it: [`lsf_core::util::command`] for
/// `lsf`, `npx local-storefront app check` when installed with npm.
pub fn as_typed(command: &str) -> String {
    match command.strip_prefix("lsf ") {
        Some(rest) => format!("{} {rest}", lsf_core::util::command()),
        None => command.to_string(),
    }
}

/// `lsf app function build` and `lsf app build`: builds the extensions of an app (`handles`, or
/// all), with `build`, and says what it wrote.
pub fn run_build(
    app: Option<&Path>,
    handles: &[String],
    command: &str,
    build: fn(&App, &Selection) -> Result<(), String>,
) -> Result<std::process::ExitCode, String> {
    let app_dir = app_of(app, command)?;
    let shopify_app = config::load(&app_dir)?;
    // `lsf app build` checks the theme app extensions as Shopify does: there is nothing to build.
    let themes = command == "lsf app build";
    let (theme_handles, handles): (Vec<String>, Vec<String>) = handles
        .iter()
        .cloned()
        .partition(|handle| themes && shopify_app.theme_extension(handle).is_some());
    let selection = if handles.is_empty() && !theme_handles.is_empty() {
        Selection::only(Vec::<String>::new())
    } else {
        Selection::resolve(&shopify_app, &handles)?
    };
    build(&shopify_app, &selection)?;
    let shopify_app = config::load(&app_dir)?;
    let mut built = 0;
    for function in selection.functions(&shopify_app) {
        if function.built() {
            eprintln!(
                "built:  {}: {}",
                function.handle,
                function.wasm_path.display()
            );
            built += 1;
        }
    }
    for extension in selection.ui_extensions(&shopify_app) {
        if extension.buildable() && extension.built() {
            eprintln!(
                "built:  {}: {}",
                extension.handle,
                extension.bundle.display()
            );
            built += 1;
        }
    }
    let (checked, failed) = if themes && (handles.is_empty() || !theme_handles.is_empty()) {
        crate::theme_extensions::validate_app(&shopify_app, &theme_handles)
    } else {
        (0, false)
    };
    if failed {
        // The extension's to fix, as a build that fails: exit 1, not 2.
        return Err(crate::output::fixable(
            "a theme app extension has errors that Shopify rejects (see above)".to_string(),
        ));
    }
    if built == 0 && checked == 0 {
        return Err(
            "nothing was built: the app has no extension lsf or its build command builds"
                .to_string(),
        );
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// The command `--watch` runs to build a JavaScript Function: lsf itself
/// (`lsf app function build`), in a process of its own as any build.
pub fn javascript_build_command(app_root: &Path, handle: &str) -> String {
    let lsf = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lsf"));
    format!(
        "{} app function build --app {} {}",
        shell_quote(&lsf.to_string_lossy()),
        shell_quote(&app_root.to_string_lossy()),
        shell_quote(handle)
    )
}

/// `text` as one word of the shell [`build_command`] runs (a path on Windows has no `"`).
fn shell_quote(text: &str) -> String {
    if cfg!(windows) {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The build command of a Function, to run in its folder `dir` through the shell.
pub fn build_command(command: &str, dir: &Path) -> Command {
    #[cfg(windows)]
    let mut process = {
        use std::os::windows::process::CommandExt as _;
        let mut process = Command::new("cmd");
        // The command as written: an argument would be quoted again the way programs read
        // their arguments, which cmd does not. `/S` takes off the quotes around it only.
        process.raw_arg(format!("/S /C \"{command}\""));
        process
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    };
    process.current_dir(dir);
    process
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
                id: Some(discount.id),
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
        // The server adds those of the Admin API (`price_with` of its checkout).
        code_discounts: Vec::new(),
        function_metafields: settings
            .functions
            .iter()
            .map(|(handle, fields)| (handle.clone(), metafields(fields)))
            .collect(),
        // As on Shopify: a Function that is not a discount runs through an enabled owner.
        owners: Some(settings.owners.clone()),
        block_on_failure: settings
            .block_on_failure
            .iter()
            .filter(|(_, block)| **block)
            .map(|(handle, _)| handle.clone())
            .collect(),
        // The server says so when the Admin API uninstalled the app (`ServerState::checkout_config`).
        uninstalled: false,
        tax_rate_bps: settings.tax_rate_bps,
    }
}

/// The scenario a checkout was opened from, when it still exists and can run.
pub fn scenario_named<'a>(store: &'a Store, name: Option<&str>) -> Option<&'a CheckoutScenario> {
    store
        .scenario(name?)
        .filter(|scenario| scenario.problems.is_empty())
}

/// The settings a checkout runs with: those of its scenario, otherwise those of the store. A
/// scenario that names its extensions runs the Functions it names whether or not the shop has
/// an enabled owner of them (a payment customization, …): it tries a Function alone. Otherwise
/// only the Functions with an enabled owner run, as on Shopify.
pub fn config_for(store: &Store, scenario: Option<&str>) -> CheckoutConfig {
    match scenario_named(store, scenario) {
        Some(scenario) => {
            let mut config = config_from_settings(&scenario.checkout);
            if scenario.extensions.is_some() {
                config.owners = None;
            }
            config
        }
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
    match scenario {
        Some(scenario) => narrow_to_scenario(app, selection, scenario),
        None => Ok(selection),
    }
}

/// `selection` narrowed to the extensions a scenario names, which must be ones of the app.
pub fn narrow_to_scenario(
    app: &App,
    selection: Selection,
    scenario: &CheckoutScenario,
) -> Result<Selection, String> {
    match &scenario.extensions {
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
    let address = entered.shipping_address.as_ref().map(|address| {
        Address {
            first_name: address.first_name.clone(),
            last_name: address.last_name.clone(),
            company: address.company.clone(),
            address1: address.address1.clone(),
            address2: address.address2.clone(),
            city: address.city.clone(),
            country_code: address.country_code.clone(),
            province_code: address.province_code.clone(),
            zip: address.zip.clone(),
            phone: address.phone.clone(),
        }
        .normalized()
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

/// The store as a checkout prices it: in the currency of the buyer's country (the one of the
/// shipping address when the store sells there, else the one of the session), at its market's
/// rate or fixed prices, as Shopify's checkout follows the market of the address. The served
/// checkout, `lsf app check` and `lsf app function run` all run the Functions on it, so that
/// they read the same cart and `presentmentCurrencyRate`.
pub fn presented_for_checkout(
    store: &std::sync::Arc<Store>,
    session: &Session,
    checkout: &CheckoutState,
) -> std::sync::Arc<Store> {
    let country = checkout
        .shipping_country()
        .filter(|code| store.country(code).is_some())
        .map(str::to_string)
        .unwrap_or_else(|| lsf_app::checkout::input::buyer_country(store, session));
    store.presented_for(&country)
}

/// Opens `scenario` on `session`, which the caller made (`Session::initial` for a command, the
/// server's initial session for the server): applies its cart and buyer, then the country of the
/// address it entered, as entering the address does (markets decide on it). Returns what the
/// buyer entered, and what is wrong with the cart or the buyer of the scenario.
pub fn apply_scenario(
    store: &Store,
    scenario: &CheckoutScenario,
    session: &mut Session,
) -> (CheckoutState, Diagnostics) {
    let mut problems = Diagnostics::new();
    lsf_core::store::build::apply_session_input(
        store,
        &scenario.session,
        session,
        &mut problems,
        &scenario.file,
        "/session",
    );
    let entered = entered_by(scenario);
    if let Some(country) = entered
        .shipping_address
        .as_ref()
        .and_then(|address| address.country_code.as_deref())
        .and_then(|code| store.country(code))
    {
        session.country = Some(country.iso_code.clone());
    }
    (entered, problems)
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

/// A handle (or a scenario name) as a name of its own, for file names and page anchors: `a-z`,
/// `0-9` and `-` stay (all a handle of Shopify's may have), every other byte is `_` and two hex
/// digits, `_` included. Two names never share one.
pub fn encoded_handle(handle: &str) -> String {
    let mut name = String::with_capacity(handle.len());
    for byte in handle.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' {
            name.push(char::from(byte));
        } else {
            name.push_str(&format!("_{byte:02x}"));
        }
    }
    name
}

/// An amount in cents as people read it: `19.99 EUR`.
pub fn display_money(cents: i64, currency: &str) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    format!(
        "{sign}{}.{:02} {currency}",
        cents.abs() / 100,
        cents.abs() % 100
    )
}

/// Writes the demo store into `dir`, with the scenarios `(file, content)` in `checkouts/`: the
/// store data of the tests that run scenarios.
#[cfg(test)]
pub fn write_demo_scenarios(dir: &Path, scenarios: &[(&str, &str)]) {
    for (name, content) in lsf_core::store::demo::FILES {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
        std::fs::write(path, content).expect("demo file");
    }
    std::fs::create_dir_all(dir.join("checkouts")).expect("checkouts");
    for (name, content) in scenarios {
        std::fs::write(dir.join("checkouts").join(name), content).expect("scenario");
    }
}

/// Test data: the store data of `dir` (the demo store of [`write_demo_scenarios`]) with France
/// in a market that pays in euros at 0.5.
#[cfg(test)]
pub fn sell_to_france_in_euros_at_half(dir: &Path) {
    let path = dir.join("store.json");
    let mut store: Json =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("store.json")).expect("json");
    store["localization"]["markets"] = serde_json::json!([
        {"handle": "europe", "countries": ["FR"], "currency": "EUR", "rate": "0.5"}
    ]);
    std::fs::write(path, store.to_string()).expect("store.json");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scenario of the demo store in which the buyer entered `entered`.
    fn scenario_entering(entered: Json) -> CheckoutScenario {
        let (store, _) = lsf_core::store::load::load(
            &lsf_core::store::load::DataSource::Demo,
            &lsf_core::store::load::LoadOptions::default(),
        );
        CheckoutScenario {
            name: "entered".into(),
            file: "checkouts/entered.json".into(),
            title: "Entered".into(),
            description: None,
            session: Default::default(),
            entered: serde_json::from_value(entered).unwrap(),
            language: None,
            look: None,
            extensions: None,
            checkout: store.checkout,
            overrides: Vec::new(),
            expect: None,
            problems: Vec::new(),
        }
    }

    /// The `.graphql` files of a Rust Function are read by its `typegen` macro, which cargo does
    /// not know of: one newer than the module has the crate's roots touched, and only then.
    #[test]
    fn a_query_newer_than_the_module_touches_the_roots_of_the_crate() {
        let dir = std::env::temp_dir().join(format!("lsf-typegen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        let at = |seconds: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        let write = |path: &str, time: u64| {
            std::fs::write(dir.join(path), path).unwrap();
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(dir.join(path))
                .unwrap();
            file.set_modified(at(time)).unwrap();
        };
        let modified = |path: &str| {
            std::fs::metadata(dir.join(path))
                .unwrap()
                .modified()
                .unwrap()
        };
        write("src/main.rs", 1_000);
        write("src/run.graphql", 1_000);
        write("schema.graphql", 1_000);
        write("target/function.wasm", 2_000);
        let module = dir.join("target/function.wasm");

        // Not a crate.
        write("src/run.graphql", 3_000);
        assert!(refresh_typegen(&dir, &module).is_empty());
        write("Cargo.toml", 1_000);
        // A query newer than the module.
        assert_eq!(
            refresh_typegen(&dir, &module),
            vec![(
                PathBuf::from("src/run.graphql"),
                PathBuf::from("src/main.rs")
            )]
        );
        assert!(modified("src/main.rs") > at(3_000));
        assert_eq!(
            std::fs::read_to_string(dir.join("src/main.rs")).unwrap(),
            "src/main.rs"
        );
        // The module built since: nothing to touch.
        write("src/main.rs", 1_000);
        write("target/function.wasm", 4_000);
        assert!(refresh_typegen(&dir, &module).is_empty());
        assert_eq!(modified("src/main.rs"), at(1_000));
        // The schema counts too.
        write("schema.graphql", 5_000);
        assert_eq!(refresh_typegen(&dir, &module).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A Rust Function's build command writes the module `cargo build` writes; the build links
    /// it to the Function Wasm API, as `shopify app build` does, and it runs.
    #[test]
    fn a_build_links_what_a_build_command_wrote_with_shopify_s_trampoline() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures");
        let root = std::env::temp_dir().join(format!("lsf-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("extensions/limit");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(root.join("shopify.app.toml"), "name = \"App\"\n").unwrap();
        std::fs::copy(
            fixtures.join("app/extensions/quantity-limit/src/run.graphql"),
            dir.join("src/run.graphql"),
        )
        .unwrap();
        let raw = fixtures.join("trampoline/quantity-limit.raw.wasm");
        std::fs::write(
            dir.join("shopify.extension.toml"),
            format!(
                r#"api_version = "2027-01"

[[extensions]]
name = "Limit"
handle = "limit"
type = "function"

  [[extensions.targeting]]
  target = "purchase.validation.run"
  input_query = "src/run.graphql"
  export = "run"

  [extensions.build]
  command = "mkdir -p dist && cp '{}' dist/function.wasm"
"#,
                raw.display()
            ),
        )
        .unwrap();
        let shopify_app = config::load(&root).unwrap();
        build_functions(&shopify_app, &Selection::all()).unwrap();
        let module = dir.join("dist/function.wasm");
        assert_eq!(
            std::fs::read(&module).unwrap(),
            std::fs::read(fixtures.join("app/extensions/quantity-limit/dist/function.wasm"))
                .unwrap()
        );
        let execution = lsf_app::checkout::runner::run(
            &module,
            "run",
            &serde_json::json!({
                "cart": { "lines": [{ "quantity": 1 }], "buyerIdentity": null },
                "buyerJourney": { "step": "CHECKOUT_COMPLETION" },
                "validation": { "metafield": null }
            }),
        );
        assert_eq!(execution.error, None, "{}", execution.logs);
        std::fs::remove_dir_all(&root).ok();
    }

    /// `--build` then the diagnostics: those of the app as the build left it, so a module the
    /// build wrote is not reported missing.
    #[test]
    fn the_diagnostics_after_a_build_are_those_of_the_built_app() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures");
        let root = std::env::temp_dir().join(format!("lsf-open-built-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("extensions/limit");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(root.join("shopify.app.toml"), "name = \"App\"\n").unwrap();
        std::fs::copy(
            fixtures.join("app/extensions/quantity-limit/src/run.graphql"),
            dir.join("src/run.graphql"),
        )
        .unwrap();
        let built = fixtures.join("app/extensions/quantity-limit/dist/function.wasm");
        std::fs::write(
            dir.join("shopify.extension.toml"),
            format!(
                r#"api_version = "2025-07"

[[extensions]]
name = "Limit"
handle = "limit"
type = "function"

  [[extensions.targeting]]
  target = "purchase.validation.run"
  input_query = "src/run.graphql"
  export = "run"

  [extensions.build]
  command = "mkdir -p dist && cp '{}' dist/function.wasm"
"#,
                built.display()
            ),
        )
        .unwrap();
        let missing = |app: &App| {
            app.diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.starts_with("no Wasm module"))
                .count()
        };
        let args = |build| AppArgs {
            app: Some(root.clone()),
            extensions: Vec::new(),
            build,
        };
        let (unbuilt, _) = open(&args(false), &root).unwrap();
        assert_eq!(missing(&unbuilt), 1, "{:?}", unbuilt.diagnostics);
        let (built, _) = open(&args(true), &root).unwrap();
        assert_eq!(missing(&built), 0, "{:?}", built.diagnostics);
        assert!(built.function("limit").unwrap().built());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_scenario_address_keeps_what_is_written_but_blank_fields_and_capitalizes_the_country() {
        let entered = |address: Json| {
            entered_by(&scenario_entering(
                serde_json::json!({"shipping_address": address}),
            ))
            .shipping_address
        };
        let address = entered(serde_json::json!({
            "first_name": " Ada ", "last_name": "  ", "city": "Paris", "country_code": "fr"
        }))
        .unwrap();
        assert_eq!(address.first_name.as_deref(), Some(" Ada "), "not trimmed");
        assert_eq!(address.last_name, None);
        assert_eq!(address.city.as_deref(), Some("Paris"));
        assert_eq!(address.country_code.as_deref(), Some("FR"));
        // The country is capitalized, not trimmed.
        let spaced = entered(serde_json::json!({"country_code": " fr "})).unwrap();
        assert_eq!(spaced.country_code.as_deref(), Some(" FR "));
        // Blank fields only: no address.
        assert_eq!(entered(serde_json::json!({"city": " ", "zip": ""})), None);
    }

    #[test]
    fn money_prints_with_its_currency() {
        assert_eq!(display_money(1999, "EUR"), "19.99 EUR");
        assert_eq!(display_money(-5, "USD"), "-0.05 USD");
        assert_eq!(display_money(0, "USD"), "0.00 USD");
    }

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
        // Settings without `enabled`: the shop has no owner of it, so it does not run.
        assert_eq!(config.owners, Some(Default::default()));
    }

    #[test]
    fn the_owners_decide_what_runs_unless_the_scenario_names_its_extensions() {
        let overlay = serde_json::json!({"checkout": {"functions": {
            "payment-rules": {"enabled": true},
            "delivery-rules": {"enabled": false, "metafields": {"$app:config.mode": "strict"}},
            "quantity-limit": {"metafields": {"$app:config.max": 2}}
        }}});
        let directory =
            std::env::temp_dir().join(format!("lsf-checkout-owners-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("checkouts")).unwrap();
        std::fs::write(directory.join("store.json"), overlay.to_string()).unwrap();
        std::fs::write(
            directory.join("checkouts/named.json"),
            r#"{"extensions": ["quantity-limit"]}"#,
        )
        .unwrap();
        // A scenario may give an owner its own state; one that says nothing keeps the store's.
        std::fs::write(
            directory.join("checkouts/shop.json"),
            r#"{"checkout": {"functions": {"delivery-rules": {"enabled": true}, "payment-rules": {"metafields": {"$app:config.x": "y"}}}}}"#,
        )
        .unwrap();
        let (store, diagnostics) = lsf_core::store::load::load(
            &lsf_core::store::load::DataSource::Directory(directory.clone()),
            &lsf_core::store::load::LoadOptions::default(),
        );
        std::fs::remove_dir_all(&directory).ok();
        assert!(!diagnostics.has_errors(), "{diagnostics}");
        let owners = |scenario: Option<&str>| config_for(&store, scenario).owners;
        let expected: indexmap::IndexMap<String, bool> = [
            ("payment-rules".to_string(), true),
            ("delivery-rules".to_string(), false),
        ]
        .into_iter()
        .collect();
        assert_eq!(owners(None), Some(expected));
        assert_eq!(owners(Some("named")), None, "the scenario names what runs");
        let shop = owners(Some("shop")).unwrap();
        assert_eq!(shop.get("delivery-rules"), Some(&true));
        assert_eq!(shop.get("payment-rules"), Some(&true));
        assert_eq!(shop.get("quantity-limit"), None);
    }

    #[test]
    fn an_app_discount_of_the_admin_api_is_in_the_checkout_configuration() {
        use lsf_app::admin::domains::{app as admin_app, content};
        use lsf_app::admin::state::{ShopState, value};
        use lsf_core::store::load::{DataSource, LoadOptions, load, load_with_overlays};

        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures");
        let shopify_app = lsf_app::config::load(&fixtures.join("admin-app")).unwrap();
        let source = DataSource::Directory(fixtures.join("rich"));
        let (store, _) = load(&source, &LoadOptions::default());
        let mut state = ShopState::seeded(&store, Some(&shopify_app));
        // What `discountAutomaticAppCreate` makes, with the metafield it was given.
        let id = state.new_gid("DiscountAutomaticNode");
        state.insert(value::object(serde_json::json!({
            "__typename": "DiscountAutomaticNode",
            "id": id,
            "automaticDiscount": {
                "__typename": "DiscountAutomaticApp",
                "title": "Spring 15",
                "appDiscountType": {
                    "functionId": admin_app::function_id(Some(&shopify_app), "volume-discount")
                },
                "discountClasses": ["ORDER"],
                "startsAt": "2026-03-01T00:00:00Z",
            },
            "metafields": [],
        })));
        let namespace =
            content::admin_namespace(admin_app::app_id(Some(&shopify_app)), "$app:config");
        state.insert(value::object(serde_json::json!({
            "__typename": "Metafield",
            "id": content::metafield_gid(&id, &namespace, "settings"),
            "namespace": namespace,
            "key": "settings",
            "type": "json",
            "value": r#"{"percent":15}"#,
            "owner": id,
            "ownerType": "DISCOUNT",
        })));

        let overlay = lsf_app::admin::sync::overlay(&state, &store).expect("an overlay");
        let (synced, diagnostics) =
            load_with_overlays(&source, &LoadOptions::default(), &[overlay]);
        assert!(!diagnostics.has_errors(), "{diagnostics}");
        let config = config_from_store(&synced);
        assert_eq!(config.discounts.len(), 3);
        let created = &config.discounts[2];
        let number = lsf_app::admin::state::parse_gid(&id).unwrap().1;
        assert_eq!(created.id, Some(number));
        assert_eq!(
            (created.function.as_str(), created.title.as_str()),
            ("volume-discount", "Spring 15")
        );
        assert_eq!(created.classes, [DiscountClass::Order]);
        assert_eq!(
            (
                created.metafields[0].namespace.as_str(),
                created.metafields[0].key.as_str(),
                created.metafields[0].value.as_str()
            ),
            ("$app:config", "settings", r#"{"percent":15}"#)
        );
        // The discounts of the data keep the ids their Functions read.
        assert_eq!(config.discounts[0].id, Some(1));
        assert_eq!(config.discounts[1].id, Some(2));
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

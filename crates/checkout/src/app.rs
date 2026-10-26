//! Reads a Shopify app directory: `shopify.app.toml` and `extensions/*/shopify.extension.toml`.
//!
//! Functions and checkout UI extensions are described; other extension types are listed and
//! ignored. Problems that do not prevent loading (a missing build, an unknown target, a query
//! file that does not exist) become [`Diagnostic`]s so the checkout can still run what works.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// A Shopify app on disk.
#[derive(Clone, Debug, Default)]
pub struct App {
    pub root: PathBuf,
    /// `name` of `shopify.app.toml`.
    pub name: Option<String>,
    pub client_id: Option<String>,
    pub handle: Option<String>,
    pub functions: Vec<FunctionExtension>,
    pub ui_extensions: Vec<UiExtension>,
    /// Extensions of another type (theme app extension, admin action, ...): listed, not used.
    pub other: Vec<OtherExtension>,
    pub diagnostics: Vec<Diagnostic>,
}

impl App {
    pub fn function(&self, handle: &str) -> Option<&FunctionExtension> {
        self.functions
            .iter()
            .find(|function| function.handle == handle)
    }

    pub fn ui_extension(&self, handle: &str) -> Option<&UiExtension> {
        self.ui_extensions
            .iter()
            .find(|extension| extension.handle == handle)
    }

    /// The handles of the Functions and checkout UI extensions, in the order they were found.
    pub fn handles(&self) -> Vec<&str> {
        self.functions
            .iter()
            .map(|function| function.handle.as_str())
            .chain(
                self.ui_extensions
                    .iter()
                    .map(|extension| extension.handle.as_str()),
            )
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct FunctionExtension {
    pub handle: String,
    pub name: Option<String>,
    /// The directory of `shopify.extension.toml`.
    pub dir: PathBuf,
    pub api_version: Option<String>,
    pub targets: Vec<FunctionTarget>,
    /// `[extensions.build] command`.
    pub build_command: Option<String>,
    /// The Wasm module (`[extensions.build] path`, `dist/function.wasm` by default), absolute.
    pub wasm_path: PathBuf,
    /// `[extensions.input.variables]`: which metafield of the owner holds the query variables.
    pub input_variables: Option<InputVariables>,
}

impl FunctionExtension {
    pub fn built(&self) -> bool {
        self.wasm_path.is_file()
    }
}

#[derive(Clone, Debug)]
pub struct FunctionTarget {
    pub target: String,
    pub kind: TargetKind,
    /// The path as written in the extension file.
    pub input_query_path: Option<String>,
    /// The content of the input query file; empty when there is none.
    pub input_query: String,
    /// The Wasm export to call (`run` when not set).
    pub export: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputVariables {
    pub namespace: String,
    pub key: String,
}

/// What a Function target does in the checkout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    /// `cart.transform.run`.
    CartTransform,
    /// `cart.lines.discounts.generate.run`: product and order discounts.
    LinesDiscounts,
    /// `purchase.product-discount.run` (legacy).
    ProductDiscount,
    /// `purchase.order-discount.run` (legacy).
    OrderDiscount,
    /// `cart.delivery-options.discounts.generate.run`.
    DeliveryDiscounts,
    /// `purchase.shipping-discount.run` (legacy).
    ShippingDiscount,
    /// `cart.delivery-options.transform.run` and `purchase.delivery-customization.run`.
    DeliveryCustomization,
    /// `cart.payment-methods.transform.run` and `purchase.payment-customization.run`.
    PaymentCustomization,
    /// `cart.validations.generate.run` and `purchase.validation.run`.
    Validation,
    /// A target the local checkout does not run: fulfillment constraints, order routing,
    /// pickup generators, the `.fetch` targets of discounts.
    Unsupported,
}

impl TargetKind {
    pub fn parse(target: &str) -> TargetKind {
        match target {
            "cart.transform.run" => TargetKind::CartTransform,
            "cart.lines.discounts.generate.run" => TargetKind::LinesDiscounts,
            "purchase.product-discount.run" => TargetKind::ProductDiscount,
            "purchase.order-discount.run" => TargetKind::OrderDiscount,
            "cart.delivery-options.discounts.generate.run" => TargetKind::DeliveryDiscounts,
            "purchase.shipping-discount.run" => TargetKind::ShippingDiscount,
            "cart.delivery-options.transform.run" | "purchase.delivery-customization.run" => {
                TargetKind::DeliveryCustomization
            }
            "cart.payment-methods.transform.run" | "purchase.payment-customization.run" => {
                TargetKind::PaymentCustomization
            }
            "cart.validations.generate.run" | "purchase.validation.run" => TargetKind::Validation,
            _ => TargetKind::Unsupported,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UiExtension {
    pub handle: String,
    pub name: Option<String>,
    pub dir: PathBuf,
    pub api_version: Option<String>,
    pub targets: Vec<UiTarget>,
    /// `[extensions.capabilities]` as JSON.
    pub capabilities: Value,
    /// `[extensions.settings]` as JSON.
    pub settings: Value,
    /// The built bundle, `dist/<handle>.js`.
    pub bundle: PathBuf,
}

impl UiExtension {
    pub fn built(&self) -> bool {
        self.bundle.is_file()
    }
}

#[derive(Clone, Debug)]
pub struct UiTarget {
    pub target: String,
    /// The source module the target renders, as written (`./src/Checkout.jsx`).
    pub module: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OtherExtension {
    pub handle: String,
    pub kind: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// The handle of the extension the problem is in, if any.
    pub extension: Option<String>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.extension {
            Some(handle) => write!(formatter, "{handle}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

/// The extensions the checkout uses. Every extension unless the user named some.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    handles: Option<BTreeSet<String>>,
}

impl Selection {
    /// Every extension of the app.
    pub fn all() -> Selection {
        Selection { handles: None }
    }

    /// Only the named extensions, without checking they exist; see [`Selection::resolve`].
    pub fn only<I, S>(handles: I) -> Selection
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Selection {
            handles: Some(handles.into_iter().map(Into::into).collect()),
        }
    }

    /// `--extension` flags: all the extensions when none is given, otherwise the named ones,
    /// which must exist in the app.
    pub fn resolve(app: &App, requested: &[String]) -> Result<Selection, String> {
        if requested.is_empty() {
            return Ok(Selection::all());
        }
        let known = app.handles();
        let unknown: Vec<&str> = requested
            .iter()
            .map(String::as_str)
            .filter(|handle| !known.contains(handle))
            .collect();
        if !unknown.is_empty() {
            let known = if known.is_empty() {
                "the app has no Function or checkout UI extension".to_string()
            } else {
                format!("the app has: {}", known.join(", "))
            };
            return Err(format!(
                "no extension named {} ({known})",
                unknown
                    .iter()
                    .map(|handle| format!("`{handle}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(Selection::only(requested.iter().cloned()))
    }

    pub fn contains(&self, handle: &str) -> bool {
        self.handles
            .as_ref()
            .is_none_or(|handles| handles.contains(handle))
    }

    /// The Functions of the app that are enabled, in the order of the app.
    pub fn functions<'a>(&'a self, app: &'a App) -> impl Iterator<Item = &'a FunctionExtension> {
        app.functions
            .iter()
            .filter(move |function| self.contains(&function.handle))
    }

    /// The checkout UI extensions of the app that are enabled, in the order of the app.
    pub fn ui_extensions<'a>(&'a self, app: &'a App) -> impl Iterator<Item = &'a UiExtension> {
        app.ui_extensions
            .iter()
            .filter(move |extension| self.contains(&extension.handle))
    }
}

/// Loads the app in `dir`. Fails only when `dir` is not a directory; everything else that is
/// wrong with the app is reported in [`App::diagnostics`].
pub fn load(dir: &Path) -> Result<App, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut app = App {
        root: root.clone(),
        ..App::default()
    };

    load_app_config(&mut app);

    let extensions_dir = root.join("extensions");
    let mut files = Vec::new();
    match std::fs::read_dir(&extensions_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let file = entry.path().join("shopify.extension.toml");
                if file.is_file() {
                    files.push(file);
                }
            }
        }
        Err(_) => app.warn(None, "the app has no `extensions` directory".to_string()),
    }
    files.sort();
    for file in files {
        load_extension_file(&mut app, &file);
    }

    app.check_handles();
    Ok(app)
}

impl App {
    fn warn(&mut self, extension: Option<&str>, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            extension: extension.map(str::to_string),
            message,
        });
    }

    fn error(&mut self, extension: Option<&str>, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            extension: extension.map(str::to_string),
            message,
        });
    }

    fn check_handles(&mut self) {
        let mut seen = BTreeSet::new();
        let mut duplicates = Vec::new();
        let handles = self
            .functions
            .iter()
            .map(|function| &function.handle)
            .chain(self.ui_extensions.iter().map(|extension| &extension.handle))
            .chain(self.other.iter().map(|extension| &extension.handle));
        for handle in handles {
            if !seen.insert(handle.clone()) {
                duplicates.push(handle.clone());
            }
        }
        for handle in duplicates {
            self.error(
                Some(&handle),
                "several extensions have this handle; the first one is used".to_string(),
            );
        }
    }
}

/// `shopify.app.toml`, or the first `shopify.app.*.toml` (Shopify CLI configuration files for
/// another environment).
fn load_app_config(app: &mut App) {
    let mut candidates = vec![app.root.join("shopify.app.toml")];
    if let Ok(entries) = std::fs::read_dir(&app.root) {
        let mut others: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("shopify.app.") && name.ends_with(".toml"))
            })
            .collect();
        others.sort();
        candidates.extend(others);
    }
    let Some(file) = candidates.into_iter().find(|path| path.is_file()) else {
        app.warn(
            None,
            "no shopify.app.toml: is this the root of the app?".to_string(),
        );
        return;
    };
    let table = match read_toml(&file) {
        Ok(table) => table,
        Err(message) => {
            app.error(None, message);
            return;
        }
    };
    let text = |key: &str| table.get(key).and_then(|v| v.as_str()).map(str::to_string);
    app.name = text("name");
    app.client_id = text("client_id");
    app.handle = text("handle");
}

fn read_toml(file: &Path) -> Result<toml::Table, String> {
    let content = std::fs::read_to_string(file)
        .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    content
        .parse::<toml::Table>()
        .map_err(|error| format!("{}: {}", file.display(), error.message()))
}

fn load_extension_file(app: &mut App, file: &Path) {
    let dir = file.parent().expect("a file has a parent").to_path_buf();
    let dir_name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("extension")
        .to_string();
    let table = match read_toml(file) {
        Ok(table) => table,
        Err(message) => {
            app.error(Some(&dir_name), message);
            return;
        }
    };

    // `[[extensions]]` is the form `shopify app generate` writes; a file without it describes
    // one extension at its top level.
    let entries: Vec<&toml::Table> = match table.get("extensions").and_then(|v| v.as_array()) {
        Some(array) => array.iter().filter_map(|v| v.as_table()).collect(),
        None => vec![&table],
    };
    // The API version may sit at the top of the file, above `[[extensions]]`.
    let file_api_version = table
        .get("api_version")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    for entry in entries {
        let kind = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        // Several extensions in one file: the directory name only stands for the first one.
        let handle = entry
            .get("handle")
            .and_then(|v| v.as_str())
            .map_or_else(|| dir_name.clone(), str::to_string);
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let api_version = entry
            .get("api_version")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| file_api_version.clone());
        match kind {
            "function" => {
                let function = load_function(app, &dir, &handle, name, api_version, entry);
                app.functions.push(function);
            }
            "ui_extension" => {
                let extension = load_ui_extension(app, &dir, &handle, name, api_version, entry);
                app.ui_extensions.push(extension);
            }
            "" => app.error(Some(&handle), "no `type` in the extension file".to_string()),
            other => app.other.push(OtherExtension {
                handle,
                kind: other.to_string(),
            }),
        }
    }
}

fn load_function(
    app: &mut App,
    dir: &Path,
    handle: &str,
    name: Option<String>,
    api_version: Option<String>,
    entry: &toml::Table,
) -> FunctionExtension {
    let mut targets = Vec::new();
    for target in entry
        .get("targeting")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_table())
    {
        let Some(target_name) = target.get("target").and_then(|v| v.as_str()) else {
            app.error(
                Some(handle),
                "a `targeting` entry has no `target`".to_string(),
            );
            continue;
        };
        let kind = TargetKind::parse(target_name);
        if kind == TargetKind::Unsupported {
            app.warn(
                Some(handle),
                format!("target `{target_name}` is not run by the local checkout"),
            );
        }
        let input_query_path = target
            .get("input_query")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let input_query = match &input_query_path {
            Some(path) => match std::fs::read_to_string(dir.join(path)) {
                Ok(query) => query,
                Err(error) => {
                    if kind != TargetKind::Unsupported {
                        app.error(
                            Some(handle),
                            format!("target `{target_name}`: cannot read the input query `{path}`: {error}"),
                        );
                    }
                    String::new()
                }
            },
            None => {
                if kind != TargetKind::Unsupported {
                    app.error(
                        Some(handle),
                        format!("target `{target_name}` has no `input_query`"),
                    );
                }
                String::new()
            }
        };
        targets.push(FunctionTarget {
            target: target_name.to_string(),
            kind,
            input_query_path,
            input_query,
            export: target
                .get("export")
                .and_then(|v| v.as_str())
                .unwrap_or("run")
                .to_string(),
        });
    }
    if targets.is_empty() {
        app.warn(Some(handle), "the Function has no target".to_string());
    }

    let build = entry.get("build").and_then(|v| v.as_table());
    let build_command = build
        .and_then(|build| build.get("command"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let wasm_path = dir.join(
        build
            .and_then(|build| build.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("dist/function.wasm"),
    );
    if !wasm_path.is_file() {
        app.warn(
            Some(handle),
            match &build_command {
                Some(command) => format!(
                    "no Wasm module at {}: build it with `{command}` (or `shopify app build`)",
                    wasm_path.display()
                ),
                None => format!(
                    "no Wasm module at {}: run `shopify app build`",
                    wasm_path.display()
                ),
            },
        );
    }

    let input_variables = entry
        .get("input")
        .and_then(|v| v.as_table())
        .and_then(|input| input.get("variables"))
        .and_then(|v| v.as_table())
        .and_then(|variables| {
            Some(InputVariables {
                namespace: variables.get("namespace")?.as_str()?.to_string(),
                key: variables.get("key")?.as_str()?.to_string(),
            })
        });

    FunctionExtension {
        handle: handle.to_string(),
        name,
        dir: dir.to_path_buf(),
        api_version,
        targets,
        build_command,
        wasm_path,
        input_variables,
    }
}

/// The targets of the checkout UI extensions the extension host knows how to place.
fn renders_in_checkout(target: &str) -> bool {
    target.starts_with("purchase.checkout.") || target.starts_with("purchase.thank-you.")
}

fn load_ui_extension(
    app: &mut App,
    dir: &Path,
    handle: &str,
    name: Option<String>,
    api_version: Option<String>,
    entry: &toml::Table,
) -> UiExtension {
    let mut targets = Vec::new();
    // `[[extensions.targeting]]`, and `extension_points = [...]`, the older way to list targets.
    for target in entry
        .get("targeting")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_table())
    {
        let Some(target_name) = target.get("target").and_then(|v| v.as_str()) else {
            app.error(
                Some(handle),
                "a `targeting` entry has no `target`".to_string(),
            );
            continue;
        };
        targets.push(UiTarget {
            target: target_name.to_string(),
            module: target
                .get("module")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    for point in entry
        .get("extension_points")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
    {
        targets.push(UiTarget {
            target: point.to_string(),
            module: None,
        });
    }
    if targets.is_empty() {
        app.warn(
            Some(handle),
            "the extension has no target, nothing will render".to_string(),
        );
    }
    for target in &targets {
        if !renders_in_checkout(&target.target) {
            app.warn(
                Some(handle),
                format!(
                    "target `{}` is not a checkout or thank-you page target: it is not rendered",
                    target.target
                ),
            );
        }
    }

    let bundle = dir.join("dist").join(format!("{handle}.js"));
    if !bundle.is_file() {
        app.warn(
            Some(handle),
            format!(
                "not built (missing {}): run `shopify app build`",
                bundle.display()
            ),
        );
    }

    UiExtension {
        handle: handle.to_string(),
        name,
        dir: dir.to_path_buf(),
        api_version,
        targets,
        capabilities: entry.get("capabilities").map_or(Value::Null, toml_to_json),
        settings: entry.get("settings").map_or(Value::Null, toml_to_json),
        bundle,
    }
}

fn toml_to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(string) => Value::String(string.clone()),
        toml::Value::Integer(number) => Value::from(*number),
        toml::Value::Float(number) => {
            serde_json::Number::from_f64(*number).map_or(Value::Null, Value::Number)
        }
        toml::Value::Boolean(flag) => Value::Bool(*flag),
        toml::Value::Datetime(date) => Value::String(date.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.clone(), toml_to_json(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, content: &str) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    fn temp_app(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lsf-checkout-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn loads_functions_and_ui_extensions() {
        let root = temp_app("loads");
        write(
            &root,
            "shopify.app.toml",
            "name = \"Demo\"\nclient_id = \"abc\"\nhandle = \"demo\"\n",
        );
        write(
            &root,
            "extensions/volume/shopify.extension.toml",
            r#"
api_version = "2025-10"

[[extensions]]
name = "Volume"
handle = "volume-discount"
type = "function"

  [[extensions.targeting]]
  target = "cart.lines.discounts.generate.run"
  input_query = "src/run.graphql"
  export = "cart_lines_discounts_generate_run"

  [[extensions.targeting]]
  target = "cart.delivery-options.discounts.generate.run"
  input_query = "src/missing.graphql"
  export = "cart_delivery_options_discounts_generate_run"

  [extensions.build]
  command = "cargo build"
  path = "out/volume.wasm"

  [extensions.input.variables]
  namespace = "$app:config"
  key = "settings"
"#,
        );
        write(
            &root,
            "extensions/volume/src/run.graphql",
            "query Input { cart { lines { id } } }",
        );
        write(
            &root,
            "extensions/banner/shopify.extension.toml",
            r#"
api_version = "2025-10"

[[extensions]]
name = "Banner"
type = "ui_extension"
handle = "banner"

  [[extensions.targeting]]
  target = "purchase.checkout.block.render"
  module = "./src/Checkout.jsx"

  [[extensions.targeting]]
  target = "customer-account.order-status.block.render"
  module = "./src/Order.jsx"

  [extensions.capabilities]
  network_access = true

  [extensions.settings]
    [[extensions.settings.fields]]
    key = "title"
    type = "single_line_text_field"
"#,
        );
        write(&root, "extensions/banner/dist/banner.js", "// built");
        write(
            &root,
            "extensions/theme/shopify.extension.toml",
            "[[extensions]]\ntype = \"theme\"\nname = \"Theme\"\nhandle = \"theme-ext\"\n",
        );

        let app = load(&root).unwrap();
        assert_eq!(app.name.as_deref(), Some("Demo"));
        assert_eq!(app.client_id.as_deref(), Some("abc"));

        let function = app.function("volume-discount").unwrap();
        assert_eq!(function.api_version.as_deref(), Some("2025-10"));
        assert_eq!(function.targets.len(), 2);
        assert_eq!(function.targets[0].kind, TargetKind::LinesDiscounts);
        assert_eq!(
            function.targets[0].export,
            "cart_lines_discounts_generate_run"
        );
        assert!(function.targets[0].input_query.contains("cart"));
        assert_eq!(function.targets[1].kind, TargetKind::DeliveryDiscounts);
        assert!(function.targets[1].input_query.is_empty());
        assert!(function.wasm_path.ends_with("out/volume.wasm"));
        assert_eq!(
            function.input_variables,
            Some(InputVariables {
                namespace: "$app:config".into(),
                key: "settings".into()
            })
        );

        let banner = app.ui_extension("banner").unwrap();
        assert!(banner.built());
        assert_eq!(banner.targets.len(), 2);
        assert_eq!(
            banner.targets[0].module.as_deref(),
            Some("./src/Checkout.jsx")
        );
        assert_eq!(banner.capabilities["network_access"], Value::Bool(true));
        assert_eq!(banner.settings["fields"][0]["key"], "title");

        assert_eq!(app.other.len(), 1);
        assert_eq!(app.other[0].kind, "theme");

        let messages: Vec<String> = app.diagnostics.iter().map(ToString::to_string).collect();
        assert!(
            messages.iter().any(|m| m.contains("volume-discount: target `cart.delivery-options.discounts.generate.run`: cannot read the input query")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.starts_with("volume-discount: no Wasm module")
                    && m.contains("cargo build")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m
                    .starts_with("banner: target `customer-account.order-status.block.render`")),
            "{messages:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_ui_extension_without_bundle_says_to_build() {
        let root = temp_app("bundle");
        write(
            &root,
            "extensions/banner/shopify.extension.toml",
            "[[extensions]]\ntype = \"ui_extension\"\nhandle = \"banner\"\n[[extensions.targeting]]\ntarget = \"purchase.checkout.block.render\"\n",
        );
        let app = load(&root).unwrap();
        assert!(!app.ui_extension("banner").unwrap().built());
        assert!(
            app.diagnostics
                .iter()
                .any(|d| d.message.contains("shopify app build"))
        );
        assert!(
            app.diagnostics
                .iter()
                .any(|d| d.message.contains("no shopify.app.toml"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_flat_extension_file_and_the_handle_default() {
        let root = temp_app("flat");
        write(&root, "shopify.app.production.toml", "name = \"Prod\"\n");
        write(
            &root,
            "extensions/check/shopify.extension.toml",
            "type = \"function\"\napi_version = \"2025-01\"\n[[targeting]]\ntarget = \"purchase.validation.run\"\ninput_query = \"run.graphql\"\n",
        );
        write(
            &root,
            "extensions/check/run.graphql",
            "query Input { cart { lines { quantity } } }",
        );
        let app = load(&root).unwrap();
        assert_eq!(app.name.as_deref(), Some("Prod"));
        let function = app.function("check").unwrap();
        assert_eq!(function.targets[0].kind, TargetKind::Validation);
        assert_eq!(function.targets[0].export, "run");
        assert!(function.wasm_path.ends_with("dist/function.wasm"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn selection_checks_the_handles() {
        let root = temp_app("selection");
        write(
            &root,
            "extensions/a/shopify.extension.toml",
            "[[extensions]]\ntype = \"function\"\nhandle = \"a\"\n",
        );
        let app = load(&root).unwrap();
        assert!(Selection::resolve(&app, &[]).unwrap().contains("anything"));
        let selection = Selection::resolve(&app, &["a".to_string()]).unwrap();
        assert!(selection.contains("a"));
        assert!(!selection.contains("b"));
        let error = Selection::resolve(&app, &["nope".to_string()]).unwrap_err();
        assert!(error.contains("`nope`") && error.contains("a"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        assert!(load(Path::new("/definitely/not/here")).is_err());
    }
}

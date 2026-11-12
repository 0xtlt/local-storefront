//! Reads a data directory into a [`Store`].
//!
//! Layout of a data directory:
//!
//! ```text
//! shopify-local/
//!   *.json            any number of files, each holding any of: shop, products, collections,
//!                     pages, blogs, menus, customers, companies, gift_cards, locations,
//!                     selling_plan_groups, swatches, metaobjects, localization, files,
//!                     session, checkout, now, theme_settings, app. They are merged.
//!   products/*.json   one product per file (or an array); the file name is the default handle
//!   collections/*.json, pages/*.json, blogs/*.json, customers/*.json, menus/*.json
//!   checkouts/*.json  checkout scenarios, one per file
//!   files/            images and other files, served under /cdn/shop/files/
//!   schema/           JSON Schemas written by `lsf init` for editor support (ignored here)
//! ```
//!
//! [`role_of`] says which of these a file is: what a server watching the directory reloads
//! when it changes.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use super::build::{
    BuildOptions, LeftOut, MergedInput, MergedScenario, Origin, Sourced, build, undated,
};
use super::validate::{FileKind, validate};
use super::{Store, demo, model};
use crate::diagnostics::Diagnostics;
use crate::json::strip_comments;

/// Where the store data comes from.
#[derive(Clone, Debug)]
pub enum DataSource {
    /// A data directory on disk.
    Directory(PathBuf),
    /// The built-in demo store.
    Demo,
}

/// What a file of a data directory is to the store: what [`SourceData::read`] reads it as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRole {
    /// The store's own data: a data file at the root, an entity of `products/`, `collections/`,
    /// `pages/`, `blogs/`, `customers/` or `menus/`, or a file of `files/` (the store reads the
    /// sizes of its images).
    Store,
    /// A checkout scenario: a file of `checkouts/`. It sets up the local checkout on top of the
    /// store, and nothing else reads it.
    Scenario,
    /// A file the store does not read: `schema/`, `README.md`, a file of another folder, a name
    /// that starts with a dot.
    Unread,
}

/// The role of the file at `relative` in a data directory (`/` between folders), as
/// [`SourceData::read`] reads the directory.
pub fn role_of(relative: &str) -> FileRole {
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.iter().any(|part| part.starts_with('.')) {
        return FileRole::Unread;
    }
    match parts.as_slice() {
        [name] if name.ends_with(".json") => FileRole::Store,
        ["files", _, ..] => FileRole::Store,
        [directory, name] if name.ends_with(".json") => {
            match FileKind::ALL
                .iter()
                .find(|kind| kind.directory() == Some(directory))
            {
                Some(FileKind::CheckoutScenario) => FileRole::Scenario,
                Some(_) => FileRole::Store,
                None => FileRole::Unread,
            }
        }
        _ => FileRole::Unread,
    }
}

/// One data file, already read.
struct DataFile {
    /// Path relative to the data directory, with forward slashes.
    name: String,
    kind: FileKind,
    content: String,
}

fn read_directory(root: &Path, diagnostics: &mut Diagnostics) -> Vec<DataFile> {
    let mut files = Vec::new();
    let mut read = |relative: String, kind: FileKind, files: &mut Vec<DataFile>| {
        match std::fs::read_to_string(root.join(&relative)) {
            Ok(content) => files.push(DataFile {
                name: relative,
                kind,
                content,
            }),
            Err(error) => {
                diagnostics.error(
                    "unreadable_file",
                    &relative,
                    "",
                    format!("cannot read the file: {error}"),
                );
            }
        }
    };
    let json_files = |directory: &Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".json") && !name.starts_with('.'))
            .collect();
        names.sort();
        names
    };
    for name in json_files(root) {
        read(name, FileKind::Store, &mut files);
    }
    for kind in FileKind::ALL {
        let Some(directory) = kind.directory() else {
            continue;
        };
        for name in json_files(&root.join(directory)) {
            read(format!("{directory}/{name}"), kind, &mut files);
        }
    }
    files
}

fn demo_files() -> Vec<DataFile> {
    demo::FILES
        .iter()
        .map(|(name, content)| DataFile {
            name: (*name).to_string(),
            kind: match name.split_once('/') {
                Some(("products", _)) => FileKind::Product,
                _ => FileKind::Store,
            },
            content: (*content).to_string(),
        })
        .collect()
}

/// Validates one entity: what can be read of it, without the fields it does not know (reported,
/// and left out, so that the rest is used). `None` when another problem keeps it out.
fn checked(
    kind: FileKind,
    file: &str,
    json: &Json,
    pointer: &str,
    diagnostics: &mut Diagnostics,
) -> Option<Json> {
    let errors_before = diagnostics.error_count();
    let unknown = validate(kind, file, json, pointer, diagnostics);
    (diagnostics.error_count() - errors_before <= unknown.errors).then(|| unknown.without(json))
}

/// Parses, validates and deserializes one entity. Reports problems and returns `None` when the
/// entity cannot be used.
fn entity<T: DeserializeOwned>(
    kind: FileKind,
    file: &str,
    json: &Json,
    pointer: &str,
    diagnostics: &mut Diagnostics,
) -> Option<T> {
    let json = checked(kind, file, json, pointer, diagnostics)?;
    match serde_json::from_value::<T>(json) {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.error("invalid_value", file, pointer, error.to_string());
            None
        }
    }
}

/// The entities of a per-entity file: a single object, or an array of them.
fn entities<T: DeserializeOwned>(
    kind: FileKind,
    file: &str,
    json: &Json,
    diagnostics: &mut Diagnostics,
) -> Vec<Sourced<T>> {
    match json {
        Json::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let pointer = format!("/{index}");
                entity(kind, file, item, &pointer, diagnostics).map(|value| Sourced {
                    value,
                    origin: Origin::new(file, pointer),
                })
            })
            .collect(),
        single => entity(kind, file, single, "", diagnostics)
            .map(|value| Sourced {
                value,
                origin: Origin::new(file, ""),
            })
            .into_iter()
            .collect(),
    }
}

fn file_stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn merge(files: Vec<DataFile>, diagnostics: &mut Diagnostics) -> MergedInput {
    let mut merged = MergedInput::default();
    for file in files {
        let name = file.name.as_str();
        let json: Json = match serde_json::from_str(&strip_comments(&file.content)) {
            Ok(json) => json,
            Err(error) => {
                let diagnostic = diagnostics
                    .error("invalid_json", name, "", format!("not valid JSON: {error}"))
                    .hint(format!(
                        "look at line {}, column {}",
                        error.line(),
                        error.column()
                    ))
                    .to_string();
                // A product that cannot be read is named after its file.
                if file.kind == FileKind::Product {
                    merged.left_out.push(LeftOut {
                        kind: "product",
                        key: crate::util::handleize(&file_stem(name)),
                        file: name.to_string(),
                    });
                }
                // A scenario that cannot be read is listed with its error, not hidden.
                if file.kind == FileKind::CheckoutScenario {
                    merged.scenarios.push(MergedScenario {
                        name: file_stem(name),
                        file: name.to_string(),
                        input: None,
                        problems: vec![diagnostic],
                    });
                }
                continue;
            }
        };
        match file.kind {
            FileKind::Store => {
                let before = diagnostics.items.len();
                let (input, skipped) = match entity::<model::StoreInput>(
                    FileKind::Store,
                    name,
                    &json,
                    "",
                    diagnostics,
                ) {
                    Some(input) => (input, Skipped::default()),
                    // What has errors is left out, and the rest of the file is read: the
                    // references of the rest are checked in the same run.
                    None => match salvage_store(name, &json, &diagnostics.items[before..]) {
                        Some(salvaged) => salvaged,
                        None => continue,
                    },
                };
                for (section, kind, key) in [
                    ("products", "product", "handle"),
                    ("customers", "customer", "email"),
                    ("collections", "collection", "handle"),
                    ("pages", "page", "handle"),
                ] {
                    for index in skipped.indices(section) {
                        if let Some(item) = json.get(section).and_then(|items| items.get(index)) {
                            merged.left_out.push(LeftOut {
                                kind,
                                key: entity_key(item, key, None),
                                file: name.to_string(),
                            });
                        }
                    }
                }
                merge_store_file(&mut merged, input, name, diagnostics, &skipped);
            }
            FileKind::Product => {
                let single = json.is_object();
                let products =
                    entities::<model::ProductInput>(FileKind::Product, name, &json, diagnostics);
                record_left_out(
                    &mut merged,
                    FileKind::Product,
                    "product",
                    "handle",
                    name,
                    &json,
                    products.len(),
                );
                for mut product in products {
                    // A file holding a single product is named after its handle.
                    if single && product.value.handle.is_none() {
                        product.value.handle = Some(crate::util::handleize(&file_stem(name)));
                    }
                    merged.products.push(product);
                }
            }
            FileKind::Collection => {
                let single = json.is_object();
                for mut collection in entities::<model::CollectionInput>(
                    FileKind::Collection,
                    name,
                    &json,
                    diagnostics,
                ) {
                    if single && collection.value.handle.is_none() {
                        collection.value.handle = Some(crate::util::handleize(&file_stem(name)));
                    }
                    merged.collections.push(collection);
                }
            }
            FileKind::Page => {
                let single = json.is_object();
                for mut page in
                    entities::<model::PageInput>(FileKind::Page, name, &json, diagnostics)
                {
                    if single && page.value.handle.is_none() {
                        page.value.handle = Some(crate::util::handleize(&file_stem(name)));
                    }
                    merged.pages.push(page);
                }
            }
            FileKind::Blog => {
                let single = json.is_object();
                for mut blog in
                    entities::<model::BlogInput>(FileKind::Blog, name, &json, diagnostics)
                {
                    if single && blog.value.handle.is_none() {
                        blog.value.handle = Some(crate::util::handleize(&file_stem(name)));
                    }
                    merged.blogs.push(blog);
                }
            }
            FileKind::Customer => {
                let customers =
                    entities::<model::CustomerInput>(FileKind::Customer, name, &json, diagnostics);
                record_left_out(
                    &mut merged,
                    FileKind::Customer,
                    "customer",
                    "email",
                    name,
                    &json,
                    customers.len(),
                );
                merged.customers.extend(customers);
            }
            FileKind::Menu => {
                if let Some(menu) =
                    entity::<model::MenuInput>(FileKind::Menu, name, &json, "", diagnostics)
                {
                    let handle = file_stem(name);
                    if merged.menus.contains_key(&handle) {
                        diagnostics.error(
                            "duplicate_menu",
                            name,
                            "",
                            format!("the menu \"{handle}\" is defined more than once"),
                        );
                    }
                    merged.menus.insert(
                        handle,
                        Sourced {
                            value: menu,
                            origin: Origin::new(name, ""),
                        },
                    );
                }
            }
            FileKind::Session => {}
            FileKind::CheckoutScenario => {
                let mut problems = Diagnostics::new();
                let input = entity::<model::CheckoutScenarioInput>(
                    FileKind::CheckoutScenario,
                    name,
                    &json,
                    "",
                    &mut problems,
                );
                let problems_text = problems.items.iter().map(ToString::to_string).collect();
                diagnostics.extend(problems);
                merged.scenarios.push(MergedScenario {
                    name: file_stem(name),
                    file: name.to_string(),
                    input,
                    problems: problems_text,
                });
            }
        }
    }
    merged
}

/// The entries of a data file left out because they have errors: by section (`products`,
/// `metaobjects/designer`), the indices they had in the file, in order.
#[derive(Debug, Default)]
struct Skipped(std::collections::HashMap<String, Vec<usize>>);

impl Skipped {
    fn indices(&self, section: &str) -> Vec<usize> {
        self.0.get(section).cloned().unwrap_or_default()
    }

    /// The index in the file of the `kept`-th entry of a section that was read.
    fn original(&self, section: &str, kept: usize) -> usize {
        let mut index = kept;
        for skipped in self.0.get(section).into_iter().flatten() {
            if *skipped <= index {
                index += 1;
            }
        }
        index
    }
}

/// A data file with errors, without the entries that have them: the entries of its lists, of
/// its maps (`menus`, `swatches`), and its sections of one (`shop`, `localization`, ...). `None`
/// when nothing can be read of it even so.
fn salvage_store(
    file: &str,
    json: &Json,
    problems: &[crate::diagnostics::Diagnostic],
) -> Option<(model::StoreInput, Skipped)> {
    const LISTS: [&str; 9] = [
        "products",
        "collections",
        "pages",
        "blogs",
        "customers",
        "companies",
        "gift_cards",
        "locations",
        "selling_plan_groups",
    ];
    let mut salvaged = json.clone();
    let mut skipped = Skipped::default();
    let mut removed_keys: Vec<(String, String)> = Vec::new();
    for problem in problems {
        if problem.severity != crate::diagnostics::Severity::Error
            || problem.code == "unknown_field"
        {
            continue;
        }
        let segments: Vec<&str> = problem.path.split('/').skip(1).collect();
        match segments.as_slice() {
            [] | [""] => return None,
            [section, index, ..] if LISTS.contains(section) => {
                let index: usize = index.parse().ok()?;
                let list = skipped.0.entry((*section).to_string()).or_default();
                if !list.contains(&index) {
                    list.push(index);
                }
            }
            ["metaobjects", kind, index, ..] => {
                let index: usize = index.parse().ok()?;
                let list = skipped.0.entry(format!("metaobjects/{kind}")).or_default();
                if !list.contains(&index) {
                    list.push(index);
                }
            }
            [
                section @ ("menus" | "swatches" | "files" | "metaobjects"),
                key,
                ..,
            ] => removed_keys.push(((*section).to_string(), (*key).to_string())),
            [section, ..] => removed_keys.push(((*section).to_string(), String::new())),
        }
    }
    for (section, indices) in &mut skipped.0 {
        indices.sort_unstable();
        let pointer = format!("/{section}");
        let list = salvaged.pointer_mut(&pointer)?.as_array_mut()?;
        for index in indices.iter().rev() {
            if *index < list.len() {
                list.remove(*index);
            }
        }
    }
    for (section, key) in removed_keys {
        let object = salvaged.as_object_mut()?;
        if key.is_empty() {
            object.remove(&section);
        } else if let Some(entries) = object.get_mut(&section).and_then(Json::as_object_mut) {
            entries.remove(&key.replace("~1", "/").replace("~0", "~"));
        }
    }
    // What is left must read without an error; its unknown fields are reported already.
    let mut again = Diagnostics::new();
    let json = checked(FileKind::Store, file, &salvaged, "", &mut again)?;
    let input = serde_json::from_value::<model::StoreInput>(json).ok()?;
    Some((input, skipped))
}

/// What an entity is named by in references: its `handle` (or `email`), else the handle of its
/// title, else of its file.
fn entity_key(item: &Json, field: &str, file: Option<&str>) -> String {
    if let Some(key) = item.get(field).and_then(Json::as_str) {
        return key.to_string();
    }
    if field == "handle" {
        if let Some(stem) = file {
            return crate::util::handleize(&file_stem(stem));
        }
        if let Some(title) = item.get("title").and_then(Json::as_str) {
            return crate::util::handleize(title);
        }
    }
    String::new()
}

/// Records the entities of a per-entity file that were left out because of their errors.
fn record_left_out(
    merged: &mut MergedInput,
    file_kind: FileKind,
    kind: &'static str,
    field: &str,
    file: &str,
    json: &Json,
    read: usize,
) {
    let items: Vec<&Json> = match json {
        Json::Array(items) => items.iter().collect(),
        single => vec![single],
    };
    if read == items.len() {
        return;
    }
    let single = json.is_object();
    for item in items {
        // Checked again, on the side: the entities that were read are not left out.
        if checked(file_kind, file, item, "", &mut Diagnostics::new()).is_some() {
            continue;
        }
        let key = entity_key(item, field, single.then_some(file));
        if !key.is_empty() {
            merged.left_out.push(LeftOut {
                kind,
                key,
                file: file.to_string(),
            });
        }
    }
}

fn merge_store_file(
    merged: &mut MergedInput,
    input: model::StoreInput,
    file: &str,
    diagnostics: &mut Diagnostics,
    skipped: &Skipped,
) {
    let origin = |pointer: String| Origin::new(file, pointer);
    let singleton = |name: &str, already: Option<&str>, diagnostics: &mut Diagnostics| {
        if let Some(previous) = already {
            diagnostics
                .error(
                    "duplicate_section",
                    file,
                    &format!("/{name}"),
                    format!("\"{name}\" is already defined in {previous}"),
                )
                .hint(format!("keep a single \"{name}\" across the data files"));
        }
    };
    if let Some(shop) = input.shop {
        singleton(
            "shop",
            merged.shop.as_ref().map(|shop| shop.origin.file.as_str()),
            diagnostics,
        );
        merged.shop = Some(Sourced {
            value: shop,
            origin: origin("/shop".to_string()),
        });
    }
    if let Some(localization) = input.localization {
        singleton(
            "localization",
            merged.localization.as_ref().map(|l| l.origin.file.as_str()),
            diagnostics,
        );
        merged.localization = Some(Sourced {
            value: localization,
            origin: origin("/localization".to_string()),
        });
    }
    if let Some(session) = input.session {
        singleton(
            "session",
            merged.session.as_ref().map(|s| s.origin.file.as_str()),
            diagnostics,
        );
        merged.session = Some(Sourced {
            value: session,
            origin: origin("/session".to_string()),
        });
    }
    if let Some(checkout) = input.checkout {
        singleton(
            "checkout",
            merged.checkout.as_ref().map(|c| c.origin.file.as_str()),
            diagnostics,
        );
        merged.checkout = Some(Sourced {
            value: checkout,
            origin: origin("/checkout".to_string()),
        });
    }
    if let Some(now) = input.now {
        singleton(
            "now",
            merged.now.as_ref().map(|n| n.origin.file.as_str()),
            diagnostics,
        );
        merged.now = Some(Sourced {
            value: now,
            origin: origin("/now".to_string()),
        });
    }
    for (index, value) in input.products.into_iter().enumerate() {
        merged.products.push(Sourced {
            value,
            origin: origin(format!("/products/{}", skipped.original("products", index))),
        });
    }
    for (index, value) in input.collections.into_iter().enumerate() {
        merged.collections.push(Sourced {
            value,
            origin: origin(format!(
                "/collections/{}",
                skipped.original("collections", index)
            )),
        });
    }
    for (index, value) in input.pages.into_iter().enumerate() {
        merged.pages.push(Sourced {
            value,
            origin: origin(format!("/pages/{}", skipped.original("pages", index))),
        });
    }
    for (index, value) in input.blogs.into_iter().enumerate() {
        merged.blogs.push(Sourced {
            value,
            origin: origin(format!("/blogs/{}", skipped.original("blogs", index))),
        });
    }
    for (index, value) in input.customers.into_iter().enumerate() {
        merged.customers.push(Sourced {
            value,
            origin: origin(format!(
                "/customers/{}",
                skipped.original("customers", index)
            )),
        });
    }
    for (index, value) in input.companies.into_iter().enumerate() {
        merged.companies.push(Sourced {
            value,
            origin: origin(format!(
                "/companies/{}",
                skipped.original("companies", index)
            )),
        });
    }
    for (index, value) in input.gift_cards.into_iter().enumerate() {
        merged.gift_cards.push(Sourced {
            value,
            origin: origin(format!(
                "/gift_cards/{}",
                skipped.original("gift_cards", index)
            )),
        });
    }
    for (index, value) in input.locations.into_iter().enumerate() {
        merged.locations.push(Sourced {
            value,
            origin: origin(format!(
                "/locations/{}",
                skipped.original("locations", index)
            )),
        });
    }
    for (index, value) in input.selling_plan_groups.into_iter().enumerate() {
        merged.selling_plan_groups.push(Sourced {
            value,
            origin: origin(format!(
                "/selling_plan_groups/{}",
                skipped.original("selling_plan_groups", index)
            )),
        });
    }
    for (name, value) in input.swatches {
        let origin = origin(format!("/swatches/{name}"));
        merged.swatches.insert(name, Sourced { value, origin });
    }
    for (handle, value) in input.menus {
        if merged.menus.contains_key(&handle) {
            diagnostics.error(
                "duplicate_menu",
                file,
                &format!("/menus/{handle}"),
                format!("the menu \"{handle}\" is defined more than once"),
            );
        }
        let origin = origin(format!("/menus/{handle}"));
        merged.menus.insert(handle, Sourced { value, origin });
    }
    for (kind, entries) in input.metaobjects {
        for (index, value) in entries.into_iter().enumerate() {
            let index = skipped.original(&format!("metaobjects/{kind}"), index);
            let origin = origin(format!("/metaobjects/{kind}/{index}"));
            merged
                .metaobjects
                .push((kind.clone(), Sourced { value, origin }));
        }
    }
    merged.files.extend(input.files);
    for key in input.theme_settings.keys() {
        merged
            .theme_settings_origins
            .insert(key.clone(), origin(format!("/theme_settings/{key}")));
    }
    merged.theme_settings.extend(input.theme_settings);
    merge_app(&mut merged.app, input.app);
}

/// Merges the `app` of a data file into what the others gave: metafield by metafield, embed by
/// embed.
fn merge_app(merged: &mut model::AppInstallationInput, input: Option<model::AppInstallationInput>) {
    let Some(input) = input else {
        return;
    };
    for (namespace, fields) in input.metafields {
        merged
            .metafields
            .entry(namespace)
            .or_default()
            .extend(fields);
    }
    merged.embeds.extend(input.embeds);
}

/// Options that depend on the theme being served.
#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    /// The locales the theme has translations for, default first.
    pub theme_locales: Vec<String>,
    /// The theme's `config/settings_schema.json`, which the `theme_settings` of the data are
    /// checked against. `None` without a theme: they are not checked.
    pub settings_schema: Option<std::sync::Arc<Json>>,
}

/// Loads and validates the store. The store is returned even when the data has errors (built
/// from whatever is valid), together with the diagnostics.
pub fn load(source: &DataSource, options: &LoadOptions) -> (Store, Diagnostics) {
    load_with_overlays(source, options, &[])
}

/// The name diagnostics use for data that did not come from a file.
pub const OVERLAY_FILE: &str = "(request body)";

/// The name of the overlay of what the Admin API changed (`lsf_app::admin::sync`).
pub const ADMIN_OVERLAY_FILE: &str = "(admin api)";

/// Data applied on top of the files: the entities it removes, then the entities it adds or
/// replaces. The data of a session (`PUT /__lsf/session`) is one, and so are the changes the
/// Admin API made to the store (`lsf_app::admin::sync`), which the session's data then goes on top of.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    /// What diagnostics name the overlay instead of a file: [`OVERLAY_FILE`] for a session's
    /// data. Each overlay has its own, so that the problems of one are told apart from those of
    /// the others and of the files.
    pub name: String,
    /// The entities removed, before `data` applies.
    pub removed: Removals,
    /// Data in the format of a data file (any of its sections): an entity whose key (the handle
    /// of a product, the email of a customer, the code of a gift card, …) is that of an entity
    /// already there replaces it where it is, the others are added after. An entity that gives
    /// its `id` replaces the one with that id, whatever its key: its handle (or email, or name)
    /// can change and its place stays. `shop` is merged field by field, `checkout` field by
    /// field and the settings of its Functions Function by Function; `localization`, `session`
    /// and `now` replace the files' own.
    pub data: Json,
}

impl Overlay {
    /// An overlay that adds and replaces, and removes nothing.
    pub fn new(name: impl Into<String>, data: Json) -> Overlay {
        Overlay {
            name: name.into(),
            removed: Removals::default(),
            data,
        }
    }
}

/// The entities an overlay removes, each by the key an overlay replaces it by. Removing an
/// entity the data does not have does nothing. The others keep their ids and their dates: an
/// entity without a date after a removed one keeps the date its position gave it.
///
/// What the data that stays says of a removed entity is not changed: a collection that lists a
/// removed product by its handle reports it as unknown. An overlay that removes an entity
/// replaces what names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Removals {
    /// Handles.
    pub products: Vec<String>,
    /// Handles.
    pub collections: Vec<String>,
    /// Handles.
    pub pages: Vec<String>,
    /// Handles. A blog goes with its articles.
    pub blogs: Vec<String>,
    /// Handles.
    pub menus: Vec<String>,
    /// Emails, in any case.
    pub customers: Vec<String>,
    /// Names, in any case.
    pub companies: Vec<String>,
    /// Codes, in any case.
    pub gift_cards: Vec<String>,
    /// Names, in any case.
    pub locations: Vec<String>,
    /// Names, in any case.
    pub selling_plan_groups: Vec<String>,
    /// Handles, by type.
    pub metaobjects: IndexMap<String, Vec<String>>,
    /// The metafields of the shop, as `namespace.key`: `shop` is merged field by field, so a
    /// metafield it no longer has is removed here.
    pub shop_metafields: Vec<String>,
}

impl Removals {
    pub fn is_empty(&self) -> bool {
        *self == Removals::default()
    }
}

/// The handle an entity gives, else the one its title makes.
fn effective_handle<'a>(handle: &'a Option<String>, title: &str) -> Cow<'a, str> {
    match handle {
        Some(handle) => Cow::Borrowed(handle),
        None => Cow::Owned(crate::util::handleize(title)),
    }
}

/// Merges `patch` into `base`, key by key; anything that is not an object replaces the base.
fn merge_json(base: &mut Json, patch: &Json) {
    match (base, patch) {
        (Json::Object(base), Json::Object(patch)) => {
            for (key, value) in patch {
                merge_json(base.entry(key.clone()).or_insert(Json::Null), value);
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// Removes what an overlay removes from the merged data. The entities without a date that come
/// after a removed one get the date they had, which their position gives them.
fn apply_removals(merged: &mut MergedInput, removed: &Removals) {
    if removed.is_empty() {
        return;
    }
    // The keys removed, in sets: each entity is looked up once.
    fn listed(keys: &[String]) -> HashSet<&str> {
        keys.iter().map(String::as_str).collect()
    }
    fn listed_any_case(keys: &[String]) -> HashSet<String> {
        keys.iter().map(|key| key.to_lowercase()).collect()
    }
    let date =
        |index: usize| Some(undated(index).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

    for (index, product) in merged.products.iter_mut().enumerate() {
        product.value.created_at = product.value.created_at.take().or_else(|| date(index));
    }
    let products = listed(&removed.products);
    merged.products.retain(|product| {
        let handle = effective_handle(&product.value.handle, &product.value.title);
        !products.contains(handle.as_ref())
    });
    for (index, collection) in merged.collections.iter_mut().enumerate() {
        let value = &mut collection.value;
        value.published_at = value.published_at.take().or_else(|| date(index));
    }
    let collections = listed(&removed.collections);
    merged.collections.retain(|collection| {
        let handle = effective_handle(&collection.value.handle, &collection.value.title);
        !collections.contains(handle.as_ref())
    });
    for (index, page) in merged.pages.iter_mut().enumerate() {
        page.value.published_at = page.value.published_at.take().or_else(|| date(index));
    }
    let pages = listed(&removed.pages);
    merged.pages.retain(|page| {
        let handle = effective_handle(&page.value.handle, &page.value.title);
        !pages.contains(handle.as_ref())
    });
    let blogs = listed(&removed.blogs);
    merged.blogs.retain(|blog| {
        let handle = effective_handle(&blog.value.handle, &blog.value.title);
        !blogs.contains(handle.as_ref())
    });
    let menus = listed(&removed.menus);
    merged
        .menus
        .retain(|handle, _| !menus.contains(handle.as_str()));
    let customers = listed_any_case(&removed.customers);
    merged
        .customers
        .retain(|customer| !customers.contains(&customer.value.email.to_lowercase()));
    for (index, company) in merged.companies.iter_mut().enumerate() {
        company.value.created_at = company.value.created_at.take().or_else(|| date(index));
    }
    let companies = listed_any_case(&removed.companies);
    merged
        .companies
        .retain(|company| !companies.contains(&company.value.name.to_lowercase()));
    let gift_cards = listed_any_case(&removed.gift_cards);
    merged
        .gift_cards
        .retain(|card| !gift_cards.contains(&card.value.code.to_lowercase()));
    let locations = listed_any_case(&removed.locations);
    merged
        .locations
        .retain(|location| !locations.contains(&location.value.name.to_lowercase()));
    let selling_plan_groups = listed_any_case(&removed.selling_plan_groups);
    merged
        .selling_plan_groups
        .retain(|group| !selling_plan_groups.contains(&group.value.name.to_lowercase()));
    let metaobjects: Vec<(&str, HashSet<&str>)> = removed
        .metaobjects
        .iter()
        .map(|(kind, handles)| (kind.as_str(), listed(handles)))
        .collect();
    merged.metaobjects.retain(|(kind, entry)| {
        !metaobjects.iter().any(|(removed_kind, handles)| {
            super::same_metaobject_type(removed_kind, kind)
                && handles.contains(entry.value.handle.as_str())
        })
    });
    if let Some(shop) = &mut merged.shop {
        for name in &removed.shop_metafields {
            let Some((namespace, key)) = name.split_once('.') else {
                continue;
            };
            if let Some(fields) = shop.value.metafields.get_mut(namespace) {
                fields.shift_remove(key);
                if fields.is_empty() {
                    shop.value.metafields.shift_remove(namespace);
                }
            }
        }
    }
}

/// Applies the data of an overlay on top of the merged data: entities with the key of an
/// existing one replace it, the others are added, and `shop` is merged field by field.
fn apply_overlay(
    merged: &mut MergedInput,
    name: &str,
    overlay: &Json,
    diagnostics: &mut Diagnostics,
) {
    let Some(overlay) = checked(FileKind::Store, name, overlay, "", diagnostics) else {
        return;
    };
    // `shop` is merged from the JSON, field by field: only what it gives changes.
    let shop_patch = overlay.get("shop").cloned();
    let input = match serde_json::from_value::<model::StoreInput>(overlay) {
        Ok(input) => input,
        Err(error) => {
            diagnostics.error("invalid_value", name, "", error.to_string());
            return;
        }
    };
    let origin = |pointer: String| Origin::new(name, pointer);
    if let Some(patch) = shop_patch {
        let mut shop = merged
            .shop
            .as_ref()
            .and_then(|shop| serde_json::to_value(&shop.value).ok())
            .unwrap_or(Json::Object(serde_json::Map::new()));
        merge_json(&mut shop, &patch);
        if let Ok(value) = serde_json::from_value::<model::ShopInput>(shop) {
            merged.shop = Some(Sourced {
                value,
                origin: origin("/shop".to_string()),
            });
        }
    }
    upsert(&mut merged.products, input.products, name, "products");
    upsert(
        &mut merged.collections,
        input.collections,
        name,
        "collections",
    );
    upsert(&mut merged.pages, input.pages, name, "pages");
    upsert(&mut merged.blogs, input.blogs, name, "blogs");
    upsert(&mut merged.customers, input.customers, name, "customers");
    upsert(&mut merged.companies, input.companies, name, "companies");
    upsert(&mut merged.gift_cards, input.gift_cards, name, "gift_cards");
    upsert(&mut merged.locations, input.locations, name, "locations");
    upsert(
        &mut merged.selling_plan_groups,
        input.selling_plan_groups,
        name,
        "selling_plan_groups",
    );
    for (name, value) in input.swatches {
        let origin = origin(format!("/swatches/{name}"));
        merged.swatches.insert(name, Sourced { value, origin });
    }
    for (handle, value) in input.menus {
        let origin = origin(format!("/menus/{handle}"));
        merged.menus.insert(handle, Sourced { value, origin });
    }
    for (kind, entries) in input.metaobjects {
        let replaced: HashSet<&str> = entries.iter().map(|entry| entry.handle.as_str()).collect();
        merged.metaobjects.retain(|(existing, entry)| {
            !super::same_metaobject_type(existing, &kind)
                || !replaced.contains(entry.value.handle.as_str())
        });
        for (index, value) in entries.into_iter().enumerate() {
            let origin = origin(format!("/metaobjects/{kind}/{index}"));
            merged
                .metaobjects
                .push((kind.clone(), Sourced { value, origin }));
        }
    }
    if let Some(value) = input.localization {
        merged.localization = Some(Sourced {
            value,
            origin: origin("/localization".to_string()),
        });
    }
    if let Some(value) = input.session {
        merged.session = Some(Sourced {
            value,
            origin: origin("/session".to_string()),
        });
    }
    if let Some(patch) = input.checkout {
        // As a scenario's: what it gives replaces the store data's, field by field, and the
        // settings of the Functions are merged by Function.
        let value = match merged.checkout.take() {
            Some(base) => {
                let base = base.value;
                let mut functions = base.functions;
                functions.extend(patch.functions);
                let mut extensions = base.extensions;
                extensions.extend(patch.extensions);
                model::CheckoutInput {
                    shipping_rates: patch.shipping_rates.or(base.shipping_rates),
                    payment_methods: patch.payment_methods.or(base.payment_methods),
                    discounts: patch.discounts.or(base.discounts),
                    functions,
                    extensions,
                    tax_rate: patch.tax_rate.or(base.tax_rate),
                }
            }
            None => patch,
        };
        merged.checkout = Some(Sourced {
            value,
            origin: origin("/checkout".to_string()),
        });
    }
    if let Some(value) = input.now {
        merged.now = Some(Sourced {
            value,
            origin: origin("/now".to_string()),
        });
    }
    merged.files.extend(input.files);
    for key in input.theme_settings.keys() {
        merged
            .theme_settings_origins
            .insert(key.clone(), origin(format!("/theme_settings/{key}")));
    }
    merged.theme_settings.extend(input.theme_settings);
    merge_app(&mut merged.app, input.app);
}

/// An entity of a list that an overlay upserts into ([`upsert`]).
trait Upserted {
    /// The kind of the ids the build derives from the keys ([`crate::util::stable_id`]), for an
    /// entity with ids. One without (a gift card, a selling plan group) replaces by its key only.
    const KIND: Option<&'static str>;
    /// The key it replaces an entity of: a handle, an email in lowercase, …
    fn key(&self) -> Cow<'_, str>;
    /// The id it gives.
    fn id(&self) -> Option<u64> {
        None
    }
}

/// Implements [`Upserted`] for an input with ids of `$kind`, or for one without ids.
macro_rules! upserted {
    ($type:ty, $kind:literal, |$entity:ident| $key:expr) => {
        impl Upserted for $type {
            const KIND: Option<&'static str> = Some($kind);
            fn key(&self) -> Cow<'_, str> {
                let $entity = self;
                $key
            }
            fn id(&self) -> Option<u64> {
                self.id
            }
        }
    };
    ($type:ty, |$entity:ident| $key:expr) => {
        impl Upserted for $type {
            const KIND: Option<&'static str> = None;
            fn key(&self) -> Cow<'_, str> {
                let $entity = self;
                $key
            }
        }
    };
}

upserted!(model::ProductInput, "product", |product| effective_handle(
    &product.handle,
    &product.title
));
upserted!(model::CollectionInput, "collection", |collection| {
    effective_handle(&collection.handle, &collection.title)
});
upserted!(model::PageInput, "page", |page| effective_handle(
    &page.handle,
    &page.title
));
upserted!(model::BlogInput, "blog", |blog| effective_handle(
    &blog.handle,
    &blog.title
));
upserted!(model::CustomerInput, "customer", |customer| Cow::Owned(
    customer.email.to_lowercase()
));
upserted!(model::CompanyInput, "company", |company| Cow::Owned(
    company.name.to_lowercase()
));
upserted!(model::GiftCardInput, |card| Cow::Owned(
    card.code.to_uppercase()
));
upserted!(model::LocationInput, "location", |location| Cow::Owned(
    location.name.to_lowercase()
));
upserted!(model::SellingPlanGroupInput, |group| Cow::Owned(
    group.name.to_lowercase()
));

/// Upserts the entities of an overlay's `field` into `list`, one after the other. An entity
/// replaces the one with its id when it gives one (an entity whose key changed stays where it
/// was), the first with its key that this overlay did not write otherwise, else goes after the
/// others: two entities of one key in an overlay (two companies of one name, which Shopify
/// allows) are both kept, for the build to say that the store data cannot have both.
fn upsert<T: Upserted>(list: &mut Vec<Sourced<T>>, incoming: Vec<T>, name: &str, field: &str) {
    let places = places(list, &incoming);
    for ((index, value), place) in incoming.into_iter().enumerate().zip(places) {
        let sourced = Sourced {
            value,
            origin: Origin::new(name, format!("/{field}/{index}")),
        };
        match place {
            Some(at) => list[at] = sourced,
            None => list.push(sourced),
        }
    }
}

/// Where each of `incoming` goes in `list` as [`upsert`] says: in place of the entity at the
/// position, or after the others (`None`). The keys and the ids of `list` are read once into
/// maps, so that the cost grows with the length of `list` plus that of `incoming`.
fn places<T: Upserted>(list: &[Sourced<T>], incoming: &[T]) -> Vec<Option<usize>> {
    if incoming.is_empty() {
        return Vec::new();
    }
    let keys: Vec<Cow<'_, str>> = list.iter().map(|existing| existing.value.key()).collect();
    // The positions of each key, in order, with how many of the first ones this overlay wrote:
    // an entity is only replaced by its key while the overlay did not write over it.
    let mut by_key: HashMap<&str, (usize, Vec<usize>)> = HashMap::with_capacity(keys.len());
    for (at, key) in keys.iter().enumerate() {
        by_key.entry(key).or_default().1.push(at);
    }
    let mut ids = T::KIND
        .filter(|_| incoming.iter().any(|value| value.id().is_some()))
        .map(|kind| Ids::new(list, &keys, kind));
    let mut written = vec![false; list.len()];
    let mut len = list.len();
    incoming
        .iter()
        .map(|value| {
            let key = value.key();
            let by_id = value
                .id()
                .and_then(|id| ids.as_ref().and_then(|ids| ids.first(id)));
            let place = by_id.or_else(|| {
                let (skipped, positions) = by_key.get_mut(key.as_ref())?;
                while positions.get(*skipped).is_some_and(|&at| written[at]) {
                    *skipped += 1;
                }
                positions.get(*skipped).copied()
            });
            let at = place.unwrap_or(len);
            if let Some(written) = written.get_mut(at) {
                *written = true;
            }
            if place.is_none() {
                len += 1;
            }
            if let (Some(ids), Some(kind)) = (&mut ids, T::KIND) {
                ids.set(
                    at,
                    value
                        .id()
                        .unwrap_or_else(|| crate::util::stable_id(kind, &key)),
                );
            }
            place
        })
        .collect()
}

/// The id of each entity of a list while an overlay writes it ([`places`]): the one it gives,
/// else the one the build derives from its key. With the positions of each id, in order.
struct Ids {
    of: Vec<u64>,
    at: HashMap<u64, Vec<usize>>,
}

impl Ids {
    fn new<T: Upserted>(list: &[Sourced<T>], keys: &[Cow<'_, str>], kind: &str) -> Ids {
        let mut ids = Ids {
            of: Vec::with_capacity(list.len()),
            at: HashMap::with_capacity(list.len()),
        };
        for (existing, key) in list.iter().zip(keys) {
            let id = existing
                .value
                .id()
                .unwrap_or_else(|| crate::util::stable_id(kind, key));
            ids.at.entry(id).or_default().push(ids.of.len());
            ids.of.push(id);
        }
        ids
    }

    /// The first position of an entity with `id`.
    fn first(&self, id: u64) -> Option<usize> {
        self.at.get(&id)?.first().copied()
    }

    /// The entity at `at` (or after the others) has `id` now.
    fn set(&mut self, at: usize, id: u64) {
        if let Some(old) = self.of.get_mut(at) {
            if *old == id {
                return;
            }
            if let Some(positions) = self.at.get_mut(old)
                && let Ok(index) = positions.binary_search(&at)
            {
                positions.remove(index);
            }
            *old = id;
        } else {
            self.of.push(id);
        }
        let positions = self.at.entry(id).or_default();
        if let Err(index) = positions.binary_search(&at) {
            positions.insert(index, at);
        }
    }
}

/// Loads the store with extra data applied on top of the files, as tests do to set up the
/// exact situation they need without touching the shared fixtures. Its problems are reported
/// under [`OVERLAY_FILE`].
pub fn load_with_overlay(
    source: &DataSource,
    options: &LoadOptions,
    overlay: Option<&Json>,
) -> (Store, Diagnostics) {
    let overlays: Vec<Overlay> = overlay
        .map(|data| Overlay::new(OVERLAY_FILE, data.clone()))
        .into_iter()
        .collect();
    load_with_overlays(source, options, &overlays)
}

/// Loads the store with overlays applied on top of the files, one after the other: each removes
/// what it removes, then adds and replaces, on top of the files and the overlays before it.
pub fn load_with_overlays(
    source: &DataSource,
    options: &LoadOptions,
    overlays: &[Overlay],
) -> (Store, Diagnostics) {
    SourceData::read(source).build(options, overlays)
}

/// The store data as its files gave it: read, checked and merged once, before any overlay.
/// [`SourceData::build`] makes a store of it, with overlays on top, as often as needed and
/// without reading the files again: a server builds its store again whenever the Admin API
/// changes it, on top of the files it started from (`lsf serve --static` never looks at them
/// again, and a file changed on disk that was not reloaded must not leak in). The pixel sizes
/// of the images of `files/` are read once too, by the first build that needs each.
pub struct SourceData {
    merged: MergedInput,
    /// The problems of reading and merging the files.
    diagnostics: Diagnostics,
    /// `files/`, for the sizes of the images.
    files_directory: Option<PathBuf>,
    /// The sizes read so far, by path in `files/` (`None`: not an image there).
    images: Mutex<HashMap<String, Option<(u32, u32)>>>,
}

impl SourceData {
    /// Reads the files of `source`.
    pub fn read(source: &DataSource) -> SourceData {
        let mut diagnostics = Diagnostics::new();
        let (files, files_directory) = match source {
            DataSource::Directory(root) => (
                read_directory(root, &mut diagnostics),
                Some(root.join("files")),
            ),
            DataSource::Demo => (demo_files(), None),
        };
        let merged = merge(files, &mut diagnostics);
        SourceData {
            merged,
            diagnostics,
            files_directory,
            images: Mutex::new(HashMap::new()),
        }
    }

    /// The store of the files with `overlays` applied on top, one after the other, and its
    /// problems: those of the files, then those of each overlay, then those of the build.
    pub fn build(&self, options: &LoadOptions, overlays: &[Overlay]) -> (Store, Diagnostics) {
        let mut diagnostics = self.diagnostics.clone();
        let mut merged = self.merged.clone();
        for overlay in overlays {
            apply_removals(&mut merged, &overlay.removed);
            if !overlay.data.is_null() {
                apply_overlay(&mut merged, &overlay.name, &overlay.data, &mut diagnostics);
            }
        }
        let probe = |src: &str| -> Option<(u32, u32)> {
            let directory = self.files_directory.as_ref()?;
            let images = || {
                self.images
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            };
            if let Some(known) = images().get(src) {
                return *known;
            }
            let size = image::image_dimensions(directory.join(src)).ok();
            images().insert(src.to_string(), size);
            size
        };
        let (store, build_diagnostics) = build(
            merged,
            &BuildOptions {
                theme_locales: options.theme_locales.clone(),
                settings_schema: options.settings_schema.as_deref(),
                probe_image: &probe,
            },
        );
        diagnostics.extend(build_diagnostics);
        (store, diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_store_is_valid() {
        let (store, diagnostics) = load(&DataSource::Demo, &LoadOptions::default());
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert_eq!(store.products.len(), 8);
        let tee = store
            .product_by_handle("organic-cotton-t-shirt")
            .expect("tee");
        assert_eq!(tee.variants.len(), 12);
        assert_eq!(tee.options[0].values, vec!["White", "Black", "Sage"]);
        assert!(!tee.variants[3].available);
        assert_eq!(tee.variants[0].media_index, Some(0));
        let apparel = store.collection_by_handle("apparel").expect("apparel");
        assert_eq!(apparel.products.len(), 2);
        assert_eq!(
            store
                .collection_by_handle("all")
                .expect("all")
                .products
                .len(),
            8
        );
        assert_eq!(
            store
                .collection_by_handle("sale")
                .expect("sale")
                .products
                .len(),
            2
        );
        assert_eq!(store.menu("main-menu").expect("menu").levels(), 3);
        assert_eq!(store.customers[0].orders[0].line_items.len(), 2);
        assert_eq!(store.languages.len(), 2);
        // A B2B customer buys for a company, at every location unless the data says which.
        let buyer = store
            .customer_by_email("alex.morgan@example.com")
            .expect("buyer");
        let company = &store.companies[buyer.company.expect("company")];
        assert_eq!(company.name, "Northwind Hotels");
        assert_eq!(buyer.company_locations, vec![0, 1]);
        assert_eq!(buyer.current_location(None, &store), Some(0));
        assert_eq!(
            buyer.current_location(Some(company.locations[1].id), &store),
            Some(1)
        );
        assert_eq!(store.customers[0].company, None);
        // The password page has a password.
        assert_eq!(store.shop.password, "password");
        // Option values take the swatch the store gives to their name.
        assert_eq!(
            tee.options[0]
                .swatch("Sage")
                .and_then(|s| s.color.as_deref()),
            Some("#9caf88")
        );
        assert!(tee.options[1].swatch("M").is_none());
        // Every location stocks a variant unless the variant says which ones do.
        assert_eq!(store.locations.len(), 2);
        assert_eq!(
            tee.variants[0].store_availabilities,
            vec![(0, true), (1, true)]
        );
        assert_eq!(
            tee.variants[3].store_availabilities,
            vec![(0, false), (1, false)]
        );
        let mug = store.product_by_handle("ceramic-mug").expect("mug");
        assert_eq!(
            mug.variants[1].store_availabilities,
            vec![(0, true), (1, false)]
        );
        // The socks are sold by subscription too.
        let socks = store.product_by_handle("merino-crew-socks").expect("socks");
        assert_eq!(socks.selling_plan_groups, vec![0]);
        assert!(tee.selling_plan_groups.is_empty());
        let group = &store.selling_plan_groups[0];
        assert_eq!(group.options, vec!["Delivery frequency"]);
        assert_eq!(group.id.len(), 40);
        assert_eq!(group.plans[0].options, vec!["Every month"]);
        assert_eq!(group.plans[0].allocation(&socks.variants[0]).price, 1440);
        assert!(store.selling_plan_of(socks, group.plans[1].id).is_some());
        assert!(store.selling_plan_of(tee, group.plans[1].id).is_none());
    }

    fn inline(data: serde_json::Value) -> (crate::store::Store, Diagnostics) {
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&data))
    }

    #[test]
    fn a_variant_said_unavailable_is_sold_out() {
        // On Shopify an untracked variant is always available: `available: false` means tracked
        // with nothing in stock, so that themes say "Sold out", not "Unavailable".
        let (store, diagnostics) = inline(serde_json::json!({"products": [
            {"title": "Sold out thing", "price": "10.00", "available": false},
            {"title": "Backorder", "price": "10.00", "variants": [
                {"available": false, "inventory_policy": "continue"},
            ]},
            {"title": "Untracked", "price": "10.00", "variants": [
                {"available": false, "inventory_tracked": false},
            ]},
            {"title": "Counted", "price": "10.00", "variants": [
                {"available": false, "inventory_quantity": 3},
            ]},
        ]}));
        // Unavailable with stock: Shopify never has both, and the data is warned about it.
        let found: Vec<(&str, &str, &str)> = diagnostics
            .items
            .iter()
            .map(|item| {
                (
                    item.code,
                    item.path.as_str(),
                    if item.severity == crate::diagnostics::Severity::Warning {
                        "warning"
                    } else {
                        "error"
                    },
                )
            })
            .collect();
        assert_eq!(
            found,
            [(
                "unavailable_in_stock",
                "/products/3/variants/0/available",
                "warning"
            )],
            "{diagnostics}"
        );
        assert!(
            diagnostics.items[0]
                .message
                .contains("lsf keeps it unavailable with 3 in stock"),
            "{diagnostics}"
        );
        let variant =
            |handle: &str| store.product_by_handle(handle).expect(handle).variants[0].clone();
        let sold_out = variant("sold-out-thing");
        assert!(!sold_out.available);
        assert!(sold_out.inventory_tracked);
        assert_eq!(sold_out.inventory_quantity, 0);
        assert_eq!(sold_out.inventory_policy, model::InventoryPolicy::Deny);
        // What the data says otherwise is kept.
        assert!(!variant("backorder").inventory_tracked);
        assert!(!variant("untracked").inventory_tracked);
        let counted = variant("counted");
        assert!(counted.inventory_tracked);
        assert_eq!(counted.inventory_quantity, 3);
    }

    #[test]
    fn typed_metafields_are_checked_against_their_type() {
        let (_, diagnostics) = inline(serde_json::json!({
            "products": [{"title": "Thing", "price": "1.00", "metafields": {"custom": {
                "count": {"type": "number_integer", "value": "two"},
                "related": {"type": "list.product_reference", "value": ["ceramic-mug", "ceramic-mugg", "does-not-exist"]},
                "guide": {"type": "page_reference", "value": "contact"},
                "weight": {"type": "weight", "value": {"value": 2, "unit": "kgs"}},
                "kind": {"type": "number_interger", "value": 2},
                "plain": "inferred, never wrong",
            }}}],
            "metaobjects": {"designer": [{"handle": "sam", "fields": {
                "age": {"type": "number_integer", "value": 3.5},
                "friend": {"type": "metaobject_reference", "value": "designer/samm"},
            }}]},
        }));
        let found: Vec<String> = diagnostics
            .items
            .iter()
            .map(|item| {
                format!(
                    "{} {} {}",
                    item.code,
                    item.path,
                    item.hint.as_deref().unwrap_or_default()
                )
            })
            .collect();
        let at = |path: &str| {
            found
                .iter()
                .find(|line| line.contains(&format!(" {path} ")))
                .unwrap_or_else(|| panic!("nothing at {path}: {found:#?}"))
                .clone()
        };
        let base = "/products/0/metafields/custom";
        assert!(at(&format!("{base}/count/value")).starts_with("invalid_metafield"));
        assert_eq!(
            at(&format!("{base}/related/value/1")),
            format!("unknown_reference {base}/related/value/1 did you mean \"ceramic-mug\"?")
        );
        assert!(at(&format!("{base}/related/value/2")).starts_with("unknown_reference"));
        assert!(at(&format!("{base}/weight/value/unit")).ends_with("did you mean \"kg\"?"));
        assert!(at(&format!("{base}/kind/type")).starts_with("unknown_metafield_type"));
        assert!(at("/metaobjects/designer/0/fields/age/value").starts_with("invalid_metafield"));
        assert!(
            at("/metaobjects/designer/0/fields/friend/value")
                .ends_with("did you mean \"designer/sam\"?")
        );
        assert_eq!(found.len(), 7, "{found:#?}");
    }

    /// Issue #43: `faq.size`, `size`, `faq:size` and `{"type": "faq", "handle": "size"}` passed
    /// `validate` and rendered `nil`. Every reference Liquid cannot resolve is an error that
    /// names the form lsf takes, and a GID names the entity of that id.
    #[test]
    fn a_reference_lsf_does_not_resolve_is_an_error_with_the_form_it_takes() {
        use crate::store::metafield_references::Referenced;
        let (demo, _) = inline(serde_json::json!({}));
        let mug = demo.product_by_handle("ceramic-mug").expect("mug");
        let variant = &mug.variants[0];
        let page = &demo.pages[0];
        let faq_size = crate::util::stable_id("metaobject", "faq/size");
        let (store, diagnostics) = inline(serde_json::json!({
            "metaobjects": {"faq": [
                {"handle": "size", "fields": {"question": "Which size?"}},
                {"handle": "returns", "fields": {"question": "Returns?"}},
            ]},
            "products": [{"title": "Thing", "price": "1.00", "metafields": {"custom": {
                "faqs": {"type": "list.metaobject_reference", "value": [
                    "faq/size",
                    "faq.size",
                    "size",
                    "faq:size",
                    {"type": "faq", "handle": "size"},
                    format!("gid://shopify/Metaobject/{faq_size}"),
                    "gid://shopify/Metaobject/123",
                ]},
                "mug": {"type": "product_reference", "value": format!("gid://shopify/Product/{}", mug.id)},
                "mug_path": {"type": "product_reference", "value": "/products/ceramic-mug"},
                "gone": {"type": "product_reference", "value": "gid://shopify/Product/1"},
                "not_a_page": {"type": "page_reference", "value": format!("gid://shopify/Product/{}", mug.id)},
                "page": {"type": "page_reference", "value": format!("gid://shopify/Page/{}", page.id)},
                "variant": {"type": "variant_reference", "value": format!("gid://shopify/ProductVariant/{}", variant.id)},
                "variant_id": {"type": "variant_reference", "value": variant.id.to_string()},
                "file": {"type": "file_reference", "value": "gid://shopify/MediaImage/1"},
            }}}],
        }));
        let found: Vec<String> = diagnostics
            .items
            .iter()
            .map(|item| {
                format!(
                    "{} {} | {} | {}",
                    item.code,
                    item.path,
                    item.message,
                    item.hint.as_deref().unwrap_or_default()
                )
            })
            .collect();
        let at = |path: &str| {
            found
                .iter()
                .find(|line| line.contains(&format!(" {path} |")))
                .unwrap_or_else(|| panic!("nothing at {path}: {found:#?}"))
                .clone()
        };
        let base = "/products/0/metafields/custom";
        for index in 1..=3 {
            assert!(
                at(&format!("{base}/faqs/value/{index}")).ends_with("| use \"faq/size\""),
                "{found:#?}"
            );
        }
        assert!(
            at(&format!("{base}/faqs/value/4")).ends_with("| use \"faq/size\""),
            "{found:#?}"
        );
        let unknown_gid = at(&format!("{base}/faqs/value/6"));
        assert!(
            unknown_gid.starts_with("unknown_reference"),
            "{unknown_gid}"
        );
        assert!(unknown_gid.contains("\"faq/size\""), "{unknown_gid}");
        assert!(
            at(&format!("{base}/mug_path/value")).ends_with("| use \"ceramic-mug\""),
            "{found:#?}"
        );
        assert!(at(&format!("{base}/gone/value")).starts_with("unknown_reference"));
        assert!(at(&format!("{base}/not_a_page/value")).starts_with("unknown_reference"));
        assert!(
            at(&format!("{base}/variant_id/value")).ends_with(&format!("| use {}", variant.id)),
            "{found:#?}"
        );
        assert!(at(&format!("{base}/file/value")).starts_with("unknown_reference"));
        assert_eq!(found.len(), 10, "{found:#?}");
        assert!(
            diagnostics
                .items
                .iter()
                .all(|item| item.severity == crate::diagnostics::Severity::Error)
        );

        // What resolves is what Liquid reads.
        let product = store.product_by_handle("thing").expect("thing");
        let value = |key: &str| &product.metafields["custom"][key].value;
        let faqs = value("faqs").as_array().expect("a list");
        for (index, expected) in [(0, Some("size")), (1, None), (5, Some("size")), (6, None)] {
            let found = store
                .metafield_reference("metaobject_reference", &faqs[index])
                .map(|target| match target {
                    Referenced::Metaobject(entry) => store.metaobjects[entry].handle.as_str(),
                    other => panic!("{other:?}"),
                });
            assert_eq!(found, expected, "faqs[{index}]");
        }
        assert_eq!(
            store.metafield_reference("product_reference", value("mug")),
            store.product_index("ceramic-mug").map(Referenced::Product)
        );
        assert!(
            store
                .metafield_reference("page_reference", value("page"))
                .is_some()
        );
        assert!(
            store
                .metafield_reference("variant_reference", value("variant"))
                .is_some()
        );
    }

    /// A GID the Admin API keeps names an entity deleted since: it reads nothing, as on
    /// Shopify, and is no mistake of the data.
    #[test]
    fn a_gid_of_the_admin_api_that_names_nothing_is_no_error() {
        let overlay = Overlay::new(
            ADMIN_OVERLAY_FILE,
            serde_json::json!({"products": [{"title": "Thing", "price": "1.00", "metafields": {
                "custom": {"designer": {"type": "metaobject_reference", "value": "gid://shopify/Metaobject/123"}}
            }}]}),
        );
        let (_, diagnostics) =
            load_with_overlays(&DataSource::Demo, &LoadOptions::default(), &[overlay]);
        assert!(diagnostics.is_empty(), "{diagnostics}");
    }

    #[test]
    fn a_metaobject_is_translated_field_by_field() {
        let rich_text = |text: &str| {
            serde_json::json!({"type": "root", "children": [
                {"type": "paragraph", "children": [{"type": "text", "value": text}]},
            ]})
        };
        let (store, diagnostics) = inline(serde_json::json!({"metaobjects": {"faq": [{
            "handle": "shipping",
            "fields": {
                "question": "How long does shipping take?",
                "answer": {"type": "rich_text_field", "value": rich_text("Two days.")},
                "tags": {"type": "list.single_line_text_field", "value": ["delivery"]},
                "order": 2,
            },
            "translations": {"FR": {
                "question": "Combien de temps prend la livraison ?",
                "answer": rich_text("Deux jours."),
                "tags": ["livraison"],
            }},
        }]}}));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let entry = store.metaobject("faq", "shipping").expect("the entry");
        let question = |locale: &str| {
            entry
                .field_in("question", locale)
                .map(|field| field.value.clone())
        };
        assert_eq!(
            question("fr"),
            Some(serde_json::json!("Combien de temps prend la livraison ?"))
        );
        // The primary language, and a language without a translation, show the field itself.
        assert_eq!(
            question("en"),
            Some(serde_json::json!("How long does shipping take?"))
        );
        let answer = entry.field_in("answer", "fr").expect("the answer");
        assert_eq!(answer.kind, "rich_text_field");
        assert_eq!(answer.value, rich_text("Deux jours."));
        assert_eq!(
            entry
                .field_in("order", "fr")
                .map(|field| field.value.clone()),
            Some(serde_json::json!(2))
        );
        assert!(entry.field_in("nope", "fr").is_none());
        // The language is written as `localization.languages` has it.
        assert!(entry.translations.contains_key("fr"));
    }

    #[test]
    fn a_translation_written_as_json_text_is_kept_as_the_field_value_is() {
        // The Admin API's `value` of a rich text is its JSON text: the field takes it, and so
        // does its translation.
        let rich_text = |text: &str| {
            serde_json::json!({"type": "root", "children": [
                {"type": "paragraph", "children": [{"type": "text", "value": text}]},
            ]})
        };
        let (store, diagnostics) = inline(serde_json::json!({"metaobjects": {"faq": [{
            "handle": "shipping",
            "fields": {
                "answer": {"type": "rich_text_field", "value": rich_text("Two days.").to_string()},
            },
            "translations": {"fr": {"answer": rich_text("Deux jours.").to_string()}},
        }]}}));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let entry = store.metaobject("faq", "shipping").expect("the entry");
        let answer = |locale: &str| {
            entry
                .field_in("answer", locale)
                .expect("the answer")
                .value
                .clone()
        };
        assert_eq!(answer("en"), rich_text("Two days."));
        assert_eq!(answer("fr"), rich_text("Deux jours."));
    }

    #[test]
    fn the_translations_of_a_metaobject_are_checked() {
        let (store, diagnostics) = inline(serde_json::json!({"metaobjects": {"faq": [{
            "handle": "shipping",
            "fields": {
                "question": "How long does shipping take?",
                "answer": {"type": "rich_text_field", "value": {"type": "root", "children": []}},
                "order": 2,
                "related": {"type": "product_reference", "value": "ceramic-mug"},
                "link": {"type": "url", "value": "https://example.com/shipping"},
            },
            "translations": {
                "fr": {
                    "questoin": "Combien de temps ?",
                    "order": 3,
                    "related": "canvas-tote-bag",
                    "link": "exemple.fr/livraison",
                    "answer": "",
                },
                "de": {"question": "Wie lange dauert der Versand?"},
                "en": {"question": "How long?"},
            },
        }]}}));
        let found: Vec<String> = diagnostics
            .items
            .iter()
            .map(|item| {
                format!(
                    "{} {} {}",
                    item.code,
                    item.path,
                    item.hint.as_deref().unwrap_or_default()
                )
            })
            .collect();
        let at = |path: &str| {
            found
                .iter()
                .find(|line| line.contains(&format!(" {path} ")))
                .unwrap_or_else(|| panic!("nothing at {path}: {found:#?}"))
                .clone()
        };
        let base = "/metaobjects/faq/0/translations";
        assert_eq!(
            at(&format!("{base}/fr/questoin")),
            format!("unknown_field {base}/fr/questoin did you mean \"question\"?")
        );
        assert!(at(&format!("{base}/fr/order")).starts_with("untranslatable_field"));
        assert!(at(&format!("{base}/fr/related")).starts_with("untranslatable_field"));
        // A translation is a value of the field's type.
        assert!(at(&format!("{base}/fr/link")).starts_with("invalid_metafield"));
        assert!(at(&format!("{base}/fr/answer")).starts_with("invalid_value"));
        assert!(at(&format!("{base}/de")).starts_with("unknown_language"));
        // The primary language is the fields themselves.
        assert!(at(&format!("{base}/en")).starts_with("unknown_language"));
        assert_eq!(found.len(), 7, "{found:#?}");
        // What is wrong is left out: the fields show their own values.
        let entry = store.metaobject("faq", "shipping").expect("the entry");
        assert_eq!(
            entry
                .field_in("link", "fr")
                .map(|field| field.value.clone()),
            Some(serde_json::json!("https://example.com/shipping"))
        );
        assert!(!entry.translations.contains_key("de"));
    }

    #[test]
    fn what_reads_of_a_file_with_errors_is_checked_in_the_same_run() {
        let files = vec![
            DataFile {
                name: "store.json".to_string(),
                kind: FileKind::Store,
                content: r#"{
                    "products": [
                        {"title": "Broken", "price": 1.5},
                        {"title": "Fine", "price": "2.00", "collections": ["nope"]}
                    ],
                    "collections": [{"title": "Picks", "products": ["broken", "fine", "gone"]}]
                }"#
                .to_string(),
            },
            DataFile {
                name: "products/unread.json".to_string(),
                kind: FileKind::Product,
                content: r#"{"title": "Unread", "price": "1.00", "tags": 5}"#.to_string(),
            },
            DataFile {
                name: "menus.json".to_string(),
                kind: FileKind::Store,
                content: r#"{"collections": [{"title": "More", "products": ["unread"]}]}"#
                    .to_string(),
            },
        ];
        let mut diagnostics = Diagnostics::new();
        let merged = merge(files, &mut diagnostics);
        let source = SourceData {
            merged,
            diagnostics,
            files_directory: None,
            images: Mutex::new(HashMap::new()),
        };
        let (store, diagnostics) = source.build(&LoadOptions::default(), &[]);
        let found: Vec<String> = diagnostics
            .items
            .iter()
            .map(|item| {
                format!(
                    "{} {} {} | {}",
                    item.code,
                    item.file,
                    item.path,
                    item.hint.as_deref().unwrap_or_default()
                )
            })
            .collect();
        // The product without errors of the file is read, with its own references.
        assert!(store.product_by_handle("fine").is_some());
        assert!(
            found
                .iter()
                .any(|line| line
                    .starts_with("unknown_collection store.json /products/1/collections/0")),
            "{found:#?}"
        );
        // Its place in the file is the one it has there.
        let picks = store.collection_by_handle("picks").expect("picks");
        assert_eq!(picks.products.len(), 1);
        // What names a product left out says why, rather than suggesting another.
        let broken = found
            .iter()
            .find(|line| line.contains("/collections/0/products/0"))
            .expect("broken");
        assert!(broken.ends_with("the product \"broken\" is left out because store.json has errors: fix them, and this reference works"), "{broken}");
        let unread = found
            .iter()
            .find(|line| line.starts_with("unknown_product menus.json"))
            .expect("unread");
        assert!(
            unread.contains("because products/unread.json has errors"),
            "{unread}"
        );
        let gone = found
            .iter()
            .find(|line| line.contains("/collections/0/products/2"))
            .expect("gone");
        assert!(!gone.contains("left out"), "{gone}");
    }

    #[test]
    fn customers_are_named_by_email_default_or_none() {
        let (store, _) = load(&DataSource::Demo, &LoadOptions::default());
        let email = |who: &str| {
            store
                .customer_named(who)
                .map(|customer| customer.map(|customer| customer.email.as_str()))
        };
        assert_eq!(email("none"), Ok(None));
        assert_eq!(email("default"), Ok(Some("jane.doe@example.com")));
        assert_eq!(
            email("ALEX.MORGAN@example.com"),
            Ok(Some("alex.morgan@example.com"))
        );
        assert!(email("nobody@example.com").is_err());
    }

    #[test]
    fn a_store_without_customers_has_a_default_one() {
        let directory =
            std::env::temp_dir().join(format!("lsf-no-customers-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            directory.join("store.json"),
            r#"{"products": [{"title": "Thing", "price": 100}], "session": {"customer": "default"}}"#,
        )
        .expect("store");
        let (store, diagnostics) = load(
            &DataSource::Directory(directory.clone()),
            &LoadOptions::default(),
        );
        std::fs::remove_dir_all(&directory).ok();
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert_eq!(store.customers.len(), 1);
        assert_eq!(
            store.customers[0].email,
            crate::store::build::DEFAULT_CUSTOMER_EMAIL
        );
        assert_eq!(
            crate::Session::initial(&store).customer_id,
            Some(store.customers[0].id)
        );
    }

    #[test]
    fn company_mistakes_are_explained() {
        let (_, diagnostics) = inline(serde_json::json!({
            "companies": [{"name": "Empty Co", "locations": []}],
            "customers": [
                {"email": "a@example.com", "company": "Northwind Hotel"},
                {"email": "b@example.com", "company": "Northwind Hotels", "company_locations": ["Northwind Portlnd"]},
                {"email": "c@example.com", "company_locations": ["Anywhere"]}
            ],
            "session": {"customer": "jane.doe@example.com", "company_location": "Northwind Portland"}
        }));
        let report = diagnostics.to_string();
        for expected in [
            "the company \"Empty Co\" has no location",
            "there is no company named \"Northwind Hotel\"",
            "did you mean \"Northwind Hotels\"?",
            "has no location named \"Northwind Portlnd\"",
            "did you mean \"Northwind Portland\"?",
            "lists company locations but names no company",
            "jane.doe@example.com does not buy for a company",
        ] {
            assert!(
                report.contains(expected),
                "missing {expected:?} in:\n{report}"
            );
        }
    }

    #[test]
    fn swatches_locations_and_selling_plans_are_resolved() {
        let (store, diagnostics) = inline(serde_json::json!({
            "swatches": {"Sand": {"color": "rgb(216, 199, 168)", "image": "swatches/sand.jpg"}},
            "locations": [{"name": "Pop-up", "pick_up_enabled": false}],
            "selling_plan_groups": [{
                "name": "Pre-order",
                "options": ["Ships", "Payment"],
                "selling_plans": [{
                    "name": "Ships in March, deposit",
                    "options": ["In March", "Deposit"],
                    "recurring_deliveries": false,
                    "checkout_charge": {"value_type": "percentage", "value": 50}
                }]
            }],
            "products": [{
                "title": "Sun Hat",
                "price": 4000,
                "options": [{"name": "Color", "values": [
                    "sand",
                    {"name": "Black", "swatch": "#000"},
                    {"name": "Floral", "swatch": {"image": "swatches/floral.jpg"}},
                    "Plain"
                ]}],
                "variants": [
                    {"options": ["sand"], "store_availabilities": {"pop-up": true}},
                    {"options": ["Black"], "store_availabilities": {}},
                    {"options": ["Floral"]},
                    {"options": ["Plain"]}
                ],
                "selling_plan_groups": ["pre-order"],
                "requires_selling_plan": true
            }],
            "session": {"cart": {"items": [
                {"variant": "sun-hat", "selling_plan": "ships in march, deposit"}
            ]}}
        }));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let hat = store.product_by_handle("sun-hat").expect("hat");
        let color = &hat.options[0];
        // The store's swatch, whatever the case of the name; then the value's own.
        let sand = color.swatch("sand").expect("sand");
        assert_eq!(sand.color.as_deref(), Some("rgb(216, 199, 168)"));
        assert_eq!(sand.image.as_ref().expect("image").src, "swatches/sand.jpg");
        assert_eq!(
            color.swatch("Black").expect("black").color.as_deref(),
            Some("#000")
        );
        let floral = color.swatch("Floral").expect("floral");
        assert!(floral.color.is_none() && floral.image.is_some());
        assert!(color.swatch("Plain").is_none());
        assert_eq!(store.image_size("swatches/floral.jpg"), Some((1200, 1200)));

        // The overlay's location comes after the two of the demo store.
        assert_eq!(store.locations.len(), 3);
        assert!(!store.locations[2].pick_up_enabled);
        assert_eq!(hat.variants[0].store_availabilities, vec![(2, true)]);
        assert!(hat.variants[1].store_availabilities.is_empty());
        assert_eq!(hat.variants[2].store_availabilities.len(), 3);

        assert!(hat.requires_selling_plan);
        let (group, plan) = store
            .selling_plan_of(
                hat,
                store.session_defaults.cart_lines[0]
                    .selling_plan
                    .expect("plan"),
            )
            .expect("the hat is sold with the plan");
        assert_eq!(group.name, "Pre-order");
        assert_eq!(plan.options, vec!["In March", "Deposit"]);
        assert!(!plan.recurring_deliveries);
        let allocation = plan.allocation(&hat.variants[0]);
        assert_eq!(
            (allocation.price, allocation.checkout_charge_amount),
            (4000, 2000)
        );
    }

    #[test]
    fn swatch_location_and_selling_plan_mistakes_are_explained() {
        let (_, diagnostics) = inline(serde_json::json!({
            "swatches": {"Sand": "sandy", "Empty": {}},
            "selling_plan_groups": [
                {"name": "Empty group", "selling_plans": [], "products": ["merino-crew-sock"]},
                {"name": "Odd", "options": ["Frequency", "Size"], "selling_plans": [
                    {"name": "One", "id": 7, "options": ["Monthly"], "price_adjustments": [{"value_type": "percentage", "value": "10.5"}]},
                    {"name": "one", "id": 7, "options": ["Monthly", "Big"], "price_adjustments": [
                        {"value_type": "percentage", "value": 150},
                        {"value_type": "price", "value": "12.00"},
                        {"value_type": "fixed_amount", "value": 100}
                    ]}
                ]}
            ],
            "products": [
                {"title": "Lamp", "price": 100, "variants": [{"store_availabilities": {"Otawa flagship": true}}]},
                {"title": "Desk", "price": 100, "selling_plan_groups": ["Subscribe and saves"]},
                {"title": "Chair", "price": 100, "requires_selling_plan": true}
            ],
            "session": {"cart": {"items": [
                {"variant": "MUG-OAT", "selling_plan": "Deliver every month, 10% off"},
                {"variant": "SOCK-S", "selling_plan": "Deliver every mnth, 10% off"},
                {"variant": "chair"}
            ]}}
        }));
        let report = diagnostics.to_string();
        for expected in [
            "error[invalid_color]: \"sandy\" is not a color",
            "error[empty_swatch]: the swatch has neither a color nor an image",
            "error[missing_selling_plan]: the selling plan group \"Empty group\" has no selling plan",
            "there is no product with the handle \"merino-crew-sock\"",
            "did you mean \"merino-crew-socks\"?",
            "the selling plan gives 1 option value but the group has 2 options: Frequency, Size",
            "error[invalid_value]: \"10.5\" is not a percentage",
            "error[invalid_value]: 150 is not a percentage",
            "another selling plan of this group is already named \"one\"",
            "the selling plan id 7 is already used by \"One\"",
            "a selling plan can have at most 2 price adjustments, this one has 3",
            "error[unknown_location]: there is no location named \"Otawa flagship\"",
            "did you mean \"Ottawa flagship\"?",
            "error[unknown_selling_plan]: there is no selling plan group named \"Subscribe and saves\"",
            "did you mean \"Subscribe and save\"?",
            "the product \"Chair\" requires a selling plan but is sold with none",
            "the product \"ceramic-mug\" is not sold with a selling plan named \"Deliver every month, 10% off\"",
            "\"ceramic-mug\" is sold without selling plans",
            "did you mean \"Deliver every month, 10% off\"?",
            "error[missing_selling_plan]: the product \"chair\" can only be bought with a selling plan",
        ] {
            assert!(
                report.contains(expected),
                "missing {expected:?} in:\n{report}"
            );
        }
    }

    #[test]
    fn a_field_the_format_does_not_have_leaves_the_rest_of_the_file_in() {
        let directory = std::env::temp_dir().join(format!("lsf-unknown-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            directory.join("store.json"),
            r#"{
                "shop": {"name": "Kept", "nmae": "typo", "customer_accounts": "legacy"},
                "products": [{"title": "Thing", "price": 100}],
                "customers": [{"email": "old@example.com", "password": "secret"}]
            }"#,
        )
        .expect("store");
        let (store, diagnostics) = load(
            &DataSource::Directory(directory.clone()),
            &LoadOptions::default(),
        );
        std::fs::remove_dir_all(&directory).ok();
        // One error for the typo, not one for everything the file held.
        assert_eq!(diagnostics.error_count(), 1, "{diagnostics}");
        assert!(diagnostics.to_string().contains("unknown field \"nmae\""));
        assert_eq!(store.shop.name, "Kept");
        assert_eq!(store.products.len(), 1);
        // What classic customer accounts had is ignored, with a warning.
        assert_eq!(store.customers.len(), 1);
        let classic: Vec<_> = diagnostics
            .items
            .iter()
            .filter(|item| item.code == "classic_customer_accounts")
            .collect();
        assert_eq!(classic.len(), 2, "{diagnostics}");
        assert!(
            classic
                .iter()
                .all(|item| item.severity == crate::diagnostics::Severity::Warning)
        );
    }

    #[test]
    fn locations_and_selling_plan_groups_have_unique_names() {
        let directory = std::env::temp_dir().join(format!("lsf-duplicates-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            directory.join("store.json"),
            r#"{
                "products": [{"title": "Thing", "price": 100}],
                "locations": [{"name": "Depot"}, {"name": "depot"}],
                "selling_plan_groups": [
                    {"name": "Monthly", "selling_plans": [{"name": "Every month"}]},
                    {"name": "monthly", "selling_plans": [{"name": "Each month"}]}
                ]
            }"#,
        )
        .expect("store");
        let (_, diagnostics) = load(
            &DataSource::Directory(directory.clone()),
            &LoadOptions::default(),
        );
        std::fs::remove_dir_all(&directory).ok();
        let report = diagnostics.to_string();
        for expected in [
            "error[duplicate_location]: another location is already named \"depot\"",
            "error[duplicate_selling_plan]: another selling plan group is already named \"monthly\"",
        ] {
            assert!(
                report.contains(expected),
                "missing {expected:?} in:\n{report}"
            );
        }
    }

    #[test]
    fn overlays_replace_and_add() {
        let overlay = serde_json::json!({
            "shop": {"name": "Overlaid"},
            "products": [
                {"title": "Canvas Tote Bag", "price": 100, "available": false},
                {"title": "Brand new", "price": 500}
            ]
        });
        let (store, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert_eq!(store.shop.name, "Overlaid");
        assert_eq!(store.shop.currency, "USD");
        assert_eq!(store.products.len(), 9);
        let tote = store.product_by_handle("canvas-tote-bag").expect("tote");
        assert_eq!(tote.variants[0].price, 100);
        assert!(!tote.available());
    }

    #[test]
    fn the_data_read_once_builds_the_store_again_without_reading_the_files() {
        let directory =
            std::env::temp_dir().join(format!("lsf-source-data-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("products")).expect("directory");
        std::fs::write(
            directory.join("products/thing.json"),
            r#"{"title": "Thing", "price": 100}"#,
        )
        .expect("product");
        let source = DataSource::Directory(directory.clone());
        let data = SourceData::read(&source);
        // A file added and one changed after the read are not in the stores built from it.
        std::fs::write(
            directory.join("products/other.json"),
            r#"{"title": "Other", "price": 200}"#,
        )
        .expect("product");
        std::fs::write(directory.join("products/thing.json"), "{ not json").expect("product");
        let overlay = Overlay::new(
            OVERLAY_FILE,
            serde_json::json!({"products": [{"title": "Added", "price": 300}]}),
        );
        let (store, diagnostics) = data.build(&LoadOptions::default(), &[overlay]);
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let handles: Vec<&str> = store.products.iter().map(|p| p.handle.as_str()).collect();
        assert_eq!(handles, ["thing", "added"]);
        let (again, _) = data.build(&LoadOptions::default(), &[]);
        assert_eq!(again.products.len(), 1);
        assert_eq!(again.products[0].id, store.products[0].id);
        // Loading reads the files as they are now.
        let (now, diagnostics) = load(&source, &LoadOptions::default());
        std::fs::remove_dir_all(&directory).ok();
        assert!(diagnostics.has_errors());
        assert_eq!(now.products.len(), 1);
        assert_eq!(now.products[0].handle, "other");
    }

    #[test]
    fn overlays_remove_before_they_add() {
        let (before, _) = load(&DataSource::Demo, &LoadOptions::default());
        let mut removed = Removals {
            products: vec!["scented-candle".to_string()],
            collections: vec!["empty".to_string()],
            // The main menu links to the contact page, the gift card belongs to Jane: what
            // names a removed entity goes with it.
            pages: vec!["contact".to_string()],
            menus: vec!["main-menu".to_string()],
            customers: vec!["JANE.DOE@example.com".to_string()],
            gift_cards: vec!["wcgx7x97g74jgdgc".to_string(), "missing-code".to_string()],
            shop_metafields: vec!["custom.announcement".to_string()],
            ..Removals::default()
        };
        removed
            .metaobjects
            .insert("designer".to_string(), vec!["nobody".to_string()]);
        let overlay = Overlay {
            name: "(admin api)".to_string(),
            removed,
            // Removed first, then added again under the same handle: a replacement.
            data: serde_json::json!({"products": [
                {"title": "Wool beanie", "handle": "wool-beanie", "price": 100}
            ]}),
        };
        let (store, diagnostics) =
            load_with_overlays(&DataSource::Demo, &LoadOptions::default(), &[overlay]);
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert!(store.product_by_handle("scented-candle").is_none());
        assert_eq!(store.products.len(), before.products.len() - 1);
        assert!(store.collection_by_handle("empty").is_none());
        assert!(store.page_index("contact").is_none());
        // Every store has a main menu: the default one comes back.
        let main_menu = store.menu("main-menu").expect("main menu");
        assert_eq!(main_menu.title, "Main menu");
        assert_eq!(main_menu.links.len(), 2);
        assert!(store.menu("footer").is_some());
        assert!(store.customer_by_email("jane.doe@example.com").is_none());
        assert_eq!(store.gift_cards.len(), 1);
        assert!(store.shop.metafields.is_empty());
        assert_eq!(
            store
                .product_by_handle("wool-beanie")
                .expect("beanie")
                .variants[0]
                .price,
            100
        );
        // What stays keeps its id, and the products without a date the date they had.
        for product in &store.products {
            let old = before.product_by_handle(&product.handle).expect("kept");
            assert_eq!(product.id, old.id);
            if product.handle != "wool-beanie" {
                assert_eq!(product.created_at, old.created_at, "{}", product.handle);
            }
        }
        let tote = store.product_by_handle("canvas-tote-bag").expect("tote");
        assert_eq!(
            tote.collections
                .iter()
                .map(|index| store.collections[*index].handle.as_str())
                .collect::<Vec<_>>(),
            ["frontpage", "accessories"]
        );
    }

    #[test]
    fn overlays_apply_one_after_the_other_under_their_own_names() {
        let first = Overlay::new(
            "(admin api)",
            serde_json::json!({"products": [{"title": "Brand new", "price": 500}]}),
        );
        let mut second = Overlay::new(
            OVERLAY_FILE,
            serde_json::json!({"products": [{"title": "Brand new", "price": 700, "prise": 1}]}),
        );
        second.removed.products.push("canvas-tote-bag".to_string());
        let (store, diagnostics) =
            load_with_overlays(&DataSource::Demo, &LoadOptions::default(), &[first, second]);
        assert_eq!(
            store
                .product_by_handle("brand-new")
                .expect("added")
                .variants[0]
                .price,
            700
        );
        // The second overlay's mistake is its own; removing the tote leaves the home page
        // collection naming a product that is gone.
        let files: Vec<(&str, &str)> = diagnostics
            .items
            .iter()
            .map(|item| (item.file.as_str(), item.code))
            .collect();
        assert!(
            files.contains(&(OVERLAY_FILE, "unknown_field")),
            "{files:?}"
        );
        assert!(
            files.contains(&("collections.json", "unknown_product")),
            "{files:?}"
        );
        assert!(
            !files.iter().any(|(file, _)| *file == "(admin api)"),
            "{files:?}"
        );
    }

    #[test]
    fn an_entity_with_its_id_replaces_the_one_with_that_id() {
        let (before, _) = load(&DataSource::Demo, &LoadOptions::default());
        let candle = before.product_by_handle("scented-candle").expect("candle");
        let position = before
            .products
            .iter()
            .position(|product| product.id == candle.id);
        let overlay = Overlay::new(
            "(admin api)",
            serde_json::json!({"products": [
                {"id": candle.id, "title": "Fig candle", "handle": "fig-candle", "price": 1800}
            ]}),
        );
        let (store, diagnostics) =
            load_with_overlays(&DataSource::Demo, &LoadOptions::default(), &[overlay]);
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert!(store.product_by_handle("scented-candle").is_none());
        let renamed = store.product_by_handle("fig-candle").expect("renamed");
        assert_eq!(renamed.id, candle.id);
        assert_eq!(
            store
                .products
                .iter()
                .position(|product| product.id == candle.id),
            position
        );
        assert_eq!(store.products.len(), before.products.len());
    }

    #[test]
    fn an_overlay_without_data_only_removes() {
        let mut overlay = Overlay::new("(admin api)", Json::Null);
        overlay
            .removed
            .companies
            .push("northwind hotels".to_string());
        overlay
            .removed
            .customers
            .push("alex.morgan@example.com".to_string());
        let (store, diagnostics) =
            load_with_overlays(&DataSource::Demo, &LoadOptions::default(), &[overlay]);
        assert!(diagnostics.is_empty(), "{diagnostics}");
        assert!(store.companies.is_empty());
        assert!(store.customer_by_email("alex.morgan@example.com").is_none());
    }

    #[test]
    fn only_active_published_products_are_on_the_storefront() {
        let (store, diagnostics) = inline(serde_json::json!({
            "products": [
                {"title": "Draft dress", "price": 100, "status": "draft",
                 "variants": [{"sku": "DRAFT-1"}], "collections": ["frontpage"]},
                {"title": "Old hat", "price": 100, "status": "archived"},
                {"title": "Hidden mug", "price": 100, "published": false},
                {"title": "Live candle", "price": 100, "status": "active", "published": true}
            ],
            "collections": [{"title": "Picks", "products": ["draft-dress", "live-candle"]}],
            "menus": {"picks": {"links": [{"title": "Dress", "product": "draft-dress"}]}},
            "customers": [{"email": "ann@example.com", "orders": [
                {"line_items": [{"variant": "DRAFT-1"}, {"variant": "live-candle"}],
                 "returns": [{"line_items": [{"variant": "draft-dress"}]}]}
            ]}]
        }));
        // What names a product that is not on the storefront leaves it out, without an error.
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let unlisted: Vec<(&str, model::ProductStatus, bool)> = store
            .unlisted_products
            .iter()
            .map(|product| (product.handle.as_str(), product.status, product.published))
            .collect();
        assert_eq!(
            unlisted,
            [
                ("draft-dress", model::ProductStatus::Draft, true),
                ("old-hat", model::ProductStatus::Archived, true),
                ("hidden-mug", model::ProductStatus::Active, false)
            ]
        );
        assert!(store.product_by_handle("draft-dress").is_none());
        assert!(store.product_by_handle("live-candle").is_some());
        let picks = store.collection_by_handle("picks").expect("picks");
        assert_eq!(picks.products.len(), 1);
        assert!(
            !store
                .collection_by_handle("frontpage")
                .expect("frontpage")
                .products
                .iter()
                .any(|index| store.products[*index].handle == "draft-dress")
        );
        let link = &store.menu("picks").expect("menu").links[0];
        assert_eq!(
            (link.url.as_str(), &link.target),
            ("/products/draft-dress", &crate::store::LinkTarget::Http)
        );
        let order = &store
            .customer_by_email("ann@example.com")
            .expect("ann")
            .orders[0];
        assert_eq!(order.line_items.len(), 1);
        assert!(order.returns[0].lines.is_empty());
        // The ids of the products on the storefront are those they would have anyway.
        assert_eq!(
            store.product_by_handle("live-candle").expect("candle").id,
            crate::util::stable_id("product", "live-candle")
        );
    }

    #[test]
    fn discounts_have_ids() {
        let (store, diagnostics) = inline(serde_json::json!({"checkout": {"discounts": [
            {"function": "volume-discount"},
            {"function": "volume-discount", "code": "SAVE", "id": 7},
            {"function": "volume-discount"}
        ]}}));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let ids: Vec<u64> = store.checkout.discounts.iter().map(|d| d.id).collect();
        assert_eq!(ids, [1, 7, 3]);
        let (_, diagnostics) = inline(serde_json::json!({"checkout": {"discounts": [
            {"function": "volume-discount"},
            {"function": "volume-discount", "id": 1}
        ]}}));
        assert_eq!(diagnostics.items[0].code, "duplicate_id");
        assert_eq!(diagnostics.items[0].path, "/checkout/discounts/1/id");
    }

    #[test]
    fn the_checkout_section_has_defaults() {
        let (store, diagnostics) = load(&DataSource::Demo, &LoadOptions::default());
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let rates = &store.checkout.shipping_rates;
        assert_eq!(
            (rates.len(), rates[0].title.as_str(), rates[0].price),
            (1, "Standard", 500)
        );
        let methods: Vec<&str> = store
            .checkout
            .payment_methods
            .iter()
            .map(|method| method.name.as_str())
            .collect();
        assert_eq!(methods, ["Credit card", "Cash on Delivery", "Bank Deposit"]);
        assert_eq!(store.checkout.tax_rate_bps, 0);
    }

    #[test]
    fn the_checkout_of_a_session_keeps_what_it_does_not_give() {
        let directory =
            std::env::temp_dir().join(format!("lsf-checkout-overlay-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(
            directory.join("store.json"),
            r#"{
                "products": [{"title": "Thing", "price": 100}],
                "checkout": {
                    "shipping_rates": [{"handle": "express", "price": "12.90"}],
                    "functions": {
                        "delivery-rules": {"metafields": {"$app:config.mode": "strict"}},
                        "payment-rules": {"metafields": {"$app:config.mode": "off"}}
                    }
                }
            }"#,
        )
        .expect("store");
        let overlay = serde_json::json!({"checkout": {
            "discounts": [{"function": "volume-discount", "code": "SAVE"}],
            "functions": {"payment-rules": {"metafields": {"$app:config.mode": "on"}}}
        }});
        let (store, diagnostics) = load_with_overlay(
            &DataSource::Directory(directory.clone()),
            &LoadOptions::default(),
            Some(&overlay),
        );
        std::fs::remove_dir_all(&directory).ok();
        assert!(!diagnostics.has_errors(), "{diagnostics}");
        let checkout = &store.checkout;
        // What the session gives replaces the store data's; the rest stays.
        assert_eq!(checkout.discounts[0].code.as_deref(), Some("SAVE"));
        assert_eq!(checkout.shipping_rates[0].title, "express");
        // The settings of the Functions, Function by Function.
        assert_eq!(checkout.functions["delivery-rules"][0].value, "strict");
        assert_eq!(checkout.functions["payment-rules"][0].value, "on");
    }

    /// The metafields of an owner by `"namespace.key"`, or as Shopify's Admin API takes them:
    /// a list of `{namespace, key, type, value}`, a `json` value as its text or as JSON.
    #[test]
    fn owner_metafields_are_read_by_name_or_as_shopify_s_list() {
        let overlay = serde_json::json!({"checkout": {
            "discounts": [{"function": "volume-discount", "metafields": [
                {"namespace": "$app", "key": "function-configuration", "type": "json", "value": "{\"percentage\":10}"}
            ]}],
            "functions": {
                "payment-rules": {"metafields": [
                    {"namespace": "$app", "key": "function-configuration", "type": "json", "value": {"hide": ["cod"]}},
                    {"namespace": "custom", "key": "mode", "value": "on"}
                ]},
                "delivery-rules": {"metafields": {"$app.function-configuration": {"type": "json", "value": {"hide": []}}}}
            }
        }});
        let (store, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(!diagnostics.has_errors(), "{diagnostics}");
        let field = |metafield: &crate::store::CheckoutMetafield| {
            (
                metafield.namespace.clone(),
                metafield.key.clone(),
                metafield.kind.clone(),
                metafield.value.clone(),
            )
        };
        let configuration = |value: &str| {
            (
                "$app".to_string(),
                "function-configuration".to_string(),
                "json".to_string(),
                value.to_string(),
            )
        };
        assert_eq!(
            field(&store.checkout.discounts[0].metafields[0]),
            configuration(r#"{"percentage":10}"#)
        );
        let payment = &store.checkout.functions["payment-rules"];
        assert_eq!(field(&payment[0]), configuration(r#"{"hide":["cod"]}"#));
        assert_eq!(
            (
                payment[1].namespace.as_str(),
                payment[1].key.as_str(),
                payment[1].value.as_str()
            ),
            ("custom", "mode", "on")
        );
        assert_eq!(
            field(&store.checkout.functions["delivery-rules"][0]),
            configuration(r#"{"hide":[]}"#)
        );
    }

    #[test]
    fn the_checkout_section_is_resolved() {
        let overlay = serde_json::json!({"checkout": {
            "shipping_rates": [{"handle": "express", "price": "12.90", "kind": "pick_up"}],
            "payment_methods": [{"handle": "card", "name": "Card"}],
            "discounts": [{
                "function": "volume-discount",
                "code": "SAVE",
                "classes": ["PRODUCT"],
                "metafields": {"$app:config.settings": {"type": "json", "value": {"percent": 10}}}
            }],
            "functions": {"delivery-rules": {"metafields": {"$app:config.mode": "strict"}}},
            "tax_rate": 5.5
        }});
        let (store, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let checkout = &store.checkout;
        assert_eq!(checkout.shipping_rates[0].title, "express");
        assert_eq!(checkout.shipping_rates[0].price, 1290);
        assert_eq!(
            checkout.shipping_rates[0].kind,
            model::CheckoutDeliveryKind::PickUp
        );
        assert_eq!(checkout.payment_methods.len(), 1);
        let discount = &checkout.discounts[0];
        assert_eq!(discount.title, "volume-discount");
        assert_eq!(discount.code.as_deref(), Some("SAVE"));
        assert_eq!(discount.classes, [model::CheckoutDiscountClass::Product]);
        let settings = &discount.metafields[0];
        assert_eq!(
            (
                settings.namespace.as_str(),
                settings.key.as_str(),
                settings.kind.as_str()
            ),
            ("$app:config", "settings", "json")
        );
        assert_eq!(settings.value, r#"{"percent":10}"#);
        assert_eq!(checkout.functions["delivery-rules"][0].value, "strict");
        assert_eq!(checkout.tax_rate_bps, 550);
    }

    #[test]
    fn a_bad_checkout_section_is_reported() {
        let overlay = serde_json::json!({"checkout": {
            "shipping_rates": [
                {"handle": "a", "price": "-1.00"},
                {"handle": "a", "price": 100}
            ],
            "discounts": [{"function": "f", "metafields": {"nodot": "x"}}],
            "tax_rate": 120
        }});
        let (_, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        let report = diagnostics.to_string();
        for expected in [
            "error[invalid_money]: a shipping rate cannot cost less than nothing",
            "error[duplicate_handle]: there are two shipping rates called \"a\"",
            "error[invalid_value]: \"nodot\" is not a metafield name",
            "error[invalid_value]: 120 is not a tax rate",
        ] {
            assert!(
                report.contains(expected),
                "missing {expected:?} in:\n{report}"
            );
        }
    }

    #[test]
    fn company_details_have_defaults_and_can_be_set() {
        let overlay = serde_json::json!({
            "companies": [{
                "name": "Dated Co",
                "created_at": "2025-03-01T08:30:00Z",
                "locations": [
                    {"name": "Head office", "locale": "fr", "orders_count": 12, "total_spent": "1234.50",
                     "updated_at": "2025-04-01", "created_at": "2025-03-02"},
                    {"name": "Depot"}
                ]
            }],
            "customers": [{
                "email": "buyer@dated.example", "company": "Dated Co",
                "company_contact": {"title": "Purchasing manager", "locale": "de", "updated_at": "2025-05-01"}
            }, {
                "email": "plain@dated.example", "company": "Dated Co"
            }]
        });
        let (store, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let company = store
            .companies
            .iter()
            .find(|c| c.name == "Dated Co")
            .unwrap();
        let date = |text: &str| text.parse::<chrono::DateTime<chrono::Utc>>().unwrap();
        assert_eq!(company.created_at, date("2025-03-01T08:30:00Z"));
        // Without `updated_at`: the creation date.
        assert_eq!(company.updated_at, company.created_at);
        let head = &company.locations[0];
        assert_eq!(head.locale.as_deref(), Some("fr"));
        assert_eq!((head.orders_count, head.total_spent), (12, 123_450));
        assert_eq!(head.created_at.date_naive().to_string(), "2025-03-02");
        assert_eq!(head.updated_at.date_naive().to_string(), "2025-04-01");
        // A location with nothing set: the company's date, no language, nothing ordered.
        let depot = &company.locations[1];
        assert_eq!(depot.created_at, company.created_at);
        assert_eq!(depot.updated_at, company.created_at);
        assert_eq!(
            (depot.locale.clone(), depot.orders_count, depot.total_spent),
            (None, 0, 0)
        );

        let buyer = store.customer_by_email("buyer@dated.example").unwrap();
        let contact = buyer.company_contact.as_ref().unwrap();
        assert_eq!(contact.title.as_deref(), Some("Purchasing manager"));
        assert_eq!(contact.locale.as_deref(), Some("de"));
        assert_eq!(contact.created_at, company.created_at);
        assert_eq!(contact.updated_at.date_naive().to_string(), "2025-05-01");
        assert!(
            store
                .customer_by_email("plain@dated.example")
                .unwrap()
                .company_contact
                .is_none()
        );

        // The companies of the demo get fixed, distinct creation dates.
        let demo = &store.companies[0];
        assert_eq!(demo.created_at, date("2024-01-01T12:00:00Z"));
    }

    #[test]
    fn bad_company_details_are_reported() {
        let overlay = serde_json::json!({
            "companies": [{"name": "Bad Co", "created_at": "soon",
                "locations": [{"name": "A", "updated_at": "never"}]}],
            "customers": [{"email": "orphan@bad.example", "company_contact": {"title": "x"}}]
        });
        let (_, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        let report = diagnostics.to_string();
        for expected in [
            "error[invalid_date]: \"soon\" is not a date",
            "error[invalid_date]: \"never\" is not a date",
            "error[missing_company]: the customer has a company contact but names no company",
        ] {
            assert!(
                report.contains(expected),
                "missing {expected:?} in:\n{report}"
            );
        }
    }

    #[test]
    fn markets_group_countries() {
        let overlay = serde_json::json!({"localization": {
            "countries": [{"iso_code": "FR"}, {"iso_code": "de"}, {"iso_code": "CA"}],
            "markets": [{"handle": "europe", "countries": ["fr", "DE"],
                "metafields": {"custom": {"tier": "gold"}}}]
        }});
        let (store, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let market = store.market_of("de").expect("a market");
        assert_eq!(market.handle, "europe");
        assert_eq!(market.countries, ["FR", "DE"]);
        assert!(market.metafields.contains_key("custom"));
        assert!(store.market_of("CA").is_none());
        // Without markets, no country is in one.
        let (store, _) = load(&DataSource::Demo, &LoadOptions::default());
        assert!(store.markets.is_empty() && store.market_of("US").is_none());
    }

    #[test]
    fn bad_markets_are_reported() {
        let overlay = serde_json::json!({"localization": {
            "countries": [{"iso_code": "FR"}, {"iso_code": "DE"}],
            "markets": [
                {"handle": "a", "countries": ["FR", "XX"]},
                {"handle": "a", "countries": ["FR"]}
            ]
        }});
        let (_, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        let report = diagnostics.to_string();
        for expected in [
            "error[unknown_country]: \"XX\" is not one of the countries in \"localization.countries\"",
            "error[duplicate_handle]: there are two markets called \"a\"",
            "error[duplicate_market_country]: FR is already in the market \"a\"",
        ] {
            assert!(
                report.contains(expected),
                "missing {expected:?} in:\n{report}"
            );
        }
    }

    #[test]
    fn payment_methods_are_offered_in_the_list_by_default() {
        let overlay = serde_json::json!({"checkout": {"payment_methods": [
            {"handle": "card"},
            {"handle": "wallet", "placements": ["ACCELERATED_CHECKOUT", "PAYMENT_METHOD"]}
        ]}});
        let (store, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let methods = &store.checkout.payment_methods;
        assert_eq!(
            methods[0].placements,
            [model::CheckoutPaymentPlacement::PaymentMethod]
        );
        assert_eq!(
            methods[1].placements,
            [
                model::CheckoutPaymentPlacement::AcceleratedCheckout,
                model::CheckoutPaymentPlacement::PaymentMethod
            ]
        );
        for method in &store.checkout.payment_methods {
            assert!(!method.placements.is_empty());
        }
        let bad = serde_json::json!({"checkout": {"payment_methods": [
            {"handle": "card", "placements": ["EVERYWHERE"]}]}});
        let (_, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&bad));
        assert!(
            diagnostics.to_string().contains("EVERYWHERE"),
            "{diagnostics}"
        );
    }

    /// The demo store on disk, with `store_extra` as another data file and `scenarios` in
    /// `checkouts/` (name of the file, content).
    fn with_scenarios(
        label: &str,
        store_extra: Option<&str>,
        scenarios: &[(&str, &str)],
    ) -> (crate::store::Store, Diagnostics) {
        let directory =
            std::env::temp_dir().join(format!("lsf-scenarios-{label}-{}", std::process::id()));
        std::fs::remove_dir_all(&directory).ok();
        for (name, content) in demo::FILES {
            let path = directory.join(name);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
            std::fs::write(path, content).expect("demo file");
        }
        if let Some(content) = store_extra {
            std::fs::write(directory.join("checkout.json"), content).expect("extra");
        }
        std::fs::create_dir_all(directory.join("checkouts")).expect("checkouts");
        for (name, content) in scenarios {
            std::fs::write(directory.join("checkouts").join(name), content).expect("scenario");
        }
        let loaded = load(
            &DataSource::Directory(directory.clone()),
            &LoadOptions::default(),
        );
        std::fs::remove_dir_all(&directory).ok();
        loaded
    }

    #[test]
    fn a_scenario_is_read_on_top_of_the_store_data() {
        let (store, diagnostics) = with_scenarios(
            "resolved",
            Some(
                r#"{"checkout": {
                    "shipping_rates": [{"handle": "standard", "price": "4.90"}],
                    "discounts": [{"function": "volume-discount", "code": "SAVE"}],
                    "functions": {
                        "delivery-rules": {"metafields": {"$app:config.mode": "strict"}},
                        "payment-rules": {"metafields": {"$app:config.mode": "loose"}}
                    },
                    "tax_rate": 20
                }}"#,
            ),
            &[
                (
                    "b2b.json",
                    r#"{
                        "title": "A hotel orders",
                        "description": "Buys for its Seattle location.",
                        "session": {
                            "customer": "alex.morgan@example.com",
                            "company_location": "Northwind Seattle",
                            "country": "US",
                            "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 4}], "note": "Dock B"}
                        },
                        "entered": {
                            "shipping_address": {"last_name": "Morgan", "country_code": "us"},
                            "discount_codes": ["SAVE"],
                            "payment": "cod"
                        },
                        "language": "FR",
                        "variant": "shop",
                        "extensions": ["gift-message"],
                        "checkout": {
                            "payment_methods": [{"handle": "cod"}],
                            "discounts": [],
                            "functions": {"delivery-rules": {"metafields": {"$app:config.mode": "off"}}}
                        }
                    }"#,
                ),
                ("plain.json", "{}"),
                ("notes.txt", "not a scenario"),
            ],
        );
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let names: Vec<&str> = store.scenarios.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["b2b", "plain"]);

        let b2b = &store.scenarios[0];
        assert_eq!(b2b.file, "checkouts/b2b.json");
        assert_eq!(b2b.title, "A hotel orders");
        assert!(b2b.problems.is_empty(), "{:?}", b2b.problems);
        assert_eq!(b2b.language.as_deref(), Some("fr"));
        assert_eq!(b2b.look, Some(model::ScenarioLook::Shop));
        assert_eq!(
            b2b.extensions.as_deref(),
            Some(&["gift-message".to_string()][..])
        );
        assert_eq!(b2b.entered.discount_codes, ["SAVE"]);
        assert_eq!(b2b.overrides, ["payment_methods", "discounts", "functions"]);
        // What the scenario sets replaces the store's, the rest is the store's.
        assert_eq!(b2b.checkout.payment_methods.len(), 1);
        assert!(b2b.checkout.discounts.is_empty());
        assert_eq!(b2b.checkout.shipping_rates[0].price, 490);
        assert_eq!(b2b.checkout.tax_rate_bps, 2000);
        assert_eq!(b2b.checkout.functions["delivery-rules"][0].value, "off");
        assert_eq!(b2b.checkout.functions["payment-rules"][0].value, "loose");
        // The store itself is untouched.
        assert_eq!(store.checkout.payment_methods.len(), 3);
        assert_eq!(store.checkout.discounts.len(), 1);

        // The session is the one a checkout opens with.
        let mut session = crate::Session::initial(&store);
        let mut problems = Diagnostics::new();
        crate::store::build::apply_session_input(
            &store,
            &b2b.session,
            &mut session,
            &mut problems,
            &b2b.file,
            "/session",
        );
        assert!(problems.is_empty(), "{problems}");
        assert_eq!(session.cart_lines[0].quantity, 4);
        assert_eq!(session.cart_note, "Dock B");
        assert!(session.customer_id.is_some() && session.company_location.is_some());

        let plain = &store.scenarios[1];
        assert_eq!(plain.title, "plain");
        assert!(plain.overrides.is_empty());
        assert_eq!(plain.checkout.tax_rate_bps, 2000);
    }

    #[test]
    fn a_scenario_with_mistakes_is_kept_with_what_is_wrong() {
        let (store, diagnostics) = with_scenarios(
            "mistakes",
            None,
            &[
                ("typo.json", r#"{"titel": "x"}"#),
                (
                    "unknown.json",
                    r#"{
                        "session": {
                            "customer": "nobody@example.com",
                            "country": "ZZ",
                            "cart": {"items": [{"variant": "TEE-BLK-Q"}]}
                        },
                        "entered": {"shipping_address": {"country_code": "ZZ"}},
                        "language": "de",
                        "checkout": {"tax_rate": 120}
                    }"#,
                ),
                ("broken.json", "{ nope"),
                (
                    "fine.json",
                    r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M"}]}}}"#,
                ),
            ],
        );
        let by_name = |name: &str| store.scenarios.iter().find(|s| s.name == name).expect(name);
        assert!(by_name("fine").problems.is_empty());

        let typo = by_name("typo");
        assert_eq!(typo.problems.len(), 1);
        assert!(
            typo.problems[0].contains("unknown_field"),
            "{:?}",
            typo.problems
        );
        assert!(typo.problems[0].contains("did you mean \"title\"?"));

        let broken = by_name("broken");
        assert!(
            broken.problems[0].contains("invalid_json"),
            "{:?}",
            broken.problems
        );

        let unknown = by_name("unknown").problems.join("\n");
        for expected in [
            "unknown_customer",
            "unknown_country",
            "unknown_variant",
            "unknown_language",
            "invalid_value",
            "checkouts/unknown.json at /session/customer",
            "checkouts/unknown.json at /session/cart/items/0/variant",
            "checkouts/unknown.json at /entered/shipping_address/country_code",
            "checkouts/unknown.json at /language",
            "checkouts/unknown.json at /checkout/tax_rate",
        ] {
            assert!(
                unknown.contains(expected),
                "missing {expected:?} in:\n{unknown}"
            );
        }
        // `validate` sees all of it.
        for file in ["typo", "unknown", "broken"] {
            assert!(
                diagnostics
                    .items
                    .iter()
                    .any(|item| item.file == format!("checkouts/{file}.json")),
                "{file}: {diagnostics}"
            );
        }
        assert!(
            !diagnostics
                .items
                .iter()
                .any(|item| item.file == "checkouts/fine.json")
        );
    }

    #[test]
    fn what_a_scenario_has_the_buyer_choose_must_be_offered() {
        let (store, _) = with_scenarios(
            "choices",
            None,
            &[(
                "choices.json",
                r#"{
                    "entered": {"delivery": "expres", "payment": "cod"},
                    "checkout": {
                        "shipping_rates": [{"handle": "standard", "price": 1}, {"handle": "express", "price": 2}],
                        "payment_methods": [{"handle": "card"}]
                    }
                }"#,
            )],
        );
        let problems = store.scenarios[0].problems.join("\n");
        assert!(
            problems.contains("there is no shipping rate \"expres\"")
                && problems.contains("did you mean \"express\"?"),
            "{problems}"
        );
        assert!(
            problems.contains("there is no payment method \"cod\"")
                && problems.contains("the checkout offers: card"),
            "{problems}"
        );
        assert!(problems.contains("checkouts/choices.json at /entered/payment"));
    }

    #[test]
    fn a_scenario_reads_what_its_checkout_must_look_like() {
        let (store, diagnostics) = with_scenarios(
            "expect",
            None,
            &[
                (
                    "expect.json",
                    r#"{"checkout": {"payment_methods": [{"handle": "card", "name": "Credit card"}, {"handle": "paypal", "name": "PayPal", "placements": ["PAYMENT_METHOD", "ACCELERATED_CHECKOUT"]}, {"handle": "cod", "name": "Cash on Delivery"}]},
                    "expect": {
                        "step": "interaction",
                        "express_methods": {"excludes": ["paypal", {"handle": "cod"}]},
                        "payment_methods": {"includes": ["Credit card", {"handle": "card", "name": "Card"}], "equals": []},
                        "delivery_options": {"equals": [{"handle": "standard", "cost": "5.00"}]},
                        "discounts": {"includes": [{"title": "10% off", "class": "ORDER", "amount": 210}]},
                        "lines": {"equals": [{"sku": "TEE-BLK-M", "quantity": 2, "components": [], "attributes": {"gift": "yes"}}]},
                        "errors": {"equals": []},
                        "totals": {"total": "120.00", "due_later": 0}
                    }}"#,
                ),
                // An amount that is not one, and a field that is not a field.
                (
                    "wrong.json",
                    r#"{"expect": {"totals": {"total": 12.5}, "lines": {"includes": [{"skus": "TEE"}]}, "errors": {"contains": []}}}"#,
                ),
                // What `excludes` names must be offered: otherwise it checks nothing.
                (
                    "typo.json",
                    r#"{"checkout": {"payment_methods": [{"handle": "card"}, {"handle": "paypal", "name": "PayPal"}, {"handle": "cod", "name": "Cash on Delivery"}]},
                    "expect": {
                        "express_methods": {"excludes": ["paypl", "Cash on Delivery"]},
                        "delivery_options": {"excludes": [{"handle": "expres"}], "includes": ["Renamed by a Function"]}
                    }}"#,
                ),
            ],
        );
        let by_name = |name: &str| store.scenarios.iter().find(|s| s.name == name).expect(name);

        let expect = by_name("expect");
        assert!(expect.problems.is_empty(), "{:?}", expect.problems);
        let read = expect.expect.as_ref().expect("read");
        assert_eq!(read.step, Some(model::ScenarioStep::Interaction));
        let excludes = &read.express_methods.as_ref().unwrap().excludes;
        assert!(matches!(&excludes[0], model::Expected::Name(name) if name == "paypal"));
        assert!(
            matches!(&excludes[1], model::Expected::Fields(method) if method.handle.as_deref() == Some("cod"))
        );
        assert_eq!(
            read.payment_methods
                .as_ref()
                .unwrap()
                .equals
                .as_deref()
                .map(<[_]>::len),
            Some(0)
        );
        assert_eq!(
            read.totals.as_ref().unwrap().total,
            Some(model::Money::Decimal("120.00".into()))
        );
        assert!(
            !diagnostics
                .items
                .iter()
                .any(|item| item.file == "checkouts/expect.json"),
            "{diagnostics}"
        );

        let wrong = by_name("wrong").problems.join("\n");
        for expected in [
            "checkouts/wrong.json at /expect/totals/total",
            "unknown field \"skus\"",
            "did you mean \"sku\"?",
            "unknown field \"contains\"",
        ] {
            assert!(
                wrong.contains(expected),
                "missing {expected:?} in:\n{wrong}"
            );
        }

        // A warning, which `validate` prints: the scenario still opens.
        let typo = by_name("typo");
        assert!(typo.problems.is_empty(), "{:?}", typo.problems);
        let warnings: Vec<String> = diagnostics
            .items
            .iter()
            .filter(|item| item.file == "checkouts/typo.json")
            .map(|item| {
                assert_eq!(
                    item.severity,
                    crate::diagnostics::Severity::Warning,
                    "{item}"
                );
                item.to_string()
            })
            .collect();
        assert_eq!(warnings.len(), 2, "{warnings:#?}");
        assert!(
            warnings[0].contains("the checkout offers no payment method \"paypl\"")
                && warnings[0].contains("/expect/express_methods/excludes/0")
                && warnings[0]
                    .contains("did you mean \"paypal\"? The checkout offers: card, paypal, cod"),
            "{}",
            warnings[0]
        );
        assert!(
            warnings[1].contains("no delivery option \"expres\"")
                && warnings[1].contains("/expect/delivery_options/excludes/0"),
            "{}",
            warnings[1]
        );
    }

    #[test]
    fn a_file_of_the_data_directory_is_store_data_a_scenario_or_unread() {
        for (path, role) in [
            ("store.json", FileRole::Store),
            ("menus.json", FileRole::Store),
            ("products/blue-shirt.json", FileRole::Store),
            ("customers/ada.json", FileRole::Store),
            ("menus/footer.json", FileRole::Store),
            ("files/logo.png", FileRole::Store),
            ("files/nested/hero.jpg", FileRole::Store),
            ("checkouts/b2b.json", FileRole::Scenario),
            ("checkouts/notes.txt", FileRole::Unread),
            ("checkouts/nested/deep.json", FileRole::Unread),
            ("schema/product.schema.json", FileRole::Unread),
            ("README.md", FileRole::Unread),
            ("notes.txt", FileRole::Unread),
            (".DS_Store", FileRole::Unread),
            ("products/.draft.json", FileRole::Unread),
            ("other/thing.json", FileRole::Unread),
        ] {
            assert_eq!(role_of(path), role, "{path}");
        }
    }

    #[test]
    fn a_store_without_a_checkouts_folder_has_no_scenarios() {
        let (store, _) = load(&DataSource::Demo, &LoadOptions::default());
        assert!(store.scenarios.is_empty());
    }

    #[test]
    fn overlay_errors_are_reported() {
        let overlay = serde_json::json!({"products": [{"titel": "x"}]});
        let (_, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(diagnostics.has_errors());
        assert_eq!(diagnostics.items[0].file, OVERLAY_FILE);
    }

    /// The upsert of one field as an overlay applied it before its maps: each entity looks for
    /// its place along the whole list. With how many entities replaced one by its id among
    /// those before the overlay, by its id among those it wrote, by its key, and were added.
    fn upsert_one_by_one<T>(
        list: &mut Vec<Sourced<T>>,
        incoming: Vec<T>,
        field: &str,
        kind: Option<&str>,
        key: fn(&T) -> String,
        id: fn(&T) -> Option<u64>,
    ) -> [usize; 4] {
        let mut written = HashSet::new();
        let mut counts = [0; 4];
        for (index, value) in incoming.into_iter().enumerate() {
            let wanted = key(&value);
            let by_id = kind.and_then(|kind| {
                let given = id(&value)?;
                list.iter().position(|existing| {
                    id(&existing.value)
                        .unwrap_or_else(|| crate::util::stable_id(kind, &key(&existing.value)))
                        == given
                })
            });
            let position = by_id.or_else(|| {
                list.iter().enumerate().position(|(at, existing)| {
                    !written.contains(&at) && key(&existing.value) == wanted
                })
            });
            counts[match (by_id, position) {
                (Some(at), _) if written.contains(&at) => 1,
                (Some(_), _) => 0,
                (None, Some(_)) => 2,
                (None, None) => 3,
            }] += 1;
            let sourced = Sourced {
                value,
                origin: Origin::new("(admin api)", format!("/{field}/{index}")),
            };
            match position {
                Some(at) => {
                    list[at] = sourced;
                    written.insert(at);
                }
                None => {
                    written.insert(list.len());
                    list.push(sourced);
                }
            }
        }
        counts
    }

    #[test]
    fn an_overlay_of_thousands_of_entities_lands_where_one_by_one_it_did() {
        // xorshift: the same data on every run.
        struct Random(u64);
        impl Random {
            fn below(&mut self, bound: u64) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0 % bound
            }
        }
        let random = &mut Random(0x2545_f491_4f6c_dd1d);
        // Products of one pool of handles, given or made from the title ("Item 7" makes
        // `item-7`), some with ids (some of one id), some without: their id is then the one
        // their handle derives.
        fn product(random: &mut Random, pool: u64) -> (Json, Option<u64>, u64) {
            let n = random.below(pool);
            let mut json = serde_json::json!({"title": format!("Item {n}"), "price": 100});
            if random.below(10) < 7 {
                json["handle"] = serde_json::json!(format!("item-{n}"));
            }
            let id = (random.below(10) < 4).then(|| 1_000 + random.below(1_500));
            if let Some(id) = id {
                json["id"] = serde_json::json!(id);
            }
            let derived = || crate::util::stable_id("product", &format!("item-{n}"));
            (json, id, id.unwrap_or_else(derived))
        }
        let base: Vec<(Json, Option<u64>, u64)> =
            (0..2_000).map(|_| product(random, 1_500)).collect();
        let mut written: Vec<u64> = Vec::new();
        let mut products = Vec::new();
        for _ in 0..2_000 {
            let (mut json, id, effective) = product(random, 1_800);
            // The id of an entity of the files, of one this overlay wrote, of none, or its own.
            let target = match random.below(10) {
                0..=2 => Some(base[random.below(base.len() as u64) as usize].2),
                3 if !written.is_empty() => {
                    Some(written[random.below(written.len() as u64) as usize])
                }
                4 => Some(5_000_000 + random.below(1_000)),
                _ => id,
            };
            if let Some(target) = target {
                json["id"] = serde_json::json!(target);
            }
            written.push(target.unwrap_or(effective));
            products.push(json);
        }
        fn email(n: u64, upper: bool) -> String {
            format!("{}{n}@example.com", if upper { "Person" } else { "person" })
        }
        // Emails in either case; ids given, derived from another's email, or none.
        fn customer(random: &mut Random, pool: u64) -> Json {
            let n = random.below(pool);
            let mut json = serde_json::json!({"email": email(n, random.below(2) == 0)});
            match random.below(10) {
                0..=3 => json["id"] = serde_json::json!(10_000 + random.below(700)),
                4..=6 => {
                    let other = email(random.below(700), false);
                    json["id"] = serde_json::json!(crate::util::stable_id("customer", &other));
                }
                _ => {}
            }
            json
        }
        let base_customers: Vec<Json> = (0..1_000).map(|_| customer(random, 700)).collect();
        let customers: Vec<Json> = (0..1_000).map(|_| customer(random, 900)).collect();
        fn card(random: &mut Random, pool: u64) -> Json {
            let n = random.below(pool);
            let prefix = if random.below(2) == 0 { "card" } else { "CARD" };
            serde_json::json!({"code": format!("{prefix}{n}"), "initial_value": 1000})
        }
        let base_cards: Vec<Json> = (0..500).map(|_| card(random, 400)).collect();
        let cards: Vec<Json> = (0..500).map(|_| card(random, 500)).collect();
        let removed = Removals {
            products: (0..100)
                .map(|_| format!("item-{}", random.below(1_800)))
                .collect(),
            customers: (0..50)
                .map(|_| email(random.below(900), random.below(2) == 0))
                .collect(),
            gift_cards: (0..30)
                .map(|_| format!("Card{}", random.below(500)))
                .collect(),
            ..Removals::default()
        };

        fn sourced<T: DeserializeOwned>(field: &str, entries: &[Json]) -> Vec<Sourced<T>> {
            entries
                .iter()
                .enumerate()
                .map(|(index, json)| Sourced {
                    value: serde_json::from_value(json.clone()).expect("valid"),
                    origin: Origin::new("store.json", format!("/{field}/{index}")),
                })
                .collect()
        }
        let files = MergedInput {
            products: sourced(
                "products",
                &base
                    .iter()
                    .map(|(json, ..)| json.clone())
                    .collect::<Vec<_>>(),
            ),
            customers: sourced("customers", &base_customers),
            gift_cards: sourced("gift_cards", &base_cards),
            ..MergedInput::default()
        };
        let overlay = serde_json::json!({
            "products": products, "customers": customers, "gift_cards": cards
        });

        let mut merged = files.clone();
        let mut diagnostics = Diagnostics::new();
        apply_removals(&mut merged, &removed);
        apply_overlay(&mut merged, "(admin api)", &overlay, &mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics}");

        // The same, one by one, with the keys as they were made.
        let mut expected = files;
        for (index, product) in expected.products.iter_mut().enumerate() {
            let date = undated(index).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            product.value.created_at = product.value.created_at.take().or(Some(date));
        }
        let handle = |product: &model::ProductInput| {
            product
                .handle
                .clone()
                .unwrap_or_else(|| crate::util::handleize(&product.title))
        };
        expected
            .products
            .retain(|product| !removed.products.contains(&handle(&product.value)));
        let any_case = |keys: &[String], key: &str| {
            keys.iter()
                .any(|candidate| candidate.to_lowercase() == key.to_lowercase())
        };
        expected
            .customers
            .retain(|customer| !any_case(&removed.customers, &customer.value.email));
        expected
            .gift_cards
            .retain(|card| !any_case(&removed.gift_cards, &card.value.code));
        let input: model::StoreInput = serde_json::from_value(overlay).expect("valid");
        let counts = upsert_one_by_one(
            &mut expected.products,
            input.products,
            "products",
            Some("product"),
            handle,
            |product| product.id,
        );
        // Each way an entity finds its place is taken, many times.
        assert!(counts.iter().all(|count| *count > 50), "{counts:?}");
        let counts = upsert_one_by_one(
            &mut expected.customers,
            input.customers,
            "customers",
            Some("customer"),
            |customer| customer.email.to_lowercase(),
            |customer| customer.id,
        );
        assert!(counts.iter().all(|count| *count > 10), "{counts:?}");
        let counts = upsert_one_by_one(
            &mut expected.gift_cards,
            input.gift_cards,
            "gift_cards",
            None,
            |card| card.code.to_uppercase(),
            |_| None,
        );
        assert!(counts[2] > 50 && counts[3] > 50, "{counts:?}");

        fn rows<T: Serialize>(list: &[Sourced<T>]) -> Vec<String> {
            list.iter()
                .map(|entry| {
                    let value = serde_json::to_string(&entry.value).expect("json");
                    format!("{} {} {value}", entry.origin.file, entry.origin.pointer)
                })
                .collect()
        }
        assert_eq!(rows(&merged.products), rows(&expected.products));
        assert_eq!(rows(&merged.customers), rows(&expected.customers));
        assert_eq!(rows(&merged.gift_cards), rows(&expected.gift_cards));
        // The removals and the overlay changed the lists.
        assert!(merged.products.len() > 2_000 && merged.customers.len() > 1_000);
    }

    #[test]
    fn a_product_recommends_the_products_of_its_collections_first_ten_at_most() {
        // Twelve more products than the demo store has, so that ten is fewer than all of them.
        let extra: Vec<Json> = (0..12)
            .map(|index| serde_json::json!({"title": format!("Extra {index}"), "price": 100}))
            .collect();
        let overlay = serde_json::json!({ "products": extra });
        let (store, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(!diagnostics.has_errors(), "{diagnostics}");
        assert_eq!(store.products.len(), 20);
        // The other products of its collections in their order, then every other product.
        let expected = |index: usize| {
            let mut related: Vec<usize> = Vec::new();
            let members = store.products[index]
                .collections
                .iter()
                .flat_map(|&collection| store.collections[collection].products.iter().copied());
            for product in members.chain(0..store.products.len()) {
                if product != index && !related.contains(&product) {
                    related.push(product);
                }
            }
            related.truncate(10);
            related
        };
        for (index, product) in store.products.iter().enumerate() {
            assert_eq!(
                product.recommendations,
                expected(index),
                "{}",
                product.handle
            );
        }
    }
}

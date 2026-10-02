//! Reads a data directory into a [`Store`].
//!
//! Layout of a data directory:
//!
//! ```text
//! shopify-local/
//!   *.json            any number of files, each holding any of: shop, products, collections,
//!                     pages, blogs, menus, customers, metaobjects, localization, files,
//!                     session, now, theme_settings. They are merged.
//!   products/*.json   one product per file (or an array); the file name is the default handle
//!   collections/*.json, pages/*.json, blogs/*.json, customers/*.json, menus/*.json
//!   files/            images and other files, served under /cdn/shop/files/
//!   schema/           JSON Schemas written by `lsf init` for editor support (ignored here)
//! ```

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde_json::Value as Json;

use super::build::{BuildOptions, MergedInput, Origin, Sourced, build};
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

/// Parses, validates and deserializes one entity. Reports problems and returns `None` when the
/// entity cannot be used.
fn entity<T: DeserializeOwned>(
    kind: FileKind,
    file: &str,
    json: &Json,
    pointer: &str,
    diagnostics: &mut Diagnostics,
) -> Option<T> {
    let errors_before = diagnostics.error_count();
    validate(kind, file, json, pointer, diagnostics);
    if diagnostics.error_count() > errors_before {
        return None;
    }
    match serde_json::from_value::<T>(json.clone()) {
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
                diagnostics
                    .error("invalid_json", name, "", format!("not valid JSON: {error}"))
                    .hint(format!(
                        "look at line {}, column {}",
                        error.line(),
                        error.column()
                    ));
                continue;
            }
        };
        match file.kind {
            FileKind::Store => {
                let Some(input) =
                    entity::<model::StoreInput>(FileKind::Store, name, &json, "", diagnostics)
                else {
                    continue;
                };
                merge_store_file(&mut merged, input, name, diagnostics);
            }
            FileKind::Product => {
                let single = json.is_object();
                for mut product in
                    entities::<model::ProductInput>(FileKind::Product, name, &json, diagnostics)
                {
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
            FileKind::Customer => merged.customers.extend(entities::<model::CustomerInput>(
                FileKind::Customer,
                name,
                &json,
                diagnostics,
            )),
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
        }
    }
    merged
}

fn merge_store_file(
    merged: &mut MergedInput,
    input: model::StoreInput,
    file: &str,
    diagnostics: &mut Diagnostics,
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
            origin: origin(format!("/products/{index}")),
        });
    }
    for (index, value) in input.collections.into_iter().enumerate() {
        merged.collections.push(Sourced {
            value,
            origin: origin(format!("/collections/{index}")),
        });
    }
    for (index, value) in input.pages.into_iter().enumerate() {
        merged.pages.push(Sourced {
            value,
            origin: origin(format!("/pages/{index}")),
        });
    }
    for (index, value) in input.blogs.into_iter().enumerate() {
        merged.blogs.push(Sourced {
            value,
            origin: origin(format!("/blogs/{index}")),
        });
    }
    for (index, value) in input.customers.into_iter().enumerate() {
        merged.customers.push(Sourced {
            value,
            origin: origin(format!("/customers/{index}")),
        });
    }
    for (index, value) in input.gift_cards.into_iter().enumerate() {
        merged.gift_cards.push(Sourced {
            value,
            origin: origin(format!("/gift_cards/{index}")),
        });
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
            let origin = origin(format!("/metaobjects/{kind}/{index}"));
            merged
                .metaobjects
                .push((kind.clone(), Sourced { value, origin }));
        }
    }
    merged.files.extend(input.files);
    merged.theme_settings.extend(input.theme_settings);
}

/// Options that depend on the theme being served.
#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    /// The locales the theme has translations for, default first.
    pub theme_locales: Vec<String>,
}

/// Loads and validates the store. The store is returned even when the data has errors (built
/// from whatever is valid), together with the diagnostics.
pub fn load(source: &DataSource, options: &LoadOptions) -> (Store, Diagnostics) {
    load_with_overlay(source, options, None)
}

/// The name diagnostics use for data that did not come from a file.
pub const OVERLAY_FILE: &str = "(request body)";

fn effective_handle(handle: &Option<String>, title: &str) -> String {
    handle
        .clone()
        .unwrap_or_else(|| crate::util::handleize(title))
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

/// Applies a data file on top of the merged data: entities with the handle of an existing one
/// replace it, the others are added, and `shop` is merged field by field.
fn apply_overlay(merged: &mut MergedInput, overlay: &Json, diagnostics: &mut Diagnostics) {
    let Some(input) =
        entity::<model::StoreInput>(FileKind::Store, OVERLAY_FILE, overlay, "", diagnostics)
    else {
        return;
    };
    let origin = |pointer: String| Origin::new(OVERLAY_FILE, pointer);
    if let Some(patch) = overlay.get("shop") {
        let mut shop = merged
            .shop
            .as_ref()
            .and_then(|shop| serde_json::to_value(&shop.value).ok())
            .unwrap_or(Json::Object(serde_json::Map::new()));
        merge_json(&mut shop, patch);
        if let Ok(value) = serde_json::from_value::<model::ShopInput>(shop) {
            merged.shop = Some(Sourced {
                value,
                origin: origin("/shop".to_string()),
            });
        }
    }
    macro_rules! upsert {
        ($field:ident, $key:expr) => {
            for (index, value) in input.$field.into_iter().enumerate() {
                let key = $key(&value);
                let sourced = Sourced {
                    value,
                    origin: origin(format!("/{}/{index}", stringify!($field))),
                };
                match merged
                    .$field
                    .iter()
                    .position(|existing| $key(&existing.value) == key)
                {
                    Some(position) => merged.$field[position] = sourced,
                    None => merged.$field.push(sourced),
                }
            }
        };
    }
    upsert!(products, |product: &model::ProductInput| effective_handle(
        &product.handle,
        &product.title
    ));
    upsert!(collections, |collection: &model::CollectionInput| {
        effective_handle(&collection.handle, &collection.title)
    });
    upsert!(pages, |page: &model::PageInput| effective_handle(
        &page.handle,
        &page.title
    ));
    upsert!(blogs, |blog: &model::BlogInput| effective_handle(
        &blog.handle,
        &blog.title
    ));
    upsert!(customers, |customer: &model::CustomerInput| customer
        .email
        .to_lowercase());
    upsert!(gift_cards, |card: &model::GiftCardInput| card
        .code
        .to_uppercase());
    for (handle, value) in input.menus {
        let origin = origin(format!("/menus/{handle}"));
        merged.menus.insert(handle, Sourced { value, origin });
    }
    for (kind, entries) in input.metaobjects {
        merged.metaobjects.retain(|(existing, entry)| {
            *existing != kind
                || !entries
                    .iter()
                    .any(|replacement| replacement.handle == entry.value.handle)
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
    if let Some(value) = input.now {
        merged.now = Some(Sourced {
            value,
            origin: origin("/now".to_string()),
        });
    }
    merged.files.extend(input.files);
    merged.theme_settings.extend(input.theme_settings);
}

/// Loads the store with extra data applied on top of the files, as tests do to set up the
/// exact situation they need without touching the shared fixtures.
pub fn load_with_overlay(
    source: &DataSource,
    options: &LoadOptions,
    overlay: Option<&Json>,
) -> (Store, Diagnostics) {
    let mut diagnostics = Diagnostics::new();
    let (files, files_directory) = match source {
        DataSource::Directory(root) => (
            read_directory(root, &mut diagnostics),
            Some(root.join("files")),
        ),
        DataSource::Demo => (demo_files(), None),
    };
    let mut merged = merge(files, &mut diagnostics);
    if let Some(overlay) = overlay {
        apply_overlay(&mut merged, overlay, &mut diagnostics);
    }
    let probe = |src: &str| -> Option<(u32, u32)> {
        let path = files_directory.as_ref()?.join(src);
        image::image_dimensions(path).ok()
    };
    let (store, build_diagnostics) = build(
        merged,
        &BuildOptions {
            theme_locales: options.theme_locales.clone(),
            probe_image: &probe,
        },
    );
    diagnostics.extend(build_diagnostics);
    (store, diagnostics)
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
    fn overlay_errors_are_reported() {
        let overlay = serde_json::json!({"products": [{"titel": "x"}]});
        let (_, diagnostics) =
            load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
        assert!(diagnostics.has_errors());
        assert_eq!(diagnostics.items[0].file, OVERLAY_FILE);
    }
}

//! The shop's themes: `themeCreate`, `themeUpdate`, `themeDelete`, `themeDuplicate`,
//! `themePublish`, `themeFilesUpsert`, `themeFilesCopy`, `themeFilesDelete`; `theme`, `themes`,
//! `OnlineStoreTheme.files`.
//!
//! **The local theme is the main theme.** The theme the storefront serves is the developer's
//! directory (`lsf serve --theme`): it is the shop's `MAIN` theme, `OnlineStoreTheme/1` (the id
//! `content_for_header` gives `Shopify.theme`), named as its `config/settings_schema.json` says
//! (`theme_info.theme_name`). Its files are read from the directory, with their checksums and
//! sizes, whenever they are asked for. The other themes (created from a ZIP, duplicated) live in
//! the shop, their files kept as the shop's blobs.
//!
//! **Nothing is written to the directory.** A write to the local theme (`themeFilesUpsert`,
//! `…Copy`, `…Delete`) is kept as an overlay of the theme: the files written over or added, the
//! files deleted. The storefront serves the directory with the overlay on top
//! ([`storefront_overlay`], which the server gives to `lsf_core::theme::ThemeFiles::set_overlay`),
//! so a backend that edits the main theme sees its edit on the storefront, and the developer's
//! files stay theirs. A reset (`DELETE /__lsf/admin`) drops the overlay. Publishing another theme
//! makes the storefront serve that theme alone (an exclusive overlay of all its files);
//! publishing the local theme again serves the directory, which is why the local theme cannot
//! be deleted, published or not.
//!
//! Only the default shop's main theme reaches the storefront, as only its store syncs.
//!
//! Each mutation's rules and sources are in `themes::mutations`.

pub mod files;
pub mod mutations;
pub mod zip;

use std::path::{Path, PathBuf};

use lsf_core::theme::FileOverlay;
use serde_json::{Value, json};

use crate::resolve::Registry;
use crate::state::seed::Seed;
use crate::state::{Record, ShopState, gid};

/// The id of the local theme: `Shopify.theme.id` of `content_for_header` while it is the main
/// theme (`lsf_core::urls::THEME_ID`; another main theme is named by [`storefront_identity`]).
pub const LOCAL_THEME_ID: u64 = lsf_core::urls::THEME_ID;

/// The setting of the shop that holds the directory of the local theme.
pub const THEME_ROOT: &str = "themeRoot";

/// The field of a theme record that keeps its files: by filename, `{blob, checksumMd5, size,
/// contentType, createdAt, updatedAt}` (the overlay of the local theme, the whole of another),
/// or `{deleted: true}` for a file of the directory deleted.
pub const FILES: &str = "lsfFiles";

/// The field of a theme record that says it is the directory's.
pub const LOCAL: &str = "lsfLocal";

/// The field of the local theme's record that says `themeUpdate` named it: until then, its name
/// is the one its files give, which the storefront reads from them.
pub const RENAMED: &str = "lsfRenamed";

/// The name lsf-core gives a theme without `theme_info`.
pub const DEFAULT_NAME: &str = "Local theme";

pub fn local_theme_gid() -> String {
    gid("OnlineStoreTheme", LOCAL_THEME_ID)
}

/// The name a theme's `config/settings_schema.json` gives it (`theme_info.theme_name`).
pub fn theme_name(settings_schema: &str) -> Option<String> {
    let schema =
        lsf_core::json::parse_lenient("config/settings_schema.json", settings_schema).ok()?;
    schema
        .as_array()?
        .iter()
        .find(|group| group.get("name").and_then(Value::as_str) == Some("theme_info"))?
        .get("theme_name")?
        .as_str()
        .map(str::to_string)
}

/// The local theme of the seed: the main theme, with no files until the server gives its
/// directory ([`attach`]).
pub fn seed(seed: &mut Seed) {
    let created = Seed::epoch_string();
    seed.insert(record(LOCAL_THEME_ID, DEFAULT_NAME, "MAIN", true, &created));
}

/// A theme record without files: the seed's local theme, or one the mutations make.
fn record(number: u64, name: &str, role: &str, local: bool, created: &str) -> Value {
    json!({
        "__typename": "OnlineStoreTheme",
        "id": gid("OnlineStoreTheme", number),
        "name": name,
        "role": role,
        "prefix": format!("/t/{number}"),
        "processing": false,
        "processingFailed": false,
        "themeStoreId": null,
        "createdAt": created,
        "updatedAt": created,
        LOCAL: local,
        FILES: {},
    })
}

/// Gives the local theme its directory: its name, and its files from now on. Not a change of
/// the Admin API (store sync does not see it): the seed is amended.
pub fn attach(state: &mut ShopState, root: &Path) {
    let name = std::fs::read_to_string(root.join("config/settings_schema.json"))
        .ok()
        .and_then(|text| theme_name(&text))
        .unwrap_or_else(|| DEFAULT_NAME.to_string());
    let root = root.display().to_string();
    state.amend_seed(|state| {
        state.set_setting(THEME_ROOT, json!(root));
        if let Some(theme) = state.get_mut(&local_theme_gid()) {
            theme.insert("name".into(), json!(name));
        }
    });
}

/// The directory of the local theme, when the server gave it.
pub fn root(state: &ShopState) -> Option<PathBuf> {
    state
        .setting(THEME_ROOT)
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// The theme of `id`, when it is one.
pub fn theme_of(state: &ShopState, id: &str) -> Option<Record> {
    state.get_of(id, "OnlineStoreTheme").cloned()
}

/// Whether a theme is the directory's.
pub fn is_local(theme: &Record) -> bool {
    theme.get(LOCAL).and_then(Value::as_bool) == Some(true)
}

/// The main theme of a shop.
pub fn main_theme(state: &ShopState) -> Option<String> {
    state
        .keys("OnlineStoreTheme")
        .find(|key| {
            state
                .get(key)
                .and_then(|theme| theme.get("role"))
                .and_then(Value::as_str)
                == Some("MAIN")
        })
        .map(str::to_string)
}

/// What the storefront serves of the shop's main theme beside its directory: `None` when it is
/// the directory alone (the local theme is the main one and the Admin API wrote nothing to it),
/// an overlay of what the Admin API wrote to it, or, when another theme is the main one, all of
/// that theme's files, exclusively. Its generation is a hash of what it holds
/// ([`storefront_overlay_generation`]), so that the server makes and applies a new one only
/// when it changed.
pub fn storefront_overlay(state: &ShopState) -> Option<FileOverlay> {
    storefront_overlay_from(state, None)
}

/// [`storefront_overlay`], with the files of `previous` (the overlay served) kept where their
/// bytes are the same blob: no byte is copied, and a file that did not change keeps its text
/// `Arc`, which the theme's caches of parsed Liquid and JSON are keyed by.
pub fn storefront_overlay_from(
    state: &ShopState,
    previous: Option<&FileOverlay>,
) -> Option<FileOverlay> {
    let (local, entries, generation) = overlay_of(state)?;
    let mut overlay = FileOverlay {
        exclusive: !local,
        generation,
        ..FileOverlay::default()
    };
    for (filename, entry) in entries {
        let file = files::entry_bytes(state, entry).map(|bytes| {
            previous
                .and_then(|previous| previous.files.get(filename)?.as_ref())
                .filter(|kept| std::sync::Arc::ptr_eq(&kept.bytes, &bytes))
                .cloned()
                .unwrap_or_else(|| lsf_core::theme::OverlayFile::new(bytes))
        });
        overlay.files.insert(filename.clone(), file);
    }
    Some(overlay)
}

/// The generation of [`storefront_overlay`] without making it (`0` for none): from the
/// filenames and digests alone, so that a change of the shop that is not one of its main theme
/// copies nothing.
pub fn storefront_overlay_generation(state: &ShopState) -> u64 {
    overlay_of(state).map_or(0, |(_, _, generation)| generation)
}

/// Whether the main theme is the local one, its file entries and the generation of its
/// overlay: `None` when the storefront serves the directory alone.
fn overlay_of(state: &ShopState) -> Option<(bool, &serde_json::Map<String, Value>, u64)> {
    let main = main_theme(state)?;
    let theme = state.get(&main)?;
    let entries = theme.get(FILES).and_then(Value::as_object)?;
    let local = is_local(theme);
    if local && entries.is_empty() {
        return None;
    }
    let mut fingerprint = format!("{main}\0{local}");
    for (filename, entry) in entries {
        fingerprint.push('\0');
        fingerprint.push_str(filename);
        fingerprint.push('\0');
        fingerprint.push_str(
            entry
                .get("checksumMd5")
                .and_then(Value::as_str)
                .unwrap_or("deleted"),
        );
    }
    let generation = lsf_core::util::stable_id("theme-overlay", &fingerprint).max(1);
    Some((local, entries, generation))
}

/// Which theme the storefront serves, as `Shopify.theme`, the `theme` object and the asset URLs
/// name it: the shop's main theme. `None` while it is the local theme as its files name it (the
/// directory alone: id 1, the name of its `theme_info`).
pub fn storefront_identity(state: &ShopState) -> Option<lsf_core::theme::ThemeIdentity> {
    let main = main_theme(state)?;
    let theme = state.get(&main)?;
    let renamed = theme.get(RENAMED).and_then(Value::as_bool) == Some(true);
    if is_local(theme) && !renamed {
        return None;
    }
    let (_, id) = crate::state::parse_gid(&main)?;
    Some(lsf_core::theme::ThemeIdentity {
        id,
        name: theme
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        role: theme
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("MAIN")
            .to_ascii_lowercase(),
    })
}

pub fn register(registry: &mut Registry) {
    files::register(registry);
    mutations::register(registry);
    registry.query("themes", themes);
}

/// `themes(roles:, names:)`: the themes of those roles, of those names.
fn themes(
    r: &crate::resolve::ShopResolver<'_>,
    ctx: &crate::graphql::FieldContext<'_>,
    _object: &crate::resolve::Obj,
) -> Result<crate::graphql::Output<crate::resolve::Obj>, crate::graphql::FieldError> {
    let list = |name: &str| -> Option<Vec<String>> {
        ctx.arg(name).and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
    };
    let (roles, names) = (list("roles"), list("names"));
    let items: Vec<Value> = r
        .records_of("OnlineStoreTheme")
        .into_iter()
        .filter(|item| {
            let Some(theme) = r.item_map(item) else {
                return false;
            };
            let field = |name: &str| theme.get(name).and_then(Value::as_str).unwrap_or_default();
            roles
                .as_ref()
                .is_none_or(|roles| roles.iter().any(|role| role == field("role")))
                && names
                    .as_ref()
                    .is_none_or(|names| names.iter().any(|name| name == field("name")))
        })
        .collect();
    r.connection(ctx, items)
}

#[cfg(test)]
mod tests;

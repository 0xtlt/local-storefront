//! Navigation: `menuCreate`, `menuUpdate`, `menuDelete`, and the URL redirects
//! (`urlRedirectCreate`, `urlRedirectUpdate`, `urlRedirectDelete`, the `urlRedirects` query).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A menu has "a unique handle" and items "nested … up to three levels deep"
//!   (`NESTING_TOO_DEEP`). An item of a resource type names its resource by `resourceId`, which
//!   must exist (`NOT_FOUND`); its `url` is the one given, else the path of the resource
//!   (`/products/<handle>`, …). "You can update the handle for non-default menus" and a default
//!   menu (`main-menu`, `footer`) cannot be deleted (`UNABLE_TO_DELETE_DEFAULT_MENU`).
//!   `menuUpdate` replaces the items; an item given with its `id` keeps it.
//! - A redirect has a `path` (a leading `/` added when it has none) and a `target` (a path or a
//!   URL); a path is redirected once (`Path has already been taken`).
//!
//! Ids: a menu created gets `menu_gid(handle)`, the id lsf-core gives the menu of the store data
//! (store sync writes it back by its handle), else a new one when another menu holds it; its items
//! get `stable_id("menu_item", "<handle>/<i>/<j>…")`, as the seed's.
//!
//! Events: Shopify has no webhook topic for menus or redirects in 2026-10: their changes are the
//! events `menus/<action>` and `url_redirects/<action>`, which no subscription can name.

use lsf_core::util::stable_id;
use serde_json::{Map, Value, json};

use super::menu_gid;
use super::metafields::text;
use crate::admin::graphql::{FieldContext, FieldError};
use crate::admin::resolve::{Mutation, Registry, UserError, payload};
use crate::admin::state::{Record, ShopState, gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("menuCreate", menu_create)
        .mutation("menuUpdate", menu_update)
        .mutation("menuDelete", menu_delete)
        .mutation("urlRedirectCreate", redirect_create)
        .mutation("urlRedirectUpdate", redirect_update)
        .mutation("urlRedirectDelete", redirect_delete)
        .default_text("UrlRedirect", |_r, record| {
            ["path", "target"]
                .iter()
                .filter_map(|field| record.get(*field)?.as_str().map(str::to_string))
                .collect()
        })
        .default_text("Menu", |_r, record| {
            ["title", "handle"]
                .iter()
                .filter_map(|field| record.get(*field)?.as_str().map(str::to_string))
                .collect()
        });
}

/// How deep menu items nest.
const MAX_DEPTH: usize = 3;

/// The record type a menu item of `kind` names.
fn resource_type(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "PRODUCT" => "Product",
        "COLLECTION" => "Collection",
        "PAGE" => "Page",
        "BLOG" => "Blog",
        "ARTICLE" => "Article",
        "SHOP_POLICY" => "ShopPolicy",
        "METAOBJECT" => "Metaobject",
        _ => return None,
    })
}

/// The path of a menu item without a `url`: its resource's, or its type's.
fn item_url(state: &ShopState, kind: &str, resource: Option<&str>) -> Option<String> {
    let record = resource.and_then(|resource| state.get(resource));
    let handle = record.map(|record| text(record, "handle"));
    Some(match kind {
        "FRONTPAGE" => "/".to_string(),
        "CATALOG" => "/collections/all".to_string(),
        "COLLECTIONS" => "/collections".to_string(),
        "SEARCH" => "/search".to_string(),
        "PRODUCT" => format!("/products/{}", handle?),
        "COLLECTION" => format!("/collections/{}", handle?),
        "PAGE" => format!("/pages/{}", handle?),
        "BLOG" => format!("/blogs/{}", handle?),
        "ARTICLE" => {
            let record = record?;
            let blog = state.get(text(record, "blog"))?;
            format!("/blogs/{}/{}", text(blog, "handle"), text(record, "handle"))
        }
        "SHOP_POLICY" => {
            let url = text(record?, "url");
            let at = url.find("/policies/")?;
            url[at..].to_string()
        }
        "METAOBJECT" => {
            let record = record?;
            let url_handle = super::metaobjects::url_handle(state, text(record, "type"));
            format!("/pages/{url_handle}/{}", text(record, "handle"))
        }
        _ => return None,
    })
}

/// The `MenuItem`s of an input, checked: `path` is the input's, `ids` the menu's handle and the
/// positions above (what an item's id is made of).
fn items_of(
    state: &ShopState,
    input: Option<&Value>,
    field: &[String],
    ids: &str,
    depth: usize,
    errors: &mut Vec<UserError>,
) -> Vec<Value> {
    let mut items = Vec::new();
    for (index, item) in input
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let mut path = field.to_vec();
        path.push(index.to_string());
        let path_refs: Vec<&str> = path.iter().map(String::as_str).collect();
        if depth > MAX_DEPTH {
            errors.push(
                UserError::new(
                    &path_refs,
                    format!("Menu items can only be nested {MAX_DEPTH} levels deep."),
                )
                .code("NESTING_TOO_DEEP"),
            );
            continue;
        }
        let get = |name: &str| item.get(name).and_then(Value::as_str);
        let kind = get("type").unwrap_or("HTTP");
        let resource = get("resourceId");
        if let Some(type_name) = resource_type(kind) {
            match resource {
                Some(resource) if state.type_of(resource) == Some(type_name) => {}
                Some(_) => {
                    let mut at = path_refs.clone();
                    at.push("resourceId");
                    errors.push(UserError::new(&at, "Resource not found.").code("NOT_FOUND"));
                    continue;
                }
                None if get("url").is_none() => {
                    let mut at = path_refs.clone();
                    at.push("resourceId");
                    errors.push(UserError::new(&at, "Resource ID can't be blank"));
                    continue;
                }
                None => {}
            }
        }
        let title = get("title").unwrap_or_default();
        if title.trim().is_empty() {
            let mut at = path_refs.clone();
            at.push("title");
            errors.push(UserError::new(&at, "Title can't be blank"));
        }
        if kind == "HTTP" && get("url").is_none_or(|url| url.trim().is_empty()) {
            let mut at = path_refs.clone();
            at.push("url");
            errors.push(UserError::new(&at, "Url can't be blank"));
        }
        let position = format!("{ids}/{index}");
        let id = get("id")
            .filter(|id| id.starts_with("gid://shopify/MenuItem/"))
            .map_or_else(
                || gid("MenuItem", stable_id("menu_item", &position)),
                str::to_string,
            );
        let mut children_path = path.clone();
        children_path.push("items".to_string());
        let children = items_of(
            state,
            item.get("items"),
            &children_path,
            &position,
            depth + 1,
            errors,
        );
        let url = get("url")
            .map(str::to_string)
            .or_else(|| item_url(state, kind, resource));
        let tags: Vec<String> = item
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        items.push(json!({
            "id": id,
            "title": title,
            "type": kind,
            "url": url,
            "resourceId": resource,
            "tags": tags,
            "items": children,
        }));
    }
    items
}

/// The menu of a handle, other than `except`.
fn menu_of(state: &ShopState, handle: &str, except: Option<&str>) -> Option<String> {
    state
        .keys("Menu")
        .find(|id| {
            Some(*id) != except
                && state
                    .get(id)
                    .is_some_and(|menu| text(menu, "handle") == handle)
        })
        .map(str::to_string)
}

fn menu_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let title = ctx.arg_str("title").unwrap_or_default();
    let handle = lsf_core::util::handleize(ctx.arg_str("handle").unwrap_or_default());
    let mut errors = Vec::new();
    if title.trim().is_empty() {
        errors.push(UserError::new(&["title"], "Title can't be blank"));
    }
    if handle.is_empty() {
        errors.push(UserError::new(&["handle"], "Handle can't be blank"));
    } else if menu_of(state, &handle, None).is_some() {
        errors.push(UserError::new(&["handle"], "Handle has already been taken"));
    }
    let items = items_of(
        state,
        ctx.arg("items"),
        &["items".to_string()],
        &handle,
        1,
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"menu": null}), &errors));
    }
    let state = mutation.state_mut();
    let stable = menu_gid(&handle);
    let id = state.created_gid("Menu", stable);
    let now = state.timestamp();
    state.insert(value::object(json!({
        "__typename": "Menu",
        "id": id,
        "handle": handle,
        "title": title.trim(),
        "isDefault": false,
        "items": items,
        "createdAt": now,
    })));
    state.emit("menus/create", &id);
    Ok(payload(json!({"menu": id}), &[]))
}

/// `menuUpdate`: the title, the handle (not a default menu's), and the items, replaced.
fn menu_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "Menu").cloned() else {
        let error = UserError::new(&["id"], "Menu does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"menu": null}), &[error]));
    };
    let title = ctx.arg_str("title").unwrap_or_default();
    let mut errors = Vec::new();
    if title.trim().is_empty() {
        errors.push(UserError::new(&["title"], "Title can't be blank"));
    }
    let handle = match ctx.arg_str("handle").map(lsf_core::util::handleize) {
        Some(handle) if handle != text(&record, "handle") => {
            if record.get("isDefault").and_then(Value::as_bool) == Some(true) {
                errors.push(UserError::new(
                    &["handle"],
                    "The handle of a default menu can't be changed.",
                ));
            } else if handle.is_empty() {
                errors.push(UserError::new(&["handle"], "Handle can't be blank"));
            } else if menu_of(state, &handle, Some(&id)).is_some() {
                errors.push(UserError::new(&["handle"], "Handle has already been taken"));
            }
            handle
        }
        _ => text(&record, "handle").to_string(),
    };
    let items = items_of(
        state,
        ctx.arg("items"),
        &["items".to_string()],
        &handle,
        1,
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"menu": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(menu) = state.get_mut(&id) {
        menu.insert("title".into(), json!(title.trim()));
        menu.insert("handle".into(), json!(handle));
        menu.insert("items".into(), json!(items));
    }
    state.emit("menus/update", &id);
    Ok(payload(json!({"menu": id}), &[]))
}

fn menu_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "Menu").cloned() else {
        let error = UserError::new(&["id"], "Menu does not exist").code("MENU_DOES_NOT_EXIST");
        return Ok(payload(json!({"deletedMenuId": null}), &[error]));
    };
    if record.get("isDefault").and_then(Value::as_bool) == Some(true) {
        let error = UserError::new(&["id"], "Default menus can't be deleted.")
            .code("UNABLE_TO_DELETE_DEFAULT_MENU");
        return Ok(payload(json!({"deletedMenuId": null}), &[error]));
    }
    mutation.state_mut().delete(&id, "menus/delete");
    Ok(payload(json!({"deletedMenuId": id}), &[]))
}

// ---- redirects ---------------------------------------------------------------------------

/// A path as a redirect keeps it: with a leading `/`.
fn normalize_path(path: &str) -> String {
    let path = path.trim();
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// The redirect of a path, other than `except`.
fn redirect_of(state: &ShopState, path: &str, except: Option<&str>) -> Option<String> {
    state
        .keys("UrlRedirect")
        .find(|id| {
            Some(*id) != except
                && state
                    .get(id)
                    .is_some_and(|redirect| text(redirect, "path") == path)
        })
        .map(str::to_string)
}

/// Redirects `path` to `target`, replacing a redirect of the same path: what a `redirectNewHandle`
/// does. Emits `url_redirects/create` or `url_redirects/update`.
pub fn add_redirect(state: &mut ShopState, path: &str, target: &str) -> String {
    let existing = redirect_of(state, path, None);
    set_redirect(state, existing.as_deref(), path, target)
}

/// [`add_redirect`] when the caller knows the redirect of `path` already (`existing`), as an
/// import of many rows does from a map of the paths: the redirect `existing` takes the target,
/// or a new one is made.
pub fn set_redirect(
    state: &mut ShopState,
    existing: Option<&str>,
    path: &str,
    target: &str,
) -> String {
    if let Some(id) = existing.filter(|id| state.contains(id)) {
        if let Some(redirect) = state.get_mut(id) {
            redirect.insert("target".into(), json!(target));
        }
        state.emit("url_redirects/update", id);
        return id.to_string();
    }
    let id = state.new_gid("UrlRedirect");
    let now = state.timestamp();
    state.insert(value::object(json!({
        "__typename": "UrlRedirect",
        "id": id,
        "path": path,
        "target": target,
        "createdAt": now,
    })));
    state.emit("url_redirects/create", &id);
    id
}

/// The checks of a redirect's input: `(path, target)`, those given (`required` on creation).
fn check_redirect(
    state: &ShopState,
    input: &Map<String, Value>,
    except: Option<&str>,
    before: Option<&Record>,
    code: &str,
    errors: &mut Vec<UserError>,
) -> (String, String) {
    let given = |name: &str| input.get(name).and_then(Value::as_str);
    let path = given("path")
        .map(normalize_path)
        .or_else(|| before.map(|record| text(record, "path").to_string()))
        .unwrap_or_default();
    let target = given("target")
        .map(|target| target.trim().to_string())
        .or_else(|| before.map(|record| text(record, "target").to_string()))
        .unwrap_or_default();
    if path.trim_start_matches('/').is_empty() {
        errors.push(UserError::new(&["urlRedirect", "path"], "Path can't be blank").code(code));
    } else if redirect_of(state, &path, except).is_some() {
        errors.push(
            UserError::new(&["urlRedirect", "path"], "Path has already been taken").code(code),
        );
    }
    if target.is_empty() {
        errors.push(UserError::new(&["urlRedirect", "target"], "Target can't be blank").code(code));
    } else if target == path {
        errors.push(
            UserError::new(
                &["urlRedirect", "target"],
                "Target cannot be the same as path",
            )
            .code(code),
        );
    }
    (path, target)
}

fn redirect_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("urlRedirect");
    let mut errors = Vec::new();
    let (path, target) = check_redirect(
        mutation.state(),
        &input,
        None,
        None,
        "CREATE_FAILED",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"urlRedirect": null}), &errors));
    }
    let id = add_redirect(mutation.state_mut(), &path, &target);
    Ok(payload(json!({"urlRedirect": id}), &[]))
}

fn redirect_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = ctx.arg_object_or_default("urlRedirect");
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "UrlRedirect").cloned() else {
        let error = UserError::new(&["id"], "Redirect does not exist").code("DOES_NOT_EXIST");
        return Ok(payload(json!({"urlRedirect": null}), &[error]));
    };
    let mut errors = Vec::new();
    let (path, target) = check_redirect(
        state,
        &input,
        Some(&id),
        Some(&record),
        "UPDATE_FAILED",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"urlRedirect": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(redirect) = state.get_mut(&id) {
        redirect.insert("path".into(), json!(path));
        redirect.insert("target".into(), json!(target));
    }
    state.emit("url_redirects/update", &id);
    Ok(payload(json!({"urlRedirect": id}), &[]))
}

fn redirect_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("UrlRedirect") {
        let error = UserError::new(&["id"], "Redirect does not exist").code("DOES_NOT_EXIST");
        return Ok(payload(json!({"deletedUrlRedirectId": null}), &[error]));
    }
    mutation.state_mut().delete(&id, "url_redirects/delete");
    Ok(payload(json!({"deletedUrlRedirectId": id}), &[]))
}

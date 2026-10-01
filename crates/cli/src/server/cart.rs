//! The Cart AJAX API: `/cart/add.js`, `/cart/change.js`, `/cart/update.js`, `/cart/clear.js`.

use indexmap::IndexMap;
use serde_json::{Value as Json, json};
use slt_core::drops::cart::{cart_json, line_json, line_key, resolve_lines};
use slt_core::store::{CartLine, InventoryPolicy};

use super::params::{integer, text};
use super::reply::Reply;
use super::storefront::{Visit, section_ids};

fn error(status: u16, description: impl Into<String>) -> Reply {
    Reply::json(
        status,
        &json!({"status": status, "message": "Cart Error", "description": description.into()}),
    )
}

/// Whether the client expects JSON back (`/cart/add.js`) or a page (`/cart/add` from a form).
fn wants_json(visit: &Visit<'_>) -> bool {
    visit.incoming.path.ends_with(".js")
        || visit
            .incoming
            .header("accept")
            .is_some_and(|accept| accept.contains("application/json"))
        || visit
            .incoming
            .header("content-type")
            .is_some_and(|kind| kind.contains("application/json"))
        || visit.incoming.header("x-requested-with").is_some()
}

fn properties(value: Option<&Json>) -> IndexMap<String, String> {
    value
        .and_then(Json::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    let value = match value {
                        Json::String(text) => text.clone(),
                        Json::Null => return None,
                        other => other.to_string(),
                    };
                    // Empty properties are dropped, as Shopify does.
                    (!value.is_empty()).then(|| (key.clone(), value))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Adds the bundled sections the request asked for to a JSON response.
fn with_sections(visit: &Visit<'_>, params: &Json, mut body: Json) -> Json {
    let ids = section_ids(params.get("sections"));
    if !ids.is_empty() {
        let url = text(params, "sections_url");
        body["sections"] = visit.render_sections(&ids, url.as_deref());
    }
    body
}

fn cart_reply(visit: &Visit<'_>, params: &Json) -> Reply {
    if !wants_json(visit) {
        let return_to = text(params, "return_to").unwrap_or_else(|| visit.localized("/cart"));
        return Reply::redirect(&return_to);
    }
    Reply::json(200, &with_sections(visit, params, cart_json(&visit.site())))
}

/// Adds one item to the session's cart. Returns the key of the line it ended up in.
fn add_item(visit: &Visit<'_>, item: &Json) -> Result<String, Reply> {
    let store = &visit.store;
    let Some(variant_id) = integer(item, "id").map(|id| id as u64) else {
        return Err(error(
            400,
            "Parameter Missing or Invalid: Required parameter missing or invalid: items",
        ));
    };
    let Some((product, variant)) = store.variant(variant_id) else {
        return Err(error(404, "Cannot find variant"));
    };
    let quantity = integer(item, "quantity").unwrap_or(1);
    if quantity < 1 {
        return Err(error(422, "Quantity must be 1 or more"));
    }
    let quantity = quantity as u32;
    if !variant.available {
        return Err(error(
            422,
            format!(
                "The product '{}' is already sold out.",
                line_title(product, variant)
            ),
        ));
    }
    let line = CartLine {
        variant_id,
        quantity,
        properties: properties(item.get("properties")),
    };
    let key = line_key(&line);
    let in_cart: u32 = visit
        .session()
        .cart_lines
        .iter()
        .filter(|existing| existing.variant_id == variant_id)
        .map(|existing| existing.quantity)
        .sum();
    let limit = variant
        .quantity_rule
        .max
        .into_iter()
        .chain(
            (variant.inventory_tracked && variant.inventory_policy == InventoryPolicy::Deny)
                .then(|| variant.inventory_quantity.max(0) as u32),
        )
        .min();
    let mut to_add = quantity;
    let mut problem = None;
    if let Some(limit) = limit {
        if in_cart >= limit {
            return Err(error(
                422,
                format!(
                    "All {limit} {} are in your cart.",
                    line_title(product, variant)
                ),
            ));
        }
        if in_cart + quantity > limit {
            to_add = limit - in_cart;
            problem = Some(error(
                422,
                format!(
                    "You can only add {to_add} {} to the cart.",
                    line_title(product, variant)
                ),
            ));
        }
    }
    visit.update_session(|session| {
        match session
            .cart_lines
            .iter_mut()
            .find(|existing| line_key(existing) == key)
        {
            Some(existing) => existing.quantity += to_add,
            // New lines go to the top of the cart.
            None => session.cart_lines.insert(
                0,
                CartLine {
                    quantity: to_add,
                    ..line.clone()
                },
            ),
        }
    });
    match problem {
        Some(problem) => Err(problem),
        None => Ok(key),
    }
}

fn line_title(product: &slt_core::store::Product, variant: &slt_core::store::Variant) -> String {
    if product.has_only_default_variant() {
        product.title.clone()
    } else {
        format!("{} - {}", product.title, variant.title)
    }
}

pub fn add(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let items: Vec<Json> = match params.get("items") {
        Some(Json::Array(items)) => items.clone(),
        _ => vec![params.clone()],
    };
    let many = params.get("items").is_some();
    let mut keys = Vec::new();
    for item in &items {
        match add_item(visit, item) {
            Ok(key) => keys.push(key),
            Err(reply) => {
                if wants_json(visit) {
                    return reply;
                }
                return Reply::redirect(&visit.localized("/cart"));
            }
        }
    }
    if !wants_json(visit) {
        let return_to = text(&params, "return_to").unwrap_or_else(|| visit.localized("/cart"));
        return Reply::redirect(&return_to);
    }
    let site = visit.site();
    let lines = resolve_lines(&site);
    let added: Vec<Json> = keys
        .iter()
        .filter_map(|key| lines.iter().find(|line| line.key() == *key))
        .map(|line| line_json(&site, line))
        .collect();
    let body = if many {
        json!({ "items": added })
    } else {
        added.into_iter().next().unwrap_or(Json::Null)
    };
    Reply::json(200, &with_sections(visit, &params, body))
}

/// The index of the cart line a `change` request targets: by 1-based `line`, by line key, or
/// by variant id.
fn target_line(lines: &[CartLine], params: &Json) -> Option<usize> {
    if let Some(line) = integer(params, "line") {
        return (line >= 1 && (line as usize) <= lines.len()).then(|| line as usize - 1);
    }
    let id = text(params, "id")?;
    lines
        .iter()
        .position(|line| line_key(line) == id)
        .or_else(|| {
            lines
                .iter()
                .position(|line| line.variant_id.to_string() == id)
        })
}

pub fn change(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let lines = visit.session().cart_lines;
    let Some(index) = target_line(&lines, &params) else {
        return if wants_json(visit) {
            error(400, "no valid id or line parameter")
        } else {
            Reply::redirect(&visit.localized("/cart"))
        };
    };
    let quantity = integer(&params, "quantity");
    let new_properties = params
        .get("properties")
        .map(|value| properties(Some(value)));
    visit.update_session(|session| {
        if let Some(properties) = new_properties {
            session.cart_lines[index].properties = properties;
        }
        match quantity {
            Some(quantity) if quantity <= 0 => {
                session.cart_lines.remove(index);
            }
            Some(quantity) => session.cart_lines[index].quantity = quantity as u32,
            None => {}
        }
    });
    cart_reply(visit, &params)
}

/// Applies `updates` (by variant id or key, or positionally), `note` and `attributes`.
fn apply_updates(visit: &Visit<'_>, params: &Json) {
    let note = text(params, "note");
    let attributes = params
        .get("attributes")
        .map(|value| properties(Some(value)));
    let updates = params.get("updates").cloned();
    visit.update_session(|session| {
        if let Some(note) = note {
            session.cart_note = note;
        }
        if let Some(attributes) = attributes {
            for (key, value) in attributes {
                session.cart_attributes.insert(key, value);
            }
        }
        let quantity_of = |value: &Json| match value {
            Json::Number(number) => number.as_i64(),
            Json::String(text) => text.trim().parse::<i64>().ok(),
            _ => None,
        };
        match &updates {
            Some(Json::Array(quantities)) => {
                for (index, quantity) in quantities.iter().enumerate() {
                    if let (Some(line), Some(quantity)) =
                        (session.cart_lines.get_mut(index), quantity_of(quantity))
                    {
                        line.quantity = quantity.max(0) as u32;
                    }
                }
            }
            Some(Json::Object(by_id)) => {
                for (id, quantity) in by_id {
                    let Some(quantity) = quantity_of(quantity) else {
                        continue;
                    };
                    let position = session.cart_lines.iter().position(|line| {
                        line_key(line) == *id || line.variant_id.to_string() == *id
                    });
                    match position {
                        Some(position) => {
                            session.cart_lines[position].quantity = quantity.max(0) as u32
                        }
                        // Updating a variant that is not in the cart adds it.
                        None => {
                            if let Some(variant_id) =
                                id.parse::<u64>().ok().filter(|_| quantity > 0)
                            {
                                session.cart_lines.push(CartLine {
                                    variant_id,
                                    quantity: quantity as u32,
                                    properties: IndexMap::new(),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        session.cart_lines.retain(|line| line.quantity > 0);
    });
}

pub fn update(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    apply_updates(visit, &params);
    cart_reply(visit, &params)
}

pub fn clear(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    visit.update_session(|session| {
        session.cart_lines.clear();
        session.cart_note.clear();
        session.cart_attributes.clear();
    });
    cart_reply(visit, &params)
}

/// The cart page's form: updates quantities and the note, then goes to checkout or back.
pub fn submit(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    apply_updates(visit, &params);
    if params.get("checkout").is_some() {
        Reply::redirect(&visit.localized("/checkout"))
    } else {
        Reply::redirect(&visit.localized("/cart"))
    }
}

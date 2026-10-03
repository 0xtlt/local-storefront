//! The Cart AJAX API: `/cart/add.js`, `/cart/change.js`, `/cart/update.js`, `/cart/clear.js`.

use indexmap::IndexMap;
use lsf_core::drops::cart::{cart_json, line_json, line_key, resolve_lines};
use lsf_core::store::{CartLine, InventoryPolicy};
use serde_json::{Value as Json, json};

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

/// What a request says about the selling plan of a line.
enum PlanParam {
    /// Nothing: the parameter is not there.
    Absent,
    /// No plan: the parameter is empty.
    Cleared,
    Id(u64),
    /// Something that is not the id of a plan.
    Invalid,
}

fn selling_plan(params: &Json) -> PlanParam {
    match params.get("selling_plan") {
        None => PlanParam::Absent,
        Some(Json::Null) => PlanParam::Cleared,
        Some(Json::String(text)) if text.trim().is_empty() => PlanParam::Cleared,
        Some(Json::Number(number)) => number.as_u64().map_or(PlanParam::Invalid, PlanParam::Id),
        Some(Json::String(text)) => text
            .trim()
            .parse::<u64>()
            .map_or(PlanParam::Invalid, PlanParam::Id),
        Some(_) => PlanParam::Invalid,
    }
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

/// `GET /cart.js`: the cart, with the sections the request asked for.
pub fn show(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    Reply::json(
        200,
        &with_sections(visit, &params, cart_json(&visit.site())),
    )
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
    let selling_plan = match selling_plan(item) {
        PlanParam::Absent | PlanParam::Cleared => None,
        PlanParam::Id(id) if store.selling_plan_of(product, id).is_some() => Some(id),
        PlanParam::Id(_) | PlanParam::Invalid => {
            return Err(error(
                422,
                format!(
                    "The selling plan is not available for {}.",
                    line_title(product, variant)
                ),
            ));
        }
    };
    if selling_plan.is_none() && product.requires_selling_plan {
        return Err(error(
            422,
            "Variant can only be purchased with a selling plan.",
        ));
    }
    let line = CartLine {
        variant_id,
        quantity,
        properties: properties(item.get("properties")),
        selling_plan,
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

fn line_title(product: &lsf_core::store::Product, variant: &lsf_core::store::Variant) -> String {
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
    // `selling_plan` moves the line to another plan, or to none when it is empty.
    let product = visit
        .store
        .variant(lines[index].variant_id)
        .map(|(product, _)| product);
    let new_plan = match (selling_plan(&params), product) {
        (PlanParam::Absent, _) | (_, None) => Ok(None),
        (PlanParam::Cleared, Some(product)) if product.requires_selling_plan => {
            Err("Variant can only be purchased with a selling plan.")
        }
        (PlanParam::Cleared, _) => Ok(Some(None)),
        (PlanParam::Id(id), Some(product))
            if visit.store.selling_plan_of(product, id).is_some() =>
        {
            Ok(Some(Some(id)))
        }
        (PlanParam::Id(_) | PlanParam::Invalid, _) => {
            Err("The selling plan is not available for this item.")
        }
    };
    let new_plan = match new_plan {
        Ok(plan) => plan,
        Err(problem) => {
            return if wants_json(visit) {
                error(422, problem)
            } else {
                Reply::redirect(&visit.localized("/cart"))
            };
        }
    };
    visit.update_session(|session| {
        if let Some(properties) = new_properties {
            session.cart_lines[index].properties = properties;
        }
        if let Some(plan) = new_plan {
            session.cart_lines[index].selling_plan = plan;
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
    // An attribute set to nothing is removed; the others are added or replaced.
    let attributes: Option<Vec<(String, Option<String>)>> = params
        .get("attributes")
        .and_then(Json::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    let value = match value {
                        Json::Null => None,
                        Json::String(text) if text.is_empty() => None,
                        Json::String(text) => Some(text.clone()),
                        other => Some(other.to_string()),
                    };
                    (key.clone(), value)
                })
                .collect()
        });
    let updates = params.get("updates").cloned();
    visit.update_session(|session| {
        if let Some(note) = note {
            session.cart_note = note;
        }
        if let Some(attributes) = attributes {
            for (key, value) in attributes {
                match value {
                    Some(value) => {
                        session.cart_attributes.insert(key, value);
                    }
                    None => {
                        session.cart_attributes.shift_remove(&key);
                    }
                }
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
                                    selling_plan: None,
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

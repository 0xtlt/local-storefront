//! The control API under `/__slt/`: what tests and tools use to inspect the server and to put
//! a session in the exact state they need.

use std::sync::Arc;

use serde_json::{Value as Json, json};
use slt_core::Session;
use slt_core::diagnostics::Diagnostics;
use slt_core::store::build::resolve_cart;
use slt_core::store::load::{OVERLAY_FILE, load_with_overlay};
use slt_core::store::model::SessionInput;
use slt_core::store::validate::{FileKind, schema, validate};
use slt_liquid::filters::escape_html;

use super::reply::Reply;
use super::{Incoming, SESSION_COOKIE, ServerState};

pub fn handle(state: &ServerState, incoming: &Incoming) -> Reply {
    let path = incoming.path.trim_start_matches("/__slt").trim_matches('/');
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    match (incoming.method.as_str(), segments.as_slice()) {
        ("GET", []) => dashboard(state),
        ("GET", ["status"]) => Reply::json(200, &status(state)),
        ("GET", ["livereload"]) => Reply::text(200, state.change_token().to_string()),
        ("POST", ["reload"]) => {
            let loaded = state.reload();
            Reply::json(200, &diagnostics_json(&loaded.diagnostics))
        }
        ("GET", ["schema", name]) => {
            let name = name.trim_end_matches(".json").trim_end_matches(".schema");
            match FileKind::ALL.iter().find(|kind| kind.name() == name) {
                Some(kind) => Reply::json(200, &schema(*kind)),
                None => Reply::not_found(),
            }
        }
        ("GET", ["session"]) => {
            let (id, _) = state.session_id(incoming);
            Reply::json(200, &session_json(state, &id))
        }
        ("PUT" | "POST", ["session"]) => set_session(state, incoming),
        ("DELETE", ["session"]) => {
            let (id, _) = state.session_id(incoming);
            state.reset_session(&id);
            Reply::json(200, &json!({ "ok": true }))
        }
        _ => Reply::not_found(),
    }
}

fn diagnostics_json(diagnostics: &Diagnostics) -> Json {
    json!({
        "ok": !diagnostics.has_errors(),
        "errors": diagnostics.error_count(),
        "warnings": diagnostics.warning_count(),
        "diagnostics": diagnostics.items,
    })
}

fn status(state: &ServerState) -> Json {
    let loaded = state.loaded();
    let store = &loaded.store;
    json!({
        "theme": state.app.theme.files().root().display().to_string(),
        "data": state.app.describe_source(),
        "data_diagnostics": diagnostics_json(&loaded.diagnostics),
        "templates": state.app.theme.template_names(),
        "locales": store.languages.iter().map(|language| language.iso_code.clone()).collect::<Vec<_>>(),
        "counts": {
            "products": store.products.len(),
            "collections": store.collections.len(),
            "pages": store.pages.len(),
            "blogs": store.blogs.len(),
            "customers": store.customers.len(),
            "sessions": state.session_count(),
        },
        "routes": routes(state),
    })
}

/// Every storefront URL the current data gives a page to.
pub fn routes(state: &ServerState) -> Vec<String> {
    let loaded = state.loaded();
    let store = &loaded.store;
    let mut routes = vec![
        "/".to_string(),
        "/collections".to_string(),
        "/cart".to_string(),
        "/search".to_string(),
    ];
    routes.extend(
        store
            .collections
            .iter()
            .map(|collection| format!("/collections/{}", collection.handle)),
    );
    routes.extend(
        store
            .products
            .iter()
            .map(|product| format!("/products/{}", product.handle)),
    );
    routes.extend(
        store
            .pages
            .iter()
            .map(|page| format!("/pages/{}", page.handle)),
    );
    for blog in &store.blogs {
        routes.push(format!("/blogs/{}", blog.handle));
        routes.extend(
            blog.articles
                .iter()
                .map(|article| format!("/blogs/{}/{}", blog.handle, article.handle)),
        );
    }
    routes.extend(store.gift_cards.iter().map(|card| card.path(store.shop.id)));
    routes.extend(
        store
            .shop
            .policies
            .iter()
            .map(|policy| format!("/policies/{}", policy.handle)),
    );
    routes
}

fn session_json(state: &ServerState, id: &str) -> Json {
    let (session, store) = state.snapshot(id);
    json!({
        "id": id,
        "customer": session.customer_id.and_then(|customer| store.customer_by_id(customer)).map(|customer| customer.email.clone()),
        "country": session.country,
        "cart": {
            "note": session.cart_note,
            "attributes": session.cart_attributes,
            "items": session.cart_lines.iter().map(|line| json!({
                "variant": line.variant_id,
                "quantity": line.quantity,
                "properties": line.properties,
            })).collect::<Vec<_>>(),
        },
        "custom_data": state.with_session(id, |entry| entry.store.is_some()),
    })
}

/// `PUT /__slt/session`: replaces the session's state.
///
/// The body is a session (`customer`, `cart`, `country`) and may carry a `data` object in the
/// store data format that is applied on top of the data files for this session only.
fn set_session(state: &ServerState, incoming: &Incoming) -> Reply {
    let (id, is_new) = state.session_id(incoming);
    let mut body = incoming.params.clone();
    if !body.is_object() {
        return Reply::json(
            400,
            &json!({"ok": false, "error": "the body must be a JSON object"}),
        );
    }
    let overlay = body
        .as_object_mut()
        .and_then(|object| object.remove("data"));

    let mut diagnostics = Diagnostics::new();
    validate(FileKind::Session, OVERLAY_FILE, &body, "", &mut diagnostics);
    if diagnostics.has_errors() {
        return Reply::json(422, &diagnostics_json(&diagnostics));
    }
    let input: SessionInput = match serde_json::from_value(body) {
        Ok(input) => input,
        Err(error) => return Reply::json(422, &json!({"ok": false, "error": error.to_string()})),
    };

    // Session-specific data: the files plus the overlay.
    let custom_store = match &overlay {
        Some(overlay) => {
            let (store, overlay_diagnostics) =
                load_with_overlay(&state.app.source, &state.app.load_options(), Some(overlay));
            let mut relevant = Diagnostics::new();
            relevant.items = overlay_diagnostics
                .items
                .into_iter()
                .filter(|item| item.file == OVERLAY_FILE)
                .collect();
            if relevant.has_errors() {
                return Reply::json(422, &diagnostics_json(&relevant));
            }
            Some(Arc::new(store))
        }
        None => None,
    };
    let store = custom_store
        .clone()
        .unwrap_or_else(|| state.loaded().store.clone());

    let mut session = Session::initial(&store);
    if let Some(email) = &input.customer {
        match store.customer_by_email(email) {
            Some(customer) => session.customer_id = Some(customer.id),
            None => {
                diagnostics
                    .error("unknown_customer", OVERLAY_FILE, "/customer", format!("there is no customer with the email \"{email}\""))
                    .hint("add the customer to the store data, or to \"data\".\"customers\" in this request");
            }
        }
    }
    if let Some(country) = &input.country {
        match store.country(country) {
            Some(country) => session.country = Some(country.iso_code.clone()),
            None => {
                diagnostics.error(
                    "unknown_country",
                    OVERLAY_FILE,
                    "/country",
                    format!("\"{country}\" is not one of the store's countries"),
                );
            }
        }
    }
    if let Some(cart) = &input.cart {
        session.cart_lines = resolve_cart(&store, cart, &mut diagnostics, OVERLAY_FILE);
        session.cart_note = cart.note.clone().unwrap_or_default();
        session.cart_attributes = cart.attributes.clone();
    }
    if diagnostics.has_errors() {
        return Reply::json(422, &diagnostics_json(&diagnostics));
    }

    state.with_session(&id, |entry| {
        entry.session = session;
        entry.store = custom_store;
    });
    let reply = Reply::json(
        200,
        &json!({ "ok": true, "session": session_json(state, &id) }),
    );
    if is_new {
        reply.header(
            "set-cookie",
            format!("{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Lax"),
        )
    } else {
        reply
    }
}

fn dashboard(state: &ServerState) -> Reply {
    let loaded = state.loaded();
    let diagnostics = &loaded.diagnostics;
    let problems: String = if diagnostics.is_empty() {
        "<p class=\"ok\">The store data is valid.</p>".to_string()
    } else {
        let items: String = diagnostics
            .items
            .iter()
            .map(|item| format!("<li><pre>{}</pre></li>", escape_html(&item.to_string())))
            .collect();
        format!(
            "<p class=\"bad\">{} error(s), {} warning(s) in the store data.</p><ul>{items}</ul>",
            diagnostics.error_count(),
            diagnostics.warning_count()
        )
    };
    let links: String = routes(state)
        .iter()
        .map(|route| format!("<li><a href=\"{0}\">{0}</a></li>", escape_html(route)))
        .collect();
    Reply::html(
        200,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>slt · local storefront</title>\
             <style>body{{font:15px/1.5 system-ui,sans-serif;max-width:56rem;margin:3rem auto;padding:0 1rem;color:#1a1a1a}}\
             code,pre{{font:13px ui-monospace,monospace}}pre{{white-space:pre-wrap;background:#f5f5f5;padding:.75rem;border-radius:6px}}\
             .ok{{color:#0a7a3c}}.bad{{color:#b3261e}}ul{{padding-left:1.2rem}}li{{margin:.15rem 0}}\
             dt{{font-weight:600;margin-top:.5rem}}dd{{margin:0}}</style></head><body>\
             <h1>slt · local storefront</h1>\
             <dl><dt>Theme</dt><dd><code>{}</code></dd><dt>Store data</dt><dd><code>{}</code></dd></dl>\
             <h2>Store data</h2>{problems}\
             <h2>Pages</h2><ul>{links}</ul>\
             <h2>Control API</h2><ul>\
             <li><code>GET /__slt/status</code>: this page as JSON</li>\
             <li><code>GET|PUT|DELETE /__slt/session</code>: read, set or reset the session (cart, customer, per-session data)</li>\
             <li><code>GET /__slt/schema/store</code>: JSON Schema of the data format</li>\
             <li><code>POST /__slt/reload</code>: reload the data files</li></ul>\
             </body></html>",
            escape_html(&state.app.theme.files().root().display().to_string()),
            escape_html(&state.app.describe_source()),
        ),
    )
}

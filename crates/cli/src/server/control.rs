//! The control API under `/__lsf/`: what tests and tools use to inspect the server and to put
//! a session in the exact state they need.

use std::sync::Arc;

use lsf_core::Session;
use lsf_core::diagnostics::Diagnostics;
use lsf_core::render::cost::Costs;
use lsf_core::render::{ProfileOptions, Target};
use lsf_core::store::build::{resolve_cart, resolve_company_location};
use lsf_core::store::load::{OVERLAY_FILE, load_with_overlay};
use lsf_core::store::model::SessionInput;
use lsf_core::store::validate::{FileKind, schema, validate};
use lsf_liquid::filters::escape_html;
use serde_json::{Value as Json, json};

use super::reply::Reply;
use super::storefront::Visit;
use super::throttle::Throttle;
use super::{Incoming, SESSION_COOKIE, ServerState, speedscope};
use crate::profile::Unit;

pub fn handle(state: &ServerState, incoming: &Incoming) -> Reply {
    let path = incoming.path.trim_start_matches("/__lsf").trim_matches('/');
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    match (incoming.method.as_str(), segments.as_slice()) {
        ("GET", []) => dashboard(state, incoming),
        ("GET", ["status"]) => Reply::json(200, &status(state)),
        ("GET", ["livereload"]) => Reply::text(200, state.observe_changes().to_string()),
        ("GET", ["profile"]) => profile(state, incoming),
        ("GET", ["speedscope", file]) => speedscope::file(file),
        ("GET", ["timings"]) => Reply::json(200, &state.timings.to_json()),
        ("DELETE", ["timings"]) => {
            state.timings.clear();
            Reply::json(200, &json!({ "ok": true }))
        }
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
        ("GET", ["login"]) => login(state, incoming),
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
        "throttle": state.options.throttle.to_json(),
        "templates": state.app.theme.template_names(),
        "locales": store.languages.iter().map(|language| language.iso_code.clone()).collect::<Vec<_>>(),
        "customer_accounts": if lsf_core::site::hosted_accounts(&state.app.theme, store) { "new" } else { "legacy" },
        "customers": store.customers.iter().map(|customer| json!({
            "email": customer.email,
            "company": customer.company.map(|index| store.companies[index].name.clone()),
        })).collect::<Vec<_>>(),
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
        "/account".to_string(),
    ];
    if ["json", "liquid"].iter().any(|extension| {
        state
            .app
            .theme
            .files()
            .exists(&format!("templates/password.{extension}"))
    }) {
        routes.push("/password".to_string());
    }
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

/// `GET /__lsf/profile?path=<path>`: what the render of a page spends its time in, for the
/// session that asks, or with `points=1` what it would cost a storefront. Text for a person,
/// a flame graph with `html=1`, and with `format=speedscope` the file the flame graph is
/// drawn from, which any page may fetch.
fn profile(state: &ServerState, incoming: &Incoming) -> Reply {
    let Some(path) = incoming
        .query_param("path")
        .filter(|path| path.starts_with('/'))
    else {
        return Reply::text(
            400,
            "say which page: /__lsf/profile?path=/collections/all\n\
             also: html=1 (a flame graph), points=1 (what the page costs a storefront, not the \
             time it takes here), cost=product=50,metafield=5 (the points of a kind), lines=1 \
             (every tag and output), all=1 (every row), runs=<n>, section_id=<id>, \
             format=speedscope\n",
        );
    };
    let flag = |name: &str| incoming.query_param(name).is_some_and(|value| value != "0");
    // A flame graph is drawn by the viewer, which fetches this profile in its own format.
    // The page of the viewer is served here: the address stays one whose parameters can be
    // changed.
    if flag("html") {
        let mut query = form_urlencoded::Serializer::new(String::new());
        for (name, value) in &incoming.query {
            if name != "html" && name != "format" {
                query.append_pair(name, value);
            }
        }
        query.append_pair("format", "speedscope");
        let profile_url = format!("/__lsf/profile?{}", query.finish());
        return speedscope::page(&profile_url);
    }
    let costs = match Costs::default().with(incoming.query_param("cost").unwrap_or_default()) {
        Ok(costs) => costs,
        Err(problem) => return Reply::text(400, format!("cost: {problem}\n")),
    };
    let options = ProfileOptions {
        lines: flag("lines"),
        costs,
    };
    let unit = if flag("points") {
        Unit::Points
    } else {
        Unit::Time
    };
    let runs = incoming
        .query_param("runs")
        .and_then(|runs| runs.parse().ok())
        .unwrap_or(crate::profile::DEFAULT_RUNS);
    let target = match incoming.query_param("section_id") {
        Some(id) => Target::Section(id.to_string()),
        None => Target::Page,
    };
    let (session_id, _) = state.session_id(incoming);
    let (_, store) = state.snapshot(&session_id);
    let request = state.storefront_request(&store, incoming, path, Vec::new());
    let visit = Visit {
        state,
        incoming,
        session_id,
        store,
        request,
    };
    let site = visit.site_for(path);
    let measured = crate::profile::measure(&state.app.renderer, &site, &target, &options, runs);
    match incoming.query_param("format") {
        Some("speedscope" | "json") => Reply::json(
            200,
            &crate::profile::speedscope(&measured.profile, path, unit),
        )
        .header("access-control-allow-origin", "*"),
        _ => Reply::text(
            200,
            crate::profile::text(path, &measured, unit, flag("all")),
        ),
    }
}

/// `GET /__lsf/login?customer=<email|default|none>&return_to=<path>`: logs the browser in as
/// a customer of the data, without a password. A link a person can follow.
fn login(state: &ServerState, incoming: &Incoming) -> Reply {
    let (id, is_new) = state.session_id(incoming);
    let (_, store) = state.snapshot(&id);
    let who = incoming.query_param("customer").unwrap_or("default");
    let customer = match store.customer_named(who) {
        Ok(customer) => customer.map(|customer| customer.id),
        Err(problem) => return Reply::json(422, &json!({"ok": false, "error": problem})),
    };
    state.with_session(&id, |entry| {
        if entry.session.customer_id != customer {
            entry.session.customer_id = customer;
            entry.session.company_location = None;
        }
    });
    let target = super::account::local_path(incoming.query_param("return_to"))
        .unwrap_or_else(|| "/account".to_string());
    let reply = Reply::redirect(&target);
    if is_new {
        reply.header(
            "set-cookie",
            format!("{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Lax"),
        )
    } else {
        reply
    }
}

fn session_json(state: &ServerState, id: &str) -> Json {
    let (session, store) = state.snapshot(id);
    let customer = session
        .customer_id
        .and_then(|customer| store.customer_by_id(customer));
    let company =
        customer.and_then(|customer| customer.company.map(|index| &store.companies[index]));
    let location = customer
        .and_then(|customer| customer.current_location(session.company_location, &store))
        .zip(company)
        .map(|(index, company)| company.locations[index].name.clone());
    json!({
        "id": id,
        "customer": customer.map(|customer| customer.email.clone()),
        "company": company.map(|company| company.name.clone()),
        "company_location": location,
        "country": session.country,
        "cart": {
            "note": session.cart_note,
            "attributes": session.cart_attributes,
            "items": session.cart_lines.iter().map(|line| {
                let mut item = json!({
                    "variant": line.variant_id,
                    "quantity": line.quantity,
                    "properties": line.properties,
                });
                if let Some(selling_plan) = line.selling_plan {
                    item["selling_plan"] = json!(selling_plan);
                }
                item
            }).collect::<Vec<_>>(),
        },
        "custom_data": state.with_session(id, |entry| entry.store.is_some()),
        "throttle": state.with_session(id, |entry| entry.throttle.as_ref().map(Throttle::to_json)),
    })
}

/// `PUT /__lsf/session`: replaces the session's state.
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
    // The session's own throttle: a matter of the server, not of the store data.
    let throttle = match body
        .as_object_mut()
        .and_then(|object| object.remove("throttle"))
    {
        None | Some(Json::Null) => None,
        Some(rules) => match Throttle::from_json(&rules) {
            Ok(throttle) => Some(throttle),
            Err(problem) => {
                let mut diagnostics = Diagnostics::new();
                diagnostics.error("invalid_throttle", OVERLAY_FILE, "/throttle", problem);
                return Reply::json(422, &diagnostics_json(&diagnostics));
            }
        },
    };

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

    let mut session = state.initial_session(&store);
    if let Some(who) = &input.customer {
        match store.customer_named(who) {
            Ok(customer) => {
                let customer = customer.map(|customer| customer.id);
                if session.customer_id != customer {
                    session.customer_id = customer;
                    session.company_location = None;
                }
            }
            Err(problem) => {
                diagnostics
                    .error("unknown_customer", OVERLAY_FILE, "/customer", problem)
                    .hint("add the customer to the store data, or to \"data\".\"customers\" in this request. \"default\" and \"none\" also work");
            }
        }
    }
    if let Some(name) = &input.company_location {
        let customer = session.customer_id.and_then(|id| store.customer_by_id(id));
        match resolve_company_location(&store, customer, name) {
            Ok(location) => session.company_location = Some(location),
            Err(problem) => {
                diagnostics.error(
                    "unknown_location",
                    OVERLAY_FILE,
                    "/company_location",
                    problem,
                );
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
        entry.session = Session {
            cart_token: std::mem::take(&mut entry.session.cart_token),
            ..session
        };
        entry.store = custom_store;
        entry.throttle = throttle;
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

fn dashboard(state: &ServerState, incoming: &Incoming) -> Reply {
    let (id, _) = state.session_id(incoming);
    let (session, store) = state.snapshot(&id);
    let customers = super::account::chooser(&store, session.customer_id, "/__lsf");
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
        .map(|route| {
            let path =
                percent_encoding::utf8_percent_encode(route, percent_encoding::NON_ALPHANUMERIC);
            format!(
                "<li><a href=\"{0}\">{0}</a> <span class=\"muted\">profile: \
                 <a href=\"/__lsf/profile?path={path}\">time</a>, \
                 <a href=\"/__lsf/profile?path={path}&amp;points=1\">points</a>, \
                 <a href=\"/__lsf/profile?path={path}&amp;html=1\">flame graph</a></span></li>",
                escape_html(route),
            )
        })
        .collect();
    Reply::html(
        200,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>lsf · local storefront</title>\
             <style>body{{font:15px/1.5 system-ui,sans-serif;max-width:56rem;margin:3rem auto;padding:0 1rem;color:#1a1a1a}}\
             code,pre{{font:13px ui-monospace,monospace}}pre{{white-space:pre-wrap;background:#f5f5f5;padding:.75rem;border-radius:6px}}\
             .ok{{color:#0a7a3c}}.bad{{color:#b3261e}}ul{{padding-left:1.2rem}}li{{margin:.15rem 0}}\
             dt{{font-weight:600;margin-top:.5rem}}dd{{margin:0}}\
             .muted{{color:#666}}.badge{{font-size:.75rem;border:1px solid #bbb;border-radius:99px;padding:.05rem .5rem}}\
             .people{{list-style:none;padding:0}}</style></head><body>\
             <h1>lsf · local storefront</h1>\
             <dl><dt>Theme</dt><dd><code>{}</code></dd><dt>Store data</dt><dd><code>{}</code></dd></dl>\
             <h2>Store data</h2>{problems}\
             <h2>Customer</h2><p>Choose who this browser is logged in as.</p>{customers}\
             <h2>Pages</h2><ul>{links}</ul>\
             <h2>Control API</h2><ul>\
             <li><code>GET /__lsf/status</code>: this page as JSON</li>\
             <li><code>GET|PUT|DELETE /__lsf/session</code>: read, set or reset the session (cart, customer, per-session data)</li>\
             <li><code>GET /__lsf/login?customer=&lt;email|default|none&gt;</code>: log this browser in as a customer</li>\
             <li><code>GET /__lsf/schema/store</code>: JSON Schema of the data format</li>\
             <li><code>GET /__lsf/profile?path=&lt;path&gt;</code>: what the render of a page spends its time in: sections, blocks, snippets. With <code>points=1</code>, what it would cost a storefront. With <code>html=1</code>, a flame graph</li>\
             <li><code>GET|DELETE /__lsf/timings</code>: how long the templates, the sections and the blocks took to render so far, or forget it</li>\
             <li><code>POST /__lsf/reload</code>: reload the data files</li></ul>\
             </body></html>",
            escape_html(&state.app.theme.files().root().display().to_string()),
            escape_html(&state.app.describe_source()),
        ),
    )
}

//! Store sync through the handlers: what the backend changes through the Admin API is what the
//! storefront and the checkout serve next.
//!
//! The mutations lsf does not simulate yet (`customerCreate`, `metafieldsSet`,
//! `discountAutomaticAppCreate`, `publishablePublish`) are made as they would make their
//! records, through `AdminApi::change_shop`, which is where every change of the default shop goes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsf_admin::domains::{app as app_domain, catalog, content};
use lsf_admin::state::{ShopState, value};
use lsf_core::theme::Revalidate;
use serde_json::{Value as Json, json};

use super::super::reply::Reply;
use super::super::{ServeOptions, ServerState};
use super::{
    ScenarioData, admin_query, admin_request, admin_server, body_json, buyer_details,
    checkout_server_with, copy_directory, fixture_app, get, header, log_in_as, operations, post,
    request, text,
};
use crate::app::App;

/// The fixture theme's server, serving the Admin API and the checkout of the fixture app, which
/// asks for no scope: the backend gets those it needs to change products.
fn checkout_admin_server() -> ServerState {
    let state = checkout_server_with(fixture_app(), &[], false, None, Some(Default::default()));
    let installed = admin_request(
        &state,
        "POST",
        "/__lsf/admin/install",
        r#"{"scopes": "read_products,write_products"}"#,
    );
    assert_eq!(installed.status, 200, "{}", text(&installed));
    state
}

/// Changes the default shop as a mutation would.
fn change(state: &ServerState, change: impl FnOnce(&mut ShopState)) {
    let admin = state.admin.as_ref().expect("the Admin API");
    admin
        .api
        .change_shop(&admin.api.default_shop(), change)
        .expect("the default shop");
}

/// A metafield set on an owner, as `metafieldsSet` makes it.
fn set_metafield(
    shop: &mut ShopState,
    owner: &str,
    owner_type: &str,
    (namespace, key): (&str, &str),
    kind: &str,
    text: &str,
) {
    let id = content::metafield_gid(owner, namespace, key);
    let now = shop.timestamp();
    shop.insert(value::object(json!({
        "__typename": "Metafield",
        "id": id,
        "namespace": namespace,
        "key": key,
        "type": kind,
        "value": text,
        "jsonValue": content::json_value(kind, text),
        "owner": owner,
        "ownerType": owner_type,
        "createdAt": now,
        "updatedAt": now,
    })));
    if let Some(Json::Array(ids)) = shop
        .get_mut(owner)
        .and_then(|record| record.get_mut("metafields"))
        && !ids.contains(&json!(id))
    {
        ids.push(json!(id));
    }
}

fn product_gid(state: &ServerState, handle: &str) -> String {
    catalog::product_gid(
        state
            .loaded()
            .store
            .product_by_handle(handle)
            .expect("a product"),
    )
}

/// `productUpdate` of a product's title.
fn rename(state: &ServerState, handle: &str, title: &str) {
    let id = product_gid(state, handle);
    let updated = admin_query(
        state,
        &format!(
            r#"mutation {{ productUpdate(product: {{id: "{id}", title: "{title}"}}) {{ userErrors {{ message }} }} }}"#
        ),
    );
    assert_eq!(
        updated["data"]["productUpdate"]["userErrors"],
        json!([]),
        "{updated}"
    );
}

/// The `<h1>` of a product page.
fn title_on_page(state: &ServerState, handle: &str, session: &str) -> String {
    let page = text(&get(state, &format!("/products/{handle}"), session));
    page.split_once("<h1>")
        .and_then(|(_, rest)| rest.split_once("</h1>"))
        .map(|(title, _)| title.to_string())
        .unwrap_or_else(|| panic!("no title in {page}"))
}

/// Puts `session` in a cart of `items`, without data of its own, and returns the path of its
/// checkout.
fn cart_checkout(state: &ServerState, session: &str, items: Json) -> String {
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": items}}),
        session,
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    header(&get(state, "/checkout", session), "location")
        .expect("a location")
        .to_string()
}

/// The run of a Function in the checkout's JSON.
fn run_of<'a>(checkout: &'a Json, handle: &str) -> &'a Json {
    checkout["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .find(|run| run["handle"] == handle)
        .unwrap_or_else(|| panic!("no run of {handle}: {checkout}"))
}

fn sync_status(state: &ServerState) -> Json {
    body_json(&admin_request(state, "GET", "/__lsf/admin", ""))["storeSync"].clone()
}

#[test]
fn a_product_the_backend_creates_is_on_the_storefront() {
    let state = admin_server();
    let frontpage = catalog::collection_gid(
        state
            .loaded()
            .store
            .collection_by_handle("frontpage")
            .unwrap(),
    );
    let created = admin_query(
        &state,
        &format!(
            r#"mutation {{ productCreate(product: {{title: "Wool Hat", collectionsToJoin: ["{frontpage}"]}}) {{ product {{ id handle }} userErrors {{ message }} }} }}"#
        ),
    );
    let product = &created["data"]["productCreate"]["product"];
    assert_eq!(product["handle"], "wool-hat", "{created}");
    let id = product["id"].as_str().unwrap().to_string();
    // `productCreate` does not publish it on the Online Store, as on Shopify.
    assert_eq!(get(&state, "/products/wool-hat", "sync").status, 404);

    // `publishablePublish`.
    change(&state, |shop| {
        let now = shop.timestamp();
        shop.get_mut(&id)
            .unwrap()
            .insert("publishedAt".into(), json!(now));
    });
    assert_eq!(title_on_page(&state, "wool-hat", "sync"), "Wool Hat");
    // Last of the collection's five, on its second page of three.
    let collection = text(&get(&state, "/collections/frontpage?page=2", "sync"));
    assert!(
        collection.contains("<h1>Home page (5/5)</h1>") && collection.contains(">Wool Hat</a>"),
        "the collection lists it: {collection}"
    );
    // With the id the Admin API gave it: the backend and the storefront name it alike.
    let loaded = state.loaded();
    let hat = loaded.store.product_by_handle("wool-hat").unwrap();
    assert_eq!(catalog::product_gid(hat), id);
    // The files are as they were.
    assert!(loaded.base.product_by_handle("wool-hat").is_none());
}

#[test]
fn a_product_the_backend_sets_to_draft_leaves_the_storefront() {
    let state = admin_server();
    assert_eq!(title_on_page(&state, "ceramic-mug", "sync"), "Ceramic Mug");
    let id = product_gid(&state, "ceramic-mug");
    let updated = admin_query(
        &state,
        &format!(
            r#"mutation {{ productUpdate(product: {{id: "{id}", status: DRAFT}}) {{ product {{ status }} }} }}"#
        ),
    );
    assert_eq!(
        updated["data"]["productUpdate"]["product"]["status"],
        "DRAFT"
    );
    assert_eq!(get(&state, "/products/ceramic-mug", "sync").status, 404);
    assert!(!text(&get(&state, "/collections/frontpage", "sync")).contains("Ceramic Mug"));
}

#[test]
fn a_customer_the_backend_creates_can_log_in() {
    let state = checkout_admin_server();
    // `customerCreate`.
    change(&state, |shop| {
        let id = shop.new_gid("Customer");
        let now = shop.timestamp();
        let address = "gid://shopify/MailingAddress/77?model_name=CustomerAddress";
        shop.insert(value::object(json!({
            "__typename": "Customer", "id": id, "firstName": "Noa", "lastName": "Lee",
            "email": "noa@example.com", "phone": null, "state": "ENABLED", "tags": ["vip"],
            "taxExempt": false, "emailMarketingConsent": {"marketingState": "SUBSCRIBED"},
            "addresses": [], "addressesV2": [
                {"id": address, "firstName": "Noa", "lastName": "Lee", "address1": "1 Rue de la Paix",
                 "city": "Lyon", "countryCodeV2": "FR", "zip": "69001"}
            ],
            "defaultAddress": {"id": address},
            "metafields": [], "createdAt": now, "updatedAt": now,
        })));
    });

    // The storefront logs them in.
    let login = get(
        &state,
        "/__lsf/login?customer=noa@example.com&return_to=/account",
        "noa",
    );
    assert_eq!(header(&login, "location"), Some("/account"));
    assert_eq!(
        body_json(&get(&state, "/__lsf/session", "noa"))["customer"],
        "noa@example.com"
    );
    // So does the checkout, with their address.
    let path = cart_checkout(
        &state,
        "co-noa",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
    );
    let checkout = log_in_as(&state, &path, "co-noa", json!("noa@example.com"));
    assert_eq!(checkout["buyer"]["customer"]["email"], "noa@example.com");
    assert_eq!(checkout["shippingAddress"]["city"], "Lyon", "{checkout}");
}

/// The fixture theme, copied with a template that prints metafields: `?view=metafields`.
struct MetafieldTheme(PathBuf);

impl MetafieldTheme {
    fn new() -> MetafieldTheme {
        let directory = std::env::temp_dir().join(format!("lsf-sync-theme-{}", std::process::id()));
        std::fs::remove_dir_all(&directory).ok();
        copy_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme"),
            &directory,
        );
        std::fs::write(
            directory.join("templates/product.metafields.liquid"),
            "<p id=\"mf\">care={{ product.metafields.custom.care.value }}|banner={{ shop.metafields.custom.banner }}</p>",
        )
        .unwrap();
        MetafieldTheme(directory)
    }

    fn server(&self) -> ServerState {
        let app = App::open(&self.0, None, Revalidate::Never, &[]).expect("the theme");
        ServerState::new(
            app,
            ServeOptions {
                admin: Some(Default::default()),
                live_reload: false,
                watch: false,
                quiet: true,
                compress: false,
                minify: false,
                throttle: Default::default(),
                timings: false,
                customer: None,
                checkout: None,
            },
        )
        .0
    }
}

impl Drop for MetafieldTheme {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn metafields_the_backend_sets_are_read_by_liquid() {
    let theme = MetafieldTheme::new();
    let state = theme.server();
    let page = |state: &ServerState| {
        text(&get(
            state,
            "/products/organic-cotton-t-shirt?view=metafields",
            "mf",
        ))
    };
    assert!(
        page(&state).contains("care=Machine wash cold. Tumble dry low.|banner=<"),
        "{}",
        page(&state)
    );
    let tee = product_gid(&state, "organic-cotton-t-shirt");
    // `metafieldsSet`, on a product (one the data gives) and on the shop (a new one).
    change(&state, |shop| {
        let shop_gid = shop.shop_gid().unwrap().to_string();
        set_metafield(
            shop,
            &tee,
            "PRODUCT",
            ("custom", "care"),
            "single_line_text_field",
            "Hand wash",
        );
        set_metafield(
            shop,
            &shop_gid,
            "SHOP",
            ("custom", "banner"),
            "single_line_text_field",
            "Spring sale",
        );
    });
    assert!(
        page(&state).contains("care=Hand wash|banner=Spring sale<"),
        "{}",
        page(&state)
    );
}

#[test]
fn the_settings_the_backend_gives_a_validation_are_read_by_its_function() {
    let state = checkout_admin_server();
    let app = lsf_checkout::app::load(&fixture_app()).expect("the fixture app");
    let index = app
        .functions
        .iter()
        .position(|function| function.handle == "quantity-limit")
        .expect("the validation Function");
    let owner = app_domain::owner_gid("Validation", index);
    let namespace = content::admin_namespace(app_domain::app_id(Some(&app)), "$app:config");
    let three = json!([{"variant": "TEE-BLK-M", "quantity": 3}]);
    let path = cart_checkout(&state, "co-limit-before", three.clone());
    let before = body_json(&get(&state, &format!("{path}.json"), "co-limit-before"));
    assert_eq!(before["errors"], json!([]), "{before}");

    // `metafieldsSet` on the validation the app made.
    change(&state, |shop| {
        set_metafield(
            shop,
            &owner,
            "VALIDATION",
            (&namespace, "settings"),
            "json",
            r#"{"maxQuantity":2}"#,
        );
    });
    let path = cart_checkout(&state, "co-limit", three);
    let after = body_json(&get(&state, &format!("{path}.json"), "co-limit"));
    assert_eq!(after["errors"][0]["function"], "quantity-limit", "{after}");
    let input = &run_of(&after, "quantity-limit")["input"];
    assert!(
        input.to_string().contains("maxQuantity"),
        "the input reads the settings: {input}"
    );
}

#[test]
fn an_app_discount_the_backend_creates_runs_its_function_at_the_checkout() {
    let state = checkout_admin_server();
    let app = lsf_checkout::app::load(&fixture_app()).expect("the fixture app");
    let function = app_domain::function_id(Some(&app), "volume-discount");
    let namespace = content::admin_namespace(app_domain::app_id(Some(&app)), "$app:config");
    let settings = r#"{"minimumQuantity":3,"percentage":10}"#;
    // `discountAutomaticAppCreate`, with its metafields.
    change(&state, |shop| {
        let id = shop.new_gid("DiscountAutomaticNode");
        shop.insert(value::object(json!({
            "__typename": "DiscountAutomaticNode",
            "id": id,
            "automaticDiscount": {
                "__typename": "DiscountAutomaticApp",
                "title": "Volume 10",
                "discountId": id,
                "appDiscountType": {"functionId": function, "title": "Volume discount"},
                "discountClasses": ["PRODUCT"],
                "startsAt": "2026-01-01T00:00:00Z",
                "endsAt": null,
                "status": "ACTIVE",
            },
            "metafields": [],
        })));
        set_metafield(
            shop,
            &id,
            "DISCOUNT",
            (&namespace, "settings"),
            "json",
            settings,
        );
    });
    let path = cart_checkout(
        &state,
        "co-volume",
        json!([{"variant": "TEE-BLK-M", "quantity": 3}]),
    );
    // The checkout's configuration, which comes from the store, has it.
    let config = crate::checkout::config_from_store(&state.loaded().store);
    let discount = config
        .discounts
        .iter()
        .find(|discount| discount.title == "Volume 10")
        .expect("the discount");
    assert_eq!(discount.function, "volume-discount");

    let checkout = body_json(&get(&state, &format!("{path}.json"), "co-volume"));
    assert_eq!(checkout["discounts"][0]["function"], "volume-discount");
    let line = &checkout["lines"][0];
    let subtotal = line["subtotal"]["amount"].as_f64().unwrap();
    let total = line["total"]["amount"].as_f64().unwrap();
    assert!((total - subtotal * 0.9).abs() < 0.01, "{subtotal} {total}");
    // Its Function read the metafields the backend gave it.
    let input = &run_of(&checkout, "volume-discount")["input"];
    assert_eq!(input["discount"]["metafield"]["value"], settings, "{input}");
}

#[test]
fn a_session_with_its_own_data_sees_what_the_backend_changes() {
    let state = admin_server();
    let own_data = |session: &str, title: &str| {
        let reply = state.dispatch(&request(
            "PUT",
            "/__lsf/session",
            json!({"data": {"products": [{"title": title, "price": 1500}]}}),
            session,
        ));
        assert_eq!(reply.status, 200, "{}", text(&reply));
    };
    own_data("own", "Session Scarf");
    rename(&state, "ceramic-mug", "Stoneware Mug");

    // Its data, on top of what the backend changed.
    assert_eq!(title_on_page(&state, "ceramic-mug", "own"), "Stoneware Mug");
    assert_eq!(
        title_on_page(&state, "session-scarf", "own"),
        "Session Scarf"
    );
    // Another session sees the change, not the data.
    assert_eq!(
        title_on_page(&state, "ceramic-mug", "other"),
        "Stoneware Mug"
    );
    assert_eq!(get(&state, "/products/session-scarf", "other").status, 404);
    // Data set after the change goes on top of it too.
    own_data("late", "Late Scarf");
    assert_eq!(
        title_on_page(&state, "ceramic-mug", "late"),
        "Stoneware Mug"
    );
    assert_eq!(title_on_page(&state, "late-scarf", "late"), "Late Scarf");
    // And again after the next change.
    rename(&state, "ceramic-mug", "Clay Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "own"), "Clay Mug");
    assert_eq!(
        title_on_page(&state, "session-scarf", "own"),
        "Session Scarf"
    );
}

#[test]
fn open_pages_reload_when_the_backend_changes_the_store() {
    let state = admin_server();
    let token = state.observe_changes();
    // A query changes nothing.
    admin_query(&state, "{ shop { name } }");
    assert_eq!(state.observe_changes(), token);

    rename(&state, "ceramic-mug", "Stoneware Mug");
    let changed = state.observe_changes();
    assert_ne!(changed, token);
    // The store was built before the token changed: the page that reloads finds it.
    assert_eq!(
        state
            .loaded()
            .store
            .product_by_handle("ceramic-mug")
            .unwrap()
            .title,
        "Stoneware Mug"
    );
    assert_eq!(state.observe_changes(), changed);
}

#[test]
fn a_change_of_the_files_and_a_reset_bring_the_files_back() {
    // `--watch` over a data directory of the demo store.
    let data = ScenarioData::new("sync-watch", &[]);
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, Some(&data.0), Revalidate::Never, &[]).expect("the theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            admin: Some(Default::default()),
            live_reload: false,
            watch: true,
            quiet: true,
            compress: false,
            minify: false,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    rename(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Stoneware Mug");
    assert_eq!(sync_status(&state)["generation"], 1);

    // The files change: the shops are seeded again, and the store is the new files.
    let pages = data.0.join("pages.json");
    let content = std::fs::read_to_string(&pages).unwrap();
    std::fs::write(&pages, format!("{content}\n")).unwrap();
    state.observe_changes();
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Ceramic Mug");
    let loaded = state.loaded();
    assert!(Arc::ptr_eq(&loaded.store, &loaded.base));
    assert!(loaded.synced.overlay.is_none());
    assert_eq!(sync_status(&state)["overlay"], Json::Null);

    // `DELETE /__lsf/admin` does as much.
    rename(&state, "ceramic-mug", "Clay Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Clay Mug");
    assert_eq!(
        admin_request(&state, "DELETE", "/__lsf/admin", "").status,
        200
    );
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Ceramic Mug");
    let loaded = state.loaded();
    assert!(Arc::ptr_eq(&loaded.store, &loaded.base));
}

#[test]
fn an_order_placed_at_the_checkout_does_not_build_the_store_again() {
    let state = checkout_admin_server();
    let admin = state.admin.as_ref().unwrap();
    // The backend changed the store first: the order must not undo it, nor build it again.
    rename(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(get(&state, "/", "co-order").status, 200);
    let before = state.loaded();
    assert_eq!(before.synced.generation, 1);
    let changes = admin.api.store_changes();

    let path = cart_checkout(
        &state,
        "co-order",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
    );
    // A customer of the data, whose orders and amount spent change in the Admin API.
    log_in_as(&state, &path, "co-order", json!("jane.doe@example.com"));
    assert_eq!(
        operations(&state, &path, "co-order", buyer_details()).status,
        200
    );
    let reply: Reply = post(&state, &format!("{path}/complete"), json!({}), "co-order");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    // The order is in the default shop.
    assert!(admin.api.store_changes() > changes);

    // The next page asks for the overlay again, finds the same one, and keeps the store.
    assert_eq!(
        title_on_page(&state, "ceramic-mug", "co-order"),
        "Stoneware Mug"
    );
    let after = state.loaded();
    assert!(Arc::ptr_eq(&before.store, &after.store));
    assert_eq!(after.synced.generation, 1);
    let status = sync_status(&state);
    assert_eq!(
        (&status["generation"], &status["pending"]),
        (&json!(1), &json!(false)),
        "{status}"
    );
}

#[test]
fn what_the_store_data_refuses_of_a_change_is_told_and_the_rest_is_served() {
    let state = admin_server();
    // A customer whose email the store data refuses: `customerCreate` checks it, a record made
    // another way may not.
    change(&state, |shop| {
        let id = shop.new_gid("Customer");
        shop.insert(value::object(json!({
            "__typename": "Customer", "id": id, "email": "not an email", "firstName": "Kim",
            "state": "DISABLED", "tags": [], "addressesV2": [], "metafields": [],
        })));
    });
    rename(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "bad"), "Stoneware Mug");
    let loaded = state.loaded();
    assert!(
        !loaded.synced.diagnostics.is_empty(),
        "the store data says something of the overlay"
    );
    let told = &sync_status(&state)["built"]["diagnostics"];
    assert_eq!(
        told.as_array().map(Vec::len),
        Some(loaded.synced.diagnostics.items.len()),
        "{told}"
    );
    assert_eq!(told[0]["file"], lsf_admin::sync::OVERLAY_NAME, "{told}");
    assert_eq!(told[0]["code"], "invalid_email", "{told}");
}

#[test]
fn a_burst_of_mutations_is_built_once_and_pages_see_whole_stores() {
    let state = admin_server();
    // Two products renamed by one request: a page sees both changes or neither.
    let tee = product_gid(&state, "organic-cotton-t-shirt");
    let mug = product_gid(&state, "ceramic-mug");
    let round = |n: usize| {
        let updated = admin_query(
            &state,
            &format!(
                r#"mutation {{ a: productUpdate(product: {{id: "{tee}", title: "Tee {n}"}}) {{ userErrors {{ message }} }} b: productUpdate(product: {{id: "{mug}", title: "Mug {n}"}}) {{ userErrors {{ message }} }} }}"#
            ),
        );
        assert_eq!(updated["data"]["b"]["userErrors"], json!([]), "{updated}");
    };
    for n in 0..5 {
        round(n);
    }
    // Five requests, one overlay, one build: by the page that follows them.
    assert_eq!(title_on_page(&state, "ceramic-mug", "burst"), "Mug 4");
    let status = sync_status(&state);
    assert_eq!(status["generation"], 1, "{status}");
    assert_eq!(status["built"]["generation"], 1, "{status}");

    // Pages rendered while the backend goes on.
    let rounds = |page: &str| -> Option<(String, String)> {
        let number = |name: &str| {
            let start = page.find(&format!(">{name} "))? + name.len() + 2;
            let end = start + page[start..].find('<')?;
            Some(page[start..end].to_string())
        };
        Some((number("Tee")?, number("Mug")?))
    };
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for n in 5..30 {
                round(n);
            }
        });
        for reader in 0..3 {
            let state = &state;
            scope.spawn(move || {
                for _ in 0..30 {
                    let page = text(&get(state, "/collections/frontpage", &format!("r{reader}")));
                    let (tee, mug) = rounds(&page).unwrap_or_else(|| panic!("{page}"));
                    assert_eq!(tee, mug, "a page saw half a request's changes");
                }
            });
        }
    });
    assert_eq!(title_on_page(&state, "ceramic-mug", "burst"), "Mug 29");
}

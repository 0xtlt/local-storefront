//! The parts a server runs (`--only`): which routes each combination answers, and the `404` of
//! a part that is off, with its `X-LSF-Reason`.

use std::path::Path;

use lsf_core::theme::Revalidate;
use serde_json::json;

use super::super::admin::AdminSettings;
use super::super::parts::Parts;
use super::super::{ServeOptions, ServerState};
use super::{
    admin_request, body_json, checkout_server, checkout_server_with, fixture_app, get, header,
    owned, retitle, server, text, title_on_page,
};
use crate::app::App;

/// A server of the parts `parts` with the fixture app: the theme of the fixtures, the checkout
/// of the app, its Admin API.
fn server_of(parts: Parts) -> ServerState {
    let admin = parts.admin.then(|| AdminSettings {
        app_dir: Some(fixture_app()),
        ..Default::default()
    });
    if parts.checkout {
        return checkout_server_with(fixture_app(), &[], !parts.theme, None, admin);
    }
    let app = if parts.theme {
        let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
        App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme")
    } else {
        App::open_without_theme(Path::new("."), None, &[]).expect("no theme")
    };
    ServerState::new(
        app,
        ServeOptions {
            polaris: Default::default(),
            admin,
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

const THEME: Parts = Parts {
    theme: true,
    checkout: false,
    admin: false,
};
const CHECKOUT: Parts = Parts {
    theme: false,
    checkout: true,
    admin: false,
};
const ADMIN: Parts = Parts {
    theme: false,
    checkout: false,
    admin: true,
};

fn and(a: Parts, b: Parts) -> Parts {
    Parts {
        theme: a.theme || b.theme,
        checkout: a.checkout || b.checkout,
        admin: a.admin || b.admin,
    }
}

/// A route of each part, and one the theme and the checkout both answer.
const THEME_ROUTE: &str = "/collections/all";
const CHECKOUT_ROUTE: &str = "/__lsf/checkouts";
const ADMIN_ROUTE: &str = "/__lsf/admin";

/// Asserts that `path` answers the `404` of `part`, whose reason names the `--only` given.
fn assert_off(state: &ServerState, path: &str, part: &str, only: &str) {
    let reply = get(state, path, "parts");
    assert_eq!(reply.status, 404, "{path}: {}", text(&reply));
    let reason = header(&reply, "x-lsf-reason").unwrap_or_else(|| panic!("{path}: no reason"));
    assert!(
        reason.starts_with(&format!("the {part} part is not served (serving: ")),
        "{path}: {reason}"
    );
    assert!(
        reason.contains(&format!("--only {only}`")),
        "{path}: {reason}"
    );
    assert!(text(&reply).contains(reason), "{path}: {}", text(&reply));
}

#[test]
fn each_combination_answers_the_routes_of_its_parts_and_a_reasoned_404_for_the_others() {
    let combinations = [
        THEME,
        CHECKOUT,
        ADMIN,
        and(THEME, CHECKOUT),
        and(THEME, ADMIN),
        and(CHECKOUT, ADMIN),
        Parts::ALL,
    ];
    for parts in combinations {
        let state = server_of(parts);
        assert_eq!(state.parts(), parts);
        let status = body_json(&get(&state, "/__lsf/status", "parts"));
        assert_eq!(status["parts"], parts.to_json(), "{parts:?}");
        let routes = [
            (
                THEME_ROUTE,
                parts.theme,
                "theme",
                parts.with(super::super::parts::Part::Theme),
            ),
            (
                CHECKOUT_ROUTE,
                parts.checkout,
                "checkout",
                parts.with(super::super::parts::Part::Checkout),
            ),
            (
                ADMIN_ROUTE,
                parts.admin,
                "admin",
                parts.with(super::super::parts::Part::Admin),
            ),
        ];
        for (path, on, part, wanted) in routes {
            if on {
                let reply = get(&state, path, "parts");
                assert_eq!(reply.status, 200, "{parts:?} {path}: {}", text(&reply));
                assert_eq!(header(&reply, "x-lsf-reason"), None, "{parts:?} {path}");
            } else {
                assert_off(&state, path, part, &wanted.only());
            }
        }
        if parts.admin {
            let admin = body_json(&get(&state, ADMIN_ROUTE, "parts"));
            assert_eq!(admin["parts"], parts.to_json(), "{parts:?}");
            // The app of `--app`, with or without its checkout.
            assert_eq!(
                admin["app"]["clientId"], "00000000000000000000000000000000",
                "{parts:?}"
            );
        }
    }
}

#[test]
fn every_route_of_the_admin_part_is_off_without_it() {
    let state = server();
    for path in [
        "/admin",
        "/admin/api/2026-10/graphql.json",
        "/admin/oauth/access_token",
        "/admin/apps/checkout-fixtures",
        "/__lsf/admin/shops",
        "/__lsf/webhooks",
        "/__lsf/webhooks/trigger",
        "/__lsf/polaris",
        "/__lsf/cdn/shopifycloud/polaris-2.js",
        "/1234/api/2026-10/graphql.json",
    ] {
        // Without an app, the `--only` that serves it needs one.
        assert_off(&state, path, "admin", "theme,admin");
        let reason = header(&get(&state, path, "parts"), "x-lsf-reason")
            .unwrap()
            .to_string();
        assert!(reason.contains("`serve --app <dir> --only"), "{reason}");
    }
    // A theme without an app: the theme's own stand-ins for the checkout and the account.
    for path in ["/checkout", "/account"] {
        assert_eq!(get(&state, path, "parts").status, 200, "{path}");
    }
    // The control API of the checkout is the checkout's.
    for path in [
        "/__lsf/checkout",
        "/__lsf/checkouts/check",
        "/__lsf/extensions/banner/dist/banner.js",
    ] {
        assert_off(&state, path, "checkout", "theme,checkout");
    }
}

#[test]
fn without_a_theme_the_pages_and_the_profile_are_off_and_the_checkout_keeps_its_own() {
    let state = checkout_server(&[], true);
    for path in [
        "/products/anything",
        "/cart.js",
        "/search?q=tee",
        "/__lsf/profile?path=/",
    ] {
        assert_off(&state, path, "theme", "theme,checkout");
    }
    // The home page, the checkout and the customer account are the checkout's.
    assert_eq!(get(&state, "/", "parts").status, 200);
    assert_eq!(get(&state, "/checkout", "parts").status, 302);
    assert_eq!(get(&state, "/account", "parts").status, 200);

    // Neither the theme nor the checkout: the checkout's pages are the checkout's.
    let state = server_of(ADMIN);
    for path in ["/checkout", "/fr/checkout", "/checkouts/cn/token"] {
        assert_off(&state, path, "checkout", "checkout,admin");
    }
    for path in ["/", "/account", "/apps/reviews"] {
        assert_off(&state, path, "theme", "theme,admin");
    }
}

#[test]
fn the_dashboard_says_which_parts_run() {
    let page = text(&get(&server(), "/__lsf", "parts"));
    assert!(page.contains("<dd data-lsf-parts>theme "), "{page}");
    assert!(page.contains("<h2>Pages</h2>"));
    let page = text(&get(&server_of(and(CHECKOUT, ADMIN)), "/__lsf", "parts"));
    assert!(
        page.contains("<dd data-lsf-parts>checkout, admin "),
        "{page}"
    );
    // Without a theme, no page of it to list.
    assert!(!page.contains("<h2>Pages</h2>"), "{page}");
}

#[test]
fn the_theme_and_the_admin_api_without_the_checkout_show_what_the_backend_writes() {
    let state = server_of(and(THEME, ADMIN));
    let installed = admin_request(
        &state,
        "POST",
        "/__lsf/admin/install",
        r#"{"scopes": "write_products"}"#,
    );
    assert_eq!(installed.status, 200, "{}", text(&installed));
    retitle(&state, "ceramic-mug", "Clay Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "parts"), "Clay Mug");
    // An uninstall, with no checkout to stop: the storefront is served all the same.
    let uninstalled = admin_request(&state, "POST", "/__lsf/admin/uninstall", "{}");
    assert_eq!(uninstalled.status, 200, "{}", text(&uninstalled));
    assert!(state.app_uninstalled());
    assert_eq!(get(&state, "/products/ceramic-mug", "parts").status, 200);
}

#[test]
fn the_checkout_without_the_admin_api_runs_on_the_store_data() {
    let state = server_of(CHECKOUT);
    assert!(state.admin.is_none());
    assert!(!state.app_uninstalled());
    let put = state.dispatch(&super::request(
        "PUT",
        "/__lsf/session",
        json!({
            "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]},
            // The owner of the payment Function, as the store data declares it.
            "data": owned(&["payment-rules"]),
        }),
        "parts",
    ));
    assert_eq!(put.status, 200, "{}", text(&put));
    let location = header(&get(&state, "/checkout", "parts"), "location")
        .expect("a checkout")
        .to_string();
    let checkout = body_json(&get(&state, &format!("{location}.json"), "parts"));
    // The owner of the store data runs its Function: no Admin API to have the app uninstalled.
    let runs = checkout["runs"].as_array().expect("runs");
    assert!(
        runs.iter()
            .any(|run| run["handle"] == "payment-rules" && run["error"].is_null()),
        "{checkout}"
    );
}

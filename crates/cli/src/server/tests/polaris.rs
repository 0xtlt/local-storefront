//! `/__lsf/cdn/<path>`, where an embedded app loads Polaris from: Shopify's CDN through a
//! redirect (`online`), or lsf's copy (`offline`); and the mode, set by `--polaris` and changed
//! by `/__lsf/polaris`.

use std::path::Path;

use lsf_core::theme::Revalidate;
use serde_json::json;

use super::super::polaris::Mode;
use super::super::{ServeOptions, ServerState};
use super::{body_json, get, header, request, text};
use crate::app::App;

/// The fixture theme's server with the Admin API, whose part Polaris is, started in `mode`
/// (`--polaris`).
fn server_in(mode: Mode) -> ServerState {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme");
    ServerState::new(
        app,
        ServeOptions {
            polaris: mode,
            admin: Some(Default::default()),
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    )
    .0
}

fn set_mode(state: &ServerState, body: serde_json::Value) -> super::Reply {
    state.dispatch(&request("PUT", "/__lsf/polaris", body, "control"))
}

/// The copy of Polaris 2 as the CDN serves it, from the repository.
fn vendored(path: &str) -> String {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/shopify-cdn")
        .join(path);
    std::fs::read_to_string(file).expect(path)
}

#[test]
fn online_the_paths_redirect_to_shopifys_cdn_with_their_query() {
    let state = server_in(Mode::Online);
    assert_eq!(state.polaris.mode(), Mode::Online, "online by default");
    for (path, location) in [
        (
            "/__lsf/cdn/shopifycloud/polaris-2.js",
            "https://cdn.shopify.com/shopifycloud/polaris-2.js",
        ),
        (
            "/__lsf/cdn/shopifycloud/polaris.js?v=3&x=a%20b",
            "https://cdn.shopify.com/shopifycloud/polaris.js?v=3&x=a%20b",
        ),
        // Any path of the CDN, those lsf has no copy of too.
        (
            "/__lsf/cdn/shopifycloud/app-bridge.js",
            "https://cdn.shopify.com/shopifycloud/app-bridge.js",
        ),
        (
            "/__lsf/cdn/static/fonts/inter/v4/styles.css",
            "https://cdn.shopify.com/static/fonts/inter/v4/styles.css",
        ),
        // A decoded path is encoded again: nothing breaks out of the CDN's URL.
        (
            "/__lsf/cdn/a b\"<x>#.js",
            "https://cdn.shopify.com/a%20b%22%3Cx%3E%23.js",
        ),
    ] {
        let reply = get(&state, path, "online");
        assert_eq!(reply.status, 302, "{path}");
        assert_eq!(header(&reply, "location"), Some(location), "{path}");
        assert_eq!(header(&reply, "cache-control"), Some("no-store"), "{path}");
        assert_eq!(header(&reply, "access-control-allow-origin"), Some("*"));
    }
}

#[test]
fn offline_the_copy_is_served_with_the_cdns_urls_pointed_at_lsf() {
    let state = server_in(Mode::Offline);
    let reply = get(&state, "/__lsf/cdn/shopifycloud/polaris-2.js", "offline");
    assert_eq!(reply.status, 200);
    assert_eq!(
        header(&reply, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(header(&reply, "access-control-allow-origin"), Some("*"));
    assert_eq!(
        header(&reply, "cross-origin-resource-policy"),
        Some("cross-origin")
    );
    assert_eq!(header(&reply, "cache-control"), Some("no-cache"));
    let served = text(&reply);
    let original = vendored("shopifycloud/polaris-2.0.js");
    assert!(original.contains("https://cdn.shopify.com/shopifycloud/admin-ui-foundations/icons/"));
    assert!(
        !served.contains("cdn.shopify.com"),
        "every URL of the CDN is lsf's"
    );
    // The request came to `shop.test` (the tests' host): the same bytes, the CDN replaced.
    assert_eq!(
        served,
        original.replace("https://cdn.shopify.com/", "http://shop.test/__lsf/cdn/")
    );
    for part in [
        "\"http://shop.test/__lsf/cdn/shopifycloud/admin-ui-foundations/icons/\"",
        // The check of an icon's URL before fetching it: the same prefix, so it still passes.
        "startsWith(\"http://shop.test/__lsf/cdn/shopifycloud/admin-ui-foundations/\")",
        "\"http://shop.test/__lsf/cdn/static/fonts/inter/v4/styles.css\"",
        "\"http://shop.test/__lsf/cdn/static/fonts/shopifyinter/v0/styles.css\"",
    ] {
        assert!(served.contains(part), "{part}");
    }

    // Behind a proxy that says `https`, the URLs are `https`.
    let mut behind = request(
        "GET",
        "/__lsf/cdn/shopifycloud/polaris.js",
        json!({}),
        "offline",
    );
    behind
        .headers
        .insert("x-forwarded-proto".to_string(), "https".to_string());
    let served = text(&state.dispatch(&behind));
    assert!(
        served.contains("\"https://shop.test/__lsf/cdn/shopifycloud/admin-ui-foundations/icons/\"")
    );
    assert!(served.contains("version:"), "Polaris 1");
    assert!(served.contains("\"1.1.0\""));
}

#[test]
fn offline_the_aliases_are_the_copies_the_cdn_served_at_them() {
    let state = server_in(Mode::Offline);
    let polaris_1 = text(&get(&state, "/__lsf/cdn/shopifycloud/polaris-1.1.js", "a"));
    let polaris_2 = text(&get(&state, "/__lsf/cdn/shopifycloud/polaris-2.0.js", "a"));
    assert!(polaris_1.contains("\"1.1.0\"") && polaris_2.contains("\"2.0.0\""));
    for (alias, copy) in [
        ("polaris.js", &polaris_1),
        ("polaris-1.js", &polaris_1),
        ("polaris-2.js", &polaris_2),
        // The query does not choose a file.
        ("polaris-2.js?v=1", &polaris_2),
    ] {
        let reply = get(&state, &format!("/__lsf/cdn/shopifycloud/{alias}"), "a");
        assert_eq!(reply.status, 200, "{alias}");
        assert!(text(&reply) == **copy, "{alias}");
    }
}

#[test]
fn offline_icons_fonts_and_stylesheets_have_their_types_and_caching() {
    let state = server_in(Mode::Offline);
    let icon = get(
        &state,
        "/__lsf/cdn/shopifycloud/admin-ui-foundations/icons/f359e.svg",
        "a",
    );
    assert_eq!(icon.status, 200);
    // Polaris refuses an icon of another type.
    assert_eq!(header(&icon, "content-type"), Some("image/svg+xml"));
    assert_eq!(
        header(&icon, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(header(&icon, "access-control-allow-origin"), Some("*"));
    assert!(text(&icon).starts_with("<svg"));

    let css = get(&state, "/__lsf/cdn/static/fonts/inter/v4/styles.css", "a");
    assert_eq!(css.status, 200);
    assert_eq!(
        header(&css, "content-type"),
        Some("text/css; charset=utf-8")
    );
    assert_eq!(header(&css, "cache-control"), Some("no-cache"));
    // Its fonts are relative: they come from lsf with it.
    let font_name = text(&css)
        .split("url('")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .expect("a font")
        .to_string();
    let font = get(
        &state,
        &format!("/__lsf/cdn/static/fonts/inter/v4/{font_name}"),
        "a",
    );
    assert_eq!(font.status, 200, "{font_name}");
    assert_eq!(header(&font, "content-type"), Some("font/woff2"));
    assert_eq!(header(&font, "access-control-allow-origin"), Some("*"));
    assert_eq!(&font.body[..4], b"wOF2");
}

#[test]
fn offline_a_path_lsf_has_no_copy_of_is_a_404_that_says_so() {
    let state = server_in(Mode::Offline);
    for path in [
        "shopifycloud/polaris-3.js",
        "shopifycloud/polaris-1.0.js",
        "shopifycloud/app-bridge.js",
        "shopifycloud/admin-ui-foundations/icons/00000.svg",
        "README.md",
        "SHA256SUMS",
        "../README.md",
        "",
    ] {
        let reply = get(&state, &format!("/__lsf/cdn/{path}"), "a");
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(
            header(&reply, "content-type"),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(header(&reply, "x-content-type-options"), Some("nosniff"));
        let body = text(&reply);
        assert!(
            body.starts_with(&format!(
                "lsf has no offline copy of https://cdn.shopify.com/{path}\n"
            )),
            "{body}"
        );
        assert!(
            body.contains("2.0.0 (polaris-2.js, polaris-2.0.js)"),
            "{body}"
        );
        assert!(body.contains("--polaris online"), "{body}");
    }
}

#[test]
fn only_get_head_and_options_are_answered() {
    for mode in [Mode::Online, Mode::Offline] {
        let state = server_in(mode);
        let head = state.dispatch(&request(
            "HEAD",
            "/__lsf/cdn/shopifycloud/polaris-2.js",
            json!({}),
            "a",
        ));
        assert_eq!(head.status, if mode == Mode::Online { 302 } else { 200 });
        let post = state.dispatch(&request(
            "POST",
            "/__lsf/cdn/shopifycloud/polaris-2.js",
            json!({}),
            "a",
        ));
        assert_eq!(post.status, 405);
        assert_eq!(header(&post, "allow"), Some("GET, HEAD, OPTIONS"));
    }
}

/// The preflight a browser sends before a `fetch()` from an app on another port with the headers
/// lsf's App Bridge adds to its own requests (`Authorization`, `X-Requested-With`).
pub(super) fn preflight(path: &str) -> super::Incoming {
    let mut incoming = request("OPTIONS", path, json!({}), "a");
    for (name, value) in [
        ("origin", "http://localhost:3000"),
        ("access-control-request-method", "GET"),
        (
            "access-control-request-headers",
            "authorization,x-requested-with",
        ),
    ] {
        incoming.headers.insert(name.to_string(), value.to_string());
    }
    incoming
}

#[test]
fn a_preflight_of_another_origin_is_allowed_with_the_headers_it_asks_for() {
    for mode in [Mode::Online, Mode::Offline] {
        let state = server_in(mode);
        for path in [
            "/__lsf/cdn/shopifycloud/admin-ui-foundations/icons/f359e.svg",
            "/__lsf/cdn/shopifycloud/polaris-2.js",
            "/__lsf/polaris",
            "/cdn/shop/t/1/assets/base.css",
        ] {
            let reply = state.dispatch(&preflight(path));
            assert_eq!(reply.status, 204, "{path} ({mode:?})");
            assert!(reply.body.is_empty(), "{path}");
            assert_eq!(header(&reply, "access-control-allow-origin"), Some("*"));
            assert_eq!(
                header(&reply, "access-control-allow-methods"),
                Some("GET, HEAD, OPTIONS"),
                "{path}"
            );
            assert_eq!(
                header(&reply, "access-control-allow-headers"),
                Some("authorization,x-requested-with"),
                "{path}"
            );
            assert_eq!(
                header(&reply, "access-control-max-age"),
                Some("86400"),
                "{path}"
            );
        }
    }
    // Headers that are not header names are not echoed.
    let state = server_in(Mode::Online);
    let mut odd = preflight("/__lsf/cdn/shopifycloud/polaris-2.js");
    odd.headers.insert(
        "access-control-request-headers".to_string(),
        "x\r\nset-cookie: a=b".to_string(),
    );
    let reply = state.dispatch(&odd);
    assert_eq!(reply.status, 204);
    assert_eq!(header(&reply, "access-control-allow-headers"), None);
}

#[test]
fn the_control_api_reads_sets_and_resets_the_mode() {
    let state = server_in(Mode::Online);
    let status = body_json(&get(&state, "/__lsf/polaris", "control"));
    assert_eq!(status["mode"], "online");
    assert_eq!(status["default"], "online");
    assert_eq!(status["cdn"], "/__lsf/cdn/");
    assert_eq!(status["scripts"][0]["version"], "2.0.0");
    assert_eq!(status["scripts"][0]["path"], "shopifycloud/polaris-2.0.js");
    assert_eq!(
        status["scripts"][1]["aliases"],
        json!(["shopifycloud/polaris.js", "shopifycloud/polaris-1.js"])
    );
    assert!(status["files"].as_u64().unwrap() > 800);

    let path = "/__lsf/cdn/shopifycloud/polaris-2.js";
    assert_eq!(get(&state, path, "a").status, 302);
    let set = set_mode(&state, json!({ "mode": "offline" }));
    assert_eq!(set.status, 200);
    assert_eq!(body_json(&set), json!({ "ok": true, "mode": "offline" }));
    assert_eq!(header(&set, "access-control-allow-origin"), Some("*"));
    assert_eq!(get(&state, path, "a").status, 200, "at once");
    assert_eq!(
        body_json(&get(&state, "/__lsf/polaris", "control"))["mode"],
        "offline"
    );

    // A bad mode changes nothing.
    for body in [json!({ "mode": "cdn" }), json!({}), json!({ "mode": 1 })] {
        let refused = set_mode(&state, body.clone());
        assert_eq!(refused.status, 422, "{body}");
        assert_eq!(body_json(&refused)["ok"], false);
        assert!(
            body_json(&refused)["error"]
                .as_str()
                .unwrap()
                .contains("\"online\" or \"offline\""),
            "{body}"
        );
    }
    assert_eq!(state.polaris.mode(), Mode::Offline);

    // `POST` and a query do as well; `DELETE` goes back to the mode lsf started in.
    let online = state.dispatch(&request(
        "POST",
        "/__lsf/polaris?mode=online",
        json!({}),
        "control",
    ));
    assert_eq!(body_json(&online)["mode"], "online");
    assert_eq!(get(&state, path, "a").status, 302);
    set_mode(&state, json!({ "mode": "offline" }));
    let reset = state.dispatch(&request("DELETE", "/__lsf/polaris", json!({}), "control"));
    assert_eq!(body_json(&reset), json!({ "ok": true, "mode": "online" }));
    assert_eq!(get(&state, path, "a").status, 302);

    let other = state.dispatch(&request("PATCH", "/__lsf/polaris", json!({}), "control"));
    assert_eq!(other.status, 405);
}

#[test]
fn polaris_offline_starts_offline_and_resets_to_it() {
    let state = server_in(Mode::Offline);
    let status = body_json(&get(&state, "/__lsf/polaris", "control"));
    assert_eq!(
        (status["mode"].as_str(), status["default"].as_str()),
        (Some("offline"), Some("offline"))
    );
    assert_eq!(
        get(&state, "/__lsf/cdn/shopifycloud/polaris-2.js", "a").status,
        200
    );
    set_mode(&state, json!({ "mode": "online" }));
    let reset = state.dispatch(&request("DELETE", "/__lsf/polaris", json!({}), "control"));
    assert_eq!(body_json(&reset)["mode"], "offline");
    // The banner says it.
    let line = super::super::polaris::banner(&state, "http://127.0.0.1:9292").expect("a line");
    assert_eq!(
        line,
        "polaris: offline: http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris-2.js (lsf's copy of Polaris 2.0.0 and 1.1.0)"
    );
    assert!(super::super::polaris::banner(&server_in(Mode::Online), "http://x").is_none());
}

#[test]
fn the_control_apis_resets_keep_the_mode() {
    // Like a stubbed URL, the mode is the server's, not the shops' or the sessions'.
    let state = super::fixture_server(Some(Default::default())).0;
    set_mode(&state, json!({ "mode": "offline" }));
    let admin = state.dispatch(&request("DELETE", "/__lsf/admin", json!({}), "control"));
    assert_eq!(admin.status, 200, "{}", text(&admin));
    let sessions = state.dispatch(&request("DELETE", "/__lsf/sessions", json!({}), "control"));
    assert_eq!(sessions.status, 200);
    assert_eq!(state.polaris.mode(), Mode::Offline);
}

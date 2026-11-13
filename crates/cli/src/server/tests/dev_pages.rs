//! What a theme developer sees while serving a theme: live reload where it belongs, the errors
//! of the page and of the store data, and why a request finds nothing.

use std::path::{Path, PathBuf};

use lsf_core::theme::Revalidate;

use super::super::{ServeOptions, ServerState};
use super::{get, header, text};
use crate::app::App;

/// A theme and its store data written in a temporary directory: `files` are `(path, content)`
/// under the theme, the data under `shopify-local/`.
pub(super) fn theme_dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lsf-dev-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, content) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    root
}

pub(super) fn serve(root: &Path, live_reload: bool) -> ServerState {
    let app = App::open(root, None, Revalidate::Never, &[]).expect("theme");
    ServerState::new(
        app,
        ServeOptions {
            polaris: Default::default(),
            admin: None,
            live_reload,
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

const LAYOUT: (&str, &str) = (
    "layout/theme.liquid",
    "<!doctype html><html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
);

#[test]
fn live_reload_leaves_what_is_not_an_html_page_alone() {
    let root = theme_dir(
        "json-view",
        &[
            LAYOUT,
            (
                "templates/product.json-data.liquid",
                "{% layout none %}{\"t\": {{ product.title | json }}}",
            ),
            ("templates/product.liquid", "<h1>{{ product.title }}</h1>"),
            (
                "shopify-local/products/mug.json",
                r#"{"title": "Mug", "price": "10.00"}"#,
            ),
        ],
    );
    let state = serve(&root, true);
    let json = get(&state, "/products/mug?view=json-data", "view");
    assert_eq!(json.status, 200);
    assert_eq!(text(&json), "{\"t\": \"Mug\"}");
    // A page with a body gets the script, and the platform's markup.
    let page = text(&get(&state, "/products/mug", "view"));
    assert!(page.contains("<script data-lsf-live-reload>"), "{page}");
    assert!(page.find("data-lsf-live-reload") < page.find("</body>"));
    // A clean page shows no error panel.
    assert!(!page.contains("data-lsf-errors"), "{page}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn live_reload_shows_the_errors_of_the_data_and_of_the_page_in_it() {
    let root = theme_dir(
        "overlay",
        &[
            LAYOUT,
            ("templates/index.json", "{\"sections\": {,}}"),
            ("templates/product.liquid", "<h1>{{ product.title }}</h1>"),
            ("templates/404.liquid", "<h1>Not here</h1>"),
            (
                "shopify-local/products/mug.json",
                r#"{"title": "Mug", "price": "10.00"}"#,
            ),
            (
                "shopify-local/products/cup.json",
                r#"{"title": "Cup", "price": 10.5}"#,
            ),
        ],
    );
    let state = serve(&root, true);
    let home = get(&state, "/", "overlay");
    assert_eq!(header(&home, "x-lsf-liquid-errors"), Some("1"));
    let html = text(&home);
    assert!(html.contains("<aside data-lsf-errors"), "{html}");
    assert!(html.contains("templates/index.json line 1"), "{html}");
    assert!(html.contains("products/cup.json"), "{html}");
    // A product that the data error leaves out: the page says why it is the 404 page.
    let missing = text(&get(&state, "/products/cup", "overlay"));
    assert!(
        missing.contains("404: no product with the handle"),
        "{missing}"
    );

    // Without live reload, the page is as Shopify renders it.
    let quiet = serve(&root, false);
    let html = text(&get(&quiet, "/", "overlay"));
    assert!(!html.contains("data-lsf-errors"), "{html}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn live_reload_notes_the_default_menus_of_lsf_that_a_page_renders() {
    let root = theme_dir(
        "default-menus",
        &[
            (
                "layout/theme.liquid",
                "<!doctype html><html><head>{{ content_for_header }}</head><body>\
                 {{ content_for_layout }}<nav>{% for link in linklists.main-menu.links %}\
                 [{{ link.title }}]{% endfor %}</nav></body></html>",
            ),
            ("templates/index.liquid", "<h1>Home</h1>"),
            (
                "shopify-local/store.json",
                r#"{"shop": {"name": "Demo Store"}}"#,
            ),
        ],
    );
    let state = serve(&root, true);
    let html = text(&get(&state, "/", "default-menus"));
    assert!(html.contains("[Home][Catalog]"), "{html}");
    // A note, not a problem: Shopify renders the store's own menu there.
    assert!(html.contains("<aside data-lsf-notes"), "{html}");
    assert!(!html.contains("data-lsf-errors"), "{html}");
    assert!(html.contains("lsf: 1 note</b>"), "{html}");
    assert!(
        html.contains(
            "menu &quot;main-menu&quot; is lsf&#39;s default (Home, Catalog), not the store&#39;s"
        ),
        "{html}"
    );
    // Without live reload, the page is as Shopify renders it.
    let quiet = serve(&root, false);
    let html = text(&get(&quiet, "/", "default-menus"));
    assert!(!html.contains("data-lsf-notes"), "{html}");

    // A menu of the data: nothing to note.
    std::fs::write(
        root.join("shopify-local/menus.json"),
        r#"{"menus": {"main-menu": {"links": [{"title": "Shop", "url": "/collections/all"}]}}}"#,
    )
    .unwrap();
    let state = serve(&root, true);
    let html = text(&get(&state, "/", "default-menus"));
    assert!(html.contains("[Shop]"), "{html}");
    assert!(!html.contains("data-lsf-notes"), "{html}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_theme_setting_of_a_session_that_the_theme_does_not_have_is_refused() {
    let root = theme_dir(
        "settings",
        &[
            LAYOUT,
            ("templates/index.liquid", "{{ settings.cart_type }}"),
            (
                "config/settings_schema.json",
                r#"[{"name": "Cart", "settings": [{"type": "select", "id": "cart_type", "options": [{"value": "page", "label": "Page"}, {"value": "drawer", "label": "Drawer"}]}]}]"#,
            ),
        ],
    );
    let state = serve(&root, false);
    let put = |settings: serde_json::Value| {
        state.dispatch(&super::request(
            "PUT",
            "/__lsf/session",
            serde_json::json!({"data": {"theme_settings": settings}}),
            "settings",
        ))
    };
    let refused = put(serde_json::json!({"cart_typ": "page", "cart_type": "bogus"}));
    assert_eq!(refused.status, 422, "{}", text(&refused));
    let answer = super::body_json(&refused);
    assert_eq!(answer["errors"], 2, "{answer}");
    assert_eq!(answer["diagnostics"][0]["code"], "unknown_setting");
    assert_eq!(
        answer["diagnostics"][0]["hint"],
        "did you mean \"cart_type\"?"
    );
    assert_eq!(answer["diagnostics"][1]["code"], "invalid_setting");

    let taken = put(serde_json::json!({"cart_type": "drawer"}));
    assert_eq!(taken.status, 200, "{}", text(&taken));
    assert!(text(&get(&state, "/", "settings")).contains("drawer"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_css_of_a_sass_asset_says_lsf_does_not_compile_sass() {
    let root = theme_dir(
        "sass",
        &[
            LAYOUT,
            ("templates/index.liquid", "<p class=\"x\">hello</p>"),
            (
                "assets/x.scss.liquid",
                "$main: #123456;\n.x { color: $main; }\n",
            ),
        ],
    );
    let state = serve(&root, false);
    let css = get(&state, "/cdn/shop/t/1/assets/x.scss.css", "sass");
    let _ = std::fs::remove_dir_all(root);
    assert_eq!(css.status, 404);
    let reason = header(&css, "x-lsf-reason").unwrap_or_default();
    assert!(
        reason.starts_with("assets/x.scss.liquid is Sass, which lsf does not support"),
        "{reason}"
    );
    assert_eq!(text(&css), reason);
}

#[test]
fn the_agent_files_render_their_templates_with_request_and_agents_only() {
    let root = theme_dir(
        "agents",
        &[
            LAYOUT,
            ("templates/index.liquid", "home"),
            (
                "templates/agents.md.liquid",
                "# {{ shop.name }}{{ agents.store_name }} at {{ agents.store_url }}\n\
                 {{ agents.ucp_discovery_url }} {{ agents.mcp_endpoint_url }} \
                 {{ agents.sitemap_url }} {{ agents.currency }} {{ request.host }}",
            ),
            ("templates/llms.txt.liquid", "llms {{ collections.size }}"),
            (
                "shopify-local/store.json",
                r#"{"shop": {"name": "Repro", "currency": "EUR"},
                    "localization": {"languages": [{"iso_code": "en"}, {"iso_code": "fr"}]}}"#,
            ),
        ],
    );
    let state = serve(&root, false);
    let agents = get(&state, "/agents.md", "agents");
    let llms = get(&state, "/llms.txt", "agents");
    // No `llms-full.txt.liquid`: `/llms-full.txt` falls back to `agents.md.liquid`.
    let full = get(&state, "/llms-full.txt", "agents");
    let prefixed = get(&state, "/fr/llms.txt", "agents");
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(agents.status, 200, "{}", text(&agents));
    assert_eq!(
        header(&agents, "content-type"),
        Some("text/markdown; charset=utf-8")
    );
    // `shop` is not in the context: it renders blank, as on Shopify.
    assert_eq!(
        text(&agents),
        "# Repro at http://shop.test\n\
         http://shop.test/.well-known/ucp http://shop.test/api/ucp/mcp \
         http://shop.test/sitemap.xml EUR shop.test"
    );
    assert_eq!(
        header(&llms, "content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(text(&llms), "llms ");
    assert_eq!(text(&full), text(&agents));
    // These files are only served at the bare domain.
    assert_eq!(prefixed.status, 404);
    let reason = header(&prefixed, "x-lsf-not-found").unwrap_or_default();
    assert!(reason.contains("at the bare domain only"), "{reason}");
}

#[test]
fn an_agent_file_without_a_template_is_not_found_and_says_why() {
    let root = theme_dir("no-agents", &[LAYOUT, ("templates/index.liquid", "home")]);
    let state = serve(&root, false);
    let llms = get(&state, "/llms.txt", "agents");
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(llms.status, 404);
    let reason = header(&llms, "x-lsf-not-found").unwrap_or_default();
    assert!(
        reason.starts_with(
            "the theme has no template for /llms.txt: Shopify serves an llms.txt of its own"
        ),
        "{reason}"
    );
}

#[test]
fn a_page_with_the_account_component_gets_its_script_and_its_customer() {
    let root = theme_dir(
        "account-component",
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{% if template.name == 'index' %}\
                 <shopify-account menu=\"customer-account-main-menu\"><a href=\"/account\">Account</a></shopify-account>\
                 {% endif %}{{ content_for_layout }}</body></html>",
            ),
            ("templates/index.liquid", "home"),
            ("templates/cart.liquid", "cart"),
        ],
    );
    let state = serve(&root, false);
    let home = text(&get(&state, "/", "account"));
    let cart = text(&get(&state, "/cart", "account"));
    let script = get(
        &state,
        "/cdn/storefront/web-components/account.js",
        "account",
    );
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        home.contains(
            "<script src=\"//shop.test/cdn/storefront/web-components/account.js\" type=\"module\" crossorigin fetchpriority=\"low\" async></script>\
             <script id=\"shopify-customer-account-data\" type=\"application/json\">{\"customer\":null,\"shop\":{\"id\":\"gid:\\/\\/shopify\\/Shop\\/"
        ),
        "{home}"
    );
    // A page without the component gets neither.
    assert!(!cart.contains("account.js"), "{cart}");
    assert!(!cart.contains("shopify-customer-account-data"), "{cart}");
    assert_eq!(script.status, 200);
    assert!(text(&script).contains("customElements.define('shopify-account'"));
}

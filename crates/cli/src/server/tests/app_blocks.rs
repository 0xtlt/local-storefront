//! Theme app extensions: the app blocks and app embed blocks of `--app` render in the theme as
//! on Shopify. The fixture is an invented reviews app (`crates/cli/tests/fixtures/
//! theme-app-extensions`): a reviews block with settings, snippets, translations and assets, a
//! badge that an app-data metafield turns on, and app embed blocks for each target.

use std::path::{Path, PathBuf};

use lsf_core::theme::Revalidate;
use serde_json::json;

use super::super::{ServeOptions, ServerState};
use super::{get, header, request, text};
use crate::app::App;

const UID: &str = "5b1c7a52-2d0e-4c8a-9a3e-6f1d2b7c9e01";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme-app-extensions")
}

/// The fixture theme and its data, served with the app in `app_dir`.
fn server_of(app_dir: &Path, revalidate: Revalidate) -> ServerState {
    let root = fixture();
    let app = App::open(
        &root.join("theme"),
        Some(&root.join("data")),
        revalidate,
        &[],
    )
    .expect("fixture theme");
    crate::theme_extensions::attach(&app.theme, app_dir, revalidate);
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            polaris: Default::default(),
            admin: None,
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
    assert_eq!(diagnostics.error_count(), 0, "{diagnostics}");
    state
}

fn server() -> ServerState {
    server_of(&fixture().join("app"), Revalidate::Never)
}

/// A copy of the fixture app, to change its files.
struct AppCopy(PathBuf);

impl AppCopy {
    fn new(name: &str) -> AppCopy {
        let path =
            std::env::temp_dir().join(format!("lsf-app-blocks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        copy(&fixture().join("app"), &path);
        AppCopy(path)
    }

    fn write(&self, path: &str, content: &str) {
        std::fs::write(self.0.join(path), content).unwrap();
    }
}

impl Drop for AppCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The text between `start` and the next `end`.
fn between<'a>(html: &'a str, start: &str, end: &str) -> &'a str {
    let from = html
        .find(start)
        .unwrap_or_else(|| panic!("no {start} in {html}"));
    let rest = &html[from..];
    &rest[..rest.find(end).map_or(rest.len(), |to| to + end.len())]
}

#[test]
fn an_app_block_of_section_blocks_renders_with_render_block() {
    let state = server();
    let reply = get(&state, "/products/blue-shirt", "visitor");
    assert_eq!(reply.status, 200);
    let html = text(&reply);
    let block = between(
        &html,
        "<div id=\"shopify-block-reviews_app\"",
        "</div>\n\n\n</div>",
    );
    // Shopify's wrapper, with the `class` of the schema.
    assert!(
        block.starts_with(
            "<div id=\"shopify-block-reviews_app\" class=\"shopify-block shopify-app-block reviews-block\">"
        ),
        "{block}"
    );
    // Its settings from the template, the defaults of the others, the product of the page.
    assert!(block.contains("<h2>Loved by customers</h2>"), "{block}");
    assert!(block.contains("--star-color: #f5a623"), "{block}");
    assert!(block.contains("Reviews of Blue shirt"), "{block}");
    // `render` reads the extension's snippets, `asset_url` its assets.
    assert!(
        block.contains("aria-label=\"5 / 5\">★★★★★</span>"),
        "{block}"
    );
    assert!(
        block.contains(&format!(
            "src=\"//shop.test/cdn/extensions/{UID}/assets/star.svg?v="
        )),
        "{block}"
    );
    // `block.type` is `@app`, `block.id` its key, `section` the section it is in.
    assert!(
        block.contains("template--7746190929920__main reviews_app @app"),
        "{block}"
    );
    // The theme's own blocks render as before.
    assert!(html.contains("<p class=\"text\">Intro</p>"), "{html}");
}

#[test]
fn a_block_the_app_does_not_have_says_why_it_renders_nothing() {
    let state = server();
    let reply = get(&state, "/products/blue-shirt", "visitor");
    let html = text(&reply);
    assert!(!html.contains("reviewz"), "{html}");
    assert!(!html.contains("another-app"), "{html}");
    let reason = header(&reply, "x-lsf-reason").expect("a reason");
    assert!(
        reason.contains("not rendered") && reason.contains("app block"),
        "{reason}"
    );
}

#[test]
fn content_for_blocks_renders_app_blocks_and_the_apps_section_wraps_them() {
    let state = server();
    let html = text(&get(&state, "/", "visitor"));
    // In `content_for 'blocks'`, the id of a theme block; the default heading is translated
    // with the extension's schema locale.
    let block = between(
        &html,
        "<div id=\"shopify-block-A759b7971b1437a6__reviews_featured\"",
        "</div>\n\n\n</div>",
    );
    assert!(block.contains("<h2>What customers say</h2>"), "{block}");
    assert!(block.contains("★★★★</span>"), "{block}");
    // A section of type `apps` without `sections/apps.liquid`: Shopify's wrapper. The badge is
    // available: its app-data metafield is true in the store data. `tag` is its element.
    assert!(
        html.contains(
            "<div id=\"shopify-section-template--3059316750311__apps\" class=\"shopify-section\"><aside id=\"shopify-block-badge\" class=\"shopify-block shopify-app-block\"><span class=\"stars-badge\">Top rated</span>"
        ),
        "{html}"
    );
}

#[test]
fn app_embed_blocks_render_before_the_end_of_the_head_and_of_the_body() {
    let state = server();
    let html = text(&get(&state, "/products/blue-shirt", "visitor"));
    let head = between(&html, "<head>", "</head>");
    let compliance = head
        .find("<meta name=\"reviews-consent\" content=\"opt-in\">")
        .expect("the compliance_head embed");
    let loader = head
        .find("key: \"theme-key\"")
        .expect("the head embed, with the settings of settings_data.json");
    assert!(compliance < loader, "{head}");
    // Head embeds have no wrapper.
    assert!(
        !head.contains("shopify-block-11223344556677889900"),
        "{head}"
    );
    // The stylesheet and the scripts the schemas name, once each, in the head.
    let assets = format!("//shop.test/cdn/extensions/{UID}/assets/");
    assert_eq!(
        head.matches(&format!("{assets}reviews.css")).count(),
        1,
        "{head}"
    );
    assert!(
        head.contains(&format!("<script src=\"{assets}reviews.js?v=")),
        "{head}"
    );
    // The body embed is off in settings_data.json, and on in the store data (`app.embeds`),
    // with the settings of settings_data.json.
    let body = between(&html, "</main>", "</body>");
    assert!(
        body.contains(
            "<div id=\"shopify-block-99887766554433221100\" class=\"shopify-block shopify-app-block\"><div class=\"reviews-widget\" data-position=\"left\">Customer reviews</div>"
        ),
        "{body}"
    );
    assert!(head.contains(&format!("{assets}widget.js?v=")), "{head}");
}

#[test]
fn the_schema_keeps_blocks_out_of_pages_and_the_session_data_turns_embeds_off() {
    let state = server();
    // `enabled_on` names templates only: the block of the footer group renders nothing.
    let reply = get(&state, "/", "visitor");
    let html = text(&reply);
    assert!(html.contains("<footer></footer>"), "{html}");
    // `disabled_on` keeps the widget out of the cart.
    let cart = text(&get(&state, "/cart", "visitor"));
    assert!(!cart.contains("reviews-widget"), "{cart}");
    assert!(cart.contains("reviews-consent"), "{cart}");

    // A session with data of its own: the metafield is false, the embeds are off.
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"data": {"app": {
            "metafields": {"reviews": {"badge_enabled": false}},
            "embeds": {"reviews-widget": false, "consent": false, "reviews-loader": {"settings": {"api_key": "session-key"}}}
        }}}),
        "test",
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let reply = get(&state, "/", "test");
    let html = text(&reply);
    assert!(!html.contains("stars-badge"), "{html}");
    assert!(!html.contains("reviews-widget"), "{html}");
    assert!(!html.contains("reviews-consent"), "{html}");
    assert!(html.contains("key: \"session-key\""), "{html}");
    let reason = header(&reply, "x-lsf-reason").unwrap_or_default();
    assert!(
        reason.contains("available_if") || reason.contains("enabled_on"),
        "{reason}"
    );
}

#[test]
fn the_cdn_serves_the_assets_of_the_extension() {
    let state = server();
    let css = get(
        &state,
        &format!("/cdn/extensions/{UID}/assets/reviews.css?v=1"),
        "v",
    );
    assert_eq!(css.status, 200);
    assert!(text(&css).contains(".reviews"), "{}", text(&css));
    assert_eq!(
        header(&css, "content-type"),
        Some("text/css; charset=utf-8")
    );
    let svg = get(
        &state,
        &format!("/cdn/extensions/{UID}/assets/star.svg"),
        "v",
    );
    assert_eq!(svg.status, 200);
    let missing = get(
        &state,
        &format!("/cdn/extensions/{UID}/assets/nope.js"),
        "v",
    );
    assert_eq!(missing.status, 404);
    let other = get(&state, "/cdn/extensions/other/assets/reviews.css", "v");
    assert_eq!(other.status, 404);
}

#[test]
fn the_section_rendering_api_renders_app_blocks() {
    let state = server();
    let reply = get(
        &state,
        "/products/blue-shirt?sections=template--7746190929920__main",
        "visitor",
    );
    let sections: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    let html = sections["template--7746190929920__main"].as_str().unwrap();
    assert!(html.contains("shopify-block-reviews_app"), "{html}");
    assert!(html.contains("Loved by customers"), "{html}");
}

#[test]
fn a_translation_of_the_extension_follows_the_locale() {
    let state = server();
    let html = text(&get(&state, "/fr/products/blue-shirt", "visitor"));
    assert!(html.contains("Avis sur Blue shirt"), "{html}");
    assert!(html.contains("Avis clients"), "{html}");
}

#[test]
fn a_liquid_error_of_the_extension_names_its_file_and_an_edit_reloads() {
    let app = AppCopy::new("errors");
    let state = server_of(&app.0, Revalidate::Every(std::time::Duration::ZERO));
    let before = state.observe_changes();
    app.write(
        "extensions/reviews/snippets/stars.liquid",
        "{% unknowntag %}",
    );
    // Live reload sees a change of the extension, and the page renders what it is now.
    let after = state.observe_changes();
    assert_ne!(before, after);
    let reply = get(&state, "/products/blue-shirt", "visitor");
    assert_eq!(header(&reply, "x-lsf-liquid-errors"), Some("1"));
    let html = text(&reply);
    assert!(
        html.contains(
            "Liquid syntax error (extensions/reviews/snippets/stars line 1): Unknown tag 'unknowntag'"
        ),
        "{html}"
    );
}

#[test]
fn the_tools_say_what_is_wrong_with_the_app_blocks_of_the_theme() {
    let state = server();
    let theme = &state.app.theme;
    // `lsf check --app`: the block the app does not have, with a suggestion, and the blocks of
    // another app.
    let findings =
        crate::theme_extensions::check_lines(theme, Some(&fixture().join("app"))).unwrap();
    let messages: Vec<String> = findings
        .iter()
        .map(|finding| format!("{}: {}", finding.file, finding.message))
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("templates/product.json: ")
                && message.ends_with(
                    "the app product-reviews has no block `reviewz`: did you mean `reviews`?"
                )),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("1 block of the app another-app, which is not served")),
        "{messages:?}"
    );
    // Without `--app`: a note, not silence.
    let without = crate::theme_extensions::check_lines(theme, None).unwrap();
    assert_eq!(without.len(), 1);
    assert!(
        without[0]
            .message
            .contains("not checked: `--app DIR` checks them"),
        "{:?}",
        without[0]
    );
    // The `app:` line.
    assert_eq!(
        crate::theme_extensions::summary(&theme.app_extensions()).as_deref(),
        Some("1 theme extension (2 app blocks, 3 app embed blocks)")
    );
    // The home page lists the blocks.
    let home = crate::theme_extensions::home(&theme.app_extensions());
    let names: Vec<&str> = home[0]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "consent",
            "reviews-loader",
            "reviews-widget",
            "reviews",
            "stars-badge"
        ]
    );
    assert_eq!(
        home[0]["blocks"][3]["type"],
        format!("shopify://apps/product-reviews/blocks/reviews/{UID}")
    );
}

#[test]
fn app_build_checks_the_theme_app_extension() {
    let app = AppCopy::new("build");
    let shopify_app = lsf_app::config::load(&app.0).unwrap();
    assert_eq!(
        crate::theme_extensions::validate_app(&shopify_app, &[]),
        (1, false)
    );
    app.write(
        "extensions/reviews/blocks/reviews.liquid",
        "{% if a = b %}{% endif %}",
    );
    let shopify_app = lsf_app::config::load(&app.0).unwrap();
    assert_eq!(
        crate::theme_extensions::validate_app(&shopify_app, &["reviews".to_string()]),
        (1, true)
    );
}

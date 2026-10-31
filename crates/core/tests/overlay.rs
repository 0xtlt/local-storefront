//! What the Admin API changes of the theme the storefront serves: files written over the
//! directory's (or the whole theme, once another is published), and the script tags of
//! `content_for_header`. The directory is never written to.

use std::path::Path;
use std::sync::Arc;

use lsf_core::render::Target;
use lsf_core::store::load::{DataSource, LoadOptions, load};
use lsf_core::theme::{FileOverlay, OverlayFile, Revalidate};
use lsf_core::{Renderer, Request, Session, Theme};

fn theme() -> Arc<Theme> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme");
    let env = Arc::new(lsf_core::environment());
    Arc::new(Theme::open(&directory, env, Revalidate::Never).expect("fixture theme"))
}

fn home(theme: &Arc<Theme>) -> String {
    let (store, _) = load(&DataSource::Demo, &LoadOptions::default());
    let store = Arc::new(store);
    let renderer = Renderer::new(theme.clone());
    let rendered = renderer.render(
        store.clone(),
        Request::new("shop.test", "/"),
        Session::initial(&store),
        &Target::Page,
    );
    rendered.body
}

fn overlay(files: &[(&str, Option<&str>)], exclusive: bool, generation: u64) -> Arc<FileOverlay> {
    let mut overlay = FileOverlay {
        exclusive,
        generation,
        ..FileOverlay::default()
    };
    for (path, content) in files {
        overlay.files.insert(
            path.to_string(),
            content.map(|content| OverlayFile::new(content.as_bytes().to_vec())),
        );
    }
    Arc::new(overlay)
}

#[test]
fn an_overlay_is_rendered_on_top_of_the_theme_directory() {
    let theme = theme();
    let before = home(&theme);
    assert!(before.contains("Stored title"));
    let index = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme/templates/index.json"),
    )
    .unwrap();

    // A JSON template written by the Admin API: its own section, read from the overlay too.
    theme.files().set_overlay(Some(overlay(
        &[
            (
                "templates/index.json",
                Some(r#"{"sections": {"main": {"type": "admin-banner", "settings": {}}}, "order": ["main"]}"#),
            ),
            ("sections/admin-banner.liquid", Some("<p class=\"admin\">Written by the Admin API</p>")),
        ],
        false,
        1,
    )));
    let overlaid = home(&theme);
    assert!(overlaid.contains("Written by the Admin API"), "{overlaid}");
    assert!(!overlaid.contains("Stored title"));
    assert_eq!(
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme/templates/index.json")
        )
        .unwrap(),
        index,
        "the directory is not written to"
    );

    // Back to the directory alone.
    theme.files().set_overlay(None);
    assert_eq!(home(&theme), before);
}

#[test]
fn an_exclusive_overlay_is_the_whole_theme() {
    let theme = theme();
    theme.files().set_overlay(Some(overlay(
        &[(
            "layout/theme.liquid",
            Some("<html><head>{{ content_for_header }}</head><body>Another theme {{ content_for_layout }}</body></html>"),
        ), ("templates/index.liquid", Some("home"))],
        true,
        7,
    )));
    let body = home(&theme);
    assert!(body.contains("Another theme home"), "{body}");
    assert!(!body.contains("Stored title"));
}

#[test]
fn script_tags_are_loaded_by_content_for_header() {
    let theme = theme();
    assert!(!home(&theme).contains("asyncLoad"));
    theme.set_script_tags(vec![
        "https://js.example.org/bar.js".to_string(),
        "https://js.example.org/with.js?x=1".to_string(),
    ]);
    let body = home(&theme);
    assert!(body.contains("function asyncLoad()"), "{body}");
    // As Shopify writes it: slashes escaped, `&` as `\u0026`, the shop's domain in `shop=`.
    assert!(
        body.contains(
            r#"var urls = ["https:\/\/js.example.org\/bar.js?shop=local-supply-co.myshopify.com","#
        ),
        "{body}"
    );
    assert!(
        body.contains("with.js?x=1\\u0026shop=local-supply-co.myshopify.com\"];"),
        "{body}"
    );
    theme.set_script_tags(Vec::new());
    assert!(!home(&theme).contains("asyncLoad"));
}

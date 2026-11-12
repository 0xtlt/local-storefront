//! A file of `assets/` edited while the server runs is what the browser gets next, without a
//! restart: the page links it at a new `?v=`, which serves it as it is now, and live reload
//! sees the edit. Shopify's CDN keeps such a file a year at any URL: what makes the browser
//! fetch it again is the new `?v=`, there as here.

use std::path::PathBuf;
use std::time::Duration;

use lsf_core::theme::Revalidate;
use serde_json::json;

use super::super::{ServeOptions, ServerState};
use super::{body_json, changing_theme, get, header, text};
use crate::app::App;

const YEAR: Option<&str> = Some("public, max-age=31557600");
const IMMUTABLE: Option<&str> = Some("public, max-age=31536000, immutable");

/// A stylesheet that gets lighter once minified, so that it has a source map. The class names
/// (`.card-before`) say which version of the file is served.
fn stylesheet(class: &str) -> String {
    format!(".{class} {{\n  margin: 0px 0px 0px 0px;\n  padding: 0px 0px 0px 0px;\n}}\n").repeat(4)
}

/// A theme that links a stylesheet, a script, a stylesheet rendered from Liquid, an image and
/// the bundle of its `{% stylesheet %}` tags, served as `serve` serves it: the files are
/// looked at again whenever they may have changed.
fn edited_theme(name: &str) -> (PathBuf, ServerState) {
    let root = changing_theme(name);
    for directory in ["assets", "sections", "config"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("layout/theme.liquid"),
        "<html><head>{{ content_for_header }}\
         {{ 'base.css' | asset_url | stylesheet_tag }}\
         {{ 'colors.css' | asset_url | stylesheet_tag }}\
         <script src=\"{{ 'global.js' | asset_url }}\" defer></script></head>\
         <body><img src=\"{{ 'dot.png' | asset_url }}\" alt=\"\">{{ content_for_layout }}</body></html>",
    )
    .unwrap();
    std::fs::write(root.join("templates/index.liquid"), "{% section 'hero' %}").unwrap();
    write_version(&root, "before");
    let app = App::open(&root, None, Revalidate::Every(Duration::ZERO), &[]).unwrap();
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            polaris: Default::default(),
            admin: None,
            live_reload: true,
            watch: true,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    (root, state)
}

/// Writes every asset of [`edited_theme`] as `version` of it.
fn write_version(root: &std::path::Path, version: &str) {
    let write = |name: &str, content: &str| std::fs::write(root.join(name), content).unwrap();
    write("assets/base.css", &stylesheet(&format!("card-{version}")));
    write(
        "assets/global.js",
        &format!("document.documentElement.dataset.version = \"{version}\";\n"),
    );
    write(
        "assets/colors.css.liquid",
        &format!(".colors-{{{{ '{version}' }}}} {{ margin: 0 }}\n"),
    );
    write(
        "sections/hero.liquid",
        &format!(
            "<div class=\"hero\"></div>\n\
             {{% stylesheet %}}\n{}{{% endstylesheet %}}\n\
             {{% schema %}}{{\"name\": \"Hero\"}}{{% endschema %}}\n",
            stylesheet(&format!("hero-{version}"))
        ),
    );
    let red = if version == "before" { 200 } else { 20 };
    image::RgbImage::from_pixel(4, 4, image::Rgb([red, 30, 30]))
        .save(root.join("assets/dot.png"))
        .unwrap();
}

/// The path at which the home page links `file` (`assets/base.css`,
/// `compiled_assets/styles.css`), its `?v=` included.
fn linked(state: &ServerState, file: &str) -> String {
    let page = text(&get(state, "/", "edits"));
    let start = format!("//shop.test/cdn/shop/t/1/{file}?v=");
    let at = page
        .find(&start)
        .unwrap_or_else(|| panic!("no {start} in {page}"));
    let path = &page[at + "//shop.test".len()..];
    let end = path
        .find(|c: char| c == '"' || c == '\'' || c.is_whitespace())
        .unwrap();
    path[..end].to_string()
}

/// What each file is served with: its path, the text that says its version, and how long the
/// browser keeps it at the URL the page links.
const FILES: [(&str, &str, Option<&str>); 4] = [
    ("assets/base.css", "card-", YEAR),
    ("assets/global.js", "version = \"", YEAR),
    ("assets/colors.css", "colors-", YEAR),
    ("compiled_assets/styles.css", "hero-", IMMUTABLE),
];

#[test]
fn an_edited_asset_is_linked_at_a_new_url_that_serves_it_as_it_is_now() {
    let (root, state) = edited_theme("edited-assets");
    let token = state.observe_changes();
    let mut before = Vec::new();
    for (file, marker, policy) in FILES {
        let path = linked(&state, file);
        let reply = get(&state, &path, "edits");
        assert_eq!(reply.status, 200, "{path}");
        assert!(text(&reply).contains(&format!("{marker}before")), "{path}");
        assert_eq!(header(&reply, "cache-control"), policy, "{path}");
        before.push(path);
    }
    let image = linked(&state, "assets/dot.png");
    let picture = get(&state, &image, "edits");
    assert_eq!(header(&picture, "cache-control"), YEAR, "{image}");

    write_version(&root, "after");
    // Live reload sees the edit: the open pages reload.
    assert_ne!(state.observe_changes(), token);
    // The page links each file at a new URL, which serves the file as it is now.
    for ((file, marker, policy), old) in FILES.into_iter().zip(&before) {
        let path = linked(&state, file);
        assert_ne!(&path, old, "{file}");
        let reply = get(&state, &path, "edits");
        let served = text(&reply);
        assert!(
            served.contains(&format!("{marker}after")),
            "{path}: {served}"
        );
        assert!(
            !served.contains(&format!("{marker}before")),
            "{path}: {served}"
        );
        assert_eq!(header(&reply, "cache-control"), policy, "{path}");
    }
    let edited = linked(&state, "assets/dot.png");
    assert_ne!(edited, image);
    let reply = get(&state, &edited, "edits");
    assert_ne!(reply.body, picture.body);
    assert_eq!(header(&reply, "cache-control"), YEAR, "{edited}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_source_map_of_an_edited_asset_is_linked_at_a_new_url_too() {
    let (root, state) = edited_theme("edited-source-maps");
    // The link to the map carries the `?v=` of the file, as on Shopify's CDN.
    let link = |path: &str| {
        let served = text(&get(&state, path, "edits"));
        let (_, link) = served
            .rsplit_once("sourceMappingURL=")
            .unwrap_or_else(|| panic!("no link to a map in {path}: {served}"));
        link.trim_end_matches(['\n', ' ', '*', '/']).to_string()
    };
    let before = linked(&state, "assets/base.css");
    let version = before.split_once('?').unwrap().1;
    let old_map = link(&before);
    assert_eq!(
        old_map,
        format!("/cdn/shop/t/1/assets/base.css.map?{version}")
    );

    write_version(&root, "after");
    let after = linked(&state, "assets/base.css");
    let new_map = link(&after);
    assert_ne!(new_map, old_map);
    let map = get(&state, &new_map, "edits");
    assert_eq!(map.status, 200);
    assert_eq!(header(&map, "cache-control"), YEAR);
    assert_eq!(
        body_json(&map)["sourcesContent"],
        json!([stylesheet("card-after")])
    );

    // So does the link of the bundle of the `{% stylesheet %}` tags.
    let bundle = linked(&state, "compiled_assets/styles.css");
    let version = bundle.split_once('?').unwrap().1;
    assert_eq!(
        link(&bundle),
        format!("/cdn/shop/t/1/compiled_assets/styles.css.map?{version}")
    );

    let _ = std::fs::remove_dir_all(&root);
}

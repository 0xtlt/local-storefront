//! Contextual templates and section groups (`index.context.<market>.json`,
//! `header-group.context.<market>.json`): their overrides on the subfolders of their market and
//! for B2B buyers, the parent file everywhere else, as Shopify applies them.
//! https://shopify.dev/docs/storefronts/themes/architecture/templates/alternate-templates
//! https://shopify.dev/docs/storefronts/themes/architecture/section-groups

use std::path::{Path, PathBuf};

use lsf_core::theme::Revalidate;
use serde_json::{Value as Json, json};

use super::super::{ServeOptions, ServerState};
use super::{body_json, get, text};
use crate::app::App;

/// The theme and the store of the issue, in a folder of their own: a market `europe` served at
/// `/en-be/` and `/fr-be/`, a contextual `index` template and a contextual header group for it.
/// No `config/markets.json`.
fn write_theme(name: &str, store: &Json, extra: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lsf-contextual-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let note = r#"<p class="note">{{ section.id }}={{ section.settings.text }}</p>
{% schema %}
{ "name": "Note", "settings": [{ "type": "text", "id": "text", "label": "Text" }], "enabled_on": { "groups": ["header"] } }
{% endschema %}"#;
    let banner = r#"<p class="note">{{ section.id }}={{ section.settings.text }}{% for block in section.blocks %} {{ block.id }}={{ block.settings.text }}{% endfor %}</p>
{% schema %}
{ "name": "Banner", "settings": [{ "type": "text", "id": "text", "label": "Text" }],
  "blocks": [{ "type": "line", "name": "Line", "settings": [{ "type": "text", "id": "text", "label": "Text" }] }] }
{% endschema %}"#;
    let mut files = vec![
        (
            "layout/theme.liquid",
            "<!doctype html><html><head>{{ content_for_header }}</head><body>{% sections 'header-group' %}{{ content_for_layout }}</body></html>",
        ),
        ("sections/note.liquid", note),
        ("sections/banner.liquid", banner),
        (
            "sections/header-group.json",
            r#"{ "type": "header", "name": "Header", "sections": { "announce": { "type": "note", "settings": { "text": "default" } } }, "order": ["announce"] }"#,
        ),
        (
            "sections/header-group.context.europe.json",
            r#"{ "context": { "market": "europe" }, "parent": "header-group.json", "sections": { "announce": { "settings": { "text": "europe" } } } }"#,
        ),
        (
            "templates/index.json",
            r#"{ "sections": { "hero": { "type": "banner", "settings": { "text": "default" } } }, "order": ["hero"] }"#,
        ),
        (
            "templates/index.context.europe.json",
            r#"{ "context": { "market": "europe" }, "parent": "index.json", "sections": { "hero": { "settings": { "text": "europe" } } } }"#,
        ),
        ("locales/en.default.json", "{}"),
        ("locales/fr.json", "{}"),
    ];
    files.extend_from_slice(extra);
    for (path, content) in files {
        let path = root.join("theme").join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/store.json"), store.to_string()).unwrap();
    root
}

fn the_issues_store() -> Json {
    json!({
        "shop": { "name": "Probe", "currency": "USD", "address": { "country_code": "US" } },
        "localization": {
            "languages": [{ "iso_code": "en" }, { "iso_code": "fr" }],
            "countries": [{ "iso_code": "US" }, { "iso_code": "BE" }],
            "markets": [
                { "handle": "us", "countries": ["US"] },
                { "handle": "europe", "countries": ["BE"], "currency": "EUR", "rate": "0.9", "subfolder_suffix": "be" }
            ]
        }
    })
}

fn serve(root: &Path) -> ServerState {
    let app = App::open(
        &root.join("theme"),
        Some(&root.join("data")),
        Revalidate::Never,
        &[],
    )
    .expect("theme");
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            polaris: Default::default(),
            admin: None,
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
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    state
}

/// The `name=value` of each note on a page.
fn notes(html: &str) -> Vec<String> {
    html.split("<p class=\"note\">")
        .skip(1)
        .filter_map(|part| part.split_once("</p>"))
        .map(|(note, _)| {
            // The ids of the sections are lsf's: only their key matters.
            note.split(' ')
                .map(|pair| pair.rsplit_once("__").map_or(pair, |(_, key)| key))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[test]
fn a_markets_contextual_files_apply_on_its_subfolders_and_the_parents_elsewhere() {
    let root = write_theme("issue", &the_issues_store(), &[]);
    let state = serve(&root);
    for path in ["/en-be/", "/fr-be/", "/en-be", "/fr-be"] {
        let page = get(&state, path, "visitor");
        assert_eq!(page.status, 200, "{path}: {}", text(&page));
        assert_eq!(
            notes(&text(&page)),
            ["announce=europe", "hero=europe"],
            "{path}"
        );
    }
    for path in ["/", "/fr/", "/fr"] {
        let page = get(&state, path, "visitor");
        assert_eq!(page.status, 200, "{path}: {}", text(&page));
        assert_eq!(
            notes(&text(&page)),
            ["announce=default", "hero=default"],
            "{path}"
        );
    }
    // The Section Rendering API renders the sections as the page does.
    let sections = |path: &str| {
        let page = text(&get(&state, "/", "visitor"));
        let ids: Vec<String> = page
            .split("id=\"shopify-section-")
            .skip(1)
            .filter_map(|part| part.split_once('"').map(|(id, _)| id.to_string()))
            .collect();
        assert_eq!(ids.len(), 2, "{page}");
        let answer = body_json(&get(
            &state,
            &format!("{path}?sections={}", ids.join(",")),
            "visitor",
        ));
        ids.iter()
            .flat_map(|id| notes(answer[id].as_str().unwrap_or_default()))
            .collect::<Vec<_>>()
    };
    assert_eq!(sections("/fr-be/"), ["announce=europe", "hero=europe"]);
    assert_eq!(sections("/"), ["announce=default", "hero=default"]);
    // A contextual file is no alternate template of its own.
    let page = text(&get(&state, "/?view=context.europe", "visitor"));
    assert_eq!(notes(&page), ["announce=default", "hero=default"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn overrides_hide_add_and_reorder_as_the_theme_editor_writes_them() {
    let root = write_theme(
        "overrides",
        &the_issues_store(),
        &[
            (
                "templates/search.json",
                r#"{ "sections": {
                    "hero": { "type": "banner", "settings": { "text": "hero" },
                        "blocks": { "one": { "type": "line", "settings": { "text": "1" } },
                                    "two": { "type": "line", "settings": { "text": "2" } } },
                        "block_order": ["one", "two"] },
                    "other": { "type": "banner", "settings": { "text": "other" } } },
                  "order": ["hero", "other"] }"#,
            ),
            (
                "templates/search.context.europe.json",
                r#"{ "context": { "market": "europe" }, "parent": "search.json",
                  "sections": {
                    "hero": { "blocks": { "one": { "disabled": true }, "three": { "type": "line", "settings": { "text": "3" } } },
                              "block_order": ["three", "two", "one"] },
                    "other": { "disabled": true },
                    "added": { "type": "banner", "settings": { "text": "added" } } },
                  "order": ["added", "hero", "other"] }"#,
            ),
        ],
    );
    let state = serve(&root);
    let store = text(&get(&state, "/en-be/search", "visitor"));
    assert!(store.contains("hero=hero"), "{store}");
    assert_eq!(
        notes(&store)[1..],
        ["added=added", "hero=hero three=3 two=2"]
    );
    let store = text(&get(&state, "/search", "visitor"));
    assert_eq!(notes(&store)[1..], ["hero=hero one=1 two=2", "other=other"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_b2b_contextual_file_applies_to_a_buyer_for_a_company() {
    let mut store = the_issues_store();
    store["companies"] = json!([{ "name": "Northwind", "locations": [{ "name": "Portland" }] }]);
    store["customers"] = json!([
        { "email": "buyer@example.com", "company": "Northwind" }
    ]);
    store["session"] = json!({ "customer": "buyer@example.com" });
    let root = write_theme(
        "b2b",
        &store,
        &[(
            "templates/index.context.b2b.json",
            r#"{ "context": { "b2b": true }, "parent": "index.json", "sections": { "hero": { "settings": { "text": "b2b" } } } }"#,
        )],
    );
    let state = serve(&root);
    // B2B wins over the market.
    assert_eq!(
        notes(&text(&get(&state, "/", "buyer"))),
        ["announce=default", "hero=b2b"]
    );
    assert_eq!(
        notes(&text(&get(&state, "/en-be/", "buyer"))),
        ["announce=europe", "hero=b2b"]
    );
    let _ = std::fs::remove_dir_all(root);
}

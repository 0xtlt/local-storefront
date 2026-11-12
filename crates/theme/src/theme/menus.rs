//! The menus a theme names, which the store data has to have: the value of each `link_list`
//! setting (of the theme, of the sections of a JSON template or a section group, of their
//! blocks), or its default when the file does not set it, and the handles Liquid looks up
//! (`linklists.footer`, `linklists['main-menu']`).

use serde_json::Value as Json;

use super::Theme;
use super::schema::SettingDef;

/// A menu a theme names, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuReference {
    pub handle: String,
    /// Where, for a message: `sections/header-group.json at /sections/header/settings/menu`,
    /// `the default of "menu" in sections/header.liquid`, `snippets/nav.liquid line 3`.
    pub place: String,
}

/// The directories whose `.json` files are JSON templates or section groups.
/// The section groups come first: they are on every page.
const JSON_DIRECTORIES: [&str; 4] = [
    "sections",
    "templates",
    "templates/customers",
    "templates/metaobject",
];

/// The directories of the Liquid files.
const LIQUID_DIRECTORIES: [&str; 7] = [
    "layout",
    "templates",
    "templates/customers",
    "templates/metaobject",
    "sections",
    "blocks",
    "snippets",
];

/// What `linklists` has besides menus.
const LINKLISTS_PROPERTIES: [&str; 3] = ["size", "first", "last"];

impl Theme {
    /// The menus the theme names, in the order of its files: the theme settings, the section
    /// groups and the JSON templates, then the Liquid files. A handle named in several places
    /// comes once per place.
    pub fn menu_references(&self) -> Vec<MenuReference> {
        let mut found = Vec::new();
        self.settings_menus(&mut found);
        for directory in JSON_DIRECTORIES {
            for name in self.files().list(directory).iter() {
                if !name.ends_with(".json") {
                    continue;
                }
                let file = format!("{directory}/{name}");
                if let Ok(Some(json)) = self.json(&file) {
                    self.sections_menus(&file, "/sections", json.get("sections"), &mut found);
                }
            }
        }
        for directory in LIQUID_DIRECTORIES {
            for name in self.files().list(directory).iter() {
                if !name.ends_with(".liquid") {
                    continue;
                }
                let file = format!("{directory}/{name}");
                let source = self.files().read(&file).unwrap_or_default();
                for (handle, line) in linklists_handles(&source) {
                    found.push(MenuReference {
                        handle,
                        place: format!("{file} line {line}"),
                    });
                }
            }
        }
        found
    }

    /// The `link_list` settings of `config/settings_schema.json`, with their values in
    /// `config/settings_data.json` (its `current`, or the preset it names), and the sections it
    /// keeps the settings of (a theme that renders them with `{% section %}`).
    fn settings_menus(&self, found: &mut Vec<MenuReference>) {
        const SCHEMA: &str = "config/settings_schema.json";
        const DATA: &str = "config/settings_data.json";
        let schema = self.json(SCHEMA).ok().flatten();
        let data = self.json(DATA).ok().flatten();
        let current = data.as_deref().and_then(|data| data.get("current"));
        let (values, pointer) = match current {
            Some(Json::String(preset)) => (
                data.as_deref()
                    .and_then(|data| data.get("presets")?.get(preset)),
                format!("/presets/{}", escape(preset)),
            ),
            current => (current, "/current".to_string()),
        };
        let definitions = schema
            .as_deref()
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .flat_map(|group| super::schema::parse_settings(group.get("settings")));
        for definition in definitions {
            settings_menu(&definition, values, (DATA, &pointer), SCHEMA, found);
        }
        if let Some(sections) = values.and_then(|values| values.get("sections")) {
            self.sections_menus(DATA, &format!("{pointer}/sections"), Some(sections), found);
        }
    }

    /// The menus of the sections of a JSON template, a section group or the settings data:
    /// their `link_list` settings, and those of their blocks.
    fn sections_menus(
        &self,
        file: &str,
        pointer: &str,
        sections: Option<&Json>,
        found: &mut Vec<MenuReference>,
    ) {
        for (id, section) in sections.and_then(Json::as_object).into_iter().flatten() {
            let Some(kind) = section.get("type").and_then(Json::as_str) else {
                continue;
            };
            if disabled(section) {
                continue;
            }
            let schema_file = format!("sections/{kind}.liquid");
            let schema = self.schema(&schema_file);
            let at = format!("{pointer}/{}", escape(id));
            for definition in &schema.settings {
                settings_menu(
                    definition,
                    section.get("settings"),
                    (file, &format!("{at}/settings")),
                    &schema_file,
                    found,
                );
            }
            self.blocks_menus(file, &at, section, Some((&schema, &schema_file)), found);
        }
    }

    /// The menus of the blocks of a section or of a block: their settings are those the
    /// section declares for its own blocks, else those of the theme block.
    fn blocks_menus(
        &self,
        file: &str,
        pointer: &str,
        parent: &Json,
        section: Option<(&super::Schema, &str)>,
        found: &mut Vec<MenuReference>,
    ) {
        let blocks = parent.get("blocks").and_then(Json::as_object);
        for (id, block) in blocks.into_iter().flatten() {
            let Some(kind) = block.get("type").and_then(Json::as_str) else {
                continue;
            };
            // An app's block is the app's.
            if disabled(block) || kind.starts_with('@') || kind.starts_with("shopify://") {
                continue;
            }
            let at = format!("{pointer}/blocks/{}", escape(id));
            let local = section.and_then(|(schema, file)| Some((schema.block(kind)?, file)));
            let (definitions, schema_file) = match local {
                Some((local, file)) => (local.settings.clone(), file.to_string()),
                None => {
                    let schema_file = format!("blocks/{kind}.liquid");
                    (self.schema(&schema_file).settings.clone(), schema_file)
                }
            };
            for definition in &definitions {
                settings_menu(
                    definition,
                    block.get("settings"),
                    (file, &format!("{at}/settings")),
                    &schema_file,
                    found,
                );
            }
            self.blocks_menus(file, &at, block, None, found);
        }
    }
}

/// The menu a `link_list` setting names: its value in `values`, else its default.
fn settings_menu(
    definition: &SettingDef,
    values: Option<&Json>,
    (file, pointer): (&str, &str),
    schema_file: &str,
    found: &mut Vec<MenuReference>,
) {
    if definition.kind != "link_list" {
        return;
    }
    let id = &definition.id;
    let place = match values.and_then(|values| values.get(id)) {
        Some(value) => value
            .as_str()
            .map(|handle| (handle, format!("{file} at {pointer}/{}", escape(id)))),
        None => definition
            .default
            .as_ref()
            .and_then(Json::as_str)
            .map(|handle| (handle, format!("the default of \"{id}\" in {schema_file}"))),
    };
    if let Some((handle, place)) = place
        && !handle.trim().is_empty()
    {
        found.push(MenuReference {
            handle: handle.trim().to_string(),
            place,
        });
    }
}

fn disabled(entry: &Json) -> bool {
    entry
        .get("disabled")
        .and_then(Json::as_bool)
        .unwrap_or(false)
}

/// A key as a JSON pointer writes it.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The handles a Liquid source looks up in `linklists` by name, with their line:
/// `linklists.footer`, `linklists['main-menu']`, `linklists["main-menu"]`.
fn linklists_handles(source: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    let is_handle = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    for (start, _) in source.match_indices("linklists") {
        // `section.settings.linklists` or `my_linklists` is something else.
        let before = source[..start].chars().next_back();
        if before.is_some_and(|c| is_handle(c) || c == '.') {
            continue;
        }
        let rest = &source[start + "linklists".len()..];
        let handle = if let Some(rest) = rest.strip_prefix('.') {
            let end = rest.find(|c: char| !is_handle(c)).unwrap_or(rest.len());
            Some(&rest[..end])
        } else if let Some(rest) = rest.strip_prefix('[') {
            let rest = rest.trim_start();
            rest.chars()
                .next()
                .filter(|quote| *quote == '\'' || *quote == '"')
                .and_then(|quote| {
                    let inner = &rest[1..];
                    inner.find(quote).map(|end| &inner[..end])
                })
        } else {
            None
        };
        let Some(handle) = handle.filter(|handle| {
            !handle.is_empty()
                && handle.chars().all(is_handle)
                && !LINKLISTS_PROPERTIES.contains(handle)
        }) else {
            continue;
        };
        let line = source[..start].matches('\n').count() + 1;
        found.push((handle.to_string(), line));
    }
    found
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lsf_core::theme::Revalidate;

    use super::*;

    fn theme(name: &str, files: &[(&str, &str)]) -> (Theme, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("lsf-menus-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, content) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let env = Arc::new(crate::environment());
        let theme = Theme::open(&root, env, Revalidate::Never).expect("theme");
        (theme, root)
    }

    fn pairs(references: &[MenuReference]) -> Vec<(&str, &str)> {
        references
            .iter()
            .map(|reference| (reference.handle.as_str(), reference.place.as_str()))
            .collect()
    }

    #[test]
    fn liquid_lookups_by_name_are_found_with_their_line() {
        let source = "{{ linklists.footer.title }}\n\
                      {% for link in linklists['main-menu'].links %}{% endfor %}\n\
                      {{ linklists[ \"legal\" ].links.size }} {{ linklists.size }}\n\
                      {{ linklists[settings.menu] }} {{ section.settings.linklists.x }}";
        assert_eq!(
            linklists_handles(source),
            [
                ("footer".to_string(), 1),
                ("main-menu".to_string(), 2),
                ("legal".to_string(), 3)
            ]
        );
    }

    #[test]
    fn the_menus_of_settings_templates_groups_and_blocks_are_found() {
        let menu_setting = |default: &str| {
            format!(
                r#"{{"type": "link_list", "id": "menu", "label": "Menu", "default": "{default}"}}"#
            )
        };
        let header = format!(
            "{{% schema %}}{{\"name\": \"Header\", \"settings\": [{}], \"blocks\": [{{\"type\": \"@theme\"}}, {{\"type\": \"column\", \"name\": \"Column\", \"settings\": [{}]}}]}}{{% endschema %}}",
            menu_setting("main-menu"),
            menu_setting("footer")
        );
        let block = format!(
            "{{% schema %}}{{\"name\": \"Links\", \"settings\": [{}]}}{{% endschema %}}",
            menu_setting("footer")
        );
        let (theme, root) = theme(
            "settings",
            &[
                ("layout/theme.liquid", "{{ content_for_layout }}"),
                (
                    "config/settings_schema.json",
                    &format!(
                        r#"[{{"name": "theme_info"}}, {{"name": "Nav", "settings": [{}, {{"type": "text", "id": "x", "label": "X"}}]}}]"#,
                        menu_setting("main-menu")
                    ),
                ),
                (
                    "config/settings_data.json",
                    r#"{"current": {"menu": "top-menu"}}"#,
                ),
                ("sections/header.liquid", &header),
                ("blocks/links.liquid", &block),
                (
                    "sections/header-group.json",
                    r#"{"type": "header", "name": "Header", "sections": {
                        "header": {"type": "header", "settings": {"menu": "main-menu"}, "blocks": {
                            "col": {"type": "column", "settings": {"menu": ""}},
                            "links": {"type": "links", "blocks": {"inner": {"type": "links", "settings": {"menu": "legal"}}}},
                            "app": {"type": "shopify://apps/demo-app/blocks/menu/123", "settings": {"menu": "app-menu"}}
                        }},
                        "off": {"type": "header", "disabled": true, "settings": {"menu": "hidden"}}
                    }, "order": ["header", "off"]}"#,
                ),
                (
                    "templates/index.json",
                    r#"{"sections": {"main": {"type": "header"}}, "order": ["main"]}"#,
                ),
                (
                    "snippets/nav.liquid",
                    "\n{% for link in linklists['customer-care'].links %}{% endfor %}",
                ),
            ],
        );
        let references = theme.menu_references();
        let _ = std::fs::remove_dir_all(root);
        assert_eq!(
            pairs(&references),
            [
                ("top-menu", "config/settings_data.json at /current/menu"),
                (
                    "main-menu",
                    "sections/header-group.json at /sections/header/settings/menu"
                ),
                // A block without a value has its default; an empty value names nothing.
                ("footer", "the default of \"menu\" in blocks/links.liquid"),
                (
                    "legal",
                    "sections/header-group.json at /sections/header/blocks/links/blocks/inner/settings/menu"
                ),
                (
                    "main-menu",
                    "the default of \"menu\" in sections/header.liquid"
                ),
                ("customer-care", "snippets/nav.liquid line 2"),
            ]
        );
    }
}

//! The app blocks a theme names, and what is wrong with them: a block the app does not have, a
//! block of another app, a block of the app that no template uses.

use std::collections::BTreeMap;

use serde_json::Value as Json;

use super::{AppExtensions, Lookup, is_app_type};
use crate::Theme;

/// An app block a theme file names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// `templates/product.json`, `sections/footer-group.json`, `config/settings_data.json`.
    pub file: String,
    /// Where in the file: `/sections/main/blocks/reviews`.
    pub pointer: String,
    pub kind: String,
    /// `"disabled": true`.
    pub disabled: bool,
    /// An app embed block of `current.blocks` of `config/settings_data.json`.
    pub embed: bool,
}

/// How much a finding matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The theme names something that renders nothing.
    Warning,
    /// Worth knowing: a block no template uses, an app embed block that is off.
    Info,
}

/// What is wrong with the app blocks of a theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    /// The theme file, or the block file of the extension.
    pub file: String,
    pub pointer: Option<String>,
    pub message: String,
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Collects the app blocks of `value`. `embeds` is where the theme settings are, whose
/// `blocks` are app embed blocks.
fn walk(file: &str, value: &Json, pointer: &str, embeds: Option<&str>, out: &mut Vec<Reference>) {
    match value {
        Json::Object(object) => {
            if let Some(kind) = object.get("type").and_then(Json::as_str)
                && is_app_type(kind)
                && kind != "@app"
            {
                out.push(Reference {
                    file: file.to_string(),
                    pointer: pointer.to_string(),
                    kind: kind.to_string(),
                    disabled: object
                        .get("disabled")
                        .and_then(Json::as_bool)
                        .unwrap_or(false),
                    embed: embeds.is_some_and(|base| {
                        pointer
                            .rsplit_once('/')
                            .is_some_and(|(parent, _)| parent == format!("{base}/blocks"))
                    }),
                });
            }
            for (key, child) in object {
                walk(
                    file,
                    child,
                    &format!("{pointer}/{}", escape(key)),
                    embeds,
                    out,
                );
            }
        }
        Json::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk(file, item, &format!("{pointer}/{index}"), embeds, out);
            }
        }
        _ => {}
    }
}

/// The app blocks the JSON templates, the section groups and the theme settings name: app
/// blocks in sections, and the app embed blocks of `current.blocks` of
/// `config/settings_data.json`.
pub fn references(theme: &Theme) -> Vec<Reference> {
    let mut out = Vec::new();
    for directory in [
        "templates",
        "templates/customers",
        "templates/metaobject",
        "sections",
    ] {
        for name in theme.files().list(directory).iter() {
            if !name.ends_with(".json") {
                continue;
            }
            let file = format!("{directory}/{name}");
            if let Ok(Some(json)) = theme.json(&file) {
                walk(&file, &json, "", None, &mut out);
            }
        }
    }
    let file = "config/settings_data.json";
    if let Ok(Some(json)) = theme.json(file) {
        match json.get("current") {
            Some(Json::String(name)) => {
                if let Some(preset) = json.get("presets").and_then(|presets| presets.get(name)) {
                    let base = format!("/presets/{}", escape(name));
                    walk(file, preset, &base, Some(&base), &mut out);
                }
            }
            Some(current) => walk(file, current, "/current", Some("/current"), &mut out),
            None => {}
        }
    }
    out
}

/// What is wrong with the app blocks of `theme` for the apps of `apps`: what renders nothing
/// (warnings), and the blocks of the app that never render (information). `store_embeds` are the
/// app embed blocks the store data turns on (`app.embeds`).
pub fn check(theme: &Theme, apps: &AppExtensions, store_embeds: &[String]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let mut embeds_on: Vec<String> = store_embeds.to_vec();
    // The blocks of apps that are not served, by app: one finding for each.
    let mut others: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for reference in references(theme) {
        let warning = |message: String| Finding {
            level: Level::Warning,
            file: reference.file.clone(),
            pointer: Some(reference.pointer.clone()),
            message,
        };
        match apps.lookup(&reference.kind, reference.embed) {
            Lookup::Found(block) => {
                used.push(block.path());
                if reference.embed && !reference.disabled {
                    embeds_on.push(block.name.clone());
                }
                if reference.embed && !block.target.is_embed() {
                    findings.push(warning(format!(
                        "{} is an app block (its target is not `head`, `compliance_head` or `body`): as an app embed block it renders nothing",
                        block.path()
                    )));
                } else if !reference.embed && block.target.is_embed() {
                    findings.push(warning(format!(
                        "{} is an app embed block: in a section it renders nothing (it renders from `current.blocks` of config/settings_data.json, or `app.embeds` of the store data)",
                        block.path()
                    )));
                }
            }
            Lookup::Broken { path, error } => {
                used.push(path.clone());
                findings.push(warning(format!("{path} does not parse: {error}")));
            }
            Lookup::NoSuchBlock { app, block, known } => {
                let hint = lsf_core::util::closest_match(&block, known.iter().map(String::as_str))
                    .map(|name| format!(": did you mean `{name}`?"))
                    .unwrap_or_else(|| {
                        if known.is_empty() {
                            String::new()
                        } else {
                            format!(" (it has {})", known.join(", "))
                        }
                    });
                findings.push(warning(format!(
                    "{}: the app {app} has no {} `{block}`{hint}",
                    reference.kind,
                    if reference.embed {
                        "app embed block"
                    } else {
                        "block"
                    }
                )));
            }
            Lookup::OtherApp { app } => {
                if !reference.disabled {
                    others
                        .entry(app)
                        .or_default()
                        .push(format!("{}#{}", reference.file, reference.pointer));
                }
            }
            Lookup::NotAnApp => findings.push(warning(format!(
                "{}: not the type of an app block (shopify://apps/<app>/blocks/<block>/<id>)",
                reference.kind
            ))),
        }
    }
    for (app, places) in others {
        let served: Vec<&str> = apps.apps().iter().map(|app| app.handle.as_str()).collect();
        findings.push(Finding {
            level: Level::Info,
            file: places[0].split('#').next().unwrap_or_default().to_string(),
            pointer: None,
            message: format!(
                "{} of the app {app}, which is not served ({}): {} nothing",
                lsf_core::util::count(places.len(), "block", "blocks"),
                if served.is_empty() {
                    "no app is".to_string()
                } else {
                    format!("the app served is {}", served.join(", "))
                },
                if places.len() == 1 {
                    "it renders"
                } else {
                    "they render"
                }
            ),
        });
    }
    for block in apps.blocks().into_iter().filter_map(|block| block.ok()) {
        if block.target.is_embed() {
            if !embeds_on.contains(&block.name) {
                findings.push(Finding {
                    level: Level::Info,
                    file: block.path(),
                    pointer: None,
                    message: format!(
                        "app embed block `{}` is off: turn it on with `\"app\": {{\"embeds\": {{\"{}\": true}}}}` in the store data",
                        block.name, block.name
                    ),
                });
            }
        } else if !used.contains(&block.path()) {
            findings.push(Finding {
                level: Level::Info,
                file: block.path(),
                pointer: None,
                message: format!(
                    "block `{}` is in no template: add `{{\"type\": \"{}\"}}` to the blocks of a section that accepts `@app` blocks",
                    block.name,
                    block.type_name()
                ),
            });
        }
    }
    findings
}

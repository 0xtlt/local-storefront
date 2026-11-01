//! The extensions of the app that lsf-checkout lists without reading (`App::other`): its web
//! pixel extension and its Flow triggers, read from their `shopify.extension.toml` as Shopify
//! CLI deploys them.
//!
//! - A **web pixel extension** (`type = "web_pixel_extension"`, at the top of the file or in an
//!   `[[extensions]]`): its `[settings.fields.<key>]`, each a `type` (only
//!   `single_line_text_field` is supported, says shopify.dev's "Build web pixels") and
//!   `validations` (`[{name, value}]`, metafield validations). `webPixelCreate` checks the
//!   settings it is given against them.
//! - A **Flow trigger** (`type = "flow_trigger"`): its `handle` and its `[[settings.fields]]`,
//!   each a `type` and, for a custom field, a `key` ("Flow trigger reference"). A reference
//!   field (`customer_reference`, …) has no key: its payload key is fixed (`customer_id`).
//!
//! Read once, when the shop is seeded, into settings of the shop (JSON): the mutations need the
//! definitions, not the files.

use std::path::Path;

use serde_json::{Map, Value, json};

/// What the app's extensions define, as the seed keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extensions {
    /// `{handle, fields: {<key>: {type, validations}}}`.
    pub web_pixel: Option<Value>,
    /// `[{handle, name, fields: [{type, key, payloadKey}]}]`.
    pub flow_triggers: Vec<Value>,
}

/// The Flow reference fields ("Reference field types" of the trigger reference): each type, the
/// record type its id names, and its payload key.
const REFERENCE_FIELDS: [(&str, &str, &str); 5] = [
    ("customer_reference", "Customer", "customer_id"),
    ("order_reference", "Order", "order_id"),
    ("product_reference", "Product", "product_id"),
    ("company_reference", "Company", "company_id"),
    (
        "company_contact_reference",
        "CompanyContact",
        "company_contact_id",
    ),
];

/// The record type and the payload key of a Flow reference field.
pub fn reference_field(kind: &str) -> Option<(&'static str, &'static str)> {
    REFERENCE_FIELDS
        .iter()
        .find(|(name, _, _)| *name == kind)
        .map(|(_, type_name, key)| (*type_name, *key))
}

/// The extensions of the app in `root`: every `extensions/*/shopify.extension.toml`. A file that
/// does not parse is skipped (lsf-checkout reports it when it loads the app).
pub fn read(root: &Path) -> Extensions {
    let mut extensions = Extensions::default();
    let Ok(entries) = std::fs::read_dir(root.join("extensions")) else {
        return extensions;
    };
    let mut directories: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    directories.sort();
    for directory in directories {
        let Ok(text) = std::fs::read_to_string(directory.join("shopify.extension.toml")) else {
            continue;
        };
        let Ok(table) = text.parse::<toml::Table>() else {
            continue;
        };
        let name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let mut tables: Vec<&toml::Table> = match table.get("extensions") {
            Some(toml::Value::Array(items)) => items.iter().filter_map(|v| v.as_table()).collect(),
            _ => Vec::new(),
        };
        if table.contains_key("type") {
            tables.insert(0, &table);
        }
        for entry in tables {
            let handle = entry
                .get("handle")
                .and_then(|v| v.as_str())
                .map_or_else(|| name.clone(), str::to_string);
            // `[settings]` below `[[extensions]]` is a table of the file, not of the entry, in
            // TOML: Shopify CLI reads it as the extension's, and so does lsf.
            let settings = entry.get("settings").or_else(|| table.get("settings"));
            match entry.get("type").and_then(|v| v.as_str()) {
                Some("web_pixel_extension") if extensions.web_pixel.is_none() => {
                    extensions.web_pixel = Some(web_pixel(&handle, settings));
                }
                Some("flow_trigger") => extensions
                    .flow_triggers
                    .push(flow_trigger(&handle, entry, settings)),
                _ => {}
            }
        }
    }
    extensions
}

fn to_json(value: &toml::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn web_pixel(handle: &str, settings: Option<&toml::Value>) -> Value {
    let mut fields = Map::new();
    if let Some(toml::Value::Table(defined)) = settings.and_then(|settings| settings.get("fields"))
    {
        for (key, field) in defined {
            let kind = field
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("single_line_text_field");
            let validations = field
                .get("validations")
                .map(to_json)
                .unwrap_or_else(|| json!([]));
            fields.insert(
                key.clone(),
                json!({"type": kind, "validations": validations}),
            );
        }
    }
    json!({"handle": handle, "fields": fields})
}

fn flow_trigger(handle: &str, entry: &toml::Table, settings: Option<&toml::Value>) -> Value {
    let fields: Vec<Value> = match settings.and_then(|settings| settings.get("fields")) {
        Some(toml::Value::Array(items)) => items
            .iter()
            .filter_map(|field| {
                let kind = field.get("type")?.as_str()?;
                let key = field.get("key").and_then(|v| v.as_str());
                let payload_key = reference_field(kind).map(|(_, key)| key).or(key)?;
                Some(json!({"type": kind, "key": key, "payloadKey": payload_key}))
            })
            .collect(),
        _ => Vec::new(),
    };
    json!({
        "handle": handle,
        "name": entry.get("name").and_then(|v| v.as_str()),
        "fields": fields,
    })
}

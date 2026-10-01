//! The field reference of the data format, generated from the JSON Schema so that it cannot
//! drift from what the validator accepts.

use serde_json::Value as Json;

use super::validate::{FileKind, schema};

/// `ProductInput` → `Product`: the name the documentation uses for a type.
fn display_name(definition: &str) -> &str {
    definition.strip_suffix("Input").unwrap_or(definition)
}

fn anchor(definition: &str) -> String {
    display_name(definition).to_lowercase()
}

fn reference_name(node: &Json) -> Option<&str> {
    node.get("$ref")?.as_str()?.strip_prefix("#/$defs/")
}

/// How a schema node reads as a type, in Markdown.
fn type_of(node: &Json) -> String {
    if let Some(name) = reference_name(node) {
        return format!("[{}](#{})", display_name(name), anchor(name));
    }
    for keyword in ["anyOf", "oneOf"] {
        if let Some(alternatives) = node.get(keyword).and_then(Json::as_array) {
            let types: Vec<String> = alternatives
                .iter()
                .filter(|alternative| alternative.get("type") != Some(&Json::from("null")))
                .map(|alternative| match alternative {
                    Json::Bool(true) => "any JSON value".to_string(),
                    Json::Object(object) if object.contains_key("const") => {
                        format!("`{}`", object["const"])
                    }
                    other => type_of(other),
                })
                .collect();
            return types.join(" or ");
        }
    }
    if let Some(values) = node.get("enum").and_then(Json::as_array) {
        let values: Vec<String> = values.iter().map(|value| format!("`{value}`")).collect();
        return values.join(", ");
    }
    let name = match node.get("type") {
        Some(Json::String(name)) => name.as_str(),
        // `["string", "null"]`: an optional value.
        Some(Json::Array(names)) => names
            .iter()
            .filter_map(Json::as_str)
            .find(|name| *name != "null")
            .unwrap_or("null"),
        _ => return "any JSON value".to_string(),
    };
    match name {
        "array" => match node.get("items") {
            Some(items) => format!("array of {}", type_of(items)),
            None => "array".to_string(),
        },
        "object" => match node.get("additionalProperties") {
            Some(Json::Bool(true)) => "map of any JSON value".to_string(),
            Some(values @ Json::Object(_)) => format!("map of {}", type_of(values)),
            _ => "object".to_string(),
        },
        other => other.to_string(),
    }
}

/// Whether a default is the "nothing" every optional field has.
fn is_empty_default(value: &Json) -> bool {
    match value {
        Json::Null | Json::Bool(false) => true,
        Json::String(text) => text.is_empty(),
        Json::Array(items) => items.is_empty(),
        Json::Object(map) => map.values().all(is_empty_default),
        Json::Bool(true) | Json::Number(_) => false,
    }
}

/// A default worth mentioning.
fn notable_default(node: &Json) -> Option<String> {
    node.get("default")
        .filter(|value| !is_empty_default(value))
        .map(Json::to_string)
}

/// Text that fits in a table cell.
fn cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

fn describe(out: &mut String, title: &str, node: &Json) {
    out.push_str(&format!("### {title}\n\n"));
    if let Some(description) = node.get("description").and_then(Json::as_str) {
        out.push_str(description);
        out.push_str("\n\n");
    }
    if let Some(properties) = node.get("properties").and_then(Json::as_object) {
        let required: Vec<&str> = node
            .get("required")
            .and_then(Json::as_array)
            .map(|names| names.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        out.push_str("| Field | Type | Description |\n|---|---|---|\n");
        for (name, property) in properties {
            let mut description = String::new();
            if required.contains(&name.as_str()) {
                description.push_str("**Required.** ");
            }
            if let Some(text) = property.get("description").and_then(Json::as_str) {
                description.push_str(text);
            }
            if let Some(default) = notable_default(property)
                && !description.contains("Defaults to")
            {
                description.push_str(&format!(" Defaults to `{default}`."));
            }
            out.push_str(&format!(
                "| `{name}` | {} | {} |\n",
                cell(&type_of(property)),
                cell(description.trim())
            ));
        }
        out.push('\n');
        return;
    }
    // A union or an enumeration: list what can be written.
    for keyword in ["anyOf", "oneOf"] {
        if let Some(alternatives) = node.get(keyword).and_then(Json::as_array) {
            out.push_str("One of:\n\n");
            for alternative in alternatives {
                let kind = match alternative {
                    Json::Bool(true) => "any JSON value".to_string(),
                    Json::Object(object) if object.contains_key("const") => {
                        format!("`{}`", object["const"])
                    }
                    other => type_of(other),
                };
                match alternative.get("description").and_then(Json::as_str) {
                    Some(description) => {
                        out.push_str(&format!("- {kind}: {}\n", cell(description)))
                    }
                    None => out.push_str(&format!("- {kind}\n")),
                }
            }
            out.push('\n');
            return;
        }
    }
    if node.get("enum").is_some() {
        out.push_str(&format!("One of: {}.\n\n", type_of(node)));
    }
}

/// The reference of every type of the data format, as Markdown.
pub fn reference() -> String {
    let root = schema(FileKind::Store);
    let mut out = String::new();
    describe(&mut out, "Data file", &root);
    if let Some(definitions) = root.get("$defs").and_then(Json::as_object) {
        for (name, definition) in definitions {
            describe(&mut out, display_name(name), definition);
        }
    }
    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_every_type() {
        let reference = reference();
        for title in [
            "### Data file",
            "### Product",
            "### Variant",
            "### Money",
            "### Session",
        ] {
            assert!(reference.contains(title), "missing {title}");
        }
        assert!(reference.contains("| `title` | string | **Required.** |"));
        assert!(reference.contains("| `variants` | array of [Variant](#variant) |"));
        assert!(reference.contains("| `menus` | map of [Menu](#menu) |"));
        // No link points to a type that is not described.
        for (index, _) in reference.match_indices("](#") {
            let target = &reference[index + 3..];
            let target = &target[..target.find(')').expect("closed link")];
            let heading = reference
                .lines()
                .filter_map(|line| line.strip_prefix("### "))
                .any(|title| title.to_lowercase() == target);
            assert!(heading, "dangling link #{target}");
        }
    }
}

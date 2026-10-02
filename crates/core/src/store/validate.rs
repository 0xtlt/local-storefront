//! Structural validation of data files against the JSON Schema generated from [`super::model`].
//!
//! The aim is messages precise enough to act on without reading the schema: which file, which
//! JSON path, what is wrong, and what to write instead.

use std::sync::OnceLock;

use jsonschema::error::ValidationErrorKind;
use jsonschema::{ValidationError, Validator};
use schemars::{JsonSchema, schema_for};
use serde_json::Value as Json;

use super::model;
use crate::diagnostics::Diagnostics;
use crate::util::closest_match;

/// What a data file contains, which decides the schema it is checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A root-level file: any combination of shop, products, collections, ...
    Store,
    Product,
    Collection,
    Page,
    Blog,
    Customer,
    Menu,
    /// The state of one browser session (`PUT /__lsf/session`).
    Session,
}

impl FileKind {
    pub const ALL: [FileKind; 8] = [
        FileKind::Store,
        FileKind::Product,
        FileKind::Collection,
        FileKind::Page,
        FileKind::Blog,
        FileKind::Customer,
        FileKind::Menu,
        FileKind::Session,
    ];

    /// The name used for schema files and in the documentation.
    pub fn name(self) -> &'static str {
        match self {
            FileKind::Store => "store",
            FileKind::Product => "product",
            FileKind::Collection => "collection",
            FileKind::Page => "page",
            FileKind::Blog => "blog",
            FileKind::Customer => "customer",
            FileKind::Menu => "menu",
            FileKind::Session => "session",
        }
    }

    /// The directory whose files hold one entity of this kind each.
    pub fn directory(self) -> Option<&'static str> {
        match self {
            FileKind::Product => Some("products"),
            FileKind::Collection => Some("collections"),
            FileKind::Page => Some("pages"),
            FileKind::Blog => Some("blogs"),
            FileKind::Customer => Some("customers"),
            FileKind::Menu => Some("menus"),
            FileKind::Store | FileKind::Session => None,
        }
    }

    fn index(self) -> usize {
        FileKind::ALL
            .iter()
            .position(|kind| *kind == self)
            .expect("listed in ALL")
    }
}

fn schema_json<T: JsonSchema>() -> Json {
    serde_json::to_value(schema_for!(T)).expect("schemas serialize")
}

/// The JSON Schema of a file kind.
pub fn schema(kind: FileKind) -> Json {
    match kind {
        FileKind::Store => schema_json::<model::StoreInput>(),
        FileKind::Product => schema_json::<model::ProductInput>(),
        FileKind::Collection => schema_json::<model::CollectionInput>(),
        FileKind::Page => schema_json::<model::PageInput>(),
        FileKind::Blog => schema_json::<model::BlogInput>(),
        FileKind::Customer => schema_json::<model::CustomerInput>(),
        FileKind::Menu => schema_json::<model::MenuInput>(),
        FileKind::Session => schema_json::<model::SessionInput>(),
    }
}

struct Compiled {
    schema: Json,
    validator: Validator,
}

fn compiled(kind: FileKind) -> &'static Compiled {
    static CACHE: OnceLock<Vec<Compiled>> = OnceLock::new();
    &CACHE.get_or_init(|| {
        FileKind::ALL
            .iter()
            .map(|kind| {
                let schema = schema(*kind);
                let validator =
                    jsonschema::validator_for(&schema).expect("generated schemas are valid");
                Compiled { schema, validator }
            })
            .collect()
    })[kind.index()]
}

/// Validates one JSON document and appends what is wrong with it to `diagnostics`.
/// `pointer_prefix` is prepended to the reported paths (for items of an array file).
pub fn validate(
    kind: FileKind,
    file: &str,
    instance: &Json,
    pointer_prefix: &str,
    diagnostics: &mut Diagnostics,
) {
    let compiled = compiled(kind);
    for error in compiled.validator.iter_errors(instance) {
        report(&compiled.schema, &error, file, pointer_prefix, diagnostics);
    }
}

fn describe(value: &Json) -> String {
    let kind = match value {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Number(number) if number.is_f64() => "a decimal number",
        Json::Number(_) => "an integer",
        Json::String(_) => "a string",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    };
    let mut text = value.to_string();
    if text.chars().count() > 50 {
        text = format!("{}…", text.chars().take(50).collect::<String>());
    }
    format!("{kind} ({text})")
}

/// The schema node a failing keyword belongs to.
fn schema_node<'s>(schema: &'s Json, error: &ValidationError<'_>) -> Option<&'s Json> {
    let path = error.schema_path().to_string();
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    schema.pointer(parent)
}

fn type_name(kind: &jsonschema::error::TypeKind) -> String {
    let text = format!("{kind:?}").to_lowercase();
    // `Single(String)` / `Multiple(...)`: keep it readable without depending on the layout.
    for name in [
        "string", "integer", "number", "boolean", "array", "object", "null",
    ] {
        if text.contains(name) {
            return match name {
                "integer" | "array" | "object" => format!("an {name}"),
                "null" => "null".to_string(),
                other => format!("a {other}"),
            };
        }
    }
    text
}

fn report(
    schema: &Json,
    error: &ValidationError<'_>,
    file: &str,
    prefix: &str,
    diagnostics: &mut Diagnostics,
) {
    let path = format!("{prefix}{}", error.instance_path());
    let node = schema_node(schema, error);
    match error.kind() {
        ValidationErrorKind::AdditionalProperties { unexpected }
        | ValidationErrorKind::UnevaluatedProperties { unexpected } => {
            let known: Vec<&str> = node
                .and_then(|node| node.get("properties"))
                .and_then(Json::as_object)
                .map(|properties| properties.keys().map(String::as_str).collect())
                .unwrap_or_default();
            for field in unexpected {
                let diagnostic = diagnostics.error(
                    "unknown_field",
                    file,
                    &format!("{path}/{field}"),
                    format!("unknown field \"{field}\""),
                );
                match closest_match(field, known.iter().copied()) {
                    Some(suggestion) => diagnostic.hint(format!("did you mean \"{suggestion}\"?")),
                    None => diagnostic.hint(format!("allowed fields here: {}", known.join(", "))),
                };
            }
        }
        ValidationErrorKind::Required { property } => {
            let name = property.as_str().unwrap_or_default();
            let diagnostic = diagnostics.error(
                "missing_field",
                file,
                &path,
                format!("missing required field \"{name}\""),
            );
            if let Some(description) = node
                .and_then(|node| node.pointer(&format!("/properties/{name}/description")))
                .and_then(Json::as_str)
            {
                diagnostic.hint(description.to_string());
            }
        }
        ValidationErrorKind::Type { kind } => {
            diagnostics.error(
                "wrong_type",
                file,
                &path,
                format!(
                    "expected {}, found {}",
                    type_name(kind),
                    describe(error.instance())
                ),
            );
        }
        ValidationErrorKind::Enum { options } => {
            let allowed: Vec<String> = options
                .as_array()
                .map(|options| {
                    options
                        .iter()
                        .map(|option| {
                            option
                                .as_str()
                                .map_or_else(|| option.to_string(), str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let diagnostic = diagnostics.error(
                "invalid_value",
                file,
                &path,
                format!("{} is not an accepted value", describe(error.instance())),
            );
            let suggestion = error
                .instance()
                .as_str()
                .and_then(|value| closest_match(value, allowed.iter().map(String::as_str)));
            match suggestion {
                Some(suggestion) => diagnostic.hint(format!(
                    "did you mean \"{suggestion}\"? Accepted values: {}",
                    allowed.join(", ")
                )),
                None => diagnostic.hint(format!("accepted values: {}", allowed.join(", "))),
            };
        }
        ValidationErrorKind::AnyOf { context } | ValidationErrorKind::OneOfNotValid { context } => {
            // Report the alternative that has the right JSON type, if there is exactly one:
            // its errors are far more useful than "does not match any alternative".
            let matching: Vec<&Vec<ValidationError<'_>>> = context
                .iter()
                .filter(|branch| {
                    !branch.iter().any(|inner| {
                        matches!(inner.kind(), ValidationErrorKind::Type { .. })
                            && inner.instance_path() == error.instance_path()
                    })
                })
                .collect();
            if let [branch] = matching.as_slice() {
                for inner in branch.iter() {
                    report(schema, inner, file, prefix, diagnostics);
                }
                return;
            }
            let diagnostic = diagnostics.error(
                "invalid_value",
                file,
                &path,
                format!("{} is not valid here", describe(error.instance())),
            );
            if let Some(description) = node
                .and_then(|node| node.get("description"))
                .and_then(Json::as_str)
            {
                diagnostic.hint(description.to_string());
            }
        }
        ValidationErrorKind::Pattern { .. } => {
            let diagnostic = diagnostics.error(
                "invalid_format",
                file,
                &path,
                format!(
                    "{} does not have the expected format",
                    describe(error.instance())
                ),
            );
            if let Some(description) = node
                .and_then(|node| node.get("description"))
                .and_then(Json::as_str)
            {
                diagnostic.hint(description.to_string());
            }
        }
        _ => {
            diagnostics.error("invalid_value", file, &path, error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(kind: FileKind, json: Json) -> Diagnostics {
        let mut diagnostics = Diagnostics::new();
        validate(kind, "test.json", &json, "", &mut diagnostics);
        diagnostics
    }

    #[test]
    fn accepts_a_minimal_product() {
        let diagnostics = check(
            FileKind::Product,
            serde_json::json!({"title": "Shirt", "price": 1999}),
        );
        assert!(diagnostics.is_empty(), "{diagnostics}");
    }

    #[test]
    fn explains_typos() {
        let diagnostics = check(
            FileKind::Product,
            serde_json::json!({"title": "Shirt", "prise": 1999}),
        );
        assert_eq!(diagnostics.items.len(), 1, "{diagnostics}");
        assert_eq!(diagnostics.items[0].code, "unknown_field");
        assert_eq!(diagnostics.items[0].path, "/prise");
        assert_eq!(
            diagnostics.items[0].hint.as_deref(),
            Some("did you mean \"price\"?")
        );
    }

    #[test]
    fn explains_missing_fields_and_wrong_types() {
        let diagnostics = check(FileKind::Product, serde_json::json!({"tags": "sale"}));
        let codes: Vec<&str> = diagnostics.items.iter().map(|item| item.code).collect();
        assert!(codes.contains(&"missing_field"), "{diagnostics}");
        assert!(codes.contains(&"wrong_type"), "{diagnostics}");
    }

    #[test]
    fn explains_ambiguous_money() {
        let diagnostics = check(
            FileKind::Product,
            serde_json::json!({"title": "Shirt", "price": 19.99}),
        );
        assert_eq!(diagnostics.items.len(), 1, "{diagnostics}");
        assert_eq!(diagnostics.items[0].path, "/price");
        assert!(
            diagnostics.items[0]
                .hint
                .as_deref()
                .unwrap_or_default()
                .contains("cents"),
            "{diagnostics}"
        );
    }

    #[test]
    fn reports_nested_paths() {
        let diagnostics = check(
            FileKind::Store,
            serde_json::json!({"products": [{"title": "A", "price": 1}, {"title": "B", "variants": [{"prize": 2}]}]}),
        );
        assert_eq!(
            diagnostics.items[0].path, "/products/1/variants/0/prize",
            "{diagnostics}"
        );
    }

    #[test]
    fn suggests_enum_values() {
        let diagnostics = check(
            FileKind::Collection,
            serde_json::json!({"title": "A", "sort_order": "price-asc"}),
        );
        assert!(
            diagnostics.items[0]
                .hint
                .as_deref()
                .unwrap_or_default()
                .contains("price-ascending"),
            "{diagnostics}"
        );
    }
}

//! Metafield and metaobject definitions declared in `shopify-local/`: their resolved types,
//! schema validation (valid types, owner types, namespaces, keys, validations, capabilities,
//! pinning, portable metaobject type references), and enforcement against metafield values and
//! metaobject entries.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use indexmap::IndexMap;
use serde_json::Value as Json;

use super::build::{Origin, Sourced};
use super::metafield_references::parse_gid;
use super::metafield_types;
use super::model::{
    self, CapabilityFlag, DefinitionAdminAccess, DefinitionStorefrontAccess,
    MetafieldCustomerAccountAccess, MetafieldOwnerType, MetaobjectCustomerAccountAccess,
    OnlineStoreCapabilityInput, RenderableCapabilityInput,
};
use super::{Store, same_metaobject_type};
use crate::diagnostics::Diagnostics;
use crate::util::{closest_match, humanize, stable_id};

/// Maximum pinned metafield definitions per owner type (Shopify's admin limit).
pub const PINNED_LIMIT: usize = 20;

/// A resolved validation rule on a metafield definition or metaobject field definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetafieldValidation {
    /// Canonical Shopify validation name (`min`, `max`, `regex`, `choices`,
    /// `metaobject_definition_id`, `metaobject_definition_ids`, ...).
    pub name: String,
    /// Shopify validation value type (`number_integer`, `single_line_text_field`,
    /// `list.single_line_text_field`, ...).
    pub value_type: String,
    /// Canonical string value (compact JSON for list/object values; portable metaobject type
    /// name(s) or explicit GID(s) for `metaobject_definition_id(s)`).
    pub value: String,
}

/// Resolved access controls of a metafield definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetafieldAccess {
    pub admin: Option<DefinitionAdminAccess>,
    pub storefront: Option<DefinitionStorefrontAccess>,
    pub customer_account: Option<MetafieldCustomerAccountAccess>,
}

/// Resolved capabilities of a metafield definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetafieldCapabilities {
    pub admin_filterable: bool,
    pub analytics_queryable: bool,
    pub cart_to_order_copyable: bool,
    pub smart_collection_condition: bool,
    pub unique_values: bool,
}

/// A resolved metafield definition declared in the store data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetafieldDefinition {
    pub owner_type: MetafieldOwnerType,
    /// Namespace as written in the store data (`"custom"`, `"$app"`, `"$app:config"`).
    pub namespace: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    /// Shopify metafield type name (e.g. `"multi_line_text_field"`).
    pub kind: String,
    pub validations: Vec<MetafieldValidation>,
    /// 1-based pinned position if pinned, `None` otherwise.
    pub pinned_position: Option<u32>,
    pub access: MetafieldAccess,
    pub capabilities: MetafieldCapabilities,
    pub standard_template: bool,
}

/// Resolved access controls of a metaobject definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetaobjectAccess {
    pub admin: Option<DefinitionAdminAccess>,
    pub storefront: Option<DefinitionStorefrontAccess>,
    pub customer_account: Option<MetaobjectCustomerAccountAccess>,
}

/// Resolved Online Store capability of a metaobject definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnlineStoreCapability {
    pub enabled: bool,
    pub url_handle: Option<String>,
    pub create_redirects: bool,
}

/// Resolved Renderable capability of a metaobject definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderableCapability {
    pub enabled: bool,
    pub meta_title_key: Option<String>,
    pub meta_description_key: Option<String>,
}

/// Resolved capabilities of a metaobject definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetaobjectCapabilities {
    pub publishable: Option<bool>,
    pub translatable: Option<bool>,
    pub online_store: Option<OnlineStoreCapability>,
    pub renderable: Option<RenderableCapability>,
}

/// A resolved field definition of a metaobject definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaobjectFieldDefinition {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: String,
    pub required: bool,
    pub validations: Vec<MetafieldValidation>,
    pub admin_filterable: bool,
}

/// A resolved metaobject definition declared in the store data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaobjectDefinition {
    /// Type as written in the store data (e.g. `"look"`, `"$app:author"`).
    pub kind: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub display_name_key: Option<String>,
    /// Effective URL handle if explicitly specified via top-level `url_handle` or
    /// `capabilities.online_store`.
    pub url_handle: Option<String>,
    pub access: MetaobjectAccess,
    pub capabilities: MetaobjectCapabilities,
    pub standard_template: bool,
    /// `Some(...)` when `field_definitions` was declared; `None` when omitted (backward-compatible
    /// minimal definitions such as `{"url_handle": "designers"}` infer fields from entries).
    pub field_definitions: Option<Vec<MetaobjectFieldDefinition>>,
}

impl MetaobjectDefinition {
    /// Whether this definition only specifies `url_handle` (the legacy minimal form) and leaves
    /// fields and capabilities to be inferred from entries.
    pub fn is_minimal_url_handle_only(&self) -> bool {
        self.field_definitions.is_none()
            && self.name.is_none()
            && self.description.is_none()
            && self.display_name_key.is_none()
            && self.access == MetaobjectAccess::default()
            && self.capabilities == MetaobjectCapabilities::default()
            && !self.standard_template
    }
}

/// Validations supported by a Shopify metafield type: `(canonical_name, value_type)`.
/// Matches Shopify's 2026-10 `metafieldDefinitionTypes`.
pub fn supported_validations(kind: &str) -> Vec<(&'static str, String)> {
    let base = kind.strip_prefix("list.").unwrap_or(kind);
    let mut validations: Vec<(&'static str, String)> = match base {
        "single_line_text_field" => vec![
            ("min", "number_integer".into()),
            ("max", "number_integer".into()),
            ("regex", "single_line_text_field".into()),
            ("choices", "list.single_line_text_field".into()),
        ],
        "multi_line_text_field" | "id" => vec![
            ("min", "number_integer".into()),
            ("max", "number_integer".into()),
            ("regex", "single_line_text_field".into()),
        ],
        "number_integer" => vec![
            ("min", "number_integer".into()),
            ("max", "number_integer".into()),
        ],
        "number_decimal" => vec![
            ("min", "number_decimal".into()),
            ("max", "number_decimal".into()),
            ("max_precision", "number_integer".into()),
        ],
        "date" | "date_time" => vec![("min", base.into()), ("max", base.into())],
        "json" => vec![("schema", "json".into())],
        "url" | "link" => vec![("allowed_domains", "list.single_line_text_field".into())],
        "rating" => vec![
            ("scale_min", "number_decimal".into()),
            ("scale_max", "number_decimal".into()),
        ],
        "file_reference" => vec![("file_type_options", "list.single_line_text_field".into())],
        "metaobject_reference" => {
            vec![("metaobject_definition_id", "single_line_text_field".into())]
        }
        "mixed_reference" => vec![(
            "metaobject_definition_ids",
            "list.single_line_text_field".into(),
        )],
        "product_taxonomy_value_reference" => vec![(
            "product_taxonomy_attribute_handle",
            "single_line_text_field".into(),
        )],
        base if metafield_types::is_measurement(base) => {
            vec![("min", base.into()), ("max", base.into())]
        }
        _ => Vec::new(),
    };
    if kind.starts_with("list.") {
        validations.push(("list.min", "number_integer".into()));
        validations.push(("list.max", "number_integer".into()));
    }
    validations
}

/// Normalizes a portable alias (`metaobject_definition_type` / `metaobject_definition_types`) to
/// Shopify's canonical validation name.
fn canonical_validation_name(name: &str) -> &str {
    match name {
        "metaobject_definition_type" => "metaobject_definition_id",
        "metaobject_definition_types" => "metaobject_definition_ids",
        other => other,
    }
}

fn is_identifier_chars(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `namespace` is an app-owned reserved namespace (`$app`, `$app:<name>`, or `app--...`).
pub fn is_app_owned_namespace(namespace: &str) -> bool {
    namespace == "$app"
        || namespace
            .strip_prefix("$app:")
            .is_some_and(|rest| !rest.is_empty())
        || namespace.starts_with("app--")
}

/// Whether `kind` is an app-owned metaobject type (`$app:<type>` or `app--<id>--<type>`).
pub fn is_app_owned_metaobject_type(kind: &str) -> bool {
    kind.strip_prefix("$app:")
        .is_some_and(|rest| !rest.is_empty())
        || kind.starts_with("app--")
}

/// Checks whether a metafield namespace (as written in store data) is syntactically valid.
fn check_namespace(
    namespace: &str,
    standard_template: bool,
    origin: &Origin,
    diagnostics: &mut Diagnostics,
) {
    let own = namespace.strip_prefix("$app").is_some_and(|rest| {
        rest.is_empty() || rest.strip_prefix(':').is_some_and(is_identifier_chars)
    });
    let length = namespace.chars().count();
    if !own && length < 3 {
        diagnostics
            .error(
                "invalid_definition",
                &origin.file,
                &origin.pointer,
                format!("namespace \"{namespace}\" is too short (minimum is 3 characters)"),
            )
            .hint("use a namespace of 3–255 letters, digits, '-' or '_', or \"$app\"");
    } else if length > 255 {
        diagnostics.error(
            "invalid_definition",
            &origin.file,
            &origin.pointer,
            format!("namespace \"{namespace}\" is too long (maximum is 255 characters)"),
        );
    } else if !own && !is_identifier_chars(namespace) {
        diagnostics
            .error(
                "invalid_definition",
                &origin.file,
                &origin.pointer,
                format!("namespace \"{namespace}\" contains invalid characters"),
            )
            .hint("use only ASCII letters, digits, '-' and '_' (or \"$app\" / \"$app:<name>\")");
    } else if (namespace == "shopify" || namespace.starts_with("shopify--")) && !standard_template {
        diagnostics
            .error(
                "reserved_namespace",
                &origin.file,
                &origin.pointer,
                format!("namespace \"{namespace}\" is reserved for Shopify standard definitions"),
            )
            .hint("set \"standard_template\": true for a standard definition, or use another namespace such as \"custom\"");
    }
}

/// Checks a metafield definition key (2–64 characters of letters, digits, `-`, `_`).
fn check_metafield_key(key: &str, origin: &Origin, diagnostics: &mut Diagnostics) {
    let length = key.chars().count();
    if length < 2 {
        diagnostics.error(
            "invalid_definition",
            &origin.file,
            &origin.pointer,
            format!("key \"{key}\" is too short (minimum is 2 characters)"),
        );
    } else if length > 64 {
        diagnostics.error(
            "invalid_definition",
            &origin.file,
            &origin.pointer,
            format!("key \"{key}\" is too long (maximum is 64 characters)"),
        );
    } else if !is_identifier_chars(key) {
        diagnostics
            .error(
                "invalid_definition",
                &origin.file,
                &origin.pointer,
                format!("key \"{key}\" contains invalid characters"),
            )
            .hint("use only ASCII letters, digits, '-' and '_'");
    }
}

/// Checks a metaobject type name (3–255 chars after optional `$app:`, letters, digits, `-`, `_`).
fn check_metaobject_type(kind: &str, origin: &Origin, diagnostics: &mut Diagnostics) {
    let name = kind.strip_prefix("$app:").unwrap_or(kind);
    let length = name.chars().count();
    if length < 3 {
        diagnostics.error(
            "invalid_definition",
            &origin.file,
            &origin.pointer,
            format!("metaobject type \"{kind}\" is too short (minimum is 3 characters)"),
        );
    } else if length > 255 {
        diagnostics.error(
            "invalid_definition",
            &origin.file,
            &origin.pointer,
            format!("metaobject type \"{kind}\" is too long (maximum is 255 characters)"),
        );
    } else if !is_identifier_chars(name) {
        diagnostics
            .error(
                "invalid_definition",
                &origin.file,
                &origin.pointer,
                format!("metaobject type \"{kind}\" contains invalid characters"),
            )
            .hint(
                "use only ASCII letters, digits, '-' and '_' (optionally prefixed with \"$app:\")",
            );
    }
}

/// Checks name and description length / non-blankness.
fn check_name_and_description(
    name: Option<&str>,
    description: Option<&str>,
    origin: &Origin,
    diagnostics: &mut Diagnostics,
) {
    if let Some(name) = name {
        if name.trim().is_empty() {
            diagnostics.error(
                "invalid_definition",
                &origin.file,
                &origin.child("name").pointer,
                "name can't be blank",
            );
        } else if name.chars().count() > 255 {
            diagnostics.error(
                "invalid_definition",
                &origin.file,
                &origin.child("name").pointer,
                "name is too long (maximum is 255 characters)",
            );
        }
    }
    if let Some(description) = description
        && description.chars().count() > 255
    {
        diagnostics.error(
            "invalid_definition",
            &origin.file,
            &origin.child("description").pointer,
            "description is too long (maximum is 255 characters)",
        );
    }
}

/// Checks that `kind` is a valid Shopify 2026-10 metafield/metaobject field definition type.
fn check_definition_type(kind: &str, origin: &Origin, diagnostics: &mut Diagnostics) -> bool {
    if metafield_types::is_definition_type(kind) {
        return true;
    }
    let all = metafield_types::all_definition_types();
    let hint = if metafield_types::DEPRECATED.contains(&kind) {
        match kind {
            "string" => "use \"single_line_text_field\" or \"multi_line_text_field\" instead of deprecated \"string\"".to_string(),
            "integer" => "use \"number_integer\" instead of deprecated \"integer\"".to_string(),
            "json_string" => "use \"json\" instead of deprecated \"json_string\"".to_string(),
            _ => "use a current Shopify metafield type".to_string(),
        }
    } else if let Some(item) = kind.strip_prefix("list.")
        && metafield_types::is_definition_type(item)
    {
        format!("Shopify definitions do not support \"list.{item}\": use \"{item}\" or \"json\"")
    } else if let Some(close) = closest_match(kind, all.iter().map(String::as_str)) {
        format!("did you mean \"{close}\"?")
    } else {
        "use a Shopify metafield type such as \"single_line_text_field\", \"number_integer\", \"boolean\", \"product_reference\", \"list.product_reference\", ...".to_string()
    };
    diagnostics
        .error(
            "unknown_metafield_type",
            &origin.file,
            &origin.pointer,
            format!("\"{kind}\" is not a valid Shopify metafield definition type"),
        )
        .hint(hint);
    false
}

/// Converts a JSON validation value into Shopify's canonical string form.
fn validation_value_text(value: &Json) -> Option<String> {
    match value {
        Json::Null | Json::Bool(_) => None,
        Json::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// Checks whether a portable metaobject type reference (a type name like `"look"` or `"$app:author"`,
/// or a `gid://shopify/MetaobjectDefinition/<id>`) resolves to one of `known_types`.
fn matches_known_metaobject_type<'a>(
    reference: &str,
    known_types: &'a [String],
) -> Option<&'a str> {
    if let Some(("MetaobjectDefinition", id)) = parse_gid(reference) {
        return known_types
            .iter()
            .find(|known| stable_id("metaobject_definition", known) == id)
            .map(String::as_str);
    }
    known_types
        .iter()
        .find(|known| same_metaobject_type(known, reference))
        .map(String::as_str)
}

/// Validates and normalizes the `validations` list of a metafield or metaobject field definition.
fn build_validations(
    kind: &str,
    inputs: &[model::MetafieldValidationInput],
    known_metaobject_types: &[String],
    origin: &Origin,
    diagnostics: &mut Diagnostics,
) -> Vec<MetafieldValidation> {
    let supported = supported_validations(kind);
    let mut validations: Vec<MetafieldValidation> = Vec::new();
    let base = kind.strip_prefix("list.").unwrap_or(kind);

    for (index, input) in inputs.iter().enumerate() {
        let item_origin = origin.child(index);
        let canonical = canonical_validation_name(&input.name);
        let Some((_, value_type)) = supported.iter().find(|(known, _)| *known == canonical) else {
            let names: Vec<&str> = supported.iter().map(|(n, _)| *n).collect();
            let hint = if names.is_empty() {
                format!("type \"{kind}\" does not support any validations")
            } else if let Some(close) = closest_match(&input.name, names.iter().copied()) {
                format!(
                    "did you mean \"{close}\"? Supported for {kind}: {}",
                    names.join(", ")
                )
            } else {
                format!("supported validations for {kind}: {}", names.join(", "))
            };
            diagnostics
                .error(
                    "invalid_validation",
                    &item_origin.file,
                    &item_origin.child("name").pointer,
                    format!(
                        "validation \"{}\" is not supported for type \"{kind}\"",
                        input.name
                    ),
                )
                .hint(hint);
            continue;
        };
        if validations.iter().any(|v| v.name == canonical) {
            diagnostics.error(
                "duplicate_validation",
                &item_origin.file,
                &item_origin.child("name").pointer,
                format!("validation \"{canonical}\" is declared more than once"),
            );
            continue;
        }
        let Some(raw_text) = validation_value_text(&input.value) else {
            diagnostics
                .error(
                    "invalid_validation",
                    &item_origin.file,
                    &item_origin.child("value").pointer,
                    format!("validation \"{}\" needs a value", input.name),
                )
                .hint("write the validation value as a string, number, array, or object");
            continue;
        };

        // Validate value syntax against the validation's value type.
        let value_origin = item_origin.child("value");
        let normalized_value = match canonical {
            "metaobject_definition_id" => {
                let target = raw_text.trim();
                if target.is_empty() {
                    diagnostics.error(
                        "invalid_validation",
                        &value_origin.file,
                        &value_origin.pointer,
                        "metaobject_definition_id can't be blank",
                    );
                    continue;
                }
                match matches_known_metaobject_type(target, known_metaobject_types) {
                    Some(known) => {
                        if target.starts_with("gid://") {
                            target.to_string()
                        } else {
                            known.to_string()
                        }
                    }
                    None => {
                        let diag = diagnostics.error(
                            "unknown_metaobject_definition",
                            &value_origin.file,
                            &value_origin.pointer,
                            format!(
                                "metaobject definition \"{target}\" is not defined in this store"
                            ),
                        );
                        if let Some(close) =
                            closest_match(target, known_metaobject_types.iter().map(String::as_str))
                        {
                            diag.hint(format!("did you mean \"{close}\"?"));
                        } else if !known_metaobject_types.is_empty() {
                            diag.hint(format!(
                                "known metaobject types: {}",
                                known_metaobject_types.join(", ")
                            ));
                        } else {
                            diag.hint("declare it in \"metaobject_definitions\" or add entries under \"metaobjects\"");
                        }
                        continue;
                    }
                }
            }
            "metaobject_definition_ids" => {
                let parsed: Option<Vec<String>> = match &input.value {
                    Json::Array(items) => items
                        .iter()
                        .map(|item| item.as_str().map(str::to_string))
                        .collect(),
                    Json::String(text) => serde_json::from_str::<Vec<String>>(text).ok(),
                    _ => None,
                };
                let Some(targets) = parsed.filter(|list| !list.is_empty()) else {
                    diagnostics
                        .error(
                            "invalid_validation",
                            &value_origin.file,
                            &value_origin.pointer,
                            "metaobject_definition_ids must be a non-empty list of metaobject types",
                        )
                        .hint("write [\"type_a\", \"type_b\"] or \"[\\\"type_a\\\",\\\"type_b\\\"]\"");
                    continue;
                };
                let mut resolved_targets = Vec::with_capacity(targets.len());
                let mut ok = true;
                for target in &targets {
                    let target = target.trim();
                    match matches_known_metaobject_type(target, known_metaobject_types) {
                        Some(known) => {
                            resolved_targets.push(if target.starts_with("gid://") {
                                target.to_string()
                            } else {
                                known.to_string()
                            });
                        }
                        None => {
                            ok = false;
                            let diag = diagnostics.error(
                                "unknown_metaobject_definition",
                                &value_origin.file,
                                &value_origin.pointer,
                                format!("metaobject definition \"{target}\" is not defined in this store"),
                            );
                            if let Some(close) = closest_match(
                                target,
                                known_metaobject_types.iter().map(String::as_str),
                            ) {
                                diag.hint(format!("did you mean \"{close}\"?"));
                            } else if !known_metaobject_types.is_empty() {
                                diag.hint(format!(
                                    "known metaobject types: {}",
                                    known_metaobject_types.join(", ")
                                ));
                            } else {
                                diag.hint("declare it in \"metaobject_definitions\" or add entries under \"metaobjects\"");
                            }
                        }
                    }
                }
                if !ok {
                    continue;
                }
                Json::Array(resolved_targets.into_iter().map(Json::String).collect()).to_string()
            }
            "regex" => {
                if raw_text.is_empty() {
                    diagnostics.error(
                        "invalid_validation",
                        &value_origin.file,
                        &value_origin.pointer,
                        "regex validation can't be blank",
                    );
                    continue;
                }
                if let Err(err) = regex::Regex::new(&raw_text) {
                    diagnostics.error(
                        "invalid_validation",
                        &value_origin.file,
                        &value_origin.pointer,
                        format!("invalid regular expression \"{raw_text}\": {err}"),
                    );
                    continue;
                }
                raw_text
            }
            "schema" => {
                let parsed: Option<Json> = match &input.value {
                    Json::Object(_) => Some(input.value.clone()),
                    Json::String(text) => serde_json::from_str::<Json>(text).ok(),
                    _ => None,
                };
                let Some(schema_json) = parsed.filter(Json::is_object) else {
                    diagnostics
                        .error(
                            "invalid_validation",
                            &value_origin.file,
                            &value_origin.pointer,
                            "schema validation must be a JSON Schema object",
                        )
                        .hint("provide a JSON Schema object or its JSON string");
                    continue;
                };
                if let Err(err) = jsonschema::validator_for(&schema_json) {
                    diagnostics.error(
                        "invalid_validation",
                        &value_origin.file,
                        &value_origin.pointer,
                        format!("invalid JSON Schema: {err}"),
                    );
                    continue;
                }
                schema_json.to_string()
            }
            _ => {
                match validate_option_value(canonical, value_type, &input.value, &raw_text, base) {
                    Ok(canonical_value) => canonical_value,
                    Err((message, hint)) => {
                        let diag = diagnostics.error(
                            "invalid_validation",
                            &value_origin.file,
                            &value_origin.pointer,
                            message,
                        );
                        if let Some(hint) = hint {
                            diag.hint(hint);
                        }
                        continue;
                    }
                }
            }
        };

        validations.push(MetafieldValidation {
            name: canonical.to_string(),
            value_type: value_type.clone(),
            value: normalized_value,
        });
    }

    // Cross-validation checks on the complete validation list.
    if base == "rating" {
        for required in ["scale_min", "scale_max"] {
            if !validations.iter().any(|v| v.name == required) {
                diagnostics
                    .error(
                        "missing_validation",
                        &origin.file,
                        &origin.pointer,
                        format!("a {kind} definition must include the \"{required}\" validation"),
                    )
                    .hint("add both \"scale_min\" and \"scale_max\" validations (e.g. \"1.0\" and \"5.0\")");
            }
        }
        if let (Some(min), Some(max)) = (
            find_validation(&validations, "scale_min").and_then(|v| v.parse::<f64>().ok()),
            find_validation(&validations, "scale_max").and_then(|v| v.parse::<f64>().ok()),
        ) && min >= max
        {
            diagnostics.error(
                "invalid_validation",
                &origin.file,
                &origin.pointer,
                format!("scale_min ({min}) must be less than scale_max ({max})"),
            );
        }
    }

    if base == "metaobject_reference"
        && !validations
            .iter()
            .any(|v| v.name == "metaobject_definition_id")
    {
        // Only emit missing_validation if the user didn't already fail on an invalid
        // metaobject_definition_id/type entry.
        let attempted = inputs
            .iter()
            .any(|v| canonical_validation_name(&v.name) == "metaobject_definition_id");
        if !attempted {
            diagnostics
                .error(
                    "missing_validation",
                    &origin.file,
                    &origin.pointer,
                    format!("a {kind} definition must include a \"metaobject_definition_id\" validation"),
                )
                .hint("add {\"name\": \"metaobject_definition_id\", \"value\": \"<metaobject_type>\"}");
        }
    }

    if base == "mixed_reference"
        && !validations
            .iter()
            .any(|v| v.name == "metaobject_definition_ids")
    {
        let attempted = inputs
            .iter()
            .any(|v| canonical_validation_name(&v.name) == "metaobject_definition_ids");
        if !attempted {
            diagnostics
                .error(
                    "missing_validation",
                    &origin.file,
                    &origin.pointer,
                    format!("a {kind} definition must include a \"metaobject_definition_ids\" validation"),
                )
                .hint("add {\"name\": \"metaobject_definition_ids\", \"value\": [\"<type_a>\", \"<type_b>\"]}");
        }
    }

    // Check min <= max consistency.
    if let (Some(min), Some(max)) = (
        find_validation(&validations, "list.min").and_then(|v| v.parse::<i64>().ok()),
        find_validation(&validations, "list.max").and_then(|v| v.parse::<i64>().ok()),
    ) && min > max
    {
        diagnostics.error(
            "invalid_validation",
            &origin.file,
            &origin.pointer,
            format!("list.min ({min}) must be less than or equal to list.max ({max})"),
        );
    }

    match base {
        "single_line_text_field" | "multi_line_text_field" | "id" | "number_integer" => {
            if let (Some(min), Some(max)) = (
                find_validation(&validations, "min").and_then(|v| v.parse::<i64>().ok()),
                find_validation(&validations, "max").and_then(|v| v.parse::<i64>().ok()),
            ) && min > max
            {
                diagnostics.error(
                    "invalid_validation",
                    &origin.file,
                    &origin.pointer,
                    format!("min ({min}) must be less than or equal to max ({max})"),
                );
            }
        }
        "number_decimal" => {
            if let (Some(min), Some(max)) = (
                find_validation(&validations, "min").and_then(|v| v.parse::<f64>().ok()),
                find_validation(&validations, "max").and_then(|v| v.parse::<f64>().ok()),
            ) && min > max
            {
                diagnostics.error(
                    "invalid_validation",
                    &origin.file,
                    &origin.pointer,
                    format!("min ({min}) must be less than or equal to max ({max})"),
                );
            }
        }
        "date" => {
            if let (Some(min), Some(max)) = (
                find_validation(&validations, "min")
                    .and_then(|v| NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()),
                find_validation(&validations, "max")
                    .and_then(|v| NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()),
            ) && min > max
            {
                diagnostics.error(
                    "invalid_validation",
                    &origin.file,
                    &origin.pointer,
                    format!("min date ({min}) must be on or before max date ({max})"),
                );
            }
        }
        "date_time" => {
            if let (Some(min), Some(max)) = (
                find_validation(&validations, "min").and_then(parse_date_time),
                find_validation(&validations, "max").and_then(parse_date_time),
            ) && min > max
            {
                diagnostics.error(
                    "invalid_validation",
                    &origin.file,
                    &origin.pointer,
                    "min date_time must be on or before max date_time",
                );
            }
        }
        _ => {}
    }

    validations
}

fn find_validation<'a>(validations: &'a [MetafieldValidation], name: &str) -> Option<&'a str> {
    validations
        .iter()
        .find(|v| v.name == name)
        .map(|v| v.value.as_str())
}

fn validate_option_value(
    name: &str,
    value_type: &str,
    raw_json: &Json,
    raw_text: &str,
    base: &str,
) -> Result<String, (String, Option<String>)> {
    match value_type {
        "number_integer" => {
            let parsed = raw_text.trim().parse::<i64>().map_err(|_| {
                (
                    format!("validation \"{name}\" expects an integer, found \"{raw_text}\""),
                    Some("write a whole number such as \"10\" or 10".to_string()),
                )
            })?;
            let must_be_non_negative = matches!(name, "list.min" | "list.max" | "max_precision")
                || (matches!(
                    base,
                    "single_line_text_field" | "multi_line_text_field" | "id"
                ) && matches!(name, "min" | "max"));
            if must_be_non_negative && parsed < 0 {
                return Err((
                    format!("validation \"{name}\" must be non-negative, found {parsed}"),
                    None,
                ));
            }
            Ok(parsed.to_string())
        }
        "number_decimal" => {
            let parsed = raw_text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| {
                    (
                        format!(
                            "validation \"{name}\" expects a decimal number, found \"{raw_text}\""
                        ),
                        Some("write a number such as \"1.0\" or \"5.5\"".to_string()),
                    )
                })?;
            let _ = parsed;
            Ok(raw_text.trim().to_string())
        }
        "date" => {
            NaiveDate::parse_from_str(raw_text.trim(), "%Y-%m-%d").map_err(|_| {
                (
                    format!(
                        "validation \"{name}\" expects a YYYY-MM-DD date, found \"{raw_text}\""
                    ),
                    Some("use YYYY-MM-DD, e.g. \"2025-01-01\"".to_string()),
                )
            })?;
            Ok(raw_text.trim().to_string())
        }
        "date_time" => {
            parse_date_time(raw_text.trim()).ok_or_else(|| {
                (
                    format!(
                        "validation \"{name}\" expects an ISO 8601 date_time, found \"{raw_text}\""
                    ),
                    Some(
                        "use YYYY-MM-DDTHH:MM:SS or RFC 3339, e.g. \"2025-01-01T10:00:00Z\""
                            .to_string(),
                    ),
                )
            })?;
            Ok(raw_text.trim().to_string())
        }
        "list.single_line_text_field" => {
            let parsed: Option<Vec<String>> = match raw_json {
                Json::Array(items) => items
                    .iter()
                    .map(|item| item.as_str().map(str::to_string))
                    .collect(),
                Json::String(text) => serde_json::from_str::<Vec<String>>(text).ok(),
                _ => None,
            };
            let items = parsed.ok_or_else(|| {
                (
                    format!("validation \"{name}\" expects a list of strings"),
                    Some("write [\"a\", \"b\"] or \"[\\\"a\\\",\\\"b\\\"]\"".to_string()),
                )
            })?;
            if name == "file_type_options" {
                const FILE_TYPES: [&str; 3] = ["Image", "Video", "GenericFile"];
                for item in &items {
                    if !FILE_TYPES.contains(&item.as_str()) {
                        let hint = closest_match(item, FILE_TYPES.iter().copied())
                            .map(|close| {
                                format!(
                                    "did you mean \"{close}\"? Accepted: {}",
                                    FILE_TYPES.join(", ")
                                )
                            })
                            .unwrap_or_else(|| {
                                format!("accepted values: {}", FILE_TYPES.join(", "))
                            });
                        return Err((
                            format!("\"{item}\" is not a valid file_type_options value"),
                            Some(hint),
                        ));
                    }
                }
            }
            Ok(Json::Array(items.into_iter().map(Json::String).collect()).to_string())
        }
        "single_line_text_field" => {
            if raw_text.trim().is_empty() {
                return Err((format!("validation \"{name}\" can't be blank"), None));
            }
            Ok(raw_text.to_string())
        }
        // Measurement bound (e.g. `weight`, `dimension`, `volume`, ...).
        measurement => {
            let parsed: Option<Json> = match raw_json {
                Json::Object(_) => Some(raw_json.clone()),
                Json::String(text) => serde_json::from_str::<Json>(text).ok(),
                _ => None,
            };
            let obj = parsed.filter(Json::is_object).ok_or_else(|| {
                (
                    format!(
                        "validation \"{name}\" for {measurement} expects an object with \"value\" and \"unit\""
                    ),
                    Some("e.g. {\"value\": 500, \"unit\": \"grams\"}".to_string()),
                )
            })?;
            let (problems, _) = metafield_types::check(measurement, &obj, "USD");
            if let Some(first) = problems.first() {
                return Err((
                    format!(
                        "invalid {measurement} bound for \"{name}\": {}",
                        first.message
                    ),
                    Some(first.hint.clone()),
                ));
            }
            let normalized = metafield_types::normalize(measurement, &obj);
            Ok(normalized.to_string())
        }
    }
}

fn parse_date_time(text: &str) -> Option<NaiveDateTime> {
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some(at.naive_utc());
    }
    [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
}

/// Builds and validates all declared `metaobject_definitions`.
pub(super) fn build_metaobject_definitions(
    inputs: &IndexMap<String, Sourced<model::MetaobjectDefinitionInput>>,
    known_metaobject_types: &[String],
    diagnostics: &mut Diagnostics,
) -> IndexMap<String, MetaobjectDefinition> {
    let mut built = IndexMap::new();
    for (kind, sourced) in inputs {
        let input = &sourced.value;
        let origin = &sourced.origin;
        if let Some(declared_type) = &input.kind
            && declared_type != kind
        {
            diagnostics
                .error(
                    "mismatched_type",
                    &origin.file,
                    &origin.child("type").pointer,
                    format!(
                        "metaobject definition type \"{declared_type}\" does not match key \"{kind}\""
                    ),
                )
                .hint(format!("remove \"type\" or set it to \"{kind}\""));
        }
        check_metaobject_type(kind, origin, diagnostics);
        check_name_and_description(
            input.name.as_deref(),
            input.description.as_deref(),
            origin,
            diagnostics,
        );
        let app_owned = is_app_owned_metaobject_type(kind);
        if let Some(access) = &input.access
            && access.admin.is_some()
            && !app_owned
        {
            diagnostics
                .error(
                    "admin_access_not_allowed",
                    &origin.file,
                    &origin.child("access").child("admin").pointer,
                    "admin access can only be set on app-owned metaobject definitions (\"$app:<type>\")",
                )
                .hint("remove \"access.admin\" or use an app-owned type such as \"$app:look\"");
        }

        // Resolve Online Store capability and top-level `url_handle`.
        let top_url_handle = input.url_handle.as_deref().map(str::trim);
        if top_url_handle == Some("") {
            diagnostics.error(
                "invalid_definition",
                &origin.file,
                &origin.child("url_handle").pointer,
                "url_handle can't be blank",
            );
        }
        let online_store = input
            .capabilities
            .as_ref()
            .and_then(|c| c.online_store.as_ref())
            .map(|os| match os {
                OnlineStoreCapabilityInput::Bool(enabled) => OnlineStoreCapability {
                    enabled: *enabled,
                    url_handle: None,
                    create_redirects: false,
                },
                OnlineStoreCapabilityInput::Detailed(detail) => {
                    let data_handle = detail.data.as_ref().and_then(|d| d.url_handle.as_deref());
                    let url_handle = detail
                        .url_handle
                        .as_deref()
                        .or(data_handle)
                        .map(|h| h.trim().to_string());
                    if url_handle.as_deref() == Some("") {
                        diagnostics.error(
                            "invalid_definition",
                            &origin.file,
                            &origin.child("capabilities").child("online_store").pointer,
                            "online_store url_handle can't be blank",
                        );
                    }
                    let create_redirects = detail
                        .create_redirects
                        .or_else(|| detail.data.as_ref().and_then(|d| d.create_redirects))
                        .unwrap_or(false);
                    OnlineStoreCapability {
                        enabled: detail.enabled.unwrap_or(true),
                        url_handle: url_handle.filter(|h| !h.is_empty()),
                        create_redirects,
                    }
                }
            });

        if let (Some(top), Some(os)) = (top_url_handle.filter(|h| !h.is_empty()), &online_store) {
            if !os.enabled {
                diagnostics
                    .error(
                        "conflicting_capability",
                        &origin.file,
                        &origin.child("url_handle").pointer,
                        "url_handle is set while capabilities.online_store is disabled",
                    )
                    .hint("remove \"url_handle\" or enable \"capabilities.online_store\"");
            } else if let Some(cap_handle) = &os.url_handle
                && cap_handle != top
            {
                diagnostics
                    .error(
                        "conflicting_capability",
                        &origin.file,
                        &origin.child("url_handle").pointer,
                        format!(
                            "url_handle \"{top}\" conflicts with capabilities.online_store.url_handle \"{cap_handle}\""
                        ),
                    )
                    .hint("keep only one url_handle or give them the same value");
            }
        }

        let effective_url_handle = online_store
            .as_ref()
            .and_then(|os| os.enabled.then(|| os.url_handle.clone()).flatten())
            .or_else(|| top_url_handle.filter(|h| !h.is_empty()).map(str::to_string));

        let renderable = input
            .capabilities
            .as_ref()
            .and_then(|c| c.renderable.as_ref())
            .map(|r| match r {
                RenderableCapabilityInput::Bool(enabled) => RenderableCapability {
                    enabled: *enabled,
                    meta_title_key: None,
                    meta_description_key: None,
                },
                RenderableCapabilityInput::Detailed(detail) => {
                    let meta_title_key = detail
                        .meta_title_key
                        .clone()
                        .or_else(|| detail.data.as_ref().and_then(|d| d.meta_title_key.clone()));
                    let meta_description_key = detail.meta_description_key.clone().or_else(|| {
                        detail
                            .data
                            .as_ref()
                            .and_then(|d| d.meta_description_key.clone())
                    });
                    RenderableCapability {
                        enabled: detail.enabled.unwrap_or(true),
                        meta_title_key,
                        meta_description_key,
                    }
                }
            });

        // Build field definitions if declared.
        let field_definitions = input.field_definitions.as_ref().map(|fields| {
            let mut built_fields: Vec<MetaobjectFieldDefinition> = Vec::with_capacity(fields.len());
            let mut seen_keys: HashSet<String> = HashSet::new();
            let fields_origin = origin.child("field_definitions");
            for (index, field) in fields.iter().enumerate() {
                let field_origin = fields_origin.child(index);
                let key_len = field.key.chars().count();
                if !(1..=64).contains(&key_len) || !is_identifier_chars(&field.key) {
                    diagnostics
                        .error(
                            "invalid_definition",
                            &field_origin.file,
                            &field_origin.child("key").pointer,
                            format!("field key \"{}\" is invalid", field.key),
                        )
                        .hint("field keys must be 1–64 ASCII letters, digits, '-' or '_'");
                } else if !seen_keys.insert(field.key.clone()) {
                    diagnostics.error(
                        "duplicate_field_definition",
                        &field_origin.file,
                        &field_origin.child("key").pointer,
                        format!(
                            "field key \"{}\" is defined more than once on metaobject \"{kind}\"",
                            field.key
                        ),
                    );
                    continue;
                }
                check_name_and_description(
                    field.name.as_deref(),
                    field.description.as_deref(),
                    &field_origin,
                    diagnostics,
                );
                let type_ok =
                    check_definition_type(&field.kind, &field_origin.child("type"), diagnostics);
                let validations = if type_ok {
                    build_validations(
                        &field.kind,
                        &field.validations,
                        known_metaobject_types,
                        &field_origin.child("validations"),
                        diagnostics,
                    )
                } else {
                    Vec::new()
                };
                let admin_filterable = field
                    .capabilities
                    .as_ref()
                    .and_then(|c| c.admin_filterable.as_ref())
                    .is_some_and(CapabilityFlag::is_enabled);
                built_fields.push(MetaobjectFieldDefinition {
                    key: field.key.clone(),
                    name: field
                        .name
                        .as_deref()
                        .map(|n| n.trim().to_string())
                        .unwrap_or_else(|| humanize(&field.key)),
                    description: field.description.clone(),
                    kind: field.kind.clone(),
                    required: field.required,
                    validations,
                    admin_filterable,
                });
            }
            built_fields
        });

        // Validate display_name_key and renderable keys against declared field_definitions.
        if let Some(fields) = &field_definitions {
            let field_keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
            if let Some(display_key) = &input.display_name_key
                && !field_keys.contains(&display_key.as_str())
            {
                let diag = diagnostics.error(
                    "invalid_definition",
                    &origin.file,
                    &origin.child("display_name_key").pointer,
                    format!(
                        "display_name_key \"{display_key}\" is not a field of metaobject \"{kind}\""
                    ),
                );
                if let Some(close) = closest_match(display_key, field_keys.iter().copied()) {
                    diag.hint(format!("did you mean \"{close}\"?"));
                } else if !field_keys.is_empty() {
                    diag.hint(format!("defined fields: {}", field_keys.join(", ")));
                }
            }
            if let Some(renderable) = &renderable {
                for (prop, key) in [
                    ("meta_title_key", renderable.meta_title_key.as_deref()),
                    (
                        "meta_description_key",
                        renderable.meta_description_key.as_deref(),
                    ),
                ] {
                    if let Some(key) = key
                        && !field_keys.contains(&key)
                    {
                        let diag = diagnostics.error(
                            "invalid_definition",
                            &origin.file,
                            &origin.child("capabilities").child("renderable").pointer,
                            format!("{prop} \"{key}\" is not a field of metaobject \"{kind}\""),
                        );
                        if let Some(close) = closest_match(key, field_keys.iter().copied()) {
                            diag.hint(format!("did you mean \"{close}\"?"));
                        } else if !field_keys.is_empty() {
                            diag.hint(format!("defined fields: {}", field_keys.join(", ")));
                        }
                    }
                }
            }
        }

        let access = input
            .access
            .as_ref()
            .map(|a| MetaobjectAccess {
                admin: a.admin,
                storefront: a.storefront,
                customer_account: a.customer_account,
            })
            .unwrap_or_default();

        let capabilities = MetaobjectCapabilities {
            publishable: input
                .capabilities
                .as_ref()
                .and_then(|c| c.publishable.as_ref())
                .map(CapabilityFlag::is_enabled),
            translatable: input
                .capabilities
                .as_ref()
                .and_then(|c| c.translatable.as_ref())
                .map(CapabilityFlag::is_enabled),
            online_store,
            renderable,
        };

        built.insert(
            kind.clone(),
            MetaobjectDefinition {
                kind: kind.clone(),
                name: input.name.as_deref().map(|n| n.trim().to_string()),
                description: input.description.clone(),
                display_name_key: input.display_name_key.clone(),
                url_handle: effective_url_handle,
                access,
                capabilities,
                standard_template: input.standard_template.unwrap_or(false),
                field_definitions,
            },
        );
    }
    built
}

/// Builds and validates all declared `metafield_definitions`.
pub(super) fn build_metafield_definitions(
    inputs: &[Sourced<model::MetafieldDefinitionInput>],
    known_metaobject_types: &[String],
    diagnostics: &mut Diagnostics,
) -> Vec<MetafieldDefinition> {
    let mut built: Vec<MetafieldDefinition> = Vec::with_capacity(inputs.len());
    let mut seen: HashMap<(MetafieldOwnerType, String, String), Origin> = HashMap::new();
    // Track explicit pinned positions and total pinned counts per owner type.
    let mut explicit_pins: HashMap<MetafieldOwnerType, HashSet<u32>> = HashMap::new();
    let mut pinned_count: HashMap<MetafieldOwnerType, usize> = HashMap::new();
    // Store `(index_in_built, wants_auto_pin, origin)` for assigning sequential pinned positions.
    let mut auto_pin: Vec<(usize, MetafieldOwnerType, Origin)> = Vec::new();

    for sourced in inputs {
        let input = &sourced.value;
        let origin = &sourced.origin;
        let namespace = input.namespace.as_deref().unwrap_or("$app").to_string();
        let standard_template = input.standard_template.unwrap_or(false);

        if input.namespace.is_some() {
            check_namespace(
                &namespace,
                standard_template,
                &origin.child("namespace"),
                diagnostics,
            );
        }
        check_metafield_key(&input.key, &origin.child("key"), diagnostics);
        check_name_and_description(
            input.name.as_deref(),
            input.description.as_deref(),
            origin,
            diagnostics,
        );

        let identity = (input.owner_type, namespace.clone(), input.key.clone());
        if let Some(previous) = seen.get(&identity) {
            diagnostics
                .error(
                    "duplicate_metafield_definition",
                    &origin.file,
                    &origin.pointer,
                    format!(
                        "metafield definition {}.{} on {} is already defined in {}",
                        namespace,
                        input.key,
                        input.owner_type.as_str(),
                        previous.file
                    ),
                )
                .hint("keep a single definition per owner_type, namespace, and key");
            continue;
        }
        seen.insert(identity, origin.clone());

        let app_owned = is_app_owned_namespace(&namespace);
        if let Some(access) = &input.access
            && access.admin.is_some()
            && !app_owned
        {
            diagnostics
                .error(
                    "admin_access_not_allowed",
                    &origin.file,
                    &origin.child("access").child("admin").pointer,
                    "admin access can only be set on app-owned metafield definitions (\"$app\" or \"$app:<name>\")",
                )
                .hint("remove \"access.admin\" or use the \"$app\" namespace");
        }

        let capabilities = input
            .capabilities
            .as_ref()
            .map(|c| MetafieldCapabilities {
                admin_filterable: c
                    .admin_filterable
                    .as_ref()
                    .is_some_and(CapabilityFlag::is_enabled),
                analytics_queryable: c
                    .analytics_queryable
                    .as_ref()
                    .is_some_and(CapabilityFlag::is_enabled),
                cart_to_order_copyable: c
                    .cart_to_order_copyable
                    .as_ref()
                    .is_some_and(CapabilityFlag::is_enabled),
                smart_collection_condition: c
                    .smart_collection_condition
                    .as_ref()
                    .is_some_and(CapabilityFlag::is_enabled),
                unique_values: c
                    .unique_values
                    .as_ref()
                    .is_some_and(CapabilityFlag::is_enabled),
            })
            .unwrap_or_default();

        if capabilities.cart_to_order_copyable && input.owner_type != MetafieldOwnerType::Order {
            diagnostics
                .error(
                    "invalid_capability",
                    &origin.file,
                    &origin
                        .child("capabilities")
                        .child("cart_to_order_copyable")
                        .pointer,
                    format!(
                        "cart_to_order_copyable is only valid on ORDER metafield definitions, not {}",
                        input.owner_type.as_str()
                    ),
                )
                .hint("remove \"cart_to_order_copyable\" or change \"owner_type\" to \"ORDER\"");
        }

        // Pinning checks.
        let mut pinned_position = None;
        if let Some(pos) = input.pinned_position {
            if input.pinned == Some(false) {
                diagnostics
                    .error(
                        "invalid_definition",
                        &origin.file,
                        &origin.child("pinned_position").pointer,
                        "pinned_position cannot be set when pinned is false",
                    )
                    .hint("remove \"pinned\": false or remove \"pinned_position\"");
            } else if pos == 0 || pos as usize > PINNED_LIMIT {
                diagnostics.error(
                    "invalid_definition",
                    &origin.file,
                    &origin.child("pinned_position").pointer,
                    format!("pinned_position must be between 1 and {PINNED_LIMIT}, found {pos}"),
                );
            } else if !explicit_pins
                .entry(input.owner_type)
                .or_default()
                .insert(pos)
            {
                diagnostics.error(
                    "duplicate_pinned_position",
                    &origin.file,
                    &origin.child("pinned_position").pointer,
                    format!(
                        "pinned_position {pos} is already used by another {} metafield definition",
                        input.owner_type.as_str()
                    ),
                );
            } else {
                pinned_position = Some(pos);
                *pinned_count.entry(input.owner_type).or_default() += 1;
            }
        }

        let type_ok = check_definition_type(&input.kind, &origin.child("type"), diagnostics);
        let validations = if type_ok {
            build_validations(
                &input.kind,
                &input.validations,
                known_metaobject_types,
                &origin.child("validations"),
                diagnostics,
            )
        } else {
            Vec::new()
        };

        let access = input
            .access
            .as_ref()
            .map(|a| MetafieldAccess {
                admin: a.admin,
                storefront: a.storefront,
                customer_account: a.customer_account,
            })
            .unwrap_or_default();

        let index = built.len();
        if pinned_position.is_none() && input.pinned == Some(true) {
            auto_pin.push((index, input.owner_type, origin.clone()));
        }

        built.push(MetafieldDefinition {
            owner_type: input.owner_type,
            namespace,
            key: input.key.clone(),
            name: input
                .name
                .as_deref()
                .map(|n| n.trim().to_string())
                .unwrap_or_else(|| humanize(&input.key)),
            description: input.description.clone(),
            kind: input.kind.clone(),
            validations,
            pinned_position,
            access,
            capabilities,
            standard_template,
        });
    }

    // Assign sequential 1-based pinned positions to `pinned: true` definitions.
    for (index, owner_type, origin) in auto_pin {
        let count = pinned_count.entry(owner_type).or_default();
        if *count >= PINNED_LIMIT {
            diagnostics.error(
                "pinned_limit_reached",
                &origin.file,
                &origin.child("pinned").pointer,
                format!(
                    "{} has more than {PINNED_LIMIT} pinned metafield definitions (Shopify's limit)",
                    owner_type.as_str()
                ),
            );
            continue;
        }
        let taken = explicit_pins.entry(owner_type).or_default();
        let next = (1..=PINNED_LIMIT as u32)
            .find(|pos| !taken.contains(pos))
            .expect("under PINNED_LIMIT");
        taken.insert(next);
        *count += 1;
        built[index].pinned_position = Some(next);
    }

    built
}

// ---- Validating values against definition validations ------------------------------------------

/// Checks a normalized metafield or metaobject field value against a definition's non-reference
/// validations (`min`, `max`, `regex`, `choices`, `max_precision`, `scale_min`, `scale_max`,
/// `allowed_domains`, `schema`, `file_type_options`, measurement `min`/`max`, `list.min`/`list.max`).
///
/// Reference type constraints (`metaobject_definition_id`, `metaobject_definition_ids`) are checked
/// separately in [`check_metaobject_reference_validations`] once all metaobjects are built.
pub(super) fn check_value_validations(
    kind: &str,
    value: &Json,
    validations: &[MetafieldValidation],
    origin: &Origin,
    diagnostics: &mut Diagnostics,
) {
    if validations.is_empty() {
        return;
    }
    let value = metafield_types::normalize(kind, value);
    if let Some(base) = kind.strip_prefix("list.") {
        let Some(items) = value.as_array() else {
            return;
        };
        if let Some(min) =
            find_validation(validations, "list.min").and_then(|v| v.parse::<usize>().ok())
            && items.len() < min
        {
            diagnostics.error(
                "validation_failed",
                &origin.file,
                &origin.pointer,
                format!(
                    "list has {} item(s), but definition requires at least {min}",
                    items.len()
                ),
            );
        }
        if let Some(max) =
            find_validation(validations, "list.max").and_then(|v| v.parse::<usize>().ok())
            && items.len() > max
        {
            diagnostics.error(
                "validation_failed",
                &origin.file,
                &origin.pointer,
                format!(
                    "list has {} item(s), but definition allows at most {max}",
                    items.len()
                ),
            );
        }
        for (index, item) in items.iter().enumerate() {
            check_item_validations(base, item, validations, &origin.child(index), diagnostics);
        }
    } else {
        check_item_validations(kind, &value, validations, origin, diagnostics);
    }
}

fn check_item_validations(
    kind: &str,
    value: &Json,
    validations: &[MetafieldValidation],
    origin: &Origin,
    diagnostics: &mut Diagnostics,
) {
    match kind {
        "single_line_text_field" | "multi_line_text_field" | "id" => {
            let Some(text) = value.as_str() else {
                return;
            };
            let length = text.chars().count();
            if let Some(min) =
                find_validation(validations, "min").and_then(|v| v.parse::<usize>().ok())
                && length < min
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value is {length} character(s) long, minimum is {min}"),
                );
            }
            if let Some(max) =
                find_validation(validations, "max").and_then(|v| v.parse::<usize>().ok())
                && length > max
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value is {length} character(s) long, maximum is {max}"),
                );
            }
            if let Some(pattern) = find_validation(validations, "regex")
                && let Ok(re) = regex::Regex::new(pattern)
                && !re.is_match(text)
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value \"{text}\" does not match pattern \"{pattern}\""),
                );
            }
            if let Some(choices_json) = find_validation(validations, "choices")
                && let Ok(choices) = serde_json::from_str::<Vec<String>>(choices_json)
                && !choices.iter().any(|choice| choice == text)
            {
                let diag = diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value \"{text}\" is not one of the allowed choices"),
                );
                if let Some(close) = closest_match(text, choices.iter().map(String::as_str)) {
                    diag.hint(format!(
                        "did you mean \"{close}\"? Allowed choices: {}",
                        choices.join(", ")
                    ));
                } else {
                    diag.hint(format!("allowed choices: {}", choices.join(", ")));
                }
            }
        }
        "number_integer" => {
            let Some(number) = value
                .as_i64()
                .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
            else {
                return;
            };
            if let Some(min) =
                find_validation(validations, "min").and_then(|v| v.parse::<i64>().ok())
                && number < min
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value {number} must be greater than or equal to {min}"),
                );
            }
            if let Some(max) =
                find_validation(validations, "max").and_then(|v| v.parse::<i64>().ok())
                && number > max
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value {number} must be less than or equal to {max}"),
                );
            }
        }
        "number_decimal" => {
            let text = match value {
                Json::Number(n) => n.to_string(),
                Json::String(s) => s.trim().to_string(),
                _ => return,
            };
            let Ok(number) = text.parse::<f64>() else {
                return;
            };
            if let Some(min_str) = find_validation(validations, "min")
                && let Ok(min) = min_str.parse::<f64>()
                && number < min
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value {text} must be greater than or equal to {min_str}"),
                );
            }
            if let Some(max_str) = find_validation(validations, "max")
                && let Ok(max) = max_str.parse::<f64>()
                && number > max
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("value {text} must be less than or equal to {max_str}"),
                );
            }
            if let Some(precision) =
                find_validation(validations, "max_precision").and_then(|v| v.parse::<usize>().ok())
                && text
                    .split_once('.')
                    .is_some_and(|(_, fraction)| fraction.len() > precision)
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!(
                        "value {text} has more than {precision} digit(s) after the decimal point"
                    ),
                );
            }
        }
        "date" => {
            let Some(text) = value.as_str() else {
                return;
            };
            let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") else {
                return;
            };
            if let Some(min_str) = find_validation(validations, "min")
                && let Ok(min) = NaiveDate::parse_from_str(min_str, "%Y-%m-%d")
                && date < min
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("date \"{text}\" must be on or after {min_str}"),
                );
            }
            if let Some(max_str) = find_validation(validations, "max")
                && let Ok(max) = NaiveDate::parse_from_str(max_str, "%Y-%m-%d")
                && date > max
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("date \"{text}\" must be on or before {max_str}"),
                );
            }
        }
        "date_time" => {
            let Some(text) = value.as_str() else {
                return;
            };
            let Some(at) = parse_date_time(text) else {
                return;
            };
            if let Some(min_str) = find_validation(validations, "min")
                && let Some(min) = parse_date_time(min_str)
                && at < min
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("date_time \"{text}\" must be on or after {min_str}"),
                );
            }
            if let Some(max_str) = find_validation(validations, "max")
                && let Some(max) = parse_date_time(max_str)
                && at > max
            {
                diagnostics.error(
                    "validation_failed",
                    &origin.file,
                    &origin.pointer,
                    format!("date_time \"{text}\" must be on or before {max_str}"),
                );
            }
        }
        "rating" => {
            let parse = |field: &str| -> Option<f64> {
                match value.get(field)? {
                    Json::Number(n) => n.as_f64(),
                    Json::String(s) => s.trim().parse::<f64>().ok(),
                    _ => None,
                }
            };
            for field in ["scale_min", "scale_max"] {
                if let (Some(actual), Some(expected_str)) =
                    (parse(field), find_validation(validations, field))
                    && let Ok(expected) = expected_str.parse::<f64>()
                    && (actual - expected).abs() > f64::EPSILON
                {
                    diagnostics.error(
                        "validation_failed",
                        &origin.file,
                        &origin.child(field).pointer,
                        format!("rating {field} must match the definition's {expected_str}"),
                    );
                }
            }
        }
        "url" | "link" => {
            let url = if kind == "link" {
                value.get("url").and_then(Json::as_str)
            } else {
                value.as_str()
            };
            let Some(url) = url else {
                return;
            };
            if let Some(domains_json) = find_validation(validations, "allowed_domains")
                && let Ok(domains) = serde_json::from_str::<Vec<String>>(domains_json)
                && !domains.is_empty()
            {
                let host = extract_url_host(url);
                let allowed = host.as_deref().is_some_and(|host| {
                    domains.iter().any(|domain| {
                        let domain = domain.to_ascii_lowercase();
                        host == domain || host.ends_with(&format!(".{domain}"))
                    })
                });
                if !allowed {
                    let at = if kind == "link" {
                        origin.child("url")
                    } else {
                        origin.clone()
                    };
                    diagnostics
                        .error(
                            "validation_failed",
                            &at.file,
                            &at.pointer,
                            format!("URL \"{url}\" is not on an allowed domain"),
                        )
                        .hint(format!("allowed domains: {}", domains.join(", ")));
                }
            }
        }
        "json" => {
            if let Some(schema_str) = find_validation(validations, "schema")
                && let Ok(schema_json) = serde_json::from_str::<Json>(schema_str)
                && let Ok(validator) = jsonschema::validator_for(&schema_json)
            {
                let instance = match value {
                    Json::String(text) => {
                        serde_json::from_str::<Json>(text).unwrap_or_else(|_| value.clone())
                    }
                    other => other.clone(),
                };
                if let Some(err) = validator.iter_errors(&instance).next() {
                    diagnostics.error(
                        "validation_failed",
                        &origin.file,
                        &origin.pointer,
                        format!("JSON value does not match the definition's schema: {err}"),
                    );
                }
            }
        }
        "file_reference" => {
            if let Some(path) = value.as_str()
                && let Some(options_json) = find_validation(validations, "file_type_options")
                && let Ok(options) = serde_json::from_str::<Vec<String>>(options_json)
                && !options.is_empty()
            {
                let kind_of_file = file_kind_from_path(path);
                if !options.iter().any(|opt| opt == kind_of_file) {
                    diagnostics
                        .error(
                            "validation_failed",
                            &origin.file,
                            &origin.pointer,
                            format!(
                                "file \"{path}\" is a {kind_of_file}, which is not allowed by the definition"
                            ),
                        )
                        .hint(format!("allowed file types: {}", options.join(", ")));
                }
            }
        }
        kind if metafield_types::is_measurement(kind) => {
            let amount = value.get("value").and_then(|v| match v {
                Json::Number(n) => n.as_f64(),
                Json::String(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            });
            let unit = value
                .get("unit")
                .and_then(Json::as_str)
                .map(str::to_ascii_lowercase);
            let (Some(amount), Some(unit)) = (amount, unit) else {
                return;
            };
            for (bound_name, below) in [("min", true), ("max", false)] {
                let Some(bound_str) = find_validation(validations, bound_name) else {
                    continue;
                };
                let Some((limit, limit_unit)) =
                    serde_json::from_str::<Json>(bound_str).ok().and_then(|b| {
                        let v = b
                            .get("value")?
                            .as_f64()
                            .or_else(|| b.get("value")?.as_str()?.parse().ok())?;
                        let u = b.get("unit")?.as_str()?.to_ascii_lowercase();
                        Some((v, u))
                    })
                else {
                    continue;
                };
                let comparison = if let (Some(f1), Some(f2)) =
                    (measurement_factor(&unit), measurement_factor(&limit_unit))
                {
                    Some((amount * f1, limit * f2))
                } else if unit == limit_unit {
                    Some((amount, limit))
                } else {
                    None
                };
                if let Some((actual, bound_val)) = comparison {
                    let out = if below {
                        actual < bound_val
                    } else {
                        actual > bound_val
                    };
                    if out {
                        diagnostics.error(
                            "validation_failed",
                            &origin.file,
                            &origin.pointer,
                            if below {
                                format!("value must be greater than or equal to {bound_str}")
                            } else {
                                format!("value must be less than or equal to {bound_str}")
                            },
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

/// Checks `metaobject_definition_id` and `metaobject_definition_ids` constraints on a
/// `metaobject_reference` / `mixed_reference` (or list thereof) once all metaobjects are loaded.
pub(super) fn check_metaobject_reference_validations(
    kind: &str,
    value: &Json,
    validations: &[MetafieldValidation],
    store: &Store,
    origin: &Origin,
    diagnostics: &mut Diagnostics,
) {
    let base = kind.strip_prefix("list.").unwrap_or(kind);
    let allowed: Option<Vec<String>> = match base {
        "metaobject_reference" => {
            find_validation(validations, "metaobject_definition_id").map(|id| vec![id.to_string()])
        }
        "mixed_reference" => find_validation(validations, "metaobject_definition_ids")
            .and_then(|ids| serde_json::from_str::<Vec<String>>(ids).ok()),
        _ => return,
    };
    let Some(allowed) = allowed else {
        return;
    };
    let value = metafield_types::normalize(kind, value);
    let check_one = |item: &Json, at: &Origin, diagnostics: &mut Diagnostics| {
        let Some(super::metafield_references::Referenced::Metaobject(idx)) =
            store.metafield_reference(base, item)
        else {
            return;
        };
        let entry = &store.metaobjects[idx];
        let ok = allowed.iter().any(|allowed_ref| {
            if let Some(("MetaobjectDefinition", id)) = parse_gid(allowed_ref) {
                stable_id("metaobject_definition", &entry.kind) == id
            } else {
                entry.is_of(allowed_ref)
            }
        });
        if !ok {
            diagnostics
                .error(
                    "validation_failed",
                    &at.file,
                    &at.pointer,
                    format!(
                        "metaobject \"{}/{}\" has type \"{}\", which is not allowed by the definition",
                        entry.kind, entry.handle, entry.kind
                    ),
                )
                .hint(format!(
                    "expected metaobject type: {}",
                    allowed.join(", ")
                ));
        }
    };
    if kind.starts_with("list.") {
        if let Some(items) = value.as_array() {
            for (index, item) in items.iter().enumerate() {
                check_one(item, &origin.child(index), diagnostics);
            }
        }
    } else {
        check_one(&value, origin, diagnostics);
    }
}

fn extract_url_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once(':')?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let host = rest
        .strip_prefix("//")?
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?
        .split(':')
        .next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn file_kind_from_path(path: &str) -> &'static str {
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "avif" | "heic" | "ico" => {
            "Image"
        }
        "mp4" | "mov" | "webm" | "m4v" => "Video",
        _ => "GenericFile",
    }
}

fn measurement_factor(unit: &str) -> Option<f64> {
    Some(match unit {
        "g" | "grams" => 1.0,
        "kg" | "kilograms" => 1000.0,
        "oz" | "ounces" => 28.349_523_125,
        "lb" | "pounds" => 453.592_37,
        "ml" | "milliliters" => 1.0,
        "cl" | "centiliters" => 10.0,
        "l" | "liters" => 1000.0,
        "m3" | "cubic_meters" => 1_000_000.0,
        "us_fl_oz" | "us_fluid_ounces" => 29.573_529_562_5,
        "us_pt" | "us_pints" => 473.176_473,
        "us_qt" | "us_quarts" => 946.352_946,
        "us_gal" | "us_gallons" => 3_785.411_784,
        "imp_fl_oz" | "imperial_fluid_ounces" => 28.413_062_5,
        "imp_pt" | "imperial_pints" => 568.261_25,
        "imp_qt" | "imperial_quarts" => 1_136.522_5,
        "imp_gal" | "imperial_gallons" => 4_546.09,
        "mm" | "millimeters" => 1.0,
        "cm" | "centimeters" => 10.0,
        "m" | "meters" => 1000.0,
        "in" | "inches" => 25.4,
        "ft" | "feet" => 304.8,
        "yd" | "yards" => 914.4,
        _ => return None,
    })
}

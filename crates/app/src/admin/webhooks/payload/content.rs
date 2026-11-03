//! The payloads of the content family that are not the shop's: metafield definitions
//! (`metafield_definitions/create`, `/update`, `/delete`). The metaobjects' are with the shop's
//! (`shop.rs`).
//!
//! Shopify's example payloads (webhook reference of 2026-10) give the keys and their order; the
//! values follow the record:
//!
//! - `owner_type` is the name of the resource as REST writes it (`Customer`, `ProductVariant`),
//!   `type_name` the type, `options` the validations (`{name, value}`), `validation_status` and
//!   `admin_filter_status` in lowercase, `pinned_position` `0` when the definition is not pinned
//!   (the example's), the capabilities as booleans.
//! - The fields GraphQL does not have (`api_client_id`, `standard_template_id`, `deleting`,
//!   `option_linkable`, `app_config_managed`, `access_only`, the dates) are the example's: `null`
//!   or `false`.
//! - `metafield_definitions/delete`: the GID, the type as its Ruby class
//!   (`ShopifyMetafields::Types::SingleLineTextFieldType`; a list's is lsf's guess,
//!   `…::ListSingleLineTextFieldType`), and the app that made the definition (`null` for one
//!   outside the app's reserved namespace).

use serde_json::{Map, Value, json};

use super::{Lookup, boolean, field, id, lower, text};

/// The REST name of a `MetafieldOwnerType`.
fn owner_type(owner_type: &str) -> String {
    match owner_type {
        "PRODUCTVARIANT" => "ProductVariant".to_string(),
        "DRAFTORDER" => "DraftOrder".to_string(),
        "CARTTRANSFORM" => "CartTransform".to_string(),
        "API_PERMISSION" => "ApiPermission".to_string(),
        other => other
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_string() + &chars.as_str().to_ascii_lowercase(),
                    None => String::new(),
                }
            })
            .collect(),
    }
}

/// The capability of a definition, `enabled`.
fn capability(record: &Map<String, Value>, name: &str) -> Value {
    Value::Bool(
        field(record, "capabilities")
            .pointer(&format!("/{name}/enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

/// `metafield_definitions/create`, `metafield_definitions/update`: a `MetafieldDefinition`.
pub(super) fn metafield_definition(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let shop_id = lookup.shop().map_or(Value::Null, |shop| id(&shop));
    let options: Vec<Value> = field(record, "validations")
        .as_array()
        .into_iter()
        .flatten()
        .map(|validation| {
            json!({
                "name": validation.get("name").cloned().unwrap_or(Value::Null),
                "value": validation.get("value").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let access = field(record, "access");
    let filter_status = field(record, "capabilities")
        .pointer("/adminFilterable/status")
        .map_or(json!("not_filterable"), lower);
    json!({
        "id": id(record),
        "shop_id": shop_id,
        "namespace": text(record, "namespace"),
        "key": text(record, "key"),
        "name": text(record, "name"),
        "description": text(record, "description"),
        "owner_type": owner_type(field(record, "ownerType").as_str().unwrap_or_default()),
        "deleting": false,
        "type_name": field(record, "type").get("name").cloned().unwrap_or(Value::Null),
        "options": options,
        "api_client_id": Value::Null,
        "created_at": Value::Null,
        "updated_at": Value::Null,
        "validation_status": lower(field(record, "validationStatus")),
        "pinned_position": field(record, "pinnedPosition").as_i64().unwrap_or(0),
        "standard_template_id": Value::Null,
        "merchant_writeable": access.get("admin").and_then(Value::as_str).map(|admin| {
            admin.ends_with("READ_WRITE")
        }),
        "storefront_readable": access.get("storefront").and_then(Value::as_str).map(|storefront| {
            storefront == "PUBLIC_READ"
        }),
        "customer_access": access.get("customerAccount").map_or(Value::Null, lower),
        "namespace_owner_api_client_id": Value::Null,
        "unique_values": capability(record, "uniqueValues"),
        "smart_collection_condition": capability(record, "smartCollectionCondition"),
        "option_linkable": false,
        "admin_filterable": capability(record, "adminFilterable"),
        "admin_filterable_status": Value::Null,
        "app_config_managed": false,
        "access_only": false,
        "admin_filter_status": filter_status,
        "use_as_collection_condition": boolean(record, "useAsCollectionCondition", false),
    })
}

/// The Ruby class of a metafield type: `single_line_text_field` is
/// `ShopifyMetafields::Types::SingleLineTextFieldType`.
fn type_class(kind: &str) -> String {
    let camel: String = kind
        .split(['_', '.'])
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    format!("ShopifyMetafields::Types::{camel}Type")
}

/// `metafield_definitions/delete`.
pub(super) fn metafield_definition_deleted(
    record: &Map<String, Value>,
    lookup: &dyn Lookup,
) -> Value {
    let kind = field(record, "type")
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // The app made the definitions of its reserved namespace.
    let app = field(record, "namespace")
        .as_str()
        .filter(|namespace| namespace.starts_with("app--"))
        .and_then(|namespace| {
            let id = namespace.trim_start_matches("app--").split("--").next()?;
            lookup
                .record(&format!("gid://shopify/App/{id}"))
                .map(|_| json!(format!("gid://shopify/App/{id}")))
        });
    json!({
        "id": text(record, "id"),
        "type": type_class(kind),
        "created_by_app_id": app.unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_owner_types_and_types_as_rest() {
        assert_eq!(owner_type("CUSTOMER"), "Customer");
        assert_eq!(owner_type("COMPANY_LOCATION"), "CompanyLocation");
        assert_eq!(owner_type("PRODUCTVARIANT"), "ProductVariant");
        assert_eq!(
            type_class("single_line_text_field"),
            "ShopifyMetafields::Types::SingleLineTextFieldType"
        );
    }
}

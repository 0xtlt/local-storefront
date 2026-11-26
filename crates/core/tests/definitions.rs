//! Tests for declarative metafield and metaobject definitions in `shopify-local/`:
//! root data files, per-file folders, schema validation, value enforcement, and JSON schemas.

use std::fs;
use std::path::PathBuf;

use lsf_core::store::load::{DataSource, FileRole, LoadOptions, load, load_with_overlay, role_of};
use lsf_core::store::validate::{FileKind, schema};
use lsf_core::store::{DefinitionStorefrontAccess, MetafieldOwnerType};
use serde_json::json;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lsf-defs-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn load(&self) -> (lsf_core::Store, lsf_core::diagnostics::Diagnostics) {
        load(
            &DataSource::Directory(self.0.clone()),
            &LoadOptions::default(),
        )
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn codes(diagnostics: &lsf_core::diagnostics::Diagnostics) -> Vec<&'static str> {
    diagnostics.items.iter().map(|d| d.code).collect()
}

#[test]
fn issue_343_repro_example_validates_cleanly() {
    let dir = TempDir::new("repro");
    dir.write(
        "store.json",
        r#"{
  "shop": { "name": "Repro Shop", "currency": "USD" },
  "metafield_definitions": [
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "care_guide",
      "name": "Care guide",
      "type": "multi_line_text_field",
      "validations": [{ "name": "max", "value": "500" }],
      "pinned": true,
      "access": { "storefront": "PUBLIC_READ" }
    }
  ],
  "metaobject_definitions": {
    "look": {
      "name": "Look",
      "url_handle": "looks",
      "display_name_key": "title",
      "access": { "storefront": "PUBLIC_READ" },
      "capabilities": { "publishable": true, "translatable": false },
      "field_definitions": [
        { "key": "title", "name": "Title", "type": "single_line_text_field", "required": true },
        { "key": "products", "name": "Products", "type": "list.product_reference" }
      ]
    }
  }
}"#,
    );
    let (store, diagnostics) = dir.load();
    assert!(diagnostics.is_empty(), "{diagnostics}");
    assert_eq!(store.metafield_definitions.len(), 1);
    let def = &store.metafield_definitions[0];
    assert_eq!(def.owner_type, MetafieldOwnerType::Product);
    assert_eq!(def.namespace, "custom");
    assert_eq!(def.key, "care_guide");
    assert_eq!(def.name, "Care guide");
    assert_eq!(def.kind, "multi_line_text_field");
    assert_eq!(def.pinned_position, Some(1));
    assert_eq!(
        def.access.storefront,
        Some(DefinitionStorefrontAccess::PublicRead)
    );
    assert_eq!(def.validations.len(), 1);
    assert_eq!(def.validations[0].name, "max");
    assert_eq!(def.validations[0].value, "500");

    let look = store.metaobject_definitions.get("look").expect("look def");
    assert_eq!(look.name.as_deref(), Some("Look"));
    assert_eq!(look.url_handle.as_deref(), Some("looks"));
    assert_eq!(store.metaobject_url_handle("look"), "looks");
    assert_eq!(look.display_name_key.as_deref(), Some("title"));
    assert_eq!(look.capabilities.publishable, Some(true));
    assert_eq!(look.capabilities.translatable, Some(false));
    let fields = look.field_definitions.as_ref().expect("fields");
    assert_eq!(fields.len(), 2);
    assert!(fields[0].required);
    assert!(!fields[1].required);
}

#[test]
fn loads_definitions_from_root_files_and_per_file_folders() {
    assert_eq!(role_of("metafield_definitions/care.json"), FileRole::Store);
    assert_eq!(role_of("metaobject_definitions/look.json"), FileRole::Store);

    let dir = TempDir::new("folders");
    dir.write("store.json", r#"{ "shop": { "name": "Split Shop" } }"#);
    dir.write(
        "definitions.json",
        r#"{
  "metafield_definitions": [
    {
      "owner_type": "SHOP",
      "namespace": "custom",
      "key": "banner_text",
      "type": "single_line_text_field"
    }
  ]
}"#,
    );
    // Single object per file in metafield_definitions/
    dir.write(
        "metafield_definitions/care_guide.json",
        r#"{
  "$schema": "../schema/metafield-definition.schema.json",
  "owner_type": "PRODUCT",
  "namespace": "custom",
  "key": "care_guide",
  "type": "multi_line_text_field",
  "validations": [{ "name": "max", "value": 500 }]
}"#,
    );
    // Array of definitions in metafield_definitions/
    dir.write(
        "metafield_definitions/look_refs.json",
        r#"[
  {
    "owner_type": "PRODUCT",
    "namespace": "custom",
    "key": "featured_look",
    "type": "metaobject_reference",
    "validations": [{ "name": "metaobject_definition_id", "value": "look" }]
  },
  {
    "owner_type": "PRODUCT",
    "namespace": "custom",
    "key": "related_content",
    "type": "mixed_reference",
    "validations": [{ "name": "metaobject_definition_ids", "value": ["look", "product_review"] }]
  }
]"#,
    );
    // Single object in metaobject_definitions/ using file stem as default type (keeps underscore!)
    dir.write(
        "metaobject_definitions/product_review.json",
        r#"{
  "$schema": "../schema/metaobject-definition.schema.json",
  "name": "Product Review",
  "standard_template": true,
  "field_definitions": [
    { "key": "title", "type": "single_line_text_field" }
  ]
}"#,
    );
    // Array in metaobject_definitions/ with explicit type
    dir.write(
        "metaobject_definitions/editorial.json",
        r#"[
  {
    "type": "look",
    "name": "Look",
    "capabilities": {
      "online_store": { "enabled": true, "url_handle": "looks", "create_redirects": true },
      "renderable": { "enabled": true, "meta_title_key": "title" }
    },
    "field_definitions": [
      { "key": "title", "type": "single_line_text_field", "required": true }
    ]
  }
]"#,
    );

    let (store, diagnostics) = dir.load();
    assert!(diagnostics.is_empty(), "{diagnostics}");
    assert_eq!(store.metafield_definitions.len(), 4);
    assert_eq!(store.metaobject_definitions.len(), 2);
    assert!(store.metaobject_definitions.contains_key("product_review"));
    assert!(store.metaobject_definitions.contains_key("look"));
    assert_eq!(store.metaobject_url_handle("look"), "looks");
}

#[test]
fn rejects_invalid_definition_schemas_and_constraints() {
    let dir = TempDir::new("invalid-defs");
    dir.write(
        "store.json",
        r#"{
  "metafield_definitions": [
    {
      "owner_type": "PRODUCT",
      "namespace": "ab",
      "key": "x",
      "type": "string"
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "shopify",
      "key": "unavailable_reason",
      "type": "single_line_text_field"
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "bad_access",
      "type": "single_line_text_field",
      "access": { "admin": "MERCHANT_READ" },
      "capabilities": { "cart_to_order_copyable": true }
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "bad_vals",
      "type": "number_integer",
      "validations": [
        { "name": "choices", "value": ["a"] },
        { "name": "min", "value": "10" },
        { "name": "max", "value": "5" }
      ]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "bad_rating",
      "type": "rating"
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "bad_ref",
      "type": "metaobject_reference",
      "validations": [{ "name": "metaobject_definition_id", "value": "nonexistent" }]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "bad_regex",
      "type": "single_line_text_field",
      "validations": [{ "name": "regex", "value": "[unclosed" }]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "dup_key",
      "type": "single_line_text_field"
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "dup_key",
      "type": "single_line_text_field"
    }
  ],
  "metaobject_definitions": {
    "look": {
      "display_name_key": "missing_key",
      "url_handle": "looks",
      "capabilities": { "online_store": false },
      "access": { "admin": "MERCHANT_READ" },
      "field_definitions": [
        { "key": "title", "type": "single_line_text_field" },
        { "key": "title", "type": "number_integer" }
      ]
    }
  }
}"#,
    );

    let (_, diagnostics) = dir.load();
    let found = codes(&diagnostics);
    for expected in [
        "invalid_definition",
        "unknown_metafield_type",
        "reserved_namespace",
        "admin_access_not_allowed",
        "invalid_capability",
        "invalid_validation",
        "missing_validation",
        "unknown_metaobject_definition",
        "duplicate_metafield_definition",
        "conflicting_capability",
        "duplicate_field_definition",
    ] {
        assert!(
            found.contains(&expected),
            "expected {expected} in {found:?}\n{diagnostics}"
        );
    }
}

#[test]
fn enforces_metafield_definitions_against_store_values() {
    let dir = TempDir::new("enforce-mf");
    dir.write(
        "store.json",
        r#"{
  "shop": {
    "name": "Enforced Shop",
    "currency": "USD",
    "metafields": {
      "custom": {
        "tier": "bronze",
        "config": { "type": "json", "value": { "count": "not-an-int" } }
      }
    }
  },
  "metafield_definitions": [
    {
      "owner_type": "SHOP",
      "namespace": "custom",
      "key": "tier",
      "type": "single_line_text_field",
      "validations": [{ "name": "choices", "value": ["gold", "silver"] }]
    },
    {
      "owner_type": "SHOP",
      "namespace": "custom",
      "key": "config",
      "type": "json",
      "validations": [{
        "name": "schema",
        "value": { "type": "object", "properties": { "count": { "type": "integer" } }, "required": ["count"] }
      }]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "care_guide",
      "type": "multi_line_text_field",
      "validations": [{ "name": "max", "value": "10" }]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "weight_limit",
      "type": "weight",
      "validations": [{ "name": "max", "value": { "value": 500, "unit": "g" } }]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "tags_list",
      "type": "list.single_line_text_field",
      "validations": [
        { "name": "list.min", "value": "2" },
        { "name": "regex", "value": "^[a-z]+$" }
      ]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "look_ref",
      "type": "metaobject_reference",
      "validations": [{ "name": "metaobject_definition_id", "value": "look" }]
    },
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "score",
      "type": "number_integer",
      "validations": [{ "name": "min", "value": "1" }, { "name": "max", "value": "5" }]
    }
  ],
  "metaobject_definitions": {
    "look": { "field_definitions": [{ "key": "title", "type": "single_line_text_field", "required": true }] },
    "designer": { "field_definitions": [{ "key": "name", "type": "single_line_text_field", "required": true }] }
  },
  "metaobjects": {
    "look": [{ "handle": "summer", "fields": { "title": "Summer" } }],
    "designer": [{ "handle": "sam", "fields": { "name": "Sam" } }]
  },
  "products": [
    {
      "title": "Valid Shirt",
      "price": "20.00",
      "metafields": {
        "custom": {
          "care_guide": "Cold wash",
          "weight_limit": { "value": 0.4, "unit": "kg" },
          "tags_list": ["cotton", "summer"],
          "look_ref": "look/summer",
          "score": 4
        }
      }
    },
    {
      "title": "Invalid Shirt",
      "price": "20.00",
      "metafields": {
        "custom": {
          "care_guide": "This care guide is definitely over ten chars",
          "weight_limit": { "type": "weight", "value": { "value": 1, "unit": "kg" } },
          "tags_list": ["BAD123"],
          "look_ref": "designer/sam",
          "score": { "type": "single_line_text_field", "value": "4" }
        }
      }
    }
  ]
}"#,
    );

    let (store, diagnostics) = dir.load();
    // Valid Shirt's shorthand metafields adopted their definition's types.
    let valid_shirt = store.product_by_handle("valid-shirt").unwrap();
    assert_eq!(
        valid_shirt.metafields["custom"]["care_guide"].kind,
        "multi_line_text_field"
    );
    assert_eq!(
        valid_shirt.metafields["custom"]["look_ref"].kind,
        "metaobject_reference"
    );
    assert_eq!(
        valid_shirt.metafields["custom"]["tags_list"].kind,
        "list.single_line_text_field"
    );

    // Every violation on Shop and Invalid Shirt is reported, none on Valid Shirt.
    assert!(
        !diagnostics
            .items
            .iter()
            .any(|d| d.path.starts_with("/products/0")),
        "valid product had diagnostics:\n{diagnostics}"
    );
    let paths: Vec<(&str, &str)> = diagnostics
        .items
        .iter()
        .map(|d| (d.code, d.path.as_str()))
        .collect();
    assert!(
        paths.contains(&("validation_failed", "/shop/metafields/custom/tier")),
        "{paths:?}"
    );
    assert!(
        paths.contains(&("validation_failed", "/shop/metafields/custom/config/value")),
        "{paths:?}"
    );
    assert!(
        paths.contains(&(
            "validation_failed",
            "/products/1/metafields/custom/care_guide"
        )),
        "{paths:?}"
    );
    assert!(
        paths.contains(&(
            "validation_failed",
            "/products/1/metafields/custom/weight_limit/value"
        )),
        "{paths:?}"
    );
    assert!(
        paths.contains(&(
            "validation_failed",
            "/products/1/metafields/custom/tags_list"
        )),
        "{paths:?}"
    );
    assert!(
        paths.contains(&(
            "validation_failed",
            "/products/1/metafields/custom/tags_list/0"
        )),
        "{paths:?}"
    );
    assert!(
        paths.contains(&(
            "validation_failed",
            "/products/1/metafields/custom/look_ref"
        )),
        "{paths:?}"
    );
    assert!(
        paths.contains(&(
            "metafield_type_mismatch",
            "/products/1/metafields/custom/score/type"
        )),
        "{paths:?}"
    );
}

#[test]
fn enforces_metaobject_definitions_against_entries_and_preserves_legacy_url_handle() {
    let dir = TempDir::new("enforce-mo");
    dir.write(
        "store.json",
        r#"{
  "localization": { "languages": [{ "iso_code": "en" }, { "iso_code": "fr" }] },
  "metaobject_definitions": {
    "legacy_designer": { "url_handle": "designers" },
    "look": {
      "capabilities": { "translatable": false },
      "field_definitions": [
        { "key": "title", "type": "single_line_text_field", "required": true, "validations": [{ "name": "min", "value": "3" }] },
        { "key": "season", "type": "single_line_text_field", "validations": [{ "name": "choices", "value": ["spring", "summer"] }] }
      ]
    }
  },
  "metaobjects": {
    "legacy_designer": [
      { "handle": "sam", "fields": { "name": "Sam", "years": 10 } }
    ],
    "look": [
      { "handle": "ok", "fields": { "title": "Summer", "season": "summer" } },
      {
        "handle": "broken",
        "fields": { "season": "winter", "extra": "nope" },
        "translations": { "fr": { "season": "hiver" } }
      }
    ]
  }
}"#,
    );

    let (store, diagnostics) = dir.load();
    assert_eq!(store.metaobject_url_handle("legacy_designer"), "designers");
    let found: Vec<(&str, &str)> = diagnostics
        .items
        .iter()
        .map(|d| (d.code, d.path.as_str()))
        .collect();
    // Legacy definition without field_definitions produces zero errors.
    assert!(
        !found.iter().any(|(_, p)| p.contains("legacy_designer")),
        "{found:?}"
    );
    assert!(
        !found
            .iter()
            .any(|(_, p)| p.starts_with("/metaobjects/look/0")),
        "{found:?}"
    );
    assert!(
        found.contains(&("missing_required_field", "/metaobjects/look/1/fields")),
        "{found:?}"
    );
    assert!(
        found.contains(&(
            "unknown_metaobject_field",
            "/metaobjects/look/1/fields/extra"
        )),
        "{found:?}"
    );
    assert!(
        found.contains(&("validation_failed", "/metaobjects/look/1/fields/season")),
        "{found:?}"
    );
    assert!(
        found.contains(&(
            "untranslatable_metaobject",
            "/metaobjects/look/1/translations"
        )),
        "{found:?}"
    );
}

#[test]
fn supports_duplicate_detection_pinned_limits_and_overlays() {
    let dir = TempDir::new("dups-and-pins");
    dir.write(
        "store.json",
        r#"{
  "metaobject_definitions": { "look": { "name": "Look" } }
}"#,
    );
    dir.write(
        "metaobject_definitions/look.json",
        r#"{ "name": "Duplicate Look" }"#,
    );
    let (_, diagnostics) = dir.load();
    assert!(
        codes(&diagnostics).contains(&"duplicate_metaobject_definition"),
        "{diagnostics}"
    );

    // 21 pinned definitions on PRODUCT exceeds Shopify's 20-pin limit.
    let many_defs: Vec<serde_json::Value> = (0..21)
        .map(|i| {
            json!({
                "owner_type": "PRODUCT",
                "namespace": "custom",
                "key": format!("field_{i}"),
                "type": "single_line_text_field",
                "pinned": true
            })
        })
        .collect();
    let (_, diag) = load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&json!({ "metafield_definitions": many_defs })),
    );
    assert!(codes(&diag).contains(&"pinned_limit_reached"), "{diag}");
}

#[test]
fn exposes_json_schemas_for_definitions() {
    let mf_schema = schema(FileKind::MetafieldDefinition);
    assert!(mf_schema["properties"]["owner_type"].is_object());
    assert!(mf_schema["properties"]["validations"].is_object());
    assert!(mf_schema["properties"]["standard_template"].is_object());

    let mo_schema = schema(FileKind::MetaobjectDefinition);
    assert!(mo_schema["properties"]["field_definitions"].is_object());
    assert!(mo_schema["properties"]["display_name_key"].is_object());
    assert!(mo_schema["properties"]["capabilities"].is_object());
    assert!(mo_schema["properties"]["url_handle"].is_object());

    let store_schema = schema(FileKind::Store);
    assert!(store_schema["properties"]["metafield_definitions"].is_object());
    assert!(store_schema["properties"]["metaobject_definitions"].is_object());
}

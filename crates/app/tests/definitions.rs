//! Integration tests for declarative metafield and metaobject definitions in `shopify-local/`
//! loaded into the local Admin API: queries, mutations, portable metaobject type GID resolution,
//! `standardTemplate`, `$app` ownership, and coexistence with `shopify.app.toml` and inferred
//! definitions.

use std::fs;
use std::path::PathBuf;

use lsf_app::admin::graphql::{self, Request};
use lsf_app::admin::resolve::{Registry, ShopResolver};
use lsf_app::admin::state::ShopState;
use lsf_app::admin::state::seed::seed;
use lsf_core::store::load::{DataSource, LoadOptions, load};
use serde_json::{Value, json};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lsf-app-defs-{label}-{}-{}",
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

    fn seed(&self, app: Option<&lsf_app::config::App>) -> ShopState {
        let (store, diagnostics) = load(
            &DataSource::Directory(self.0.clone()),
            &LoadOptions::default(),
        );
        assert!(diagnostics.is_empty(), "store diagnostics:\n{diagnostics}");
        seed(&store, app)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut request = Request::new(query);
    if let Value::Object(variables) = variables {
        request.variables = variables;
    }
    let answer = graphql::run(
        lsf_app::admin::schema::admin(),
        &mut resolver,
        &request,
        None,
    )
    .to_json();
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

#[test]
fn boots_admin_api_with_repro_definitions_and_enforces_mutations() {
    let dir = TempDir::new("repro-admin");
    dir.write(
        "store.json",
        r#"{
  "shop": { "name": "Repro Shop", "currency": "USD" },
  "products": [
    { "title": "Linen Shirt", "price": "45.00" }
  ],
  "metafield_definitions": [
    {
      "owner_type": "PRODUCT",
      "namespace": "custom",
      "key": "care_guide",
      "name": "Care guide",
      "description": "Washing instructions",
      "type": "multi_line_text_field",
      "validations": [{ "name": "max", "value": "500" }],
      "pinned": true,
      "access": { "storefront": "PUBLIC_READ" },
      "capabilities": { "admin_filterable": true }
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

    let mut shop = dir.seed(None);
    let product_id = shop.keys("Product").next().unwrap().to_string();

    // 1. Query metafieldDefinitions
    let mf_defs = run(
        &mut shop,
        "{ metafieldDefinitions(first: 10, ownerType: PRODUCT) { nodes {
            namespace key name description ownerType pinnedPosition
            type { name }
            validations { name type value }
            access { admin storefront customerAccount }
            capabilities { adminFilterable { enabled } }
            metafieldsCount validationStatus
        } } }",
        Value::Null,
    );
    let nodes = mf_defs["data"]["metafieldDefinitions"]["nodes"]
        .as_array()
        .unwrap();
    assert_eq!(nodes.len(), 1, "{mf_defs}");
    assert_eq!(nodes[0]["namespace"], "custom");
    assert_eq!(nodes[0]["key"], "care_guide");
    assert_eq!(nodes[0]["name"], "Care guide");
    assert_eq!(nodes[0]["description"], "Washing instructions");
    assert_eq!(nodes[0]["pinnedPosition"], json!(1));
    assert_eq!(nodes[0]["type"]["name"], "multi_line_text_field");
    assert_eq!(
        nodes[0]["validations"],
        json!([{ "name": "max", "type": "number_integer", "value": "500" }])
    );
    assert_eq!(nodes[0]["access"]["storefront"], "PUBLIC_READ");
    assert_eq!(
        nodes[0]["capabilities"]["adminFilterable"]["enabled"],
        json!(true)
    );

    // 2. metafieldsSet enforces max: 500 and infers type from the declared definition
    let too_long = run(
        &mut shop,
        "mutation($m: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $m) {
            metafields { id } userErrors { field code message }
        } }",
        json!({ "m": [{
            "ownerId": product_id,
            "namespace": "custom",
            "key": "care_guide",
            "value": "a".repeat(501)
        }] }),
    );
    assert_eq!(
        too_long["data"]["metafieldsSet"]["userErrors"][0]["code"], "TOO_LONG",
        "{too_long}"
    );

    let ok_set = run(
        &mut shop,
        "mutation($m: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $m) {
            metafields { type value definition { id } } userErrors { code message }
        } }",
        json!({ "m": [{
            "ownerId": product_id,
            "namespace": "custom",
            "key": "care_guide",
            "value": "Hand wash cold"
        }] }),
    );
    assert_eq!(ok_set["data"]["metafieldsSet"]["userErrors"], json!([]));
    assert_eq!(
        ok_set["data"]["metafieldsSet"]["metafields"][0]["type"],
        "multi_line_text_field"
    );
    assert!(ok_set["data"]["metafieldsSet"]["metafields"][0]["definition"]["id"].is_string());

    // 3. productSet enforces declared PRODUCT metafield definition
    let bad_product_set = run(
        &mut shop,
        "mutation($input: ProductSetInput!) { productSet(input: $input) {
            product { id } userErrors { field code message }
        } }",
        json!({ "input": {
            "title": "Jacket",
            "metafields": [{
                "namespace": "custom",
                "key": "care_guide",
                "value": "x".repeat(501)
            }]
        } }),
    );
    assert!(
        !bad_product_set["data"]["productSet"]["userErrors"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{bad_product_set}"
    );

    // 4. Query metaobjectDefinitionByType (even with 0 entries seeded!)
    let mo_def = run(
        &mut shop,
        "{ metaobjectDefinitionByType(type: \"look\") {
            id type name displayNameKey
            access { admin storefront customerAccount }
            capabilities {
                publishable { enabled }
                translatable { enabled }
                onlineStore { enabled data { urlHandle canCreateRedirects } }
            }
            fieldDefinitions { key name required type { name } }
        } }",
        Value::Null,
    );
    let look = &mo_def["data"]["metaobjectDefinitionByType"];
    assert_eq!(look["type"], "look", "{mo_def}");
    assert_eq!(look["name"], "Look");
    assert_eq!(look["displayNameKey"], "title");
    assert_eq!(look["access"]["storefront"], "PUBLIC_READ");
    assert_eq!(look["capabilities"]["publishable"]["enabled"], json!(true));
    assert_eq!(
        look["capabilities"]["translatable"]["enabled"],
        json!(false)
    );
    assert_eq!(
        look["capabilities"]["onlineStore"]["data"]["urlHandle"],
        "looks"
    );
    assert_eq!(look["fieldDefinitions"][0]["key"], "title");
    assert_eq!(look["fieldDefinitions"][0]["required"], json!(true));

    // 5. metaobjectCreate enforces required field and publishable capability
    let missing_required = run(
        &mut shop,
        "mutation($mo: MetaobjectCreateInput!) { metaobjectCreate(metaobject: $mo) {
            metaobject { id } userErrors { code elementKey message }
        } }",
        json!({ "mo": { "type": "look", "fields": [] } }),
    );
    assert_eq!(
        missing_required["data"]["metaobjectCreate"]["userErrors"][0]["code"],
        "OBJECT_FIELD_REQUIRED",
        "{missing_required}"
    );

    let created_look = run(
        &mut shop,
        "mutation($mo: MetaobjectCreateInput!) { metaobjectCreate(metaobject: $mo) {
            metaobject { id handle displayName capabilities { publishable { status } } }
            userErrors { code message }
        } }",
        json!({ "mo": {
            "type": "look",
            "fields": [
                { "key": "title", "value": "Summer Breeze" },
                { "key": "products", "value": json!([product_id]).to_string() }
            ]
        } }),
    );
    assert_eq!(
        created_look["data"]["metaobjectCreate"]["userErrors"],
        json!([]),
        "{created_look}"
    );
    assert_eq!(
        created_look["data"]["metaobjectCreate"]["metaobject"]["displayName"],
        "Summer Breeze"
    );
    assert_eq!(
        created_look["data"]["metaobjectCreate"]["metaobject"]["capabilities"]["publishable"]["status"],
        "DRAFT"
    );
}

#[test]
fn resolves_portable_metaobject_references_and_standard_templates_and_app_toml() {
    let dir = TempDir::new("portable-and-standard");
    dir.write(
        "store.json",
        r#"{
  "shop": { "name": "Portable Shop", "currency": "USD" },
  "products": [
    {
      "title": "Mug",
      "price": "15.00",
      "metafields": {
        "custom": {
          "unstructured_note": "Inferred!"
        },
        "descriptors": {
          "care_guide": "Dishwasher safe"
        }
      }
    }
  ],
  "metafield_definitions": [
    {
      "owner_type": "PRODUCT",
      "namespace": "descriptors",
      "key": "care_guide",
      "name": "Care guide",
      "type": "multi_line_text_field",
      "validations": [{ "name": "max", "value": "500" }],
      "standard_template": true
    },
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
      "key": "related_entries",
      "type": "list.mixed_reference",
      "validations": [{ "name": "metaobject_definition_types", "value": ["look", "$app:author"] }]
    }
  ],
  "metaobject_definitions": {
    "look": {
      "name": "Look",
      "field_definitions": [
        { "key": "title", "type": "single_line_text_field", "required": true },
        {
          "key": "author",
          "type": "metaobject_reference",
          "validations": [{ "name": "metaobject_definition_id", "value": "$app:author" }]
        }
      ]
    },
    "$app:author": {
      "name": "Author",
      "access": { "admin": "MERCHANT_READ", "storefront": "PUBLIC_READ" },
      "field_definitions": [
        { "key": "name", "type": "single_line_text_field", "required": true }
      ]
    },
    "other": {
      "name": "Other",
      "field_definitions": [{ "key": "label", "type": "single_line_text_field" }]
    }
  },
  "metaobjects": {
    "look": [{ "handle": "spring", "fields": { "title": "Spring" } }],
    "$app:author": [{ "handle": "jane", "fields": { "name": "Jane" } }],
    "other": [{ "handle": "x", "fields": { "label": "X" } }]
  }
}"#,
    );

    let mut shop = dir.seed(None);
    let product_id = shop.keys("Product").next().unwrap().to_string();

    // Look up seeded MetaobjectDefinition GIDs.
    let look_def = run(
        &mut shop,
        "{ metaobjectDefinitionByType(type: \"look\") { id fieldDefinitions { key validations { name type value } } } }",
        Value::Null,
    );
    let look_def_id = look_def["data"]["metaobjectDefinitionByType"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let author_def = run(
        &mut shop,
        "{ metaobjectDefinitionByType(type: \"$app:author\") { id type access { admin storefront } } }",
        Value::Null,
    );
    let author_def_id = author_def["data"]["metaobjectDefinitionByType"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        author_def["data"]["metaobjectDefinitionByType"]["access"]["admin"],
        "MERCHANT_READ"
    );

    // Portable `$app:author` reference inside `look.field_definitions[1]` resolved to the local GID!
    assert_eq!(
        look_def["data"]["metaobjectDefinitionByType"]["fieldDefinitions"][1]["validations"],
        json!([{
            "name": "metaobject_definition_id",
            "type": "single_line_text_field",
            "value": author_def_id,
        }])
    );

    // Check metafield definitions: standardTemplate, inferred coexistence, and resolved GIDs.
    let mf_defs = run(
        &mut shop,
        "{ metafieldDefinitions(first: 20, ownerType: PRODUCT) { nodes {
            namespace key metafieldsCount validationStatus
            standardTemplate { id }
            validations { name type value }
        } } }",
        Value::Null,
    );
    let nodes = mf_defs["data"]["metafieldDefinitions"]["nodes"]
        .as_array()
        .unwrap();
    let find_def = |ns: &str, key: &str| {
        nodes
            .iter()
            .find(|n| n["namespace"] == ns && n["key"] == key)
            .unwrap_or_else(|| panic!("missing {ns}.{key} in {nodes:?}"))
    };
    // 1. Standard template linked + existing product metafield adopted!
    let care = find_def("descriptors", "care_guide");
    assert_eq!(
        care["standardTemplate"]["id"],
        "gid://shopify/StandardMetafieldDefinitionTemplate/2"
    );
    assert_eq!(care["metafieldsCount"], json!(1));
    assert_eq!(care["validationStatus"], "ALL_VALID");

    // 2. Inferred definition still created for unstructured metafield!
    let inferred = find_def("custom", "unstructured_note");
    assert_eq!(inferred["metafieldsCount"], json!(1));

    // 3. Portable `metaobject_definition_id` & `metaobject_definition_types` resolved to GIDs!
    let featured_look = find_def("custom", "featured_look");
    assert_eq!(
        featured_look["validations"],
        json!([{
            "name": "metaobject_definition_id",
            "type": "single_line_text_field",
            "value": look_def_id,
        }])
    );
    let related_entries = find_def("custom", "related_entries");
    assert_eq!(
        related_entries["validations"],
        json!([{
            "name": "metaobject_definition_ids",
            "type": "list.single_line_text_field",
            "value": json!([look_def_id, author_def_id]).to_string(),
        }])
    );

    // 4. metafieldsSet enforces resolved metaobject_definition_id GID!
    let look_entry_id = run(
        &mut shop,
        "{ metaobjectByHandle(handle: { type: \"look\", handle: \"spring\" }) { id } }",
        Value::Null,
    )["data"]["metaobjectByHandle"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other_entry_id = run(
        &mut shop,
        "{ metaobjectByHandle(handle: { type: \"other\", handle: \"x\" }) { id } }",
        Value::Null,
    )["data"]["metaobjectByHandle"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let wrong_type = run(
        &mut shop,
        "mutation($m: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $m) {
            userErrors { code message }
        } }",
        json!({ "m": [{
            "ownerId": product_id,
            "namespace": "custom",
            "key": "featured_look",
            "value": other_entry_id
        }] }),
    );
    assert_eq!(
        wrong_type["data"]["metafieldsSet"]["userErrors"][0]["code"], "INVALID_VALUE",
        "{wrong_type}"
    );

    let right_type = run(
        &mut shop,
        "mutation($m: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $m) {
            metafields { value } userErrors { code message }
        } }",
        json!({ "m": [{
            "ownerId": product_id,
            "namespace": "custom",
            "key": "featured_look",
            "value": look_entry_id
        }] }),
    );
    assert_eq!(
        right_type["data"]["metafieldsSet"]["userErrors"],
        json!([]),
        "{right_type}"
    );
}

#[test]
fn coexists_with_shopify_app_toml_and_supports_metaobject_upsert_and_standard_metaobjects() {
    let dir = TempDir::new("toml-and-upsert");
    dir.write(
        "store.json",
        r#"{
  "shop": { "name": "Coexist Shop", "currency": "USD" },
  "metafield_definitions": [
    {
      "owner_type": "ORDER",
      "namespace": "$app",
      "key": "delivery_note",
      "name": "Delivery note from JSON",
      "type": "single_line_text_field",
      "validations": [{ "name": "max", "value": "80" }],
      "capabilities": { "cart_to_order_copyable": true }
    }
  ],
  "metaobject_definitions": {
    "product_review": {
      "name": "Product Review",
      "standard_template": true,
      "display_name_key": "title",
      "field_definitions": [
        { "key": "title", "type": "single_line_text_field", "required": true },
        { "key": "body", "type": "multi_line_text_field" }
      ]
    }
  }
}"#,
    );

    // Load a shopify.app.toml declaring both an overlapping and a distinct order metafield.
    let app_dir = TempDir::new("toml-app");
    app_dir.write(
        "shopify.app.toml",
        r#"
client_id = "test-client"
name = "Test App"
application_url = "https://example.test"

[order.metafields.app.delivery_note]
type = "single_line_text_field"
name = "Delivery note from TOML"

[order.metafields.app.gift_wrap]
type = "boolean"
name = "Gift wrap"
capabilities.cart_to_order_copyable = true
"#,
    );
    let app = lsf_app::config::load(&app_dir.0).expect("load app config");
    let mut shop = dir.seed(Some(&app));

    let order_defs = run(
        &mut shop,
        "{ metafieldDefinitions(first: 20, ownerType: ORDER) { nodes {
            namespace key name validations { name value }
            capabilities { cartToOrderCopyable { enabled eligible } }
        } } }",
        Value::Null,
    );
    let nodes = order_defs["data"]["metafieldDefinitions"]["nodes"]
        .as_array()
        .unwrap();
    let delivery = nodes.iter().find(|n| n["key"] == "delivery_note").unwrap();
    // JSON definition takes precedence and keeps its max: 80 validation.
    assert_eq!(delivery["name"], "Delivery note from JSON");
    assert_eq!(
        delivery["validations"],
        json!([{ "name": "max", "value": "80" }])
    );
    assert_eq!(
        delivery["capabilities"]["cartToOrderCopyable"],
        json!({ "enabled": true, "eligible": true })
    );
    // TOML-only definition is also seeded.
    let gift_wrap = nodes.iter().find(|n| n["key"] == "gift_wrap").unwrap();
    assert_eq!(gift_wrap["name"], "Gift wrap");
    assert_eq!(
        gift_wrap["capabilities"]["cartToOrderCopyable"],
        json!({ "enabled": true, "eligible": true })
    );

    // Standard metaobject template populated on declared standard_template: true definition.
    let review_def = run(
        &mut shop,
        "{ metaobjectDefinitionByType(type: \"product_review\") {
            name standardTemplate { type name }
        } }",
        Value::Null,
    );
    assert_eq!(
        review_def["data"]["metaobjectDefinitionByType"]["standardTemplate"]["type"],
        "product_review"
    );

    // metaobjectUpsert + metaobjectUpdate enforce declared field definitions.
    let upsert_missing = run(
        &mut shop,
        "mutation { metaobjectUpsert(handle: { type: \"product_review\", handle: \"rev-1\" }, metaobject: { fields: [{ key: \"body\", value: \"Great!\" }] }) {
            metaobject { id } userErrors { code elementKey }
        } }",
        Value::Null,
    );
    assert_eq!(
        upsert_missing["data"]["metaobjectUpsert"]["userErrors"][0]["code"],
        "OBJECT_FIELD_REQUIRED"
    );

    let upsert_ok = run(
        &mut shop,
        "mutation { metaobjectUpsert(handle: { type: \"product_review\", handle: \"rev-1\" }, metaobject: { fields: [{ key: \"title\", value: \"Loved it\" }] }) {
            metaobject { id displayName } userErrors { code message }
        } }",
        Value::Null,
    );
    assert_eq!(
        upsert_ok["data"]["metaobjectUpsert"]["userErrors"],
        json!([])
    );
    let id = upsert_ok["data"]["metaobjectUpsert"]["metaobject"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let update_unknown = run(
        &mut shop,
        "mutation($id: ID!) { metaobjectUpdate(id: $id, metaobject: { fields: [{ key: \"nope\", value: \"x\" }] }) {
            userErrors { code elementKey }
        } }",
        json!({ "id": id }),
    );
    assert_eq!(
        update_unknown["data"]["metaobjectUpdate"]["userErrors"][0]["code"],
        "UNDEFINED_OBJECT_FIELD"
    );
}

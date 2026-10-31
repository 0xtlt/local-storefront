//! The mutations and queries of the content family, through queries over the rich fixture
//! (`tests/fixtures/rich`) and the demo store.

use serde_json::{Value, json};

use crate::graphql::{self, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::ShopState;

pub(super) fn rich() -> ShopState {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich");
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Directory(path),
        &Default::default(),
    );
    ShopState::seeded(&store, None)
}

/// The answer to a request, `extensions` without `cost`.
pub(super) fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let request = Request::new(query).with_variables(variables);
    let mut answer = graphql::run(crate::schema::admin(), &mut resolver, &request, None);
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

/// The topics of the events taken from the shop, in order.
pub(super) fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect()
}

/// The GID of the first record of a type with `field` equal to `value`.
pub(super) fn find(shop: &ShopState, type_name: &str, field: &str, value: &str) -> String {
    shop.keys(type_name)
        .find(|key| {
            shop.get(key)
                .and_then(|record| record.get(field))
                .and_then(Value::as_str)
                == Some(value)
        })
        .unwrap_or_else(|| panic!("no {type_name} with {field} {value}"))
        .to_string()
}

const SET: &str = "mutation($metafields: [MetafieldsSetInput!]!) {
    metafieldsSet(metafields: $metafields) {
        metafields { id namespace key type value jsonValue compareDigest ownerType owner { ... on Product { id } } definition { id } reference { ... on Product { handle } } }
        userErrors { field message code elementIndex }
    }
}";

#[test]
fn sets_metafields_on_any_owner_and_reads_them_back() {
    let mut shop = rich();
    shop.take_events();
    let product = find(&shop, "Product", "handle", "linen-dress");
    let other = find(&shop, "Product", "handle", "coffee-subscription");
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        SET,
        json!({"metafields": [
            {"ownerId": product, "namespace": "custom", "key": "care", "value": "Dry clean."},
            {"ownerId": product, "namespace": "lsf_test", "key": "count", "type": "number_integer", "value": "3"},
            {"ownerId": product, "namespace": "lsf_test", "key": "pair", "type": "product_reference", "value": other},
            {"ownerId": product, "key": "secret", "type": "json", "value": "{\"a\": [1, 2]}"},
            {"ownerId": customer, "namespace": "loyalty", "key": "tier", "type": "single_line_text_field", "value": "gold"},
        ]}),
    );
    let set = &answer["data"]["metafieldsSet"];
    assert_eq!(set["userErrors"], json!([]), "{answer}");
    let metafields = set["metafields"].as_array().unwrap();
    assert_eq!(metafields.len(), 5);
    // The type of an existing metafield's definition is used when the input gives none.
    assert_eq!(metafields[0]["type"], "multi_line_text_field");
    assert_eq!(metafields[0]["value"], "Dry clean.");
    assert!(metafields[0]["definition"]["id"].is_string());
    assert_eq!(metafields[1]["jsonValue"], json!(3));
    assert_eq!(
        metafields[1]["definition"],
        Value::Null,
        "no definition is created"
    );
    assert_eq!(metafields[2]["reference"]["handle"], "coffee-subscription");
    assert_eq!(
        metafields[3]["namespace"],
        json!(format!("app--{}", super::app_id(&shop))),
        "the app's reserved namespace"
    );
    assert_eq!(metafields[3]["value"], "{\"a\":[1,2]}");
    assert_eq!(metafields[0]["owner"]["id"], json!(product));
    assert_eq!(metafields[4]["ownerType"], "CUSTOMER");
    assert_eq!(metafields[0]["compareDigest"].as_str().unwrap().len(), 64);
    // A product's metafields change the product; a customer's have no webhook topic.
    assert_eq!(topics(&mut shop), ["products/update", "metafields/create"]);

    let read = run(
        &mut shop,
        "query($id: ID!) { product(id: $id) {
            count: metafield(namespace: \"lsf_test\", key: \"count\") { value }
            secret: metafield(key: \"secret\") { value }
            app: metafields(first: 5, namespace: \"$app\") { nodes { key } }
        } }",
        json!({"id": product}),
    );
    assert_eq!(read["data"]["product"]["count"]["value"], "3", "{read}");
    assert_eq!(read["data"]["product"]["secret"]["value"], "{\"a\":[1,2]}");
    assert_eq!(
        read["data"]["product"]["app"]["nodes"],
        json!([{"key": "secret"}])
    );

    // The same input again updates the metafield in place: same id.
    let again = run(
        &mut shop,
        SET,
        json!({"metafields": [{"ownerId": product, "namespace": "lsf_test", "key": "count", "type": "number_integer", "value": "4"}]}),
    );
    assert_eq!(
        again["data"]["metafieldsSet"]["metafields"][0]["id"],
        metafields[1]["id"]
    );
    assert_eq!(
        again["data"]["metafieldsSet"]["metafields"][0]["value"],
        "4"
    );
}

#[test]
fn refuses_wrong_metafields_and_changes_nothing() {
    let mut shop = rich();
    shop.take_events();
    let product = find(&shop, "Product", "handle", "linen-dress");
    let before = shop.get(&product).cloned();
    let answer = run(
        &mut shop,
        SET,
        json!({"metafields": [
            {"ownerId": product, "namespace": "lsf_test", "key": "ok", "type": "single_line_text_field", "value": "fine"},
            {"ownerId": product, "namespace": "lsf_test", "key": "num", "type": "number_integer", "value": "330\n"},
            {"ownerId": product, "namespace": "lsf_test", "key": "untyped", "value": "x"},
            {"ownerId": product, "namespace": "custom", "key": "care", "type": "json", "value": "{}"},
            {"ownerId": product, "namespace": "ab", "key": "x", "type": "boolean", "value": "true"},
            {"ownerId": "gid://shopify/Product/1", "namespace": "lsf_test", "key": "kk", "type": "boolean", "value": "true"},
            {"ownerId": product, "namespace": "app--42", "key": "kk", "type": "boolean", "value": "true"},
        ]}),
    );
    let set = &answer["data"]["metafieldsSet"];
    assert_eq!(set["metafields"], Value::Null);
    let errors = set["userErrors"].as_array().unwrap();
    let summary: Vec<(Value, Value, Value)> = errors
        .iter()
        .map(|error| {
            (
                error["field"].clone(),
                error["code"].clone(),
                error["message"].clone(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                json!(["metafields", "1", "value"]),
                json!("INVALID_VALUE"),
                json!("Value must be an integer.")
            ),
            (
                json!(["metafields", "2", "type"]),
                json!("BLANK"),
                json!("Type can't be blank")
            ),
            (
                json!(["metafields", "3", "type"]),
                json!("INVALID_TYPE"),
                json!(
                    "Type 'json' must be consistent with the definition's type: 'multi_line_text_field'."
                )
            ),
            (
                json!(["metafields", "4", "namespace"]),
                json!("TOO_SHORT"),
                json!("Namespace is too short (minimum is 3 characters)")
            ),
            (
                json!(["metafields", "4", "key"]),
                json!("TOO_SHORT"),
                json!("Key is too short (minimum is 2 characters)")
            ),
            (
                json!(["metafields", "5", "ownerId"]),
                json!("INVALID"),
                json!("Owner does not exist.")
            ),
            (
                json!(["metafields", "6", "namespace"]),
                json!("APP_NOT_AUTHORIZED"),
                json!(
                    "Access to this namespace and key on Metafields for this resource type is not allowed."
                )
            ),
        ],
        "{answer}"
    );
    assert_eq!(shop.get(&product).cloned(), before, "nothing changed");
    assert!(topics(&mut shop).is_empty());

    // An owner of a type without metafields is a GraphQL error, as Shopify answers it.
    let invalid = run(
        &mut shop,
        SET,
        json!({"metafields": [{"ownerId": "gid://shopify/App/1", "namespace": "lsf_test", "key": "k", "type": "boolean", "value": "true"}]}),
    );
    assert_eq!(invalid["errors"][0]["message"], "invalid id", "{invalid}");
    assert_eq!(invalid["data"]["metafieldsSet"], Value::Null);

    // More than 25.
    let many: Vec<Value> = (0..26)
        .map(|n| json!({"ownerId": product, "namespace": "lsf_test", "key": format!("k{n}"), "type": "boolean", "value": "true"}))
        .collect();
    let refused = run(&mut shop, SET, json!({"metafields": many}));
    assert_eq!(
        refused["data"]["metafieldsSet"]["userErrors"][0]["code"],
        "LESS_THAN_OR_EQUAL_TO"
    );
}

#[test]
fn compares_digests_before_setting() {
    let mut shop = rich();
    let product = find(&shop, "Product", "handle", "linen-dress");
    let set = |shop: &mut ShopState, digest: Option<Value>, value: &str| {
        let mut input = json!({"ownerId": product, "namespace": "lsf_test", "key": "cas", "type": "single_line_text_field", "value": value});
        if let Some(digest) = digest {
            input["compareDigest"] = digest;
        }
        run(shop, SET, json!({"metafields": [input]}))
    };
    // `null`: only when it does not exist yet.
    let created = set(&mut shop, Some(Value::Null), "one");
    let digest = created["data"]["metafieldsSet"]["metafields"][0]["compareDigest"].clone();
    assert!(digest.is_string(), "{created}");
    let again = set(&mut shop, Some(Value::Null), "two");
    assert_eq!(
        again["data"]["metafieldsSet"]["userErrors"][0]["code"],
        "STALE_OBJECT"
    );
    let updated = set(&mut shop, Some(digest.clone()), "two");
    assert_eq!(updated["data"]["metafieldsSet"]["userErrors"], json!([]));
    let stale = set(&mut shop, Some(digest), "three");
    assert_eq!(
        stale["data"]["metafieldsSet"]["userErrors"][0]["code"],
        "STALE_OBJECT"
    );
    // Left out: nothing is compared.
    let free = set(&mut shop, None, "three");
    assert_eq!(free["data"]["metafieldsSet"]["userErrors"], json!([]));
}

#[test]
fn deletes_metafields() {
    let mut shop = rich();
    shop.take_events();
    let product = find(&shop, "Product", "handle", "linen-dress");
    let answer = run(
        &mut shop,
        "mutation($metafields: [MetafieldIdentifierInput!]!) { metafieldsDelete(metafields: $metafields) {
            deletedMetafields { ownerId namespace key } userErrors { field message } } }",
        json!({"metafields": [
            {"ownerId": product, "namespace": "custom", "key": "care"},
            {"ownerId": product, "namespace": "custom", "key": "nothing"},
        ]}),
    );
    let deleted = &answer["data"]["metafieldsDelete"];
    assert_eq!(deleted["userErrors"], json!([]), "{answer}");
    assert_eq!(
        deleted["deletedMetafields"],
        json!([{"ownerId": product, "namespace": "custom", "key": "care"}, null])
    );
    let read = run(
        &mut shop,
        "query($id: ID!) { product(id: $id) { metafield(namespace: \"custom\", key: \"care\") { id } } }",
        json!({"id": product}),
    );
    assert_eq!(read["data"]["product"]["metafield"], Value::Null);
    assert_eq!(topics(&mut shop), ["products/update"]);
}

const CREATE_DEFINITION: &str = "mutation($definition: MetafieldDefinitionInput!) {
    metafieldDefinitionCreate(definition: $definition) {
        createdDefinition { id name namespace key ownerType pinnedPosition validationStatus metafieldsCount
            type { name category } validations { name type value } access { admin storefront } }
        userErrors { field message code }
    }
}";

#[test]
fn creates_definitions_that_validate_metafields() {
    let mut shop = rich();
    shop.take_events();
    let answer = run(
        &mut shop,
        CREATE_DEFINITION,
        json!({"definition": {
            "name": "Tier", "namespace": "loyalty", "key": "tier", "ownerType": "PRODUCT",
            "type": "single_line_text_field", "pin": true,
            "validations": [{"name": "choices", "value": "[\"gold\", \"silver\"]"}],
        }}),
    );
    let created = &answer["data"]["metafieldDefinitionCreate"];
    assert_eq!(created["userErrors"], json!([]), "{answer}");
    let definition = &created["createdDefinition"];
    assert_eq!(definition["pinnedPosition"], json!(1));
    assert_eq!(
        definition["type"],
        json!({"name": "single_line_text_field", "category": "TEXT"})
    );
    assert_eq!(
        definition["validations"],
        json!([{"name": "choices", "type": "list.single_line_text_field", "value": "[\"gold\", \"silver\"]"}])
    );
    assert_eq!(definition["access"]["admin"], "PUBLIC_READ_WRITE");
    assert_eq!(topics(&mut shop), ["metafield_definitions/create"]);

    // The same owner type, namespace and key: taken.
    let taken = run(
        &mut shop,
        CREATE_DEFINITION,
        json!({"definition": {"name": "Tier", "namespace": "loyalty", "key": "tier", "ownerType": "PRODUCT", "type": "single_line_text_field"}}),
    );
    assert_eq!(
        taken["data"]["metafieldDefinitionCreate"]["userErrors"],
        json!([{"field": ["definition", "key"], "message": "Key is in use for Product metafields on the 'loyalty' namespace.", "code": "TAKEN"}])
    );

    // Values are now checked against it; the type comes from it.
    let product = find(&shop, "Product", "handle", "linen-dress");
    let refused = run(
        &mut shop,
        SET,
        json!({"metafields": [{"ownerId": product, "namespace": "loyalty", "key": "tier", "value": "bronze"}]}),
    );
    assert_eq!(
        refused["data"]["metafieldsSet"]["userErrors"][0]["code"], "INVALID_VALUE",
        "{refused}"
    );
    let set = run(
        &mut shop,
        SET,
        json!({"metafields": [{"ownerId": product, "namespace": "loyalty", "key": "tier", "value": "gold"}]}),
    );
    let metafield = &set["data"]["metafieldsSet"]["metafields"][0];
    assert_eq!(metafield["type"], "single_line_text_field", "{set}");
    assert_eq!(metafield["definition"]["id"], definition["id"]);

    // Errors of the input.
    let wrong = run(
        &mut shop,
        CREATE_DEFINITION,
        json!({"definition": {
            "name": "", "namespace": "loyalty", "key": "x", "ownerType": "PRODUCT", "type": "nope",
            "validations": [{"name": "regex", "value": "a"}],
        }}),
    );
    let codes: Vec<&Value> = wrong["data"]["metafieldDefinitionCreate"]["userErrors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| &error["code"])
        .collect();
    assert_eq!(
        codes,
        [
            &json!("BLANK"),
            &json!("TOO_SHORT"),
            &json!("INCLUSION"),
            &json!("INVALID_OPTION")
        ],
        "{wrong}"
    );
    assert!(
        topics(&mut shop)
            .iter()
            .all(|topic| topic != "metafield_definitions/create")
    );
}

#[test]
fn lists_updates_pins_and_deletes_definitions() {
    let mut shop = rich();
    let create = |shop: &mut ShopState, key: &str| {
        let answer = run(
            shop,
            CREATE_DEFINITION,
            json!({"definition": {"name": key, "namespace": "lsf_test", "key": key, "ownerType": "CUSTOMER", "type": "number_integer", "pin": true}}),
        );
        answer["data"]["metafieldDefinitionCreate"]["createdDefinition"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{answer}"))
            .to_string()
    };
    let first = create(&mut shop, "first");
    let second = create(&mut shop, "second");
    shop.take_events();
    let list = |shop: &mut ShopState, args: &str| {
        let answer = run(
            shop,
            &format!(
                "{{ metafieldDefinitions(first: 10, ownerType: CUSTOMER{args}) {{ nodes {{ key pinnedPosition }} }} }}"
            ),
            Value::Null,
        );
        answer["data"]["metafieldDefinitions"]["nodes"].clone()
    };
    assert_eq!(
        list(
            &mut shop,
            ", namespace: \"lsf_test\", sortKey: PINNED_POSITION"
        ),
        json!([{"key": "first", "pinnedPosition": 1}, {"key": "second", "pinnedPosition": 2}])
    );
    let unpinned = run(
        &mut shop,
        "mutation($id: ID!) { metafieldDefinitionUnpin(definitionId: $id) { unpinnedDefinition { pinnedPosition } userErrors { code } } }",
        json!({"id": first}),
    );
    assert_eq!(
        unpinned["data"]["metafieldDefinitionUnpin"]["unpinnedDefinition"]["pinnedPosition"],
        Value::Null
    );
    assert_eq!(
        list(&mut shop, ", namespace: \"lsf_test\", pinnedStatus: PINNED"),
        json!([{"key": "second", "pinnedPosition": 1}])
    );
    let again = run(
        &mut shop,
        "mutation($id: ID!) { metafieldDefinitionUnpin(definitionId: $id) { userErrors { code } } }",
        json!({"id": first}),
    );
    assert_eq!(
        again["data"]["metafieldDefinitionUnpin"]["userErrors"][0]["code"],
        "NOT_PINNED"
    );

    let updated = run(
        &mut shop,
        "mutation { metafieldDefinitionUpdate(definition: {ownerType: CUSTOMER, namespace: \"lsf_test\", key: \"first\", name: \"First!\", validations: [{name: \"max\", value: \"10\"}]}) {
            updatedDefinition { name validations { name value } } userErrors { code } } }",
        Value::Null,
    );
    assert_eq!(
        updated["data"]["metafieldDefinitionUpdate"]["updatedDefinition"]["name"], "First!",
        "{updated}"
    );
    let missing = run(
        &mut shop,
        "mutation { metafieldDefinitionUpdate(definition: {ownerType: CUSTOMER, namespace: \"lsf_test\", key: \"none\", name: \"x\"}) { userErrors { code } } }",
        Value::Null,
    );
    assert_eq!(
        missing["data"]["metafieldDefinitionUpdate"]["userErrors"][0]["code"],
        "NOT_FOUND"
    );

    let customer = shop.keys("Customer").next().unwrap().to_string();
    let set = run(
        &mut shop,
        SET,
        json!({"metafields": [{"ownerId": customer, "namespace": "lsf_test", "key": "second", "value": "5"}]}),
    );
    assert_eq!(
        set["data"]["metafieldsSet"]["userErrors"],
        json!([]),
        "{set}"
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { metafieldDefinitionDelete(id: $id, deleteAllAssociatedMetafields: true) {
            deletedDefinitionId deletedDefinition { ownerType namespace key } userErrors { code } } }",
        json!({"id": second}),
    );
    let deleted = &deleted["data"]["metafieldDefinitionDelete"];
    assert_eq!(deleted["deletedDefinitionId"], json!(second));
    assert_eq!(
        deleted["deletedDefinition"],
        json!({"ownerType": "CUSTOMER", "namespace": "lsf_test", "key": "second"})
    );
    assert!(shop.get(&second).is_none());
    let read = run(
        &mut shop,
        "query($id: ID!) { customer(id: $id) { metafield(namespace: \"lsf_test\", key: \"second\") { id } } }",
        json!({"id": customer}),
    );
    assert_eq!(read["data"]["customer"]["metafield"], Value::Null, "{read}");
    let topics = topics(&mut shop);
    assert!(topics.contains(&"metafield_definitions/update".to_string()));
    assert!(topics.contains(&"metafield_definitions/delete".to_string()));
}

#[test]
fn lists_the_metafield_definition_types() {
    let mut shop = rich();
    let answer = run(
        &mut shop,
        "{ metafieldDefinitionTypes { name category supportsDefinitionMigrations supportedValidations { name type } } }",
        Value::Null,
    );
    let types = answer["data"]["metafieldDefinitionTypes"]
        .as_array()
        .unwrap();
    assert_eq!(types.len(), super::types::TYPES.len());
    assert_eq!(
        types[2],
        json!({"name": "date_time", "category": "DATE_TIME", "supportsDefinitionMigrations": true,
            "supportedValidations": [{"name": "min", "type": "date_time"}, {"name": "max", "type": "date_time"}]})
    );
}

const CREATE_TYPE: &str = "mutation($definition: MetaobjectDefinitionCreateInput!) {
    metaobjectDefinitionCreate(definition: $definition) {
        metaobjectDefinition { id type name displayNameKey fieldDefinitions { key name required type { name } }
            capabilities { publishable { enabled } } access { admin storefront } }
        userErrors { field message code elementKey }
    }
}";

const CREATE_ENTRY: &str = "mutation($metaobject: MetaobjectCreateInput!) {
    metaobjectCreate(metaobject: $metaobject) {
        metaobject { id handle type displayName values capabilities { publishable { status } }
            fields { key type value } year: field(key: \"year\") { value jsonValue } }
        userErrors { field message code elementKey }
    }
}";

/// Creates the `$app:author` type: a required name, a year, a product.
fn author_type(shop: &mut ShopState) -> Value {
    let answer = run(
        shop,
        CREATE_TYPE,
        json!({"definition": {
            "type": "$app:author", "displayNameKey": "full_name",
            "capabilities": {"publishable": {"enabled": true}},
            "fieldDefinitions": [
                {"key": "full_name", "type": "single_line_text_field", "required": true},
                {"key": "year", "type": "number_integer", "validations": [{"name": "min", "value": "1000"}]},
                {"key": "book", "type": "product_reference"},
            ],
        }}),
    );
    let created = &answer["data"]["metaobjectDefinitionCreate"];
    assert_eq!(created["userErrors"], json!([]), "{answer}");
    created["metaobjectDefinition"].clone()
}

#[test]
fn creates_metaobject_definitions_and_entries() {
    let mut shop = rich();
    shop.take_events();
    let definition = author_type(&mut shop);
    let app_type = format!("app--{}--author", super::app_id(&shop));
    assert_eq!(definition["type"], json!(app_type));
    assert_eq!(definition["name"], "Author");
    assert_eq!(definition["fieldDefinitions"][0]["name"], "Full name");
    assert_eq!(definition["access"]["admin"], "MERCHANT_READ_WRITE");
    assert_eq!(definition["access"]["storefront"], "NONE");
    assert_eq!(topics(&mut shop), ["metaobject_definitions/create"]);

    let taken = run(
        &mut shop,
        CREATE_TYPE,
        json!({"definition": {"type": "$app:author"}}),
    );
    assert_eq!(
        taken["data"]["metaobjectDefinitionCreate"]["userErrors"][0]["code"],
        "TAKEN"
    );

    let product = find(&shop, "Product", "handle", "linen-dress");
    let answer = run(
        &mut shop,
        CREATE_ENTRY,
        json!({"metaobject": {"type": "$app:author", "fields": [
            {"key": "full_name", "value": "Jane Austen"},
            {"key": "year", "value": "1811"},
            {"key": "book", "value": product},
        ]}}),
    );
    let created = &answer["data"]["metaobjectCreate"];
    assert_eq!(created["userErrors"], json!([]), "{answer}");
    let entry = &created["metaobject"];
    assert_eq!(entry["handle"], "jane-austen", "made from the display name");
    assert_eq!(entry["displayName"], "Jane Austen");
    assert_eq!(entry["capabilities"]["publishable"]["status"], "DRAFT");
    assert_eq!(
        entry["values"],
        json!({"book": product, "full_name": "Jane Austen", "year": 1811})
    );
    assert_eq!(entry["year"], json!({"value": "1811", "jsonValue": 1811}));
    // The id lsf-core gives the metaobject of the store data.
    assert_eq!(
        entry["id"],
        json!(super::metaobject_gid(&app_type, "jane-austen"))
    );
    assert_eq!(topics(&mut shop), ["metaobjects/create"]);

    // A second one with the same display name gets another handle.
    let second = run(
        &mut shop,
        CREATE_ENTRY,
        json!({"metaobject": {"type": "$app:author", "values": {"full_name": "Jane Austen"},
            "capabilities": {"publishable": {"status": "ACTIVE"}}}}),
    );
    let second = &second["data"]["metaobjectCreate"]["metaobject"];
    assert_eq!(second["handle"], "jane-austen-1");
    assert_eq!(second["capabilities"]["publishable"]["status"], "ACTIVE");

    let listed = run(
        &mut shop,
        "{ metaobjects(type: \"$app:author\", first: 5, sortKey: \"display_name\", query: \"fields.year:1811\") { nodes { handle } }
           metaobjectByHandle(handle: {type: \"$app:author\", handle: \"jane-austen-1\"}) { displayName }
           metaobjectDefinitionByType(type: \"$app:author\") { metaobjectsCount } }",
        Value::Null,
    );
    assert_eq!(
        listed["data"]["metaobjects"]["nodes"],
        json!([{"handle": "jane-austen"}]),
        "{listed}"
    );
    assert_eq!(
        listed["data"]["metaobjectByHandle"]["displayName"],
        "Jane Austen"
    );
    assert_eq!(
        listed["data"]["metaobjectDefinitionByType"]["metaobjectsCount"],
        json!(2)
    );
}

#[test]
fn refuses_wrong_metaobjects() {
    let mut shop = rich();
    author_type(&mut shop);
    shop.take_events();
    let missing_type = run(
        &mut shop,
        CREATE_ENTRY,
        json!({"metaobject": {"type": "nothing", "fields": []}}),
    );
    assert_eq!(
        missing_type["data"]["metaobjectCreate"]["userErrors"],
        json!([{"field": ["metaobject", "type"], "message": "No metaobject definition exists for type \"nothing\"", "code": "UNDEFINED_OBJECT_TYPE", "elementKey": null}])
    );
    let wrong = run(
        &mut shop,
        CREATE_ENTRY,
        json!({"metaobject": {"type": "$app:author", "fields": [
            {"key": "year", "value": "999"},
            {"key": "colour", "value": "red"},
        ]}}),
    );
    let errors = wrong["data"]["metaobjectCreate"]["userErrors"]
        .as_array()
        .unwrap();
    let codes: Vec<(&Value, &Value)> = errors
        .iter()
        .map(|error| (&error["code"], &error["elementKey"]))
        .collect();
    assert_eq!(
        codes,
        [
            (&json!("UNDEFINED_OBJECT_FIELD"), &json!("colour")),
            (&json!("OBJECT_FIELD_REQUIRED"), &json!("full_name")),
            (&json!("INVALID_VALUE"), &json!("year")),
        ],
        "{wrong}"
    );
    assert!(topics(&mut shop).is_empty());
    assert_eq!(shop.count("Metaobject"), rich().count("Metaobject"));
}

#[test]
fn updates_upserts_and_deletes_metaobjects() {
    let mut shop = rich();
    author_type(&mut shop);
    let created = run(
        &mut shop,
        CREATE_ENTRY,
        json!({"metaobject": {"type": "$app:author", "handle": "austen", "fields": [
            {"key": "full_name", "value": "Jane Austen"}, {"key": "year", "value": "1811"}]}}),
    );
    let id = created["data"]["metaobjectCreate"]["metaobject"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { metaobjectUpdate(id: $id, metaobject: {fields: [{key: \"full_name\", value: \"J. Austen\"}]}) {
            metaobject { displayName values } userErrors { code } } }",
        json!({"id": id}),
    );
    let entry = &updated["data"]["metaobjectUpdate"]["metaobject"];
    assert_eq!(entry["displayName"], "J. Austen", "{updated}");
    assert_eq!(entry["values"]["year"], json!(1811), "fields patch");
    assert_eq!(topics(&mut shop), ["metaobjects/update"]);

    // `values` replaces: the year is cleared.
    let upserted = run(
        &mut shop,
        "mutation { metaobjectUpsert(handle: {type: \"$app:author\", handle: \"austen\"}, values: {full_name: \"Jane\"}) {
            metaobject { id values } userErrors { code } } }",
        Value::Null,
    );
    let entry = &upserted["data"]["metaobjectUpsert"]["metaobject"];
    assert_eq!(entry["id"], json!(id), "{upserted}");
    assert_eq!(entry["values"]["year"], Value::Null);
    let new = run(
        &mut shop,
        "mutation { metaobjectUpsert(handle: {type: \"$app:author\", handle: \"bronte\"}, metaobject: {fields: [{key: \"full_name\", value: \"Charlotte Brontë\"}]}) {
            metaobject { id handle } userErrors { code } } }",
        Value::Null,
    );
    assert_eq!(
        new["data"]["metaobjectUpsert"]["metaobject"]["handle"],
        "bronte"
    );
    assert_eq!(
        topics(&mut shop),
        ["metaobjects/update", "metaobjects/create"]
    );

    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { metaobjectDelete(id: $id) { deletedId userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(deleted["data"]["metaobjectDelete"]["deletedId"], json!(id));
    assert!(shop.get(&id).is_none());
    let bulk = run(
        &mut shop,
        "mutation { metaobjectBulkDelete(where: {type: \"$app:author\"}) { job { done } userErrors { code } } }",
        Value::Null,
    );
    assert_eq!(
        bulk["data"]["metaobjectBulkDelete"]["job"]["done"],
        json!(true)
    );
    assert_eq!(
        topics(&mut shop),
        ["metaobjects/delete", "metaobjects/delete"]
    );
    let missing = run(
        &mut shop,
        "mutation($id: ID!) { metaobjectDelete(id: $id) { deletedId userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        missing["data"]["metaobjectDelete"]["userErrors"][0]["code"],
        "RECORD_NOT_FOUND"
    );
}

#[test]
fn updates_and_deletes_metaobject_definitions() {
    let mut shop = rich();
    let definition = author_type(&mut shop);
    let id = definition["id"].as_str().unwrap().to_string();
    let created = run(
        &mut shop,
        CREATE_ENTRY,
        json!({"metaobject": {"type": "$app:author", "fields": [{"key": "full_name", "value": "Ann"}, {"key": "year", "value": "1900"}]}}),
    );
    assert_eq!(created["data"]["metaobjectCreate"]["userErrors"], json!([]));
    shop.take_events();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { metaobjectDefinitionUpdate(id: $id, definition: {name: \"Writer\", fieldDefinitions: [
            {create: {key: \"country\", type: \"single_line_text_field\"}}, {delete: {key: \"year\"}}]}) {
            metaobjectDefinition { name fieldDefinitions { key } } userErrors { code elementKey } } }",
        json!({"id": id}),
    );
    let definition = &updated["data"]["metaobjectDefinitionUpdate"]["metaobjectDefinition"];
    assert_eq!(definition["name"], "Writer", "{updated}");
    assert_eq!(
        definition["fieldDefinitions"],
        json!([{"key": "full_name"}, {"key": "book"}, {"key": "country"}])
    );
    let entries = run(
        &mut shop,
        "{ metaobjects(type: \"$app:author\", first: 1) { nodes { values } } }",
        Value::Null,
    );
    assert_eq!(
        entries["data"]["metaobjects"]["nodes"][0]["values"],
        json!({"book": null, "country": null, "full_name": "Ann"})
    );
    assert_eq!(
        topics(&mut shop),
        ["metaobjects/update", "metaobject_definitions/update"]
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { metaobjectDefinitionDelete(id: $id) { deletedId userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        deleted["data"]["metaobjectDefinitionDelete"]["deletedId"],
        json!(id)
    );
    assert_eq!(
        topics(&mut shop),
        ["metaobjects/delete", "metaobject_definitions/delete"]
    );
}

#[test]
fn creates_updates_and_deletes_pages() {
    let mut shop = rich();
    shop.take_events();
    let created = run(
        &mut shop,
        "mutation { pageCreate(page: {title: \"About us\", body: \"<p>Hello</p>\",
            metafields: [{namespace: \"custom\", key: \"tagline\", type: \"single_line_text_field\", value: \"Hi\"}]}) {
            page { id title handle body isPublished publishedAt templateSuffix tagline: metafield(namespace: \"custom\", key: \"tagline\") { value } }
            userErrors { field message code } } }",
        Value::Null,
    );
    let page = &created["data"]["pageCreate"]["page"];
    assert_eq!(
        created["data"]["pageCreate"]["userErrors"],
        json!([]),
        "{created}"
    );
    assert_eq!(page["handle"], "about-us");
    assert_eq!(page["isPublished"], json!(true));
    assert_eq!(page["publishedAt"], json!(shop.timestamp()));
    assert_eq!(page["templateSuffix"], Value::Null);
    assert_eq!(page["tagline"]["value"], "Hi");
    assert_eq!(topics(&mut shop), ["pages/create"]);
    let id = page["id"].as_str().unwrap().to_string();

    let taken = run(
        &mut shop,
        "mutation { pageCreate(page: {title: \"x\", handle: \"about-us\"}) { page { id } userErrors { field message code } } }",
        Value::Null,
    );
    assert_eq!(
        taken["data"]["pageCreate"]["userErrors"],
        json!([{"field": ["page", "handle"], "message": "Handle has already been taken", "code": "TAKEN"}])
    );
    let blank = run(
        &mut shop,
        "mutation { pageCreate(page: {title: \" \", isPublished: true, publishDate: \"2999-01-01T00:00:00Z\"}) { userErrors { field code } } }",
        Value::Null,
    );
    assert_eq!(
        blank["data"]["pageCreate"]["userErrors"],
        json!([{"field": ["page", "title"], "code": "BLANK"}, {"field": ["page", "publishDate"], "code": "INVALID_PUBLISH_DATE"}])
    );
    // A metafield check's code the page's enum lacks is its INVALID.
    let short = run(
        &mut shop,
        "mutation { pageCreate(page: {title: \"x\", metafields: [{namespace: \"ab\", key: \"cd\", type: \"boolean\", value: \"true\"}]}) { userErrors { field code } } }",
        Value::Null,
    );
    assert_eq!(
        short["data"]["pageCreate"]["userErrors"],
        json!([{"field": ["page", "metafields", "0", "namespace"], "code": "INVALID"}])
    );

    let updated = run(
        &mut shop,
        "mutation($id: ID!) { pageUpdate(id: $id, page: {handle: \"about-the-shop\", isPublished: false, redirectNewHandle: true}) {
            page { handle isPublished publishedAt } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["pageUpdate"]["page"],
        json!({"handle": "about-the-shop", "isPublished": false, "publishedAt": null}),
        "{updated}"
    );
    let redirects = run(
        &mut shop,
        "{ urlRedirects(first: 5) { nodes { path target } } }",
        Value::Null,
    );
    assert_eq!(
        redirects["data"]["urlRedirects"]["nodes"],
        json!([{"path": "/pages/about-us", "target": "/pages/about-the-shop"}])
    );
    assert_eq!(topics(&mut shop), ["url_redirects/create", "pages/update"]);

    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { pageDelete(id: $id) { deletedPageId userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(deleted["data"]["pageDelete"]["deletedPageId"], json!(id));
    assert!(shop.get(&id).is_none());
    assert_eq!(
        shop.count("Metafield"),
        rich().count("Metafield"),
        "its metafields are gone"
    );
    let missing = run(
        &mut shop,
        "mutation($id: ID!) { pageDelete(id: $id) { deletedPageId userErrors { field code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        missing["data"]["pageDelete"]["userErrors"],
        json!([{"field": ["id"], "code": "NOT_FOUND"}])
    );
}

#[test]
fn creates_articles_in_blogs_and_moderates_comments() {
    let mut shop = rich();
    shop.take_events();
    let created = run(
        &mut shop,
        "mutation { articleCreate(blog: {title: \"Dispatches\"}, article: {title: \"Spring\", author: {name: \"Ann\"},
            body: \"<p>New</p>\", tags: [\"linen, summer\"], image: {url: \"https://cdn.example/a.jpg\", altText: \"A\"}}) {
            article { id handle author { name } tags image { url altText } blog { title handle tags commentPolicy } }
            userErrors { field message code } } }",
        Value::Null,
    );
    let article = &created["data"]["articleCreate"]["article"];
    assert_eq!(
        created["data"]["articleCreate"]["userErrors"],
        json!([]),
        "{created}"
    );
    assert_eq!(article["handle"], "spring");
    assert_eq!(article["author"]["name"], "Ann");
    assert_eq!(article["tags"], json!(["linen", "summer"]));
    assert_eq!(article["image"]["altText"], "A");
    assert_eq!(
        article["blog"],
        json!({"title": "Dispatches", "handle": "dispatches", "tags": ["linen", "summer"], "commentPolicy": "CLOSED"})
    );
    assert_eq!(topics(&mut shop), ["blogs/create", "articles/create"]);
    let blog = find(&shop, "Blog", "handle", "dispatches");

    let errors = run(
        &mut shop,
        "mutation($blog: ID!) { articleCreate(article: {title: \"Spring\", blogId: $blog, author: {}}) { userErrors { field code } }
            again: articleCreate(article: {title: \"x\", author: {name: \"A\"}}) { userErrors { field code } }
            both: articleCreate(blog: {title: \"B\"}, article: {title: \"x\", blogId: $blog, author: {name: \"A\"}}) { userErrors { code } } }",
        json!({"blog": blog}),
    );
    assert_eq!(
        errors["data"]["articleCreate"]["userErrors"],
        json!([{"field": ["article", "author"], "code": "AUTHOR_FIELD_REQUIRED"}])
    );
    assert_eq!(
        errors["data"]["again"]["userErrors"],
        json!([{"field": ["article"], "code": "BLOG_REFERENCE_REQUIRED"}])
    );
    assert_eq!(
        errors["data"]["both"]["userErrors"][0]["code"],
        "AMBIGUOUS_BLOG"
    );

    let id = article["id"].as_str().unwrap().to_string();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { articleUpdate(id: $id, article: {title: \"Summer\", tags: [\"sun\"]}) {
            article { title tags blog { tags } } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["articleUpdate"]["article"],
        json!({"title": "Summer", "tags": ["sun"], "blog": {"tags": ["sun"]}}),
        "{updated}"
    );

    // The rich fixture's comments.
    let comment = shop.keys("Comment").next().unwrap().to_string();
    let spam = run(
        &mut shop,
        "mutation($id: ID!) { commentSpam(id: $id) { comment { status isPublished } userErrors { code } } }",
        json!({"id": comment}),
    );
    assert_eq!(
        spam["data"]["commentSpam"]["comment"],
        json!({"status": "SPAM", "isPublished": false})
    );
    let approved = run(
        &mut shop,
        "mutation($id: ID!) { commentApprove(id: $id) { comment { status isPublished } } }",
        json!({"id": comment}),
    );
    assert_eq!(
        approved["data"]["commentApprove"]["comment"],
        json!({"status": "PUBLISHED", "isPublished": true})
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { commentDelete(id: $id) { deletedCommentId } }",
        json!({"id": comment}),
    );
    assert_eq!(
        deleted["data"]["commentDelete"]["deletedCommentId"],
        json!(comment)
    );

    shop.take_events();
    let gone = run(
        &mut shop,
        "mutation($id: ID!) { blogDelete(id: $id) { deletedBlogId userErrors { code } } }",
        json!({"id": blog}),
    );
    assert_eq!(gone["data"]["blogDelete"]["deletedBlogId"], json!(blog));
    assert!(shop.get(&id).is_none(), "its articles are gone");
    assert_eq!(topics(&mut shop), ["articles/delete", "blogs/delete"]);
}

#[test]
fn creates_menus_and_redirects() {
    let mut shop = rich();
    shop.take_events();
    let page = shop.keys("Page").next().unwrap().to_string();
    let page_handle = shop.get(&page).unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    let created = run(
        &mut shop,
        "mutation($page: ID!) { menuCreate(title: \"Sidebar\", handle: \"sidebar\", items: [
            {title: \"Products\", type: CATALOG, items: [{title: \"About\", type: PAGE, resourceId: $page}]},
            {title: \"Shopify\", type: HTTP, url: \"https://shopify.com\"}]) {
            menu { id handle isDefault items { title type url items { title url resourceId } } } userErrors { field message code } } }",
        json!({"page": page}),
    );
    let menu = &created["data"]["menuCreate"]["menu"];
    assert_eq!(
        created["data"]["menuCreate"]["userErrors"],
        json!([]),
        "{created}"
    );
    assert_eq!(menu["id"], json!(super::menu_gid("sidebar")));
    assert_eq!(menu["items"][0]["url"], "/collections/all");
    assert_eq!(
        menu["items"][0]["items"][0],
        json!({"title": "About", "url": format!("/pages/{page_handle}"), "resourceId": page})
    );
    assert_eq!(topics(&mut shop), ["menus/create"]);

    let deep = run(
        &mut shop,
        "mutation { menuCreate(title: \"Deep\", handle: \"deep\", items: [{title: \"a\", type: FRONTPAGE, items: [{title: \"b\", type: FRONTPAGE, items: [{title: \"c\", type: FRONTPAGE, items: [{title: \"d\", type: FRONTPAGE}]}]}]}]) { userErrors { field code } } }",
        Value::Null,
    );
    assert_eq!(
        deep["data"]["menuCreate"]["userErrors"],
        json!([{"field": ["items", "0", "items", "0", "items", "0", "items", "0"], "code": "NESTING_TOO_DEEP"}])
    );
    let main = find(&shop, "Menu", "handle", "main-menu");
    let default = run(
        &mut shop,
        "mutation($id: ID!) { menuDelete(id: $id) { deletedMenuId userErrors { code } } }",
        json!({"id": main}),
    );
    assert_eq!(
        default["data"]["menuDelete"]["userErrors"][0]["code"],
        "UNABLE_TO_DELETE_DEFAULT_MENU"
    );
    let id = menu["id"].as_str().unwrap().to_string();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { menuUpdate(id: $id, title: \"Side\", handle: \"side\", items: [{title: \"Home\", type: FRONTPAGE}]) {
            menu { title handle items { title url } } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["menuUpdate"]["menu"],
        json!({"title": "Side", "handle": "side", "items": [{"title": "Home", "url": "/"}]})
    );

    let redirect = run(
        &mut shop,
        "mutation { urlRedirectCreate(urlRedirect: {path: \"old\", target: \"/new\"}) { urlRedirect { id path target } userErrors { field message code } } }",
        Value::Null,
    );
    let redirect = &redirect["data"]["urlRedirectCreate"];
    assert_eq!(redirect["urlRedirect"]["path"], "/old", "{redirect}");
    let again = run(
        &mut shop,
        "mutation { urlRedirectCreate(urlRedirect: {path: \"/old\", target: \"/other\"}) { userErrors { field message code } } }",
        Value::Null,
    );
    assert_eq!(
        again["data"]["urlRedirectCreate"]["userErrors"],
        json!([{"field": ["urlRedirect", "path"], "message": "Path has already been taken", "code": "CREATE_FAILED"}])
    );
    let redirect_id = redirect["urlRedirect"]["id"].as_str().unwrap().to_string();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { urlRedirectUpdate(id: $id, urlRedirect: {target: \"https://example.com\"}) { urlRedirect { path target } } }",
        json!({"id": redirect_id}),
    );
    assert_eq!(
        updated["data"]["urlRedirectUpdate"]["urlRedirect"],
        json!({"path": "/old", "target": "https://example.com"})
    );
    let searched = run(
        &mut shop,
        "{ urlRedirects(first: 5, query: \"old\") { nodes { path } } urlRedirectsCount { count } }",
        Value::Null,
    );
    assert_eq!(
        searched["data"]["urlRedirects"]["nodes"],
        json!([{"path": "/old"}])
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { urlRedirectDelete(id: $id) { deletedUrlRedirectId } menuDelete(id: \"gid://shopify/Menu/1\") { userErrors { code } } }",
        json!({"id": redirect_id}),
    );
    assert_eq!(
        deleted["data"]["urlRedirectDelete"]["deletedUrlRedirectId"],
        json!(redirect_id)
    );
    assert_eq!(
        deleted["data"]["menuDelete"]["userErrors"][0]["code"],
        "MENU_DOES_NOT_EXIST"
    );
}

/// What the content mutations create is in the store the storefront and the checkout read, as
/// store sync projects it: a page, a menu, an active metaobject, a product's metafields.
#[test]
fn what_the_content_mutations_create_syncs_to_the_store() {
    use lsf_core::store::load::{DataSource, load, load_with_overlays};
    let source = DataSource::Directory(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich"),
    );
    let options = Default::default();
    let (base, _) = load(&source, &options);
    let mut shop = ShopState::seeded(&base, None);
    let product = find(&shop, "Product", "handle", "linen-dress");
    let answer = run(
        &mut shop,
        "mutation($product: ID!) {
            page: pageCreate(page: {title: \"Shipping\", body: \"<p>Fast</p>\"}) { userErrors { message } }
            menu: menuCreate(title: \"Legal\", handle: \"legal\", items: [{title: \"Search\", type: SEARCH}]) { userErrors { message } }
            type: metaobjectDefinitionCreate(definition: {type: \"badge\", fieldDefinitions: [{key: \"label\", type: \"single_line_text_field\"}]}) { userErrors { message } }
            entry: metaobjectCreate(metaobject: {type: \"badge\", handle: \"eco\", fields: [{key: \"label\", value: \"Eco\"}]}) { userErrors { message } }
            set: metafieldsSet(metafields: [
                {ownerId: $product, namespace: \"custom\", key: \"badge\", type: \"metaobject_reference\", value: \"gid://shopify/Metaobject/0\"}
            ]) { userErrors { message } }
        }",
        json!({"product": product}),
    );
    // The reference names a metaobject that does not exist: refused, nothing else changed by it.
    assert_eq!(
        answer["data"]["set"]["userErrors"][0]["message"],
        "Value references non-existent resource gid://shopify/Metaobject/0."
    );
    let entry = super::metaobject_gid("badge", "eco");
    let set = run(
        &mut shop,
        "mutation($product: ID!, $entry: String!) { metafieldsSet(metafields: [
            {ownerId: $product, namespace: \"custom\", key: \"badge\", type: \"metaobject_reference\", value: $entry},
            {ownerId: $product, namespace: \"custom\", key: \"weight_kg\", type: \"number_decimal\", value: \"1.25\"}
        ]) { userErrors { message } } }",
        json!({"product": product, "entry": entry}),
    );
    assert_eq!(
        set["data"]["metafieldsSet"]["userErrors"],
        json!([]),
        "{set}"
    );
    let overlay = crate::sync::overlay(&shop, &base).expect("the shop changed");
    let (store, diagnostics) = load_with_overlays(&source, &options, &[overlay]);
    assert!(!diagnostics.has_errors(), "{diagnostics:?}");
    assert!(store.page_index("shipping").is_some(), "the page");
    assert!(store.menu("legal").is_some(), "the menu");
    // The type is not publishable: the entry has no status, and is on the storefront.
    let badge = store.metaobject("badge", "eco").expect("the metaobject");
    assert_eq!(badge.fields["label"].value, json!("Eco"));
    let linen = store.product_by_handle("linen-dress").unwrap();
    assert_eq!(
        linen.metafields["custom"]["badge"].value,
        json!("badge/eco")
    );
    assert_eq!(linen.metafields["custom"]["weight_kg"].value, json!("1.25"));
}

const FILE_CREATE: &str = "mutation($files: [FileCreateInput!]!) { fileCreate(files: $files) {
    files { id alt fileStatus __typename
        ... on MediaImage { mimeType image { url altText } }
        ... on GenericFile { url mimeType originalFileSize }
        ... on Video { filename } }
    userErrors { field message code } } }";

#[test]
fn creates_updates_and_deletes_files() {
    let mut shop = rich();
    shop.take_events();
    super::files::record_staged_upload(
        "https://127.0.0.1:9292/__lsf/staged/42/manual.pdf",
        super::files::StagedUpload {
            filename: "manual.pdf".into(),
            mime_type: "application/pdf".into(),
            size: 2048,
        },
    );
    let created = run(
        &mut shop,
        FILE_CREATE,
        json!({"files": [
            {"originalSource": "https://cdn.example/photos/zebra.jpg?v=3", "alt": "Hero"},
            {"originalSource": "https://127.0.0.1:9292/__lsf/staged/42/manual.pdf"},
            {"originalSource": "https://cdn.example/clip.mp4", "contentType": "VIDEO", "filename": "launch.mp4"},
        ]}),
    );
    let files = &created["data"]["fileCreate"]["files"];
    assert_eq!(
        created["data"]["fileCreate"]["userErrors"],
        json!([]),
        "{created}"
    );
    assert_eq!(files[0]["__typename"], "MediaImage");
    assert_eq!(files[0]["fileStatus"], "READY");
    assert_eq!(
        files[0]["image"],
        json!({"url": "https://cdn.example/photos/zebra.jpg?v=3", "altText": "Hero"})
    );
    assert_eq!(files[1]["__typename"], "GenericFile");
    assert_eq!(files[1]["originalFileSize"], json!(2048));
    assert_eq!(files[1]["mimeType"], "application/pdf");
    assert_eq!(files[2]["filename"], "launch.mp4");
    assert_eq!(
        topics(&mut shop),
        ["files/create", "files/create", "files/create"]
    );

    // The same name: refused, or a UUID appended.
    let duplicate = run(
        &mut shop,
        FILE_CREATE,
        json!({"files": [{"originalSource": "https://other.example/zebra.jpg", "duplicateResolutionMode": "RAISE_ERROR"}]}),
    );
    assert_eq!(
        duplicate["data"]["fileCreate"]["userErrors"][0]["code"],
        "FILENAME_ALREADY_EXISTS"
    );
    let mismatched = run(
        &mut shop,
        FILE_CREATE,
        json!({"files": [{"originalSource": "https://other.example/zebra.jpg", "filename": "zebra.png"}, {"originalSource": "nope"}]}),
    );
    let codes: Vec<&Value> = mismatched["data"]["fileCreate"]["userErrors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| &error["code"])
        .collect();
    assert_eq!(
        codes,
        [
            &json!("MISMATCHED_FILENAME_AND_ORIGINAL_SOURCE"),
            &json!("INVALID_IMAGE_SOURCE_URL")
        ]
    );

    let image = files[0]["id"].as_str().unwrap().to_string();
    let product = find(&shop, "Product", "handle", "linen-dress");
    let updated = run(
        &mut shop,
        "mutation($files: [FileUpdateInput!]!) { fileUpdate(files: $files) { files { alt } userErrors { field code } } }",
        json!({"files": [{"id": image, "alt": "Front", "referencesToAdd": [product]}]}),
    );
    assert_eq!(
        updated["data"]["fileUpdate"]["files"],
        json!([{"alt": "Front"}]),
        "{updated}"
    );
    let media = shop.get(&product).unwrap()["media"].clone();
    assert!(media.as_array().unwrap().contains(&json!(image)));
    assert_eq!(topics(&mut shop), ["files/update", "products/update"]);

    let searched = run(
        &mut shop,
        "{ files(first: 5, query: \"zebra\") { nodes { id } } }",
        Value::Null,
    );
    assert_eq!(
        searched["data"]["files"]["nodes"],
        json!([{"id": image}]),
        "{searched}"
    );

    let deleted = run(
        &mut shop,
        "mutation($ids: [ID!]!) { fileDelete(fileIds: $ids) { deletedFileIds userErrors { field code } } }",
        json!({"ids": [image]}),
    );
    assert_eq!(
        deleted["data"]["fileDelete"]["deletedFileIds"],
        json!([image])
    );
    assert!(
        !shop.get(&product).unwrap()["media"]
            .as_array()
            .unwrap()
            .contains(&json!(image))
    );
    assert_eq!(topics(&mut shop), ["products/update", "files/delete"]);
    let missing = run(
        &mut shop,
        "mutation($ids: [ID!]!) { fileDelete(fileIds: $ids) { deletedFileIds userErrors { field code } } }",
        json!({"ids": [image]}),
    );
    assert_eq!(
        missing["data"]["fileDelete"]["userErrors"],
        json!([{"field": ["fileIds", "0"], "code": "FILE_DOES_NOT_EXIST"}])
    );
}

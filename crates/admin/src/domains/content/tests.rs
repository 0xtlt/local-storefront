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

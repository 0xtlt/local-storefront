//! Collections: `collectionCreate`, `collectionUpdate`, `collectionDelete`,
//! `collectionAddProducts`, `collectionAddProductsV2`, `collectionRemoveProducts`,
//! `collectionReorderProducts`, and the membership of smart collections.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A collection is created unpublished ("The created collection is unpublished by default. To
//!   make it available to customers, use the `publishablePublish` mutation"). With `products`
//!   it is a custom (manual) collection of them, sorted `MANUAL` (the reference's example of a
//!   collection made of sources); with a `ruleSet` it is a smart collection, sorted
//!   `MOST_RELEVANT` (the example "Create a smart collection"), whose products are those its
//!   rules match. A handle not given is made from the title, with a suffix when it is taken.
//! - Smart collections keep their membership as products change: each product a mutation
//!   changes is matched against every rule set again (`TAG`, `TITLE`, `TYPE`, `VENDOR`,
//!   `VARIANT_TITLE`, `VARIANT_PRICE`, `VARIANT_COMPARE_AT_PRICE`, `VARIANT_WEIGHT`,
//!   `VARIANT_INVENTORY`, `IS_PRICE_REDUCED`; all rules, or any with `appliedDisjunctively`).
//!   Text compares ignoring case; a variant rule matches a product when one of its variants
//!   matches. The category and metafield columns are not simulated: such a rule matches nothing,
//!   and says so.
//! - `collectionUpdate` refuses a `ruleSet` for a custom collection ("Cannot update rule set of a
//!   custom collection", the reference's example) and answers a `Job` when the membership is
//!   recomputed; lsf's job is already done.
//! - `collectionAddProducts` (synchronous), `collectionAddProductsV2`, `collectionRemoveProducts`,
//!   `collectionReorderProducts` (asynchronous on Shopify: lsf's job is already done) work on
//!   custom collections only ("Can't manually remove products from a smart collection",
//!   "Can't reorder products unless collection is manually sorted", the reference's examples).
//! - The collection model of 2026-10 (`collection.sources`, `sourcesToCreate`,
//!   `sourcesToUpdate`, `sourcesToDelete`): a collection made of sources is the union of their
//!   members ([`super::collection_sources`]), kept as a smart collection's are; a collection
//!   created with `collection` and no sources is a custom collection.
//! - Events: `collections/create`, `collections/update` (also when its products change),
//!   `collections/delete`.
//!
//! The store sync writes a smart collection that changed as a manual one with the products it
//! has (the store data has no rules), and does not sync a collection's publication.

use serde_json::{Map, Value, json};

use super::collection_sources as sources;
use super::records::{self, arg_objects, field, is, link_collection, objects, origin, text};
use super::{cents_of, gids};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, coded, done_job, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("collectionCreate", collection_create)
        .mutation("collectionUpdate", collection_update)
        .mutation("collectionDelete", collection_delete)
        .mutation("collectionAddProducts", add_products)
        .mutation("collectionAddProductsV2", add_products_v2)
        .mutation("collectionRemoveProducts", remove_products)
        .mutation("collectionReorderProducts", reorder_products)
        .field("Collection", "hasProduct", has_product);
}

/// `Collection.hasProduct(id:)`.
fn has_product(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let has = r
        .items_of(object, "products")
        .iter()
        .any(|product| product.as_str() == Some(id));
    Ok(Output::value(has))
}

/// Whether a collection is a smart one (it has a rule set).
pub fn is_smart(record: &Record) -> bool {
    record.get("ruleSet").is_some_and(|rules| !rules.is_null())
        || super::collection_sources::has_sources(record)
}

/// Whether a product is a member of a smart collection: of its sources (the 2026-10 model), else
/// of its rule set.
fn computed_member(state: &ShopState, collection: &Record, product: &Record) -> bool {
    if super::collection_sources::has_sources(collection) {
        return super::collection_sources::member(state, collection, product);
    }
    collection
        .get("ruleSet")
        .is_some_and(|rule_set| matches(state, product, rule_set))
}

/// The columns lsf evaluates.
const COLUMNS: &[&str] = &[
    "TAG",
    "TITLE",
    "TYPE",
    "VENDOR",
    "VARIANT_TITLE",
    "VARIANT_PRICE",
    "VARIANT_COMPARE_AT_PRICE",
    "VARIANT_WEIGHT",
    "VARIANT_INVENTORY",
    "IS_PRICE_REDUCED",
];

/// Whether a text matches a rule's relation and condition, ignoring case.
fn text_matches(value: &str, relation: &str, condition: &str) -> bool {
    let (value, condition) = (value.to_lowercase(), condition.to_lowercase());
    match relation {
        "EQUALS" => value == condition,
        "NOT_EQUALS" => value != condition,
        "STARTS_WITH" => value.starts_with(&condition),
        "ENDS_WITH" => value.ends_with(&condition),
        "CONTAINS" => value.contains(&condition),
        "NOT_CONTAINS" => !value.contains(&condition),
        _ => false,
    }
}

/// Whether a number matches a rule's relation and condition.
fn number_matches(value: Option<f64>, relation: &str, condition: f64) -> bool {
    match (relation, value) {
        ("IS_SET", value) => value.is_some(),
        ("IS_NOT_SET", value) => value.is_none(),
        (_, None) => false,
        ("EQUALS", Some(value)) => (value - condition).abs() < 1e-9,
        ("NOT_EQUALS", Some(value)) => (value - condition).abs() >= 1e-9,
        ("GREATER_THAN", Some(value)) => value > condition,
        ("LESS_THAN", Some(value)) => value < condition,
        _ => false,
    }
}

/// A weight in another unit.
pub(super) fn weight_in(value: f64, from: &str, to: &str) -> f64 {
    let grams = |unit: &str| match unit {
        "KILOGRAMS" => 1000.0,
        "POUNDS" => 453.592_37,
        "OUNCES" => 28.349_523_125,
        _ => 1.0,
    };
    value * grams(from) / grams(to)
}

/// Whether a product matches one rule.
fn rule_matches(state: &ShopState, product: &Record, rule: &Value) -> bool {
    let column = rule
        .get("column")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let relation = rule
        .get("relation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let condition = rule
        .get("condition")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let variants: Vec<&Record> = gids(product.get("variants"))
        .iter()
        .filter_map(|variant| state.get(variant))
        .collect();
    let number = condition.trim().parse::<f64>().ok();
    let any_variant = |read: &dyn Fn(&Record) -> Option<f64>| {
        variants
            .iter()
            .any(|variant| number.is_some_and(|n| number_matches(read(variant), relation, n)))
    };
    match column {
        "TAG" => {
            let tags: Vec<&str> = product
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            match relation {
                "NOT_EQUALS" => !tags.iter().any(|tag| tag.eq_ignore_ascii_case(condition)),
                _ => tags
                    .iter()
                    .any(|tag| text_matches(tag, relation, condition)),
            }
        }
        "TITLE" | "TYPE" | "VENDOR" => {
            let name = match column {
                "TITLE" => "title",
                "TYPE" => "productType",
                _ => "vendor",
            };
            text_matches(
                field(product, name).unwrap_or_default(),
                relation,
                condition,
            )
        }
        "VARIANT_TITLE" => variants.iter().any(|variant| {
            text_matches(
                field(variant, "title").unwrap_or_default(),
                relation,
                condition,
            )
        }),
        "VARIANT_PRICE" => {
            any_variant(&|variant| cents_of(variant.get("price")).map(|c| c as f64 / 100.0))
        }
        "VARIANT_COMPARE_AT_PRICE" => any_variant(&|variant| {
            cents_of(variant.get("compareAtPrice")).map(|c| c as f64 / 100.0)
        }),
        "VARIANT_INVENTORY" => any_variant(&|variant| {
            variant
                .get("inventoryQuantity")
                .and_then(Value::as_i64)
                .map(|q| q as f64)
        }),
        "VARIANT_WEIGHT" => {
            let shop_unit = records::weight_unit(state);
            any_variant(&|variant| sources::weight_of(state, variant, Some(&shop_unit)))
        }
        // "With `is_set` relation, the rule matches products with at least one variant with
        // `compare_at_price` set. With `is_not_set` relation, the rule matches matches products
        // with at least one variant with `compare_at_price` not set." (the schema).
        "IS_PRICE_REDUCED" => {
            let set = |variant: &&Record| cents_of(variant.get("compareAtPrice")).is_some();
            match relation {
                "IS_SET" => variants.iter().any(set),
                "IS_NOT_SET" => variants.iter().any(|variant| !set(variant)),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Whether a product matches a rule set.
fn matches(state: &ShopState, product: &Record, rule_set: &Value) -> bool {
    let rules: Vec<&Value> = rule_set
        .get("rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    if rules.is_empty() {
        return false;
    }
    let disjunctive = rule_set
        .get("appliedDisjunctively")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if disjunctive {
        rules.iter().any(|rule| rule_matches(state, product, rule))
    } else {
        rules.iter().all(|rule| rule_matches(state, product, rule))
    }
}

/// Puts a product in the smart collections whose rules it matches, and out of those it no
/// longer matches. What a mutation that changed a product runs (through
/// [`super::product_changed`]).
pub fn apply_rules(state: &mut ShopState, product: &str) {
    let Some(record) = state.get(product).cloned() else {
        return;
    };
    // The collections whose sources read other collections follow them: they come second.
    for following in [false, true] {
        let smart: Vec<(String, bool)> = state
            .keys("Collection")
            .filter_map(|collection| {
                let collection_record = state.get(collection)?;
                (is_smart(collection_record)
                    && super::collection_sources::reads_collections(state, collection_record)
                        == following)
                    .then(|| {
                        (
                            collection.to_string(),
                            computed_member(state, collection_record, &record),
                        )
                    })
            })
            .collect();
        for (collection, member) in smart {
            if link_collection(state, &collection, product, member) {
                sort_smart(state, &collection);
            }
        }
    }
}

/// Matches every product against a smart collection again, with `collections/update` when its
/// products changed: what a change of a source it links does.
pub fn reevaluate(state: &mut ShopState, collection: &str) {
    let before = state
        .get(collection)
        .map(|record| gids(record.get("products")));
    evaluate(state, collection);
    let after = state
        .get(collection)
        .map(|record| gids(record.get("products")));
    if before != after {
        state.touch(collection);
        state.emit("collections/update", collection);
    }
}

/// Every product of the shop matched against a smart collection's rules again.
pub(super) fn evaluate(state: &mut ShopState, collection: &str) {
    let Some(record) = state.get(collection).cloned() else {
        return;
    };
    // A collection whose last source went has no member: its membership is the union of none.
    let sourceless = record.get("sources") == Some(&json!([])) && !is_smart(&record);
    if !is_smart(&record) && !sourceless {
        return;
    }
    let products: Vec<String> = state.keys("Product").map(str::to_string).collect();
    for product in products {
        let member = state
            .get(&product)
            .is_some_and(|product| computed_member(state, &record, product));
        link_collection(state, collection, &product, member);
    }
    sort_smart(state, collection);
}

/// The products of a smart collection in its sort order (a custom collection keeps its manual
/// order): by title, by lowest price, by creation date, else in the order of their ids.
fn sort_smart(state: &mut ShopState, collection: &str) {
    let Some(record) = state.get(collection) else {
        return;
    };
    if !is_smart(record) {
        return;
    }
    let order = field(record, "sortOrder")
        .unwrap_or("MOST_RELEVANT")
        .to_string();
    let mut products = gids(record.get("products"));
    let id = |product: &String| crate::state::parse_gid(product).map_or(0, |(_, id)| id);
    let title = |product: &String| {
        state
            .get(product)
            .and_then(|record| field(record, "title"))
            .unwrap_or_default()
            .to_lowercase()
    };
    let price = |product: &String| {
        state
            .get(product)
            .and_then(|record| record.get("priceRangeV2"))
            .and_then(|range| range.get("minVariantPrice"))
            .and_then(|money| cents_of(money.get("amount")))
            .unwrap_or(0)
    };
    let created = |product: &String| {
        state
            .get(product)
            .and_then(|record| field(record, "createdAt"))
            .unwrap_or_default()
            .to_string()
    };
    match order.as_str() {
        "ALPHA_ASC" => products.sort_by_key(|p| (title(p), id(p))),
        "ALPHA_DESC" => products.sort_by_key(|p| (std::cmp::Reverse(title(p)), id(p))),
        "PRICE_ASC" => products.sort_by_key(|p| (price(p), id(p))),
        "PRICE_DESC" => products.sort_by_key(|p| (std::cmp::Reverse(price(p)), id(p))),
        "CREATED" => products.sort_by_key(|p| (created(p), id(p))),
        "CREATED_DESC" => products.sort_by_key(|p| (std::cmp::Reverse(created(p)), id(p))),
        _ => products.sort_by_key(id),
    }
    let unchanged = state
        .get(collection)
        .is_some_and(|record| gids(record.get("products")) == products);
    if !unchanged && let Some(record) = state.get_mut(collection) {
        record.insert("products".into(), json!(products));
    }
}

/// The rule set of a `CollectionRuleSetInput`, checked: errors under `["ruleSet", …]`.
fn rule_set_of(input: &Map<String, Value>, errors: &mut Vec<UserError>) -> Option<Value> {
    let rule_set = input.get("ruleSet")?.as_object()?;
    let mut rules = Vec::new();
    for (index, rule) in objects(rule_set, "rules").iter().enumerate() {
        let position = index.to_string();
        let column = text(rule, "column").unwrap_or_default();
        let relation = text(rule, "relation").unwrap_or_default();
        let condition = text(rule, "condition").unwrap_or_default().trim();
        let numeric = matches!(
            column,
            "VARIANT_PRICE" | "VARIANT_COMPARE_AT_PRICE" | "VARIANT_WEIGHT" | "VARIANT_INVENTORY"
        );
        if condition.is_empty() && !matches!(relation, "IS_SET" | "IS_NOT_SET") {
            errors.push(UserError::new(
                &["ruleSet", "rules", &position, "condition"],
                "Condition can't be blank",
            ));
        } else if numeric && condition.parse::<f64>().is_err() {
            errors.push(UserError::new(
                &["ruleSet", "rules", &position, "condition"],
                "Condition is not a number",
            ));
        }
        rules.push(json!({
            "column": column,
            "relation": relation,
            "condition": condition,
            "conditionObject": null,
        }));
    }
    Some(json!({
        "appliedDisjunctively": rule_set.get("appliedDisjunctively").and_then(Value::as_bool).unwrap_or(false),
        "rules": rules,
    }))
}

/// Notes the rules lsf does not evaluate.
fn note_rules(mutation: &mut Mutation<'_>, name: &str, rule_set: Option<&Value>) {
    let Some(rules) = rule_set
        .and_then(|rule_set| rule_set.get("rules"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for rule in rules {
        let column = rule
            .get("column")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !COLUMNS.contains(&column) {
            mutation.note(format!(
                "{name}: lsf does not evaluate the rule column {column}: it matches no product"
            ));
        }
    }
}

/// The input of a collection mutation: `collection`, else the deprecated `input`.
fn collection_input(ctx: &FieldContext<'_>) -> Map<String, Value> {
    match ctx.arg_object("collection") {
        Some(collection) => collection.clone(),
        None => ctx.arg_object_or_default("input"),
    }
}

/// Notes the inputs of a collection mutation lsf does not simulate.
fn note_ignored(mutation: &mut Mutation<'_>, name: &str, input: &Map<String, Value>) {
    for field in ["publications", "redirectNewHandle"] {
        if input
            .get(field)
            .is_some_and(|value| !value.is_null() && value != &json!([]) && value != &json!(false))
        {
            mutation.note(format!(
                "{name}: lsf does not simulate `{field}` yet: it was ignored"
            ));
        }
    }
}

/// The `image` of a collection from an `ImageInput`.
fn image_of(state: &mut ShopState, image: &Value) -> Value {
    let Some(image) = image.as_object() else {
        return Value::Null;
    };
    let Some(src) = text(image, "src").filter(|src| !src.is_empty()) else {
        return Value::Null;
    };
    let id = state.new_gid("CollectionImage");
    json!({
        "id": id,
        "url": value::file_url(&origin(state), src),
        "altText": text(image, "altText"),
        "width": null,
        "height": null,
    })
}

/// Sets the plain fields of a collection input: those given, and only those.
fn apply_fields(state: &mut ShopState, collection: &str, input: &Map<String, Value>) {
    let image = input.get("image").map(|image| image_of(state, image));
    let Some(record) = state.get_mut(collection) else {
        return;
    };
    if let Some(title) = text(input, "title") {
        record.insert("title".into(), json!(title.trim()));
    }
    if let Some(html) = text(input, "descriptionHtml") {
        record.insert("descriptionHtml".into(), json!(html));
        record.insert("description".into(), json!(value::plain_text(html)));
    }
    if let Some(order) = text(input, "sortOrder") {
        record.insert("sortOrder".into(), json!(order));
    }
    if let Some(suffix) = input.get("templateSuffix") {
        let suffix = suffix.as_str().filter(|suffix| !suffix.is_empty());
        record.insert("templateSuffix".into(), json!(suffix));
    }
    if let Some(seo) = input.get("seo").and_then(Value::as_object) {
        record.insert(
            "seo".into(),
            json!({"title": seo.get("title"), "description": seo.get("description")}),
        );
    }
    if let Some(image) = image {
        record.insert("image".into(), image);
    }
}

/// The checks of a title.
fn check_title(title: Option<&str>, required: bool, errors: &mut Vec<UserError>) {
    match title {
        Some(title) if title.trim().is_empty() => {
            errors.push(UserError::new(&["title"], "Title can't be blank"))
        }
        Some(title) if title.chars().count() > 255 => errors.push(UserError::new(
            &["title"],
            "Title is too long (maximum is 255 characters)",
        )),
        None if required => errors.push(UserError::new(&["title"], "Title can't be blank")),
        _ => {}
    }
}

/// `collectionCreate`.
fn collection_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = collection_input(ctx);
    note_ignored(mutation, "collectionCreate", &input);
    let mut errors = Vec::new();
    check_title(text(&input, "title"), true, &mut errors);
    let rule_set = rule_set_of(&input, &mut errors);
    let state = mutation.state();
    let sources = objects(&input, "sources");
    sources::check_targets(state, &sources, "sources", None, &mut errors);
    if sources.len() > sources::MAX_SOURCES {
        errors.push(UserError::new(
            &["sources"],
            format!(
                "A collection can have at most {} sources",
                sources::MAX_SOURCES
            ),
        ));
    }
    let products = gids(input.get("products"));
    for (index, product) in products.iter().enumerate() {
        if !is(state, product, "Product") {
            errors.push(UserError::new(
                &["products", &index.to_string()],
                format!("Product {product} does not exist"),
            ));
        }
    }
    let metafields = objects(&input, "metafields");
    errors.extend(records::check_metafields(
        state,
        None,
        "COLLECTION",
        &metafields,
        &["metafields"],
        None,
    ));
    if !errors.is_empty() {
        return Ok(payload(json!({"collection": null}), &errors));
    }
    note_rules(mutation, "collectionCreate", rule_set.as_ref());
    sources::note_inputs(mutation, "collectionCreate", &json!(sources));
    if rule_set.is_some() && !products.is_empty() {
        mutation.note("collectionCreate: a smart collection's products are those its rules match: `products` was ignored");
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let title = text(&input, "title").unwrap_or_default().trim().to_string();
    let handle = records::free_handle(
        state,
        "Collection",
        text(&input, "handle").unwrap_or(&title),
        "collection",
    );
    let id = state.new_gid("Collection");
    state.insert(value::object(json!({
        "__typename": "Collection",
        "id": id,
        "title": title,
        "handle": handle,
        "descriptionHtml": "",
        "description": "",
        "image": null,
        "products": [],
        "sortOrder": if rule_set.is_some() { "MOST_RELEVANT" } else { "MANUAL" },
        "ruleSet": rule_set,
        "templateSuffix": null,
        "seo": {"title": null, "description": null},
        "createdAt": now,
        "updatedAt": now,
        "metafields": [],
    })));
    apply_fields(state, &id, &input);
    let linked = sources::create_targets(state, &id, &sources);
    if !linked.is_empty()
        && let Some(record) = state.get_mut(&id)
    {
        record.insert("sources".into(), json!(linked));
    }
    if rule_set.is_some() || !linked.is_empty() {
        evaluate(state, &id);
    } else {
        for product in &products {
            link_collection(state, &id, product, true);
        }
    }
    records::set_metafields(state, &id, "COLLECTION", &metafields);
    state.emit("collections/create", &id);
    Ok(payload(json!({"collection": id}), &[]))
}

/// `collectionUpdate`.
fn collection_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = collection_input(ctx);
    note_ignored(mutation, "collectionUpdate", &input);
    let state = mutation.state();
    let id = text(&input, "id").unwrap_or_default().to_string();
    let Some(record) = state.get(&id).filter(|_| is(state, &id, "Collection")) else {
        let error = UserError::new(&["id"], "Collection does not exist");
        return Ok(payload(json!({"collection": null, "job": null}), &[error]));
    };
    let smart = is_smart(record);
    let mut errors = Vec::new();
    check_title(text(&input, "title"), false, &mut errors);
    let rule_set = rule_set_of(&input, &mut errors);
    sources::check_update(state, &id, &input, &mut errors);
    if rule_set.is_some() && !smart {
        // shopify.dev's example.
        errors.push(UserError::new(
            &["id"],
            "Cannot update rule set of a custom collection",
        ));
    }
    let handle = text(&input, "handle").map(lsf_core::util::handleize);
    if let Some(handle) = &handle
        && (handle.is_empty() || records::handle_taken(state, "Collection", handle, Some(&id)))
    {
        errors.push(UserError::new(&["handle"], "Handle has already been taken"));
    }
    let metafields = objects(&input, "metafields");
    errors.extend(records::check_metafields(
        state,
        Some(&id),
        "COLLECTION",
        &metafields,
        &["metafields"],
        None,
    ));
    if !errors.is_empty() {
        return Ok(payload(json!({"collection": null, "job": null}), &errors));
    }
    if input.contains_key("products") {
        mutation.note("collectionUpdate: `products` is for collectionCreate (use collectionAddProducts): it was ignored");
    }
    note_rules(mutation, "collectionUpdate", rule_set.as_ref());
    for field in ["sourcesToCreate", "sourcesToUpdate"] {
        if let Some(value) = input.get(field) {
            sources::note_inputs(mutation, "collectionUpdate", value);
        }
    }
    let state = mutation.state_mut();
    apply_fields(state, &id, &input);
    let resourced = sources::apply_update(state, &id, &input);
    if let Some(handle) = handle
        && let Some(record) = state.get_mut(&id)
    {
        record.insert("handle".into(), json!(handle));
    }
    let resorted = input.contains_key("sortOrder");
    let job = if let Some(rule_set) = rule_set {
        if let Some(record) = state.get_mut(&id) {
            record.insert("ruleSet".into(), rule_set);
        }
        evaluate(state, &id);
        Some(done_job(state, "collection_update"))
    } else if resourced {
        evaluate(state, &id);
        Some(done_job(state, "collection_update"))
    } else {
        if resorted {
            sort_smart(state, &id);
        }
        None
    };
    records::set_metafields(state, &id, "COLLECTION", &metafields);
    state.touch(&id);
    state.emit("collections/update", &id);
    Ok(payload(json!({"collection": id, "job": job}), &[]))
}

/// `collectionDelete`.
fn collection_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx
        .arg_object_or_default("input")
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is(mutation.state(), &id, "Collection") {
        // shopify.dev's example.
        let error = UserError::new(&["id"], "Collection does not exist");
        return Ok(payload(json!({"deletedCollectionId": null}), &[error]));
    }
    let state = mutation.state_mut();
    delete_collection(state, &id);
    Ok(payload(json!({"deletedCollectionId": id}), &[]))
}

/// Removes a collection, takes it out of its products and publications, and emits
/// `collections/delete`.
pub fn delete_collection(state: &mut ShopState, collection: &str) {
    let Some(record) = state.get(collection).cloned() else {
        return;
    };
    for product in gids(record.get("products")) {
        if let Some(Value::Array(collections)) = state
            .get_mut(&product)
            .and_then(|record| record.get_mut("collections"))
        {
            collections.retain(|known| known.as_str() != Some(collection));
        }
    }
    // The listings of the sales channels that have it.
    crate::domains::markets::removing(state, collection);
    let publications: Vec<String> = state.keys("Publication").map(str::to_string).collect();
    for publication in publications {
        records::set_listed(state, &publication, "collections", collection, false);
    }
    for metafield in gids(record.get("metafields")) {
        state.remove(&metafield);
    }
    sources::delete_owned(state, collection);
    state.delete(collection, "collections/delete");
}

/// The custom collection of `id`, or why products cannot be added to it by hand.
fn manual_collection(
    state: &ShopState,
    id: &str,
    action: &str,
) -> Result<(), (String, &'static str)> {
    let Some(record) = state.get(id).filter(|_| is(state, id, "Collection")) else {
        return Err((
            "Collection does not exist".into(),
            "COLLECTION_DOES_NOT_EXIST",
        ));
    };
    if is_smart(record) {
        return Err((
            format!(
                "Can't manually {action} products {} a smart collection",
                if action == "add" { "to" } else { "from" }
            ),
            "CANT_ADD_TO_SMART_COLLECTION",
        ));
    }
    Ok(())
}

/// Adds products to a custom collection, at the end, those it has already staying where they
/// are. Emits `collections/update` when it changed.
fn add(state: &mut ShopState, collection: &str, products: &[String]) {
    let mut changed = false;
    for product in products {
        changed |= link_collection(state, collection, product, true);
    }
    if changed {
        state.touch(collection);
        state.emit("collections/update", collection);
    }
}

/// The checks of `productIds`: every product must exist (shopify.dev's example answers
/// "Error adding gid://shopify/Product/108828309,gid://shopify/Product/20995642 to collection"
/// with `field` `["productIds"]` when they cannot be added).
fn check_products(state: &ShopState, ctx: &FieldContext<'_>) -> (Vec<String>, Option<String>) {
    let products = records::arg_strings(ctx, "productIds");
    let missing: Vec<&str> = products
        .iter()
        .filter(|product| !is(state, product, "Product"))
        .map(String::as_str)
        .collect();
    let error =
        (!missing.is_empty()).then(|| format!("Error adding {} to collection", missing.join(",")));
    (products, error)
}

/// `collectionAddProducts`.
fn add_products(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    if let Err((message, _)) = manual_collection(state, &id, "add") {
        return Ok(payload(
            json!({"collection": null}),
            &[UserError::new(&["id"], message)],
        ));
    }
    let (products, error) = check_products(state, ctx);
    if let Some(error) = error {
        return Ok(payload(
            json!({"collection": null}),
            &[UserError::new(&["productIds"], error)],
        ));
    }
    add(mutation.state_mut(), &id, &products);
    Ok(payload(json!({"collection": id}), &[]))
}

/// `collectionAddProductsV2`: asynchronous on Shopify; lsf's job is already done.
fn add_products_v2(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    if let Err((message, code)) = manual_collection(state, &id, "add") {
        return Ok(payload(
            json!({"job": null}),
            &[coded(&["id"], &message, code)],
        ));
    }
    let (products, error) = check_products(state, ctx);
    if let Some(error) = error {
        return Ok(payload(
            json!({"job": null}),
            &[UserError::new(&["productIds"], error)],
        ));
    }
    let state = mutation.state_mut();
    add(state, &id, &products);
    let job = done_job(state, "collection_add_products");
    Ok(payload(json!({"job": job}), &[]))
}

/// `collectionRemoveProducts`.
fn remove_products(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if let Err((message, _)) = manual_collection(mutation.state(), &id, "remove") {
        // shopify.dev's examples.
        return Ok(payload(
            json!({"job": null}),
            &[UserError::new(&["id"], message)],
        ));
    }
    let products = records::arg_strings(ctx, "productIds");
    let state = mutation.state_mut();
    let mut changed = false;
    for product in &products {
        changed |= link_collection(state, &id, product, false);
    }
    if changed {
        state.touch(&id);
        state.emit("collections/update", &id);
    }
    let job = done_job(state, "collection_remove_products");
    Ok(payload(json!({"job": job}), &[]))
}

/// `collectionReorderProducts`: each move takes the product out and puts it at `newPosition`
/// (from 0), in the order of the moves.
fn reorder_products(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = state.get(&id).filter(|_| is(state, &id, "Collection")) else {
        let error = coded(&["id"], "Collection does not exist", "COLLECTION_NOT_FOUND");
        return Ok(payload(json!({"job": null}), &[error]));
    };
    if field(record, "sortOrder") != Some("MANUAL") {
        // shopify.dev's example.
        let error = coded(
            &["id"],
            "Can't reorder products unless collection is manually sorted",
            "MANUALLY_SORTED_COLLECTION",
        );
        return Ok(payload(json!({"job": null}), &[error]));
    }
    let mut products = gids(record.get("products"));
    let moves = arg_objects(ctx, "moves");
    let mut errors = Vec::new();
    for (index, change) in moves.iter().enumerate() {
        let product = text(change, "id").unwrap_or_default();
        if !products.iter().any(|known| known == product) {
            errors.push(coded(
                &["moves", &index.to_string(), "id"],
                "Product is not in the collection",
                "INVALID_MOVE",
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"job": null}), &errors));
    }
    for change in &moves {
        records::apply_move(&mut products, change);
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("products".into(), json!(products));
    }
    state.touch(&id);
    state.emit("collections/update", &id);
    let job = done_job(state, "collection_reorder_products");
    Ok(payload(json!({"job": job}), &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_text_and_numbers_as_rules_do() {
        assert!(text_matches("Summer Shoe", "CONTAINS", "shoe"));
        assert!(text_matches("Summer Shoe", "STARTS_WITH", "summer"));
        assert!(!text_matches("Summer Shoe", "NOT_CONTAINS", "SHOE"));
        assert!(number_matches(Some(20.0), "GREATER_THAN", 19.99));
        assert!(!number_matches(None, "LESS_THAN", 10.0));
        assert!(number_matches(None, "IS_NOT_SET", 0.0));
        assert!((weight_in(1.0, "KILOGRAMS", "GRAMS") - 1000.0).abs() < 1e-9);
    }
}

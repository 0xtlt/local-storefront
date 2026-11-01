//! The collection sources of 2026-10: a collection is "a union of sources", each a conditions
//! source (`CollectionConditionsSource`: typed conditions and manual selections, minus
//! exclusions) or a sub-collection source (`CollectionSubCollectionsSource`: the members of
//! other collections). An app's shareable sources are made with
//! `collectionConditionsSourceCreate`, `…Update`, `…Delete`, and linked into collections by
//! reference (`CollectionShareableSourceInput`).
//!
//! As shopify.dev describes them ("Use the new collections model", "Migrate to the new
//! collections model", read on 2026-10-31, and the reference of 2026-10):
//!
//! - "A collection has one or more sources, and its membership is the union of what each source
//!   resolves to. Each source contributes products through inclusions (conditions and explicit
//!   selections) and can remove them through exclusions, evaluated per source."
//! - Conditions are typed (`productTag`, `productTitle`, `productType`, `productVendor`,
//!   `productStatus`, `productCategory`, the variant's `variantTitle`, `variantPrice`,
//!   `variantCompareAtPrice`, `variantInventory`, `variantWeight`, and the metafields of a
//!   definition), each with its relation and, for a list of values, `matchType` (`ANY` or `ALL`
//!   of the values); `inclusion.matchType` says whether all the conditions or any of them must
//!   hold. Exclusions are by tag, type, vendor, category, or by collection ("up to 5 per source",
//!   which "can't currently be combined with any other exclusion condition type"), and by manual
//!   selection.
//! - `targetType: PRODUCTS`: "the entire product is included when any of its variants satisfies
//!   the conditions"; `VARIANTS`: "only the variants that satisfy the conditions are included".
//!   lsf keeps membership by product: a variant-scoped source puts a product in when one of its
//!   variants matches (the schema of 2026-10 has no variant-level membership field to read), and
//!   says so. "Manually targeting a whole product isn't allowed on a variant-scoped source."
//! - "Sub-collection chains are restricted to a depth of one": a sub-collection target can't
//!   itself have a sub-collection source (`CHAIN_REFERENCE`), nor be the collection itself
//!   (`SELF_REFERENCE`).
//! - Shareable sources: "`collectionConditionsSourceCreate` returns a saved source that's linked
//!   to zero collections"; they "persist when unlinked from a collection, or when a collection is
//!   deleted"; "Deleting a source also unlinks it from every collection that currently uses it";
//!   "When an app updates the source, every collection that links it picks up the change". A
//!   collection's own sources "are deleted as soon as they no longer belong to one".
//! - Limits: 10 sources per collection, 60 conditions per source ("Each value counts
//!   separately"), 1,000 manual exclusions, 10 sub-collections.
//!
//! Records: a source is a record of its type (`CollectionConditionsSource/<n>`,
//! `CollectionSubCollectionsSource/<n>`), its conditions inline with ids of their type
//! (`CollectionSourceInclusionConditionProductTag/<n>`, as the reference's example), the
//! collection that owns it under `lsfCollection` (none for a shareable one). A collection lists
//! its sources in `sources`. A collection of the legacy model (the store data's, a `ruleSet`, a
//! custom one) has no source records: `Collection.sources` answers the one source it amounts to,
//! built from its rules or its products.
//!
//! Not simulated: product categories (lsf keeps no `category`: a category condition matches
//! nothing, and says so). The messages without a published wording are lsf's (each says so).

use serde_json::{Map, Value, json};

use super::records::{error_at, field, in_collection, input, objects, text};
use super::{cents_of, gids};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("collectionConditionsSourceCreate", source_create)
        .mutation("collectionConditionsSourceUpdate", source_update)
        .mutation("collectionConditionsSourceDelete", source_delete)
        .query("collectionConditionsSources", sources_of_app)
        .query("collectionConditionsSourcesByApp", sources_by_app)
        .field("Collection", "sources", collection_sources)
        .field(
            "Collection",
            "subCollectionEligibility",
            sub_collection_eligibility,
        )
        .field("CollectionConditionsSource", "products", source_products);
}

/// The type of a conditions source.
pub const CONDITIONS: &str = "CollectionConditionsSource";

/// The type of a sub-collection source.
pub const SUB_COLLECTIONS: &str = "CollectionSubCollectionsSource";

/// The record field of a source that names the collection it belongs to.
const OWNER: &str = "lsfCollection";

/// The most sources a collection has.
pub const MAX_SOURCES: usize = 10;

/// The most conditions a source has, each value counted.
const MAX_CONDITIONS: usize = 60;

/// The most collections a source excludes.
const MAX_EXCLUDED_COLLECTIONS: usize = 5;

/// The most collections a sub-collection source draws from.
const MAX_SUB_COLLECTIONS: usize = 10;

/// The most products a source excludes by hand.
const MAX_EXCLUSIONS: usize = 1000;

/// The typed inclusion conditions: the input's field, the type's suffix.
const INCLUSION_KINDS: &[(&str, &str)] = &[
    ("metafieldBoolean", "MetafieldBoolean"),
    ("metafieldDecimal", "MetafieldDecimal"),
    ("metafieldInt", "MetafieldInt"),
    ("metafieldInteger", "MetafieldInteger"),
    ("metafieldMetaobject", "MetafieldMetaobject"),
    ("metafieldMetaobjectList", "MetafieldMetaobjectList"),
    ("metafieldString", "MetafieldString"),
    ("metafieldStringList", "MetafieldStringList"),
    ("productCategory", "ProductCategory"),
    ("productStatus", "ProductStatus"),
    ("productTag", "ProductTag"),
    ("productTitle", "ProductTitle"),
    ("productType", "ProductType"),
    ("productVendor", "ProductVendor"),
    ("variantCompareAtPrice", "VariantCompareAtPrice"),
    ("variantInventory", "VariantInventory"),
    ("variantPrice", "VariantPrice"),
    ("variantTitle", "VariantTitle"),
    ("variantWeight", "VariantWeight"),
];

/// The typed exclusion conditions.
const EXCLUSION_KINDS: &[(&str, &str)] = &[
    ("collection", "Collection"),
    ("productCategory", "ProductCategory"),
    ("productTag", "ProductTag"),
    ("productType", "ProductType"),
    ("productVendor", "ProductVendor"),
];

/// The types of every condition a source can hold, made by name from the kinds: what a test
/// checks each served version has.
#[cfg(test)]
pub(crate) fn condition_type_names() -> Vec<String> {
    [false, true]
        .into_iter()
        .flat_map(|exclusion| {
            kinds(exclusion)
                .iter()
                .map(move |(_, suffix)| format!("{}{suffix}", prefix(exclusion)))
        })
        .collect()
}

/// The prefix of the type of an inclusion or an exclusion condition.
fn prefix(exclusion: bool) -> &'static str {
    if exclusion {
        "CollectionSourceExclusionCondition"
    } else {
        "CollectionSourceInclusionCondition"
    }
}

fn kinds(exclusion: bool) -> &'static [(&'static str, &'static str)] {
    if exclusion {
        EXCLUSION_KINDS
    } else {
        INCLUSION_KINDS
    }
}

// --- Reading ---

/// Whether a collection's membership comes from sources of the 2026-10 model.
pub fn has_sources(record: &Record) -> bool {
    record
        .get("sources")
        .and_then(Value::as_array)
        .is_some_and(|sources| !sources.is_empty())
}

/// Whether a collection's sources read other collections (sub-collections, exclusions by
/// collection): its membership follows theirs.
pub fn reads_collections(state: &ShopState, record: &Record) -> bool {
    gids(record.get("sources")).iter().any(|source| {
        state.get(source).is_some_and(|source| {
            field(source, "__typename") == Some(SUB_COLLECTIONS)
                || !excluded_collections(source).is_empty()
        })
    })
}

/// The collections a conditions source excludes.
fn excluded_collections(source: &Record) -> Vec<String> {
    source
        .get("exclusion")
        .and_then(|exclusion| exclusion.get("conditions"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|condition| {
            condition.get("__typename").and_then(Value::as_str)
                == Some("CollectionSourceExclusionConditionCollection")
        })
        .flat_map(|condition| gids(condition.get("values")))
        .collect()
}

/// Whether a product is a member of a collection of the 2026-10 model: of any of its sources.
pub fn member(state: &ShopState, collection: &Record, product: &Record) -> bool {
    gids(collection.get("sources"))
        .iter()
        .filter_map(|source| state.get(source))
        .any(|source| source_member(state, source, product))
}

/// Whether a product is a member of a source.
pub fn source_member(state: &ShopState, source: &Record, product: &Record) -> bool {
    let id = field(product, "id").unwrap_or_default();
    if field(source, "__typename") == Some(SUB_COLLECTIONS) {
        return gids(source.get("collections"))
            .iter()
            .any(|collection| in_collection(state, collection, id));
    }
    let Some(inclusion) = source.get("inclusion") else {
        return false;
    };
    let selected = |part: &Value| {
        part.get("selections")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|selection| selection.get("product").and_then(Value::as_str) == Some(id))
    };
    let included = selected(inclusion) || conditions_hold(state, inclusion, product, false);
    if !included {
        return false;
    }
    let excluded = source.get("exclusion").is_some_and(|exclusion| {
        selected(exclusion) || conditions_hold(state, exclusion, product, true)
    });
    !excluded
}

/// Whether the conditions of an inclusion (or an exclusion) hold for a product, by the part's
/// `matchType` (`ALL` by default for an inclusion, `ANY` for an exclusion). No condition holds
/// nothing.
fn conditions_hold(state: &ShopState, part: &Value, product: &Record, exclusion: bool) -> bool {
    let conditions: Vec<&Value> = part
        .get("conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    if conditions.is_empty() {
        return false;
    }
    let default = if exclusion { "ANY" } else { "ALL" };
    let all = part
        .get("matchType")
        .and_then(Value::as_str)
        .unwrap_or(default)
        == "ALL";
    let holds = |condition: &&Value| condition_holds(state, condition, product);
    if all {
        conditions.iter().all(holds)
    } else {
        conditions.iter().any(holds)
    }
}

/// Whether a text satisfies a relation with a value, ignoring case.
fn text_relation(text: &str, relation: &str, value: &str) -> bool {
    let (text, value) = (text.to_lowercase(), value.to_lowercase());
    match relation {
        "EQUALS" | "TAGGED_WITH" => text == value,
        "NOT_EQUALS" | "NOT_TAGGED_WITH" => text != value,
        "CONTAINS" => text.contains(&value),
        "DOES_NOT_CONTAIN" => !text.contains(&value),
        "STARTS_WITH" => text.starts_with(&value),
        "ENDS_WITH" => text.ends_with(&value),
        _ => false,
    }
}

/// Whether a number satisfies a relation with a value.
fn number_relation(number: Option<f64>, relation: &str, value: Option<f64>) -> bool {
    match (relation, number, value) {
        ("IS_SET", number, _) => number.is_some(),
        ("IS_NOT_SET", number, _) => number.is_none(),
        ("EQUALS", Some(number), Some(value)) => (number - value).abs() < 1e-9,
        ("NOT_EQUALS", Some(number), Some(value)) => (number - value).abs() >= 1e-9,
        ("GREATER_THAN", Some(number), Some(value)) => number > value,
        ("LESS_THAN", Some(number), Some(value)) => number < value,
        _ => false,
    }
}

/// The strings of a condition's `values`.
fn values_of(condition: &Value) -> Vec<String> {
    condition
        .get("values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Object(map) => map.get("id").and_then(Value::as_str).map(str::to_string),
            _ => None,
        })
        .collect()
}

/// Whether `test` holds for any (or all, by the condition's `matchType`) of its values.
fn by_values(condition: &Value, test: impl Fn(&str) -> bool) -> bool {
    let values = values_of(condition);
    if values.is_empty() {
        return false;
    }
    if condition.get("matchType").and_then(Value::as_str) == Some("ALL") {
        values.iter().all(|value| test(value))
    } else {
        values.iter().any(|value| test(value))
    }
}

/// The decimal of a `MoneyV2`, a `Weight` or a number.
fn decimal(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        Value::Object(map) => decimal(map.get("amount").or_else(|| map.get("value"))),
        _ => None,
    }
}

/// Whether one condition holds for a product (a variant condition: for one of its variants).
fn condition_holds(state: &ShopState, condition: &Value, product: &Record) -> bool {
    let type_name = condition
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = type_name
        .strip_prefix("CollectionSourceInclusionCondition")
        .or_else(|| type_name.strip_prefix("CollectionSourceExclusionCondition"))
        .unwrap_or_default();
    let relation = condition
        .get("relation")
        .and_then(Value::as_str)
        .unwrap_or("EQUALS");
    let variants: Vec<&Record> = gids(product.get("variants"))
        .iter()
        .filter_map(|variant| state.get(variant))
        .collect();
    let text_of = |name: &str| field(product, name).unwrap_or_default().to_string();
    match kind {
        "ProductTag" => {
            let tags = super::tags_of(product.get("tags"));
            let tagged = |value: &str| tags.iter().any(|tag| tag.eq_ignore_ascii_case(value));
            if relation == "NOT_TAGGED_WITH" {
                by_values(condition, |value| !tagged(value))
            } else {
                by_values(condition, tagged)
            }
        }
        "ProductTitle" => by_values(condition, |v| text_relation(&text_of("title"), relation, v)),
        "ProductType" => by_values(condition, |v| {
            text_relation(&text_of("productType"), relation, v)
        }),
        "ProductVendor" => by_values(condition, |v| {
            text_relation(&text_of("vendor"), relation, v)
        }),
        "ProductStatus" => by_values(condition, |v| {
            text_relation(&text_of("status"), relation, v)
        }),
        "Collection" => {
            let id = text_of("id");
            values_of(condition)
                .iter()
                .any(|collection| in_collection(state, collection, &id))
        }
        "VariantTitle" => variants.iter().any(|variant| {
            by_values(condition, |v| {
                text_relation(field(variant, "title").unwrap_or_default(), relation, v)
            })
        }),
        "VariantPrice" | "VariantCompareAtPrice" | "VariantInventory" | "VariantWeight" => {
            let wanted = decimal(condition.get("value"));
            let unit = condition
                .get("value")
                .and_then(|value| value.get("unit"))
                .and_then(Value::as_str)
                .map(str::to_string);
            variants.iter().any(|variant| {
                let number = match kind {
                    "VariantPrice" => cents_of(variant.get("price")).map(|c| c as f64 / 100.0),
                    "VariantCompareAtPrice" => {
                        cents_of(variant.get("compareAtPrice")).map(|c| c as f64 / 100.0)
                    }
                    "VariantInventory" => variant
                        .get("inventoryQuantity")
                        .and_then(Value::as_i64)
                        .map(|q| q as f64),
                    _ => weight_of(state, variant, unit.as_deref()),
                };
                number_relation(number, relation, wanted)
            })
        }
        kind if kind.starts_with("Metafield") => metafield_holds(state, condition, kind, product),
        _ => false,
    }
}

/// A variant's weight, in `unit` (its own unit when none is given).
pub fn weight_of(state: &ShopState, variant: &Record, unit: Option<&str>) -> Option<f64> {
    let weight = field(variant, "inventoryItem")
        .and_then(|item| state.get(item))
        .and_then(|item| item.get("measurement"))
        .and_then(|measurement| measurement.get("weight"))?;
    let amount = weight.get("value").and_then(Value::as_f64)?;
    let from = weight
        .get("unit")
        .and_then(Value::as_str)
        .unwrap_or("KILOGRAMS");
    Some(super::collections::weight_in(
        amount,
        from,
        unit.unwrap_or(from),
    ))
}

/// Whether a metafield condition holds: the product's metafield of the definition (or one of its
/// variants', for a definition of variants).
fn metafield_holds(state: &ShopState, condition: &Value, kind: &str, product: &Record) -> bool {
    let Some(definition) = condition
        .get("definition")
        .and_then(Value::as_str)
        .and_then(|definition| state.get(definition))
    else {
        return false;
    };
    let namespace = field(definition, "namespace").unwrap_or_default();
    let key = field(definition, "key").unwrap_or_default();
    let owners: Vec<&Record> = if field(definition, "ownerType") == Some("PRODUCTVARIANT") {
        gids(product.get("variants"))
            .iter()
            .filter_map(|variant| state.get(variant))
            .collect()
    } else {
        vec![product]
    };
    let relation = condition
        .get("relation")
        .and_then(Value::as_str)
        .unwrap_or("EQUALS");
    owners.iter().any(|owner| {
        let Some(metafield) = gids(owner.get("metafields"))
            .iter()
            .filter_map(|metafield| state.get(metafield))
            .find(|metafield| {
                field(metafield, "namespace") == Some(namespace)
                    && field(metafield, "key") == Some(key)
            })
        else {
            return false;
        };
        let value = field(metafield, "value").unwrap_or_default();
        match kind {
            "MetafieldBoolean" => {
                condition.get("value").and_then(Value::as_bool) == Some(value == "true")
            }
            "MetafieldDecimal" | "MetafieldInt" | "MetafieldInteger" => number_relation(
                value.trim().parse().ok(),
                relation,
                decimal(condition.get("value")),
            ),
            "MetafieldMetaobject" => condition.get("value").and_then(Value::as_str) == Some(value),
            "MetafieldString" => by_values(condition, |wanted| value == wanted),
            _ => {
                // A list (`MetafieldStringList`, `MetafieldMetaobjectList`): `INCLUDES`.
                let items: Vec<String> = serde_json::from_str::<Vec<Value>>(value)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect();
                by_values(condition, |wanted| items.iter().any(|item| item == wanted))
            }
        }
    })
}

// --- Writing ---

/// The suffix of the type of the one condition an `@oneOf` input gives: `(field, suffix, its
/// fields)`.
fn condition_input(
    item: &Map<String, Value>,
    exclusion: bool,
) -> Option<(&'static str, &'static str, &Map<String, Value>)> {
    kinds(exclusion).iter().find_map(|(field, suffix)| {
        item.get(*field)
            .and_then(Value::as_object)
            .map(|fields| (*field, *suffix, fields))
    })
}

/// How many conditions a condition counts for: "Each value counts separately".
fn weight(condition: &Map<String, Value>) -> usize {
    condition
        .get("values")
        .and_then(Value::as_array)
        .map_or(1, |values| values.len().max(1))
}

/// Checks the conditions of an inclusion or an exclusion (or of their `conditionsToCreate`).
fn check_conditions(
    state: &ShopState,
    items: &[&Map<String, Value>],
    path: &[String],
    exclusion: bool,
    errors: &mut Vec<UserError>,
) -> usize {
    let mut count = 0;
    let mut collections = 0;
    let mut others = 0;
    for (index, item) in items.iter().enumerate() {
        let mut at: Vec<String> = path.to_vec();
        at.push(index.to_string());
        let Some((name, suffix, fields)) = condition_input(item, exclusion) else {
            continue;
        };
        at.push(name.to_string());
        count += weight(fields);
        if suffix == "Collection" {
            let values = gids(fields.get("values"));
            collections += values.len();
            for (position, collection) in values.iter().enumerate() {
                if !state.is_of(collection, "Collection") {
                    let field = below(&below(&at, "values"), &position.to_string());
                    errors.push(error_at(&field, "Collection does not exist"));
                }
            }
        } else {
            others += 1;
        }
        if let Some(definition) = text(fields, "definitionId")
            && !state.is_of(definition, "MetafieldDefinition")
        {
            // lsf's wording.
            errors.push(error_at(
                &below(&at, "definitionId"),
                "Metafield definition does not exist",
            ));
        }
        if suffix == "VariantCompareAtPrice" {
            let relation = text(fields, "relation").unwrap_or_default();
            let set = matches!(relation, "IS_SET" | "IS_NOT_SET");
            let given = fields.get("value").is_some_and(|value| !value.is_null());
            if set == given {
                // The schema's rule ("Required for numeric relations and must be absent for
                // IS_SET or IS_NOT_SET"), in lsf's words.
                errors.push(error_at(
                    &below(&at, "value"),
                    if set {
                        "Value must be absent for IS_SET or IS_NOT_SET"
                    } else {
                        "Value is required for a numeric relation"
                    },
                ));
            }
        }
        if fields
            .get("values")
            .and_then(Value::as_array)
            .is_some_and(|values| values.is_empty())
        {
            errors.push(error_at(&below(&at, "values"), "Values can't be blank"));
        }
    }
    if collections > MAX_EXCLUDED_COLLECTIONS {
        // lsf's wording of the guide's limit.
        errors.push(error_at(
            path,
            format!("A source can exclude at most {MAX_EXCLUDED_COLLECTIONS} collections"),
        ));
    }
    if collections > 0 && others > 0 {
        // The guide's rule, in lsf's words.
        errors.push(error_at(
            path,
            "Collection exclusions can't be combined with other exclusion conditions",
        ));
    }
    count
}

/// The path of an input field below `path`.
fn below(path: &[String], name: &str) -> Vec<String> {
    let mut path = path.to_vec();
    path.push(name.to_string());
    path
}

/// Checks the selections of an inclusion or an exclusion.
fn check_selections(
    state: &ShopState,
    items: &[&Map<String, Value>],
    path: &[String],
    variants: bool,
    errors: &mut Vec<UserError>,
) {
    for (index, item) in items.iter().enumerate() {
        let at = below(path, &index.to_string());
        let product = text(item, "productId").unwrap_or_default();
        if !state.is_of(product, "Product") {
            errors.push(error_at(&below(&at, "productId"), "Product does not exist"));
            continue;
        }
        let ids = gids(item.get("variantIds"));
        if variants && ids.is_empty() {
            // The guide's rule.
            errors.push(error_at(
                &below(&at, "variantIds"),
                "Manually targeting a whole product isn't allowed on a variant-scoped source.",
            ));
        } else if !variants && !ids.is_empty() {
            // lsf's wording: "Pass `variantIds` only when you're authoring a variant-scoped source."
            errors.push(error_at(
                &below(&at, "variantIds"),
                "Variants can only be selected on a source that targets variants",
            ));
        }
        let own = gids(state.get(product).and_then(|record| record.get("variants")));
        for (position, variant) in ids.iter().enumerate() {
            if !own.contains(variant) {
                errors.push(error_at(
                    &below(&below(&at, "variantIds"), &position.to_string()),
                    "Variant does not exist on the product",
                ));
            }
        }
    }
}

/// Checks a `CollectionCreateConditionsSourceInput`, at `path`.
fn check_conditions_source(
    state: &ShopState,
    source: &Map<String, Value>,
    path: &[String],
    errors: &mut Vec<UserError>,
) {
    if text(source, "title").is_none_or(|title| title.trim().is_empty()) {
        errors.push(error_at(&below(path, "title"), "Title can't be blank"));
    }
    let variants = text(source, "targetType") == Some("VARIANTS");
    let inclusion = source.get("inclusion").and_then(Value::as_object);
    let mut count = 0;
    let mut selections = 0;
    if let Some(inclusion) = inclusion {
        let at = below(path, "inclusion");
        count += check_conditions(
            state,
            &objects(inclusion, "conditions"),
            &below(&at, "conditions"),
            false,
            errors,
        );
        let picked = objects(inclusion, "selections");
        selections += picked.len();
        check_selections(state, &picked, &below(&at, "selections"), variants, errors);
    }
    if let Some(exclusion) = source.get("exclusion").and_then(Value::as_object) {
        let at = below(path, "exclusion");
        count += check_conditions(
            state,
            &objects(exclusion, "conditions"),
            &below(&at, "conditions"),
            true,
            errors,
        );
        let picked = objects(exclusion, "selections");
        if picked.len() > MAX_EXCLUSIONS {
            errors.push(error_at(
                &below(&at, "selections"),
                format!("A source can exclude at most {MAX_EXCLUSIONS} products"),
            ));
        }
        check_selections(state, &picked, &below(&at, "selections"), false, errors);
    }
    if count > MAX_CONDITIONS {
        // lsf's wording of the guide's limit.
        errors.push(error_at(
            path,
            format!("A source can have at most {MAX_CONDITIONS} conditions"),
        ));
    }
    if count == 0 && selections == 0 {
        // lsf's wording: a source with nothing to include has no member.
        errors.push(error_at(
            &below(path, "inclusion"),
            "A source needs at least one condition or selection",
        ));
    }
}

/// A condition record from its input: inline, with an id of its type.
fn new_condition(
    state: &mut ShopState,
    suffix: &str,
    fields: &Map<String, Value>,
    exclusion: bool,
) -> Value {
    let type_name = format!("{}{suffix}", prefix(exclusion));
    let mut condition = Map::new();
    condition.insert("__typename".into(), json!(type_name));
    condition.insert("id".into(), json!(state.new_gid(&type_name)));
    write_condition(&mut condition, suffix, fields);
    Value::Object(condition)
}

/// Writes the fields of a condition input onto the condition (those given).
fn write_condition(condition: &mut Map<String, Value>, suffix: &str, fields: &Map<String, Value>) {
    if suffix == "Collection" {
        condition.insert("matchType".into(), json!("ANY"));
    }
    for (name, value) in fields {
        let value = match (name.as_str(), suffix) {
            ("definitionId", _) => {
                condition.insert("definition".into(), value.clone());
                continue;
            }
            ("values", "ProductCategory") => json!(
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|item| json!({
                        "category": {"id": item.get("categoryId")},
                        "includeDescendants": item.get("includeDescendants").and_then(Value::as_bool).unwrap_or(false),
                    }))
                    .collect::<Vec<_>>()
            ),
            ("value", "VariantPrice" | "VariantCompareAtPrice") if value.is_object() => json!({
                "amount": value.get("amount").map(money_text),
                "currencyCode": value.get("currencyCode"),
            }),
            _ => value.clone(),
        };
        condition.insert(name.clone(), value);
    }
}

/// A money amount as `MoneyV2.amount` writes it.
fn money_text(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.to_string()),
        other => other.clone(),
    }
}

/// The selections of an input, as the record keeps them.
fn selections_of(items: &[&Map<String, Value>], variants: bool) -> Vec<Value> {
    items
        .iter()
        .map(|item| {
            if variants {
                json!({"product": item.get("productId"), "variantIds": gids(item.get("variantIds"))})
            } else {
                json!({"product": item.get("productId"), "variantIds": null})
            }
        })
        .collect()
}

/// An inclusion or an exclusion record from its create input.
fn part_of(
    state: &mut ShopState,
    part: Option<&Map<String, Value>>,
    exclusion: bool,
    variants: bool,
) -> Value {
    let Some(part) = part else {
        return if exclusion {
            Value::Null
        } else {
            json!({"matchType": "ALL", "conditions": [], "selections": []})
        };
    };
    let conditions: Vec<Value> = objects(part, "conditions")
        .iter()
        .filter_map(|item| condition_input(item, exclusion))
        .map(|(_, suffix, fields)| new_condition(state, suffix, fields, exclusion))
        .collect();
    let default = if exclusion { "ANY" } else { "ALL" };
    json!({
        "matchType": text(part, "matchType").unwrap_or(default),
        "conditions": conditions,
        "selections": selections_of(&objects(part, "selections"), variants && !exclusion),
    })
}

/// Creates a conditions source: its record, returned by GID.
fn create_conditions_source(
    state: &mut ShopState,
    source: &Map<String, Value>,
    owner: Option<&str>,
    app: Option<String>,
) -> String {
    let id = state.new_gid(CONDITIONS);
    let target = text(source, "targetType").unwrap_or("PRODUCTS").to_string();
    let variants = target == "VARIANTS";
    let inclusion = part_of(
        state,
        source.get("inclusion").and_then(Value::as_object),
        false,
        variants,
    );
    let exclusion = part_of(
        state,
        source.get("exclusion").and_then(Value::as_object),
        true,
        false,
    );
    state.insert(value::object(json!({
        "__typename": CONDITIONS,
        "id": id,
        "title": text(source, "title").unwrap_or_default().trim(),
        "description": text(source, "description"),
        "app": app,
        "shareable": owner.is_none(),
        "targetType": target,
        "inclusion": inclusion,
        "exclusion": exclusion,
        OWNER: owner,
    })));
    id
}

/// Applies a `CollectionUpdateSourceInclusionInput` (or exclusion) to a part of a source:
/// conditions created, updated, deleted, selections added and removed.
fn update_part(
    state: &mut ShopState,
    part: &mut Value,
    update: &Map<String, Value>,
    exclusion: bool,
    variants: bool,
) {
    if part.is_null() {
        *part = json!({"matchType": if exclusion { "ANY" } else { "ALL" }, "conditions": [], "selections": []});
    }
    if let Some(match_type) = text(update, "matchType") {
        part["matchType"] = json!(match_type);
    }
    let deleted = gids(update.get("conditionsToDelete"));
    let mut conditions: Vec<Value> = part["conditions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|condition| {
            !deleted
                .iter()
                .any(|id| condition.get("id").and_then(Value::as_str) == Some(id))
        })
        .collect();
    for item in objects(update, "conditionsToUpdate") {
        let id = text(item, "id").unwrap_or_default();
        let Some((_, suffix, fields)) = item
            .get("condition")
            .and_then(Value::as_object)
            .and_then(|condition| condition_input(condition, exclusion))
        else {
            continue;
        };
        if let Some(Value::Object(condition)) = conditions
            .iter_mut()
            .find(|condition| condition.get("id").and_then(Value::as_str) == Some(id))
        {
            write_condition(condition, suffix, fields);
        }
    }
    for item in objects(update, "conditionsToCreate") {
        if let Some((_, suffix, fields)) = condition_input(item, exclusion) {
            conditions.push(new_condition(state, suffix, fields, exclusion));
        }
    }
    part["conditions"] = json!(conditions);
    let mut selections = part["selections"].as_array().cloned().unwrap_or_default();
    let removed: Vec<String> = objects(update, "selectionsToRemove")
        .iter()
        .filter_map(|item| text(item, "productId").map(str::to_string))
        .collect();
    selections.retain(|selection| {
        !removed
            .iter()
            .any(|id| selection.get("product").and_then(Value::as_str) == Some(id))
    });
    for added in selections_of(&objects(update, "selectionsToAdd"), variants && !exclusion) {
        let known = selections
            .iter()
            .any(|selection| selection.get("product") == added.get("product"));
        if !known {
            selections.push(added);
        }
    }
    part["selections"] = json!(selections);
}

/// The ids of the conditions of a part of a source.
fn condition_ids(part: Option<&Value>) -> Vec<String> {
    part.and_then(|part| part.get("conditions"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|condition| condition.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Checks a `CollectionUpdateConditionsSourceInput` against the source it names.
fn check_conditions_update(
    state: &ShopState,
    source: &Record,
    update: &Map<String, Value>,
    path: &[String],
    errors: &mut Vec<UserError>,
) {
    if text(update, "title").is_some_and(|title| title.trim().is_empty()) {
        errors.push(error_at(&below(path, "title"), "Title can't be blank"));
    }
    let variants = field(source, "targetType") == Some("VARIANTS");
    for (name, exclusion) in [("inclusion", false), ("exclusion", true)] {
        let Some(part) = update.get(name).and_then(Value::as_object) else {
            continue;
        };
        let at = below(path, name);
        let known = condition_ids(source.get(name));
        for (index, id) in gids(part.get("conditionsToDelete")).iter().enumerate() {
            if !known.contains(id) {
                errors.push(error_at(
                    &below(&below(&at, "conditionsToDelete"), &index.to_string()),
                    "Condition does not exist",
                ));
            }
        }
        for (index, item) in objects(part, "conditionsToUpdate").iter().enumerate() {
            if !text(item, "id").is_some_and(|id| known.iter().any(|known| known == id)) {
                errors.push(error_at(
                    &below(
                        &below(&below(&at, "conditionsToUpdate"), &index.to_string()),
                        "id",
                    ),
                    "Condition does not exist",
                ));
            }
        }
        check_conditions(
            state,
            &objects(part, "conditionsToCreate"),
            &below(&at, "conditionsToCreate"),
            exclusion,
            errors,
        );
        for list in ["selectionsToAdd", "selectionsToRemove"] {
            check_selections(
                state,
                &objects(part, list),
                &below(&at, list),
                variants && !exclusion && list == "selectionsToAdd",
                errors,
            );
        }
    }
}

/// Applies a `CollectionUpdateConditionsSourceInput` to a source record.
fn apply_conditions_update(state: &mut ShopState, source: &str, update: &Map<String, Value>) {
    let Some(mut record) = state.get(source).cloned() else {
        return;
    };
    if let Some(title) = text(update, "title") {
        record.insert("title".into(), json!(title.trim()));
    }
    if update.contains_key("description") {
        record.insert("description".into(), json!(text(update, "description")));
    }
    let variants = field(&record, "targetType") == Some("VARIANTS");
    for (name, exclusion) in [("inclusion", false), ("exclusion", true)] {
        if let Some(part) = update.get(name).and_then(Value::as_object) {
            let mut current = record.get(name).cloned().unwrap_or(Value::Null);
            update_part(state, &mut current, part, exclusion, variants);
            record.insert(name.into(), current);
        }
    }
    if let Some(stored) = state.get_mut(source) {
        *stored = record;
    }
}

// --- The sources of a collection (`collectionCreate`, `collectionUpdate`) ---

/// Checks the `sources` of a `CollectionCreateInput` (or `sourcesToCreate`), each a
/// `CollectionCreateSourceTargetInput`, for a collection (`None` when it is being created).
pub fn check_targets(
    state: &ShopState,
    items: &[&Map<String, Value>],
    field: &str,
    collection: Option<&str>,
    errors: &mut Vec<UserError>,
) {
    for (index, item) in items.iter().enumerate() {
        let path = vec![field.to_string(), index.to_string()];
        if let Some(source) = item.get("source").and_then(Value::as_object) {
            check_conditions_source(state, source, &below(&path, "source"), errors);
        } else if let Some(sub) = item.get("subCollections").and_then(Value::as_object) {
            check_sub_collections(
                state,
                sub,
                &below(&path, "subCollections"),
                collection,
                errors,
            );
        } else if let Some(shared) = item.get("shareableSource").and_then(Value::as_object) {
            let id = text(shared, "sourceId").unwrap_or_default();
            let shareable = state.get(id).is_some_and(|source| {
                field_is(source, "__typename", CONDITIONS) && is_shareable(source)
            });
            if !shareable {
                // lsf's wording.
                errors.push(error_at(
                    &below(&below(&path, "shareableSource"), "sourceId"),
                    "Shareable source does not exist",
                ));
            }
        }
    }
}

fn is_shareable(source: &Record) -> bool {
    source.get("shareable").and_then(Value::as_bool) == Some(true)
}

fn field_is(record: &Record, name: &str, value: &str) -> bool {
    field(record, name) == Some(value)
}

/// Checks a sub-collection source: its collections exist, are not the collection itself and
/// have no sub-collection source of their own.
fn check_sub_collections(
    state: &ShopState,
    sub: &Map<String, Value>,
    path: &[String],
    collection: Option<&str>,
    errors: &mut Vec<UserError>,
) {
    if text(sub, "title").is_some_and(|title| title.trim().is_empty()) {
        errors.push(error_at(&below(path, "title"), "Title can't be blank"));
    }
    let ids = gids(sub.get("collectionIds"));
    if ids.len() > MAX_SUB_COLLECTIONS {
        errors.push(error_at(
            &below(path, "collectionIds"),
            format!("A source can include at most {MAX_SUB_COLLECTIONS} collections"),
        ));
    }
    for (index, id) in ids.iter().enumerate() {
        let at = below(&below(path, "collectionIds"), &index.to_string());
        match ineligibility(state, id, collection) {
            // The descriptions of `SubCollectionIneligibleReason`.
            Some("SELF_REFERENCE") => errors.push(error_at(
                &at,
                "The referenced collection is one of the source's owning collections (a source cannot reference itself).",
            )),
            Some("CHAIN_REFERENCE") => errors.push(error_at(
                &at,
                "The referenced collection itself owns a sub-collection source (chains of depth > 1 are not supported).",
            )),
            Some(_) => errors.push(error_at(
                &at,
                "The referenced collection id does not parse as a positive integer or does not exist in the shop.",
            )),
            None => {}
        }
    }
}

/// Why a collection cannot be a sub-collection of `owner`: a `SubCollectionIneligibleReason`.
fn ineligibility(state: &ShopState, target: &str, owner: Option<&str>) -> Option<&'static str> {
    if owner == Some(target) {
        return Some("SELF_REFERENCE");
    }
    let record = state.get_of(target, "Collection");
    let Some(record) = record else {
        return Some("INVALID_COLLECTION_REFERENCE");
    };
    let chained = gids(record.get("sources"))
        .iter()
        .filter_map(|source| state.get(source))
        .any(|source| field_is(source, "__typename", SUB_COLLECTIONS));
    chained.then_some("CHAIN_REFERENCE")
}

/// Creates the sources of a collection from `CollectionCreateSourceTargetInput`s: their GIDs
/// (a shareable source is linked, not copied).
pub fn create_targets(
    state: &mut ShopState,
    collection: &str,
    items: &[&Map<String, Value>],
) -> Vec<String> {
    let mut created = Vec::new();
    for item in items {
        if let Some(source) = item.get("source").and_then(Value::as_object) {
            created.push(create_conditions_source(
                state,
                source,
                Some(collection),
                None,
            ));
        } else if let Some(sub) = item.get("subCollections").and_then(Value::as_object) {
            let id = state.new_gid(SUB_COLLECTIONS);
            state.insert(value::object(json!({
                "__typename": SUB_COLLECTIONS,
                "id": id,
                "title": text(sub, "title").unwrap_or_default().trim(),
                "description": text(sub, "description"),
                "app": null,
                "collections": gids(sub.get("collectionIds")),
                OWNER: collection,
            })));
            created.push(id);
        } else if let Some(id) = item
            .get("shareableSource")
            .and_then(|shared| shared.get("sourceId"))
            .and_then(Value::as_str)
        {
            created.push(id.to_string());
        }
    }
    created
}

/// Checks the `sourcesToCreate`, `sourcesToUpdate` and `sourcesToDelete` of a
/// `CollectionUpdateInput` against the collection's sources.
pub fn check_update(
    state: &ShopState,
    collection: &str,
    input: &Map<String, Value>,
    errors: &mut Vec<UserError>,
) {
    let current = state
        .get(collection)
        .map(|record| gids(record.get("sources")))
        .unwrap_or_default();
    let deleted = gids(input.get("sourcesToDelete"));
    for (index, id) in deleted.iter().enumerate() {
        if !current.contains(id) {
            errors.push(UserError::new(
                &["sourcesToDelete", &index.to_string()],
                "Source does not exist",
            ));
        }
    }
    let created = objects(input, "sourcesToCreate");
    check_targets(state, &created, "sourcesToCreate", Some(collection), errors);
    for (index, item) in objects(input, "sourcesToUpdate").iter().enumerate() {
        let path = vec!["sourcesToUpdate".to_string(), index.to_string()];
        let (name, update) = match (
            item.get("condition").and_then(Value::as_object),
            item.get("subCollections").and_then(Value::as_object),
        ) {
            (Some(update), _) => ("condition", update),
            (_, Some(update)) => ("subCollections", update),
            _ => continue,
        };
        let at = below(&path, name);
        let id = text(update, "id").unwrap_or_default();
        let wanted = if name == "condition" {
            CONDITIONS
        } else {
            SUB_COLLECTIONS
        };
        let Some(source) = state.get(id).filter(|source| {
            current.iter().any(|known| known == id) && field_is(source, "__typename", wanted)
        }) else {
            errors.push(error_at(&below(&at, "id"), "Source does not exist"));
            continue;
        };
        if is_shareable(source) {
            // lsf's wording: a shareable source is its app's, changed with
            // `collectionConditionsSourceUpdate`.
            errors.push(error_at(
                &below(&at, "id"),
                "A shareable source is updated with collectionConditionsSourceUpdate",
            ));
        } else if name == "condition" {
            check_conditions_update(state, source, update, &at, errors);
        } else {
            check_sub_collections(state, update, &at, Some(collection), errors);
        }
    }
    let after =
        current.len() - deleted.iter().filter(|id| current.contains(id)).count() + created.len();
    if after > MAX_SOURCES {
        errors.push(UserError::new(
            &["sourcesToCreate"],
            format!("A collection can have at most {MAX_SOURCES} sources"),
        ));
    }
}

/// Applies a `CollectionUpdateInput`'s source changes to a collection. Whether they changed it.
pub fn apply_update(state: &mut ShopState, collection: &str, input: &Map<String, Value>) -> bool {
    let mut sources = state
        .get(collection)
        .map(|record| gids(record.get("sources")))
        .unwrap_or_default();
    let mut changed = false;
    // A collection of the legacy model that gets sources keeps what it had as its first source.
    if sources.is_empty()
        && !objects(input, "sourcesToCreate").is_empty()
        && let Some(legacy) = state.get(collection).map(legacy_source)
    {
        sources.push(materialize(state, collection, legacy));
        if let Some(record) = state.get_mut(collection) {
            record.insert("ruleSet".into(), Value::Null);
        }
    }
    for id in gids(input.get("sourcesToDelete")) {
        sources.retain(|known| *known != id);
        unlink(state, &id, collection);
        changed = true;
    }
    for item in objects(input, "sourcesToUpdate") {
        if let Some(update) = item.get("condition").and_then(Value::as_object) {
            let id = text(update, "id").unwrap_or_default().to_string();
            apply_conditions_update(state, &id, update);
            changed = true;
        } else if let Some(update) = item.get("subCollections").and_then(Value::as_object) {
            let id = text(update, "id").unwrap_or_default().to_string();
            if let Some(record) = state.get_mut(&id) {
                if let Some(title) = text(update, "title") {
                    record.insert("title".into(), json!(title.trim()));
                }
                if update.contains_key("description") {
                    record.insert("description".into(), json!(text(update, "description")));
                }
                if update.contains_key("collectionIds") {
                    record.insert(
                        "collections".into(),
                        json!(gids(update.get("collectionIds"))),
                    );
                }
            }
            changed = true;
        }
    }
    let created = create_targets(state, collection, &objects(input, "sourcesToCreate"));
    changed |= !created.is_empty();
    sources.extend(created);
    if changed && let Some(record) = state.get_mut(collection) {
        record.insert("sources".into(), json!(sources));
    }
    changed
}

/// Makes a built source (a legacy collection's) a record of the collection, its conditions
/// with ids of their own. Its GID.
fn materialize(state: &mut ShopState, collection: &str, source: Value) -> String {
    let Value::Object(mut record) = source else {
        return String::new();
    };
    let id = state.new_gid(CONDITIONS);
    record.insert("id".into(), json!(id));
    record.insert(OWNER.into(), json!(collection));
    renumber_conditions(state, &mut record, "inclusion");
    state.insert(record);
    id
}

/// Gives the conditions of a part of a source record (`inclusion`, `exclusion`) new ids of their
/// types.
fn renumber_conditions(state: &mut ShopState, record: &mut Record, part: &str) {
    if let Some(Value::Array(conditions)) = record
        .get_mut(part)
        .and_then(|part| part.get_mut("conditions"))
    {
        for condition in conditions.iter_mut() {
            let type_name = condition
                .get("__typename")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            condition["id"] = json!(state.new_gid(&type_name));
        }
    }
}

/// Takes a source out of a collection: a collection's own source is deleted with it, a
/// shareable one stays.
fn unlink(state: &mut ShopState, source: &str, collection: &str) {
    let owned = state
        .get(source)
        .is_some_and(|record| field(record, OWNER) == Some(collection));
    if owned {
        state.remove(source);
    }
}

/// Deletes the sources a collection owns: what deleting the collection does.
pub fn delete_owned(state: &mut ShopState, collection: &str) {
    let sources = state
        .get(collection)
        .map(|record| gids(record.get("sources")))
        .unwrap_or_default();
    for source in sources {
        unlink(state, &source, collection);
    }
}

/// The collections that link a source.
fn linking(state: &ShopState, source: &str) -> Vec<String> {
    state
        .keys("Collection")
        .filter(|collection| {
            state
                .get(collection)
                .is_some_and(|record| gids(record.get("sources")).iter().any(|s| s == source))
        })
        .map(str::to_string)
        .collect()
}

/// Notes what lsf does not simulate of sources' inputs: categories, variant targeting.
pub fn note_inputs(mutation: &mut Mutation<'_>, name: &str, value: &Value) {
    let text = value.to_string();
    if text.contains("\"productCategory\"") {
        mutation.note(format!(
            "{name}: lsf keeps no product category: a `productCategory` condition matches no product"
        ));
    }
    if text.contains("\"VARIANTS\"") {
        mutation.note(format!(
            "{name}: lsf keeps the membership of a variant-scoped source by product: a product is in when one of its variants matches"
        ));
    }
}

// --- Mutations ---

/// The app of the request, which owns the shareable sources it makes.
fn app_of(state: &ShopState) -> Option<String> {
    state.keys("App").next().map(str::to_string)
}

/// The shareable source of an id, the app's.
fn shareable(state: &ShopState, id: &str) -> bool {
    state.get(id).is_some_and(|source| {
        field_is(source, "__typename", CONDITIONS)
            && is_shareable(source)
            && source
                .get("app")
                .and_then(Value::as_str)
                .map(str::to_string)
                == app_of(state)
    })
}

/// `collectionConditionsSourceCreate`: a shareable source of the app, linked to no collection.
fn source_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input(ctx, "input");
    let mut errors = Vec::new();
    check_conditions_source(
        mutation.state(),
        &input,
        &["input".to_string()],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"source": null}), &errors));
    }
    note_inputs(
        mutation,
        "collectionConditionsSourceCreate",
        &Value::Object(input.clone()),
    );
    let app = app_of(mutation.state());
    let state = mutation.state_mut();
    let source = create_conditions_source(state, &input, None, app);
    // Shopify has no topic for collection sources.
    state.emit("collection_sources/create", &source);
    Ok(payload(json!({"source": source}), &[]))
}

/// `collectionConditionsSourceUpdate`: the source changes, and every collection that links it.
fn source_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input(ctx, "input");
    let state = mutation.state();
    let id = text(&input, "id").unwrap_or_default().to_string();
    if !shareable(state, &id) {
        // lsf's wording.
        let error = UserError::new(&["input", "id"], "Source does not exist");
        return Ok(payload(json!({"source": null}), &[error]));
    }
    let mut errors = Vec::new();
    if let Some(source) = state.get(&id) {
        check_conditions_update(state, source, &input, &["input".to_string()], &mut errors);
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"source": null}), &errors));
    }
    note_inputs(
        mutation,
        "collectionConditionsSourceUpdate",
        &Value::Object(input.clone()),
    );
    let state = mutation.state_mut();
    apply_conditions_update(state, &id, &input);
    state.emit("collection_sources/update", &id);
    for collection in linking(state, &id) {
        super::collections::reevaluate(state, &collection);
    }
    Ok(payload(json!({"source": id}), &[]))
}

/// `collectionConditionsSourceDelete`: the source goes, unlinked from every collection.
fn source_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !shareable(state, &id) {
        let error = UserError::new(&["id"], "Source does not exist");
        return Ok(payload(json!({"deletedId": null}), &[error]));
    }
    let collections = linking(state, &id);
    let state = mutation.state_mut();
    for collection in &collections {
        if let Some(Value::Array(sources)) = state
            .get_mut(collection)
            .and_then(|record| record.get_mut("sources"))
        {
            sources.retain(|source| source.as_str() != Some(id.as_str()));
        }
    }
    state.delete(&id, "collection_sources/delete");
    for collection in &collections {
        super::collections::reevaluate(state, collection);
    }
    Ok(payload(json!({"deletedId": id}), &[]))
}

// --- Queries ---

/// The shareable sources of an app.
fn shared_sources(state: &ShopState, app: &str) -> Vec<String> {
    state
        .all(CONDITIONS)
        .filter(|source| is_shareable(source) && field(source, "app") == Some(app))
        .filter_map(|source| field(source, "id"))
        .map(str::to_string)
        .collect()
}

/// `collectionConditionsSources(appId:)`: the shareable sources of an app.
fn sources_of_app(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let app = ctx.arg_str("appId").unwrap_or_default();
    let state = r.state();
    let items: Vec<Value> = shared_sources(state, app)
        .into_iter()
        .map(Value::String)
        .collect();
    r.connection(ctx, items)
}

/// `collectionConditionsSourcesByApp`: each app with shareable sources, and its sources.
fn sources_by_app(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let mut apps: Vec<String> = Vec::new();
    for source in state.all(CONDITIONS) {
        if is_shareable(source)
            && let Some(app) = field(source, "app")
            && !apps.iter().any(|known| known == app)
        {
            apps.push(app.to_string());
        }
    }
    let items: Vec<Value> = apps
        .iter()
        .map(|app| {
            let sources = shared_sources(state, app);
            json!({"__typename": "CollectionConditionsSourcesByApp", "id": app, "app": app, "sources": sources})
        })
        .collect();
    r.connection(ctx, items)
}

/// `CollectionConditionsSource.products`: the products the source resolves to.
fn source_products(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let Some(source) = r.map_of(object) else {
        return r.connection(ctx, Vec::new());
    };
    let items: Vec<Value> = state
        .keys("Product")
        .filter(|product| {
            state
                .get(product)
                .is_some_and(|record| source_member(state, source, record))
        })
        .map(|product| json!(product))
        .collect();
    r.connection(ctx, items)
}

/// `Collection.sources`: its source records, or the one source a collection of the legacy model
/// amounts to (its rules as typed conditions, or its products as selections), built.
fn collection_sources(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(record) = r.map_of(object) else {
        return Ok(Output::List(Vec::new()));
    };
    if has_sources(record) {
        return r.generic(ctx, object);
    }
    let legacy = legacy_source(record);
    r.convert(ctx, &json!([legacy]), false)
}

/// The source of a collection of the legacy model.
fn legacy_source(record: &Record) -> Value {
    let collection = field(record, "id").unwrap_or_default();
    let id = gid(
        CONDITIONS,
        lsf_core::util::stable_id("collection_source", collection),
    );
    let title = field(record, "title").unwrap_or_default();
    let (match_type, conditions, selections) =
        match record.get("ruleSet").filter(|rules| !rules.is_null()) {
            Some(rule_set) => {
                let disjunctive = rule_set
                    .get("appliedDisjunctively")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let conditions: Vec<Value> = rule_set
                    .get("rules")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, rule)| legacy_condition(collection, index, rule))
                    .collect();
                (
                    if disjunctive { "ANY" } else { "ALL" },
                    conditions,
                    Vec::new(),
                )
            }
            None => (
                "ALL",
                Vec::new(),
                gids(record.get("products"))
                    .iter()
                    .map(|product| json!({"product": product, "variantIds": null}))
                    .collect(),
            ),
        };
    json!({
        "__typename": CONDITIONS,
        "id": id,
        "title": title,
        "description": null,
        "app": null,
        "shareable": false,
        "targetType": "PRODUCTS",
        "inclusion": {"matchType": match_type, "conditions": conditions, "selections": selections},
        "exclusion": null,
    })
}

/// A legacy rule (`column`, `relation`, `condition`) as the typed condition it is.
fn legacy_condition(collection: &str, index: usize, rule: &Value) -> Value {
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
    let typed = |suffix: &str, fields: Value| {
        let type_name = format!("CollectionSourceInclusionCondition{suffix}");
        let mut object = json!({
            "__typename": type_name,
            "id": gid(&type_name, lsf_core::util::stable_id("collection_condition", &format!("{collection}/{index}"))),
        });
        if let (Value::Object(object), Value::Object(fields)) = (&mut object, fields) {
            object.extend(fields);
        }
        object
    };
    let text_condition = |suffix: &str| {
        let relation = match relation {
            "NOT_CONTAINS" => "DOES_NOT_CONTAIN",
            other => other,
        };
        typed(
            suffix,
            json!({"relation": relation, "matchType": "ANY", "values": [condition]}),
        )
    };
    let money = json!({"amount": condition, "currencyCode": null});
    match column {
        "TAG" => typed(
            "ProductTag",
            json!({"relation": if relation == "NOT_EQUALS" { "NOT_TAGGED_WITH" } else { "TAGGED_WITH" }, "matchType": "ANY", "values": [condition]}),
        ),
        "TITLE" => text_condition("ProductTitle"),
        "TYPE" => text_condition("ProductType"),
        "VENDOR" => text_condition("ProductVendor"),
        "VARIANT_TITLE" => text_condition("VariantTitle"),
        "VARIANT_PRICE" => typed(
            "VariantPrice",
            json!({"relation": relation, "value": money}),
        ),
        "VARIANT_COMPARE_AT_PRICE" => typed(
            "VariantCompareAtPrice",
            json!({"relation": relation, "value": money}),
        ),
        "IS_PRICE_REDUCED" => typed(
            "VariantCompareAtPrice",
            json!({"relation": relation, "value": null}),
        ),
        "VARIANT_INVENTORY" => typed(
            "VariantInventory",
            json!({"relation": relation, "value": condition.trim().parse::<i64>().unwrap_or(0)}),
        ),
        "VARIANT_WEIGHT" => typed(
            "VariantWeight",
            json!({"relation": relation, "value": {"value": condition.trim().parse::<f64>().unwrap_or(0.0), "unit": "KILOGRAMS"}}),
        ),
        _ => typed(
            "Unknown",
            json!({"relation": relation, "matchType": "ANY", "values": [condition]}),
        ),
    }
}

/// `Collection.subCollectionEligibility`: whether the collection can be another's
/// sub-collection (it has no sub-collection source of its own).
fn sub_collection_eligibility(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let key = object.key().unwrap_or_default();
    let reason = ineligibility(state, key, None);
    let state_of =
        |reason: Option<&str>| json!({"eligible": reason.is_none(), "ineligibleReason": reason});
    Ok(Output::Object(Obj::built(
        "CollectionSubCollectionEligibility",
        value::object(json!({"inclusion": state_of(reason), "exclusion": state_of(reason)})),
    )))
}

/// Copies the sources of a collection onto another (`collectionDuplicate`): its own sources
/// with new ids, the shareable ones linked. The copies' GIDs, in order.
pub fn copy_sources(state: &mut ShopState, from: &str, to: &str) -> Vec<String> {
    let sources = state
        .get(from)
        .map(|record| gids(record.get("sources")))
        .unwrap_or_default();
    let mut copies = Vec::new();
    for source in sources {
        let Some(mut record) = state.get(&source).cloned() else {
            continue;
        };
        if is_shareable(&record) {
            copies.push(source);
            continue;
        }
        let type_name = field(&record, "__typename")
            .unwrap_or(CONDITIONS)
            .to_string();
        let id = state.new_gid(&type_name);
        record.insert("id".into(), json!(id));
        record.insert(OWNER.into(), json!(to));
        for part in ["inclusion", "exclusion"] {
            renumber_conditions(state, &mut record, part);
        }
        state.insert(record);
        copies.push(id);
    }
    copies
}

//! Discounts: the app (Function) discounts the local checkout runs, and Shopify's own types
//! (basic, buy X get Y, free shipping), kept as records.
//!
//! - **Records**: a `DiscountAutomaticNode` (its `automaticDiscount` inline) or a
//!   `DiscountCodeNode` (its `codeDiscount`, with its `codes` inline), and the node's
//!   `metafields`, in the shape the seed gives the discounts of `checkout.discounts`: the store
//!   sync writes the active app discounts back there (the Function of
//!   `appDiscountType.functionId`, the title, the first code, the classes, the metafields), and
//!   the local checkout runs them. The other types are records only: the checkout runs Functions.
//! - **Queries**: `discountNodes` and `discountNodesCount` (each node as a `DiscountNode`, whose
//!   id is `DiscountNode/<n>`, the number the automatic and code nodes share), `discountNode`
//!   (by any of the three GIDs), `codeDiscountNodeByCode`, `discountCodesCount`, and the
//!   deprecated `automaticDiscountNodes`/`codeDiscountNodes`, which the generic rules answer.
//!   Their `sortKey`s (`TITLE`, `STARTS_AT`, `ENDS_AT`, `CREATED_AT`, `UPDATED_AT`) and search
//!   filters (`status`, `type`, `discount_type`, `method`, `title`, `code`, `tag`, the dates,
//!   `discount_class`, `combines_with`, `times_used`) read the discount inside the node.
//! - **Status** is the dates' (shopify.dev's examples of `discountAutomaticActivate` and
//!   `…Deactivate`): `SCHEDULED` before `startsAt`, `EXPIRED` from `endsAt`, `ACTIVE` between,
//!   by the shop's clock, whenever it is read.
//! - **Mutations**: create and update of each type, `…Activate` (a discount that has not started
//!   starts now, an expired one gets no end), `…Deactivate` (it ends now; one that has not
//!   started starts and ends now), `…Delete`, `discountAutomaticBulkDelete`,
//!   `discountCodeBulkActivate`/`Deactivate`/`Delete`, `discountRedeemCodeBulkAdd`,
//!   `discountCodeRedeemCodeBulkDelete`. The bulk ones are done at once: their `Job` is `done`.
//! - **Events**: `discounts/create`, `discounts/update`, `discounts/delete`, and one
//!   `discounts/redeemcode_added` (or `…_removed`) per code.
//!
//! Messages: "Code must be unique. Please try a different code." (`TAKEN`) and "Function not
//! found." are Shopify's, quoted in forum answers; "Function {id} not found. Ensure that it is
//! released in the current app ({client id}), and that the app is installed." is the 2026
//! wording quoted on community.shopify.com, which lsf answers for a `functionId` (and, by
//! analogy, a `functionHandle`). The bulk mutations' "Only one of …" are shopify.dev's examples.
//! "Ends at needs to be after starts_at" is the error of the REST price rules, whose GraphQL
//! wording is not published; the others are Rails' ("Title can't be blank").

use serde_json::{Map, Value, json};

use super::functions::{FunctionProblem, function_of};
use super::metafields;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::sort::SortValue;
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::{Query, SearchValue, Searchable};
use crate::state::{Record, ShopState, gid, parse_gid, value};

/// The API types of the Functions a discount runs: the Discount API, and the legacy product,
/// order and shipping discount APIs.
const DISCOUNT_APIS: &[&str] = &[
    "discounts",
    "product_discounts",
    "order_discounts",
    "shipping_discounts",
];

/// The types of the discounts inside the nodes.
const DISCOUNT_TYPES: &[&str] = &[
    "DiscountAutomaticApp",
    "DiscountAutomaticBasic",
    "DiscountAutomaticBxgy",
    "DiscountAutomaticFreeShipping",
    "DiscountCodeApp",
    "DiscountCodeBasic",
    "DiscountCodeBxgy",
    "DiscountCodeFreeShipping",
];

const NODE_TYPES: &[&str] = &["DiscountNode", "DiscountAutomaticNode", "DiscountCodeNode"];

/// The most tags a discount takes (`TOO_MANY_TAGS`: "The number of tags exceeds the maximum of
/// 5.").
const MAX_TAGS: usize = 5;

/// The most codes `discountRedeemCodeBulkAdd` takes ("Maximum: 250 codes.").
const MAX_CODES: usize = 250;

macro_rules! search_fields {
    ($registry:expr, $($field:literal),* $(,)?) => {
        for node in NODE_TYPES {
            $(
                $registry.search_field(node, $field, |r, _item, record| {
                    search_values(r.state(), record, $field)
                });
            )*
        }
    };
}

macro_rules! sort_keys {
    ($registry:expr, $(($key:literal, $field:literal)),* $(,)?) => {
        for node in NODE_TYPES {
            $(
                $registry.sort_key(node, $key, |r, item| {
                    SortValue::of(
                        r.item_map(item)
                            .and_then(inner)
                            .and_then(|discount| discount.get($field)),
                    )
                });
            )*
        }
    };
}

pub fn register(registry: &mut Registry) {
    registry
        .query("discountNodes", discount_nodes)
        .query("discountNodesCount", discount_nodes_count)
        .query("discountNode", discount_node)
        .query("codeDiscountNodeByCode", code_discount_node_by_code)
        .query("discountCodesCount", discount_codes_count)
        .default_text("DiscountRedeemCode", |_r, code| {
            code.get("code")
                .and_then(Value::as_str)
                .map(str::to_string)
                .into_iter()
                .collect()
        })
        .mutation("discountAutomaticAppCreate", |m, ctx| {
            create(m, ctx, &AUTOMATIC_APP)
        })
        .mutation("discountCodeAppCreate", |m, ctx| create(m, ctx, &CODE_APP))
        .mutation("discountAutomaticAppUpdate", |m, ctx| {
            update(m, ctx, &AUTOMATIC_APP)
        })
        .mutation("discountCodeAppUpdate", |m, ctx| update(m, ctx, &CODE_APP))
        .mutation("discountAutomaticBasicCreate", |m, ctx| {
            create(m, ctx, &AUTOMATIC_BASIC)
        })
        .mutation("discountAutomaticBasicUpdate", |m, ctx| {
            update(m, ctx, &AUTOMATIC_BASIC)
        })
        .mutation("discountAutomaticBxgyCreate", |m, ctx| {
            create(m, ctx, &AUTOMATIC_BXGY)
        })
        .mutation("discountAutomaticBxgyUpdate", |m, ctx| {
            update(m, ctx, &AUTOMATIC_BXGY)
        })
        .mutation("discountAutomaticFreeShippingCreate", |m, ctx| {
            create(m, ctx, &AUTOMATIC_FREE_SHIPPING)
        })
        .mutation("discountAutomaticFreeShippingUpdate", |m, ctx| {
            update(m, ctx, &AUTOMATIC_FREE_SHIPPING)
        })
        .mutation("discountCodeBasicCreate", |m, ctx| {
            create(m, ctx, &CODE_BASIC)
        })
        .mutation("discountCodeBasicUpdate", |m, ctx| {
            update(m, ctx, &CODE_BASIC)
        })
        .mutation("discountCodeBxgyCreate", |m, ctx| {
            create(m, ctx, &CODE_BXGY)
        })
        .mutation("discountCodeBxgyUpdate", |m, ctx| {
            update(m, ctx, &CODE_BXGY)
        })
        .mutation("discountCodeFreeShippingCreate", |m, ctx| {
            create(m, ctx, &CODE_FREE_SHIPPING)
        })
        .mutation("discountCodeFreeShippingUpdate", |m, ctx| {
            update(m, ctx, &CODE_FREE_SHIPPING)
        })
        .mutation("discountAutomaticActivate", |m, ctx| {
            activate(m, ctx, true, true)
        })
        .mutation("discountAutomaticDeactivate", |m, ctx| {
            activate(m, ctx, true, false)
        })
        .mutation("discountCodeActivate", |m, ctx| {
            activate(m, ctx, false, true)
        })
        .mutation("discountCodeDeactivate", |m, ctx| {
            activate(m, ctx, false, false)
        })
        .mutation("discountAutomaticDelete", |m, ctx| delete(m, ctx, true))
        .mutation("discountCodeDelete", |m, ctx| delete(m, ctx, false))
        .mutation("discountAutomaticBulkDelete", |m, ctx| {
            bulk(m, ctx, true, Bulk::Delete)
        })
        .mutation("discountCodeBulkDelete", |m, ctx| {
            bulk(m, ctx, false, Bulk::Delete)
        })
        .mutation("discountCodeBulkActivate", |m, ctx| {
            bulk(m, ctx, false, Bulk::Activate)
        })
        .mutation("discountCodeBulkDeactivate", |m, ctx| {
            bulk(m, ctx, false, Bulk::Deactivate)
        })
        .mutation("discountRedeemCodeBulkAdd", redeem_codes_add)
        .mutation("discountCodeRedeemCodeBulkDelete", redeem_codes_delete);
    for type_name in DISCOUNT_TYPES {
        registry
            .field(type_name, "status", |r, _ctx, object| {
                let now = r.state().timestamp();
                Ok(Output::value(
                    r.map_of(object)
                        .map_or("ACTIVE", |discount| status_at(discount, &now)),
                ))
            })
            .field(type_name, "discountClass", |r, ctx, object| {
                match r
                    .map_of(object)
                    .and_then(|discount| discount.get("discountClasses"))
                    .and_then(Value::as_array)
                    .and_then(|classes| classes.first())
                {
                    Some(class) => Ok(Output::Value(class.clone())),
                    None => r.generic(ctx, object),
                }
            });
    }
    search_fields!(
        registry,
        "status",
        "type",
        "discount_type",
        "method",
        "title",
        "code",
        "tag",
        "starts_at",
        "ends_at",
        "created_at",
        "updated_at",
        "discount_class",
        "combines_with",
        "times_used",
    );
    for node in NODE_TYPES {
        registry.default_text(node, |_r, record| default_text(record));
    }
    sort_keys!(
        registry,
        ("TITLE", "title"),
        ("STARTS_AT", "startsAt"),
        ("ENDS_AT", "endsAt"),
        ("CREATED_AT", "createdAt"),
        ("UPDATED_AT", "updatedAt"),
    );
}

// ---- reading discounts ------------------------------------------------------------------

/// The discount inside a node record, or a `DiscountNode`'s: `automaticDiscount`,
/// `codeDiscount` or `discount`.
pub fn inner(record: &Record) -> Option<&Record> {
    ["automaticDiscount", "codeDiscount", "discount"]
        .iter()
        .find_map(|field| record.get(*field))
        .and_then(Value::as_object)
}

fn inner_mut(record: &mut Record) -> Option<&mut Record> {
    let field = ["automaticDiscount", "codeDiscount"]
        .into_iter()
        .find(|field| record.contains_key(*field))?;
    record.get_mut(field).and_then(Value::as_object_mut)
}

fn text<'r>(record: &'r Record, field: &str) -> Option<&'r str> {
    record.get(field).and_then(Value::as_str)
}

/// A `DateTime` as lsf writes it (`2026-04-29T06:45:06Z`, UTC, as Shopify answers a date given
/// with an offset), `None` when it is not one.
fn date_time(text: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|date| value::date_time(date.with_timezone(&chrono::Utc)))
}

/// The status of a discount at `now`: by its dates.
pub fn status_at(discount: &Record, now: &str) -> &'static str {
    let starts = text(discount, "startsAt").and_then(date_time);
    let ends = text(discount, "endsAt").and_then(date_time);
    if ends.as_deref().is_some_and(|ends| ends <= now) {
        "EXPIRED"
    } else if starts.as_deref().is_some_and(|starts| starts > now) {
        "SCHEDULED"
    } else {
        "ACTIVE"
    }
}

/// The node of the number of a discount GID (`DiscountNode/<n>`, `DiscountAutomaticNode/<n>`,
/// `DiscountCodeNode/<n>`), among the node types given.
fn node_of(state: &ShopState, id: &str, types: &[&str]) -> Option<String> {
    let (kind, number) = parse_gid(id)?;
    if !NODE_TYPES.contains(&kind) {
        return None;
    }
    types
        .iter()
        .map(|type_name| gid(type_name, number))
        .find(|key| state.contains(key))
}

/// The `DiscountNode` of a node record: the same discount under the id the legacy APIs read.
fn as_discount_node(key: &str, record: &Record) -> Option<Record> {
    let (_, number) = parse_gid(key)?;
    let mut node = Record::new();
    node.insert("__typename".into(), json!("DiscountNode"));
    node.insert("id".into(), json!(gid("DiscountNode", number)));
    node.insert("discount".into(), Value::Object(inner(record)?.clone()));
    node.insert(
        "metafields".into(),
        record
            .get("metafields")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    Some(node)
}

/// Every node as a `DiscountNode`, in creation order.
fn discount_node_items(r: &ShopResolver<'_>) -> Vec<Value> {
    let state = r.state();
    let mut keys: Vec<&str> = state
        .keys("DiscountAutomaticNode")
        .chain(state.keys("DiscountCodeNode"))
        .collect();
    keys.sort_by_key(|key| parse_gid(key).map_or(0, |(_, id)| id));
    keys.into_iter()
        .filter_map(|key| as_discount_node(key, state.get(key)?))
        .map(Value::Object)
        .collect()
}

fn discount_nodes(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    if ctx.arg("savedSearchId").is_some_and(|id| !id.is_null()) {
        r.note("discountNodes: lsf has no saved searches: `savedSearchId` was ignored");
    }
    r.connection(ctx, discount_node_items(r))
}

fn discount_nodes_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = r.search(ctx, "DiscountNode", discount_node_items(r));
    Ok(r.count(ctx, items.len()))
}

fn discount_node(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let state = r.state();
    let node = node_of(state, id, &["DiscountAutomaticNode", "DiscountCodeNode"])
        .and_then(|key| as_discount_node(&key, state.get(&key)?));
    Ok(Output::object_or_null(
        node.map(|node| Obj::inline("DiscountNode", node)),
    ))
}

/// The code discount that has a code, ignoring case (Shopify's codes are not case-sensitive).
fn node_with_code(state: &ShopState, code: &str, except: Option<&str>) -> Option<String> {
    let code = code.trim();
    state
        .keys("DiscountCodeNode")
        .filter(|key| Some(*key) != except)
        .find(|key| {
            state
                .get(key)
                .and_then(inner)
                .and_then(|discount| discount.get("codes"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("code").and_then(Value::as_str))
                .any(|known| known.eq_ignore_ascii_case(code))
        })
        .map(str::to_string)
}

fn code_discount_node_by_code(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let code = ctx.arg_str("code").unwrap_or_default();
    Ok(Output::object_or_null(
        node_with_code(r.state(), code, None).map(|key| Obj::record(&key)),
    ))
}

/// `discountCodesCount`: the redeem codes of every code discount.
fn discount_codes_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let codes: Vec<Value> = state
        .all("DiscountCodeNode")
        .filter_map(inner)
        .filter_map(|discount| discount.get("codes").and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect();
    let items = r.search(ctx, "DiscountRedeemCode", codes);
    Ok(r.count(ctx, items.len()))
}

/// What a search filter of a discount reads (see the module's documentation).
fn search_values(state: &ShopState, record: &Record, field: &str) -> Option<Vec<SearchValue>> {
    let discount = inner(record)?;
    let kind = text(discount, "__typename").unwrap_or_default();
    let keywords = |values: Vec<String>| {
        Some(
            values
                .into_iter()
                .map(SearchValue::Keyword)
                .collect::<Vec<_>>(),
        )
    };
    let date = |name: &str| {
        Some(
            text(discount, name)
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                .map(|date| SearchValue::Date(date.with_timezone(&chrono::Utc)))
                .into_iter()
                .collect::<Vec<_>>(),
        )
    };
    match field {
        "status" => keywords(vec![
            status_at(discount, &state.timestamp()).to_ascii_lowercase(),
        ]),
        "type" | "discount_type" => {
            let mut types = vec![discount_type(discount).to_string()];
            if field == "type" && kind.ends_with("App") {
                types.push("all_with_app".to_string());
            } else if field == "type" {
                types.push("all".to_string());
                types.push("all_with_app".to_string());
            }
            keywords(types)
        }
        "method" => keywords(vec![
            if kind.starts_with("DiscountCode") {
                "code"
            } else {
                "automatic"
            }
            .to_string(),
        ]),
        "title" => Some(
            text(discount, "title")
                .map(|title| SearchValue::Text(title.to_string()))
                .into_iter()
                .collect(),
        ),
        "code" => keywords(
            discount
                .get("codes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|code| code.get("code").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
        ),
        "tag" => keywords(
            discount
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        "starts_at" => date("startsAt"),
        "ends_at" => date("endsAt"),
        "created_at" => date("createdAt"),
        "updated_at" => date("updatedAt"),
        "discount_class" => keywords(
            discount
                .get("discountClasses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect(),
        ),
        "combines_with" => {
            let combines = discount.get("combinesWith");
            keywords(
                [
                    ("orderDiscounts", "order_discounts"),
                    ("productDiscounts", "product_discounts"),
                    ("shippingDiscounts", "shipping_discounts"),
                ]
                .into_iter()
                .filter(|(flag, _)| {
                    combines
                        .and_then(|combines| combines.get(*flag))
                        .and_then(Value::as_bool)
                        == Some(true)
                })
                .map(|(_, name)| name.to_string())
                .collect(),
            )
        }
        "times_used" => Some(vec![SearchValue::Number(
            discount
                .get("asyncUsageCount")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        )]),
        _ => None,
    }
}

/// The `type` filter's value of a discount: `app`, `bogo`, `free_shipping`, `percentage` or
/// `fixed_amount` (the values shopify.dev lists for `discountNodes(query:)`).
fn discount_type(discount: &Record) -> &'static str {
    let kind = text(discount, "__typename").unwrap_or_default();
    if kind.ends_with("App") {
        "app"
    } else if kind.ends_with("Bxgy") {
        "bogo"
    } else if kind.ends_with("FreeShipping") {
        "free_shipping"
    } else if discount
        .get("customerGets")
        .and_then(|gets| gets.get("value"))
        .and_then(|value| value.get("__typename"))
        .and_then(Value::as_str)
        == Some("DiscountPercentage")
    {
        "percentage"
    } else {
        "fixed_amount"
    }
}

/// What a term without a field searches: the title and the codes.
fn default_text(record: &Record) -> Vec<String> {
    let Some(discount) = inner(record) else {
        return Vec::new();
    };
    let mut texts: Vec<String> = text(discount, "title")
        .map(str::to_string)
        .into_iter()
        .collect();
    texts.extend(
        discount
            .get("codes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|code| code.get("code").and_then(Value::as_str))
            .map(str::to_string),
    );
    texts
}

/// A discount node record as the search syntax sees it, for the bulk mutations' `search`.
struct Searched<'s> {
    state: &'s ShopState,
    record: &'s Record,
    id: u64,
}

impl Searchable for Searched<'_> {
    fn values(&self, field: &str) -> Option<Vec<SearchValue>> {
        if field == "id" {
            return Some(vec![SearchValue::Id(self.id)]);
        }
        search_values(self.state, self.record, field)
    }

    fn default_text(&self) -> Vec<String> {
        default_text(self.record)
    }
}

// ---- the kinds of discounts -------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    App,
    Basic,
    Bxgy,
    FreeShipping,
}

/// What a create or update mutation makes.
struct Kind {
    family: Family,
    automatic: bool,
    /// The type of the discount inside the node: `DiscountAutomaticApp`.
    type_name: &'static str,
    /// The input argument: `automaticAppDiscount`.
    input: &'static str,
    /// The payload's object: the discount itself for an app discount (`automaticAppDiscount`),
    /// the node for the others (`automaticDiscountNode`).
    payload_field: &'static str,
}

impl Kind {
    fn node_type(&self) -> &'static str {
        if self.automatic {
            "DiscountAutomaticNode"
        } else {
            "DiscountCodeNode"
        }
    }

    fn inner_field(&self) -> &'static str {
        if self.automatic {
            "automaticDiscount"
        } else {
            "codeDiscount"
        }
    }
}

const AUTOMATIC_APP: Kind = Kind {
    family: Family::App,
    automatic: true,
    type_name: "DiscountAutomaticApp",
    input: "automaticAppDiscount",
    payload_field: "automaticAppDiscount",
};
const CODE_APP: Kind = Kind {
    family: Family::App,
    automatic: false,
    type_name: "DiscountCodeApp",
    input: "codeAppDiscount",
    payload_field: "codeAppDiscount",
};
const AUTOMATIC_BASIC: Kind = Kind {
    family: Family::Basic,
    automatic: true,
    type_name: "DiscountAutomaticBasic",
    input: "automaticBasicDiscount",
    payload_field: "automaticDiscountNode",
};
const AUTOMATIC_BXGY: Kind = Kind {
    family: Family::Bxgy,
    automatic: true,
    type_name: "DiscountAutomaticBxgy",
    input: "automaticBxgyDiscount",
    payload_field: "automaticDiscountNode",
};
const AUTOMATIC_FREE_SHIPPING: Kind = Kind {
    family: Family::FreeShipping,
    automatic: true,
    type_name: "DiscountAutomaticFreeShipping",
    input: "freeShippingAutomaticDiscount",
    payload_field: "automaticDiscountNode",
};
const CODE_BASIC: Kind = Kind {
    family: Family::Basic,
    automatic: false,
    type_name: "DiscountCodeBasic",
    input: "basicCodeDiscount",
    payload_field: "codeDiscountNode",
};
const CODE_BXGY: Kind = Kind {
    family: Family::Bxgy,
    automatic: false,
    type_name: "DiscountCodeBxgy",
    input: "bxgyCodeDiscount",
    payload_field: "codeDiscountNode",
};
const CODE_FREE_SHIPPING: Kind = Kind {
    family: Family::FreeShipping,
    automatic: false,
    type_name: "DiscountCodeFreeShipping",
    input: "freeShippingCodeDiscount",
    payload_field: "codeDiscountNode",
};

// ---- building a discount from its input --------------------------------------------------

/// The app of the shop: its GID and client id, which an app discount's `appDiscountType`
/// names.
fn app_of(state: &ShopState) -> (Value, Value) {
    let app = state.all("App").next();
    (
        app.and_then(|app| app.get("id"))
            .cloned()
            .unwrap_or(Value::Null),
        app.and_then(|app| app.get("apiKey"))
            .cloned()
            .unwrap_or(Value::Null),
    )
}

fn combines_with(input: &Value) -> Value {
    let flag = |name: &str| input.get(name).and_then(Value::as_bool).unwrap_or(false);
    let tags = input
        .get("productDiscountsWithTagsOnSameCartLine")
        .and_then(|tags| tags.get("add"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "orderDiscounts": flag("orderDiscounts"),
        "productDiscounts": flag("productDiscounts"),
        "shippingDiscounts": flag("shippingDiscounts"),
        "productDiscountsWithTagsOnSameCartLine": tags,
    })
}

/// A `DiscountContext` from a `DiscountContextInput` (`@oneOf`): all buyers, customers,
/// customer segments or markets.
fn context(input: &Value, previous: Option<&Value>) -> Value {
    let ids = |value: Option<&Value>, previous_field: &str| -> Vec<Value> {
        let mut ids: Vec<Value> = previous
            .and_then(|previous| previous.get(previous_field))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let add = value
            .and_then(|value| value.get("add"))
            .and_then(Value::as_array);
        let remove = value
            .and_then(|value| value.get("remove"))
            .and_then(Value::as_array);
        for id in add.into_iter().flatten() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids.retain(|id| !remove.into_iter().flatten().any(|gone| gone == id));
        ids
    };
    if let Some(customers) = input.get("customers") {
        return json!({"__typename": "DiscountCustomers", "customers": ids(Some(customers), "customers")});
    }
    if let Some(segments) = input.get("customerSegments") {
        return json!({"__typename": "DiscountCustomerSegments", "segments": ids(Some(segments), "segments")});
    }
    if let Some(markets) = input.get("markets") {
        return json!({"__typename": "DiscountMarkets", "markets": ids(Some(markets), "markets")});
    }
    json!({"__typename": "DiscountBuyerSelectionAll", "all": "ALL"})
}

/// The products, variants or collections a discount applies to (`DiscountItemsInput`), the
/// lists changed from those it had.
fn items(input: &Value, previous: Option<&Value>) -> Value {
    if input.get("all").and_then(Value::as_bool) == Some(true) {
        return json!({"__typename": "AllDiscountItems", "allItems": true});
    }
    let merged = |field: &str, add: Option<&Value>, remove: Option<&Value>| -> Vec<Value> {
        let mut ids: Vec<Value> = previous
            .and_then(|previous| previous.get(field))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for id in add.and_then(Value::as_array).into_iter().flatten() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        let remove = remove.and_then(Value::as_array);
        ids.retain(|id| !remove.into_iter().flatten().any(|gone| gone == id));
        ids
    };
    if let Some(collections) = input.get("collections") {
        return json!({
            "__typename": "DiscountCollections",
            "collections": merged("collections", collections.get("add"), collections.get("remove")),
        });
    }
    if let Some(products) = input.get("products") {
        return json!({
            "__typename": "DiscountProducts",
            "products": merged("products", products.get("productsToAdd"), products.get("productsToRemove")),
            "productVariants": merged(
                "productVariants",
                products.get("productVariantsToAdd"),
                products.get("productVariantsToRemove"),
            ),
        });
    }
    previous
        .cloned()
        .unwrap_or_else(|| json!({"__typename": "AllDiscountItems", "allItems": true}))
}

/// A `DiscountCustomerGetsValue` from its input: a percentage, an amount, or an effect on a
/// quantity (buy X get Y).
fn gets_value(input: &Value, currency: &str) -> Value {
    if let Some(percentage) = input.get("percentage").and_then(Value::as_f64) {
        return json!({"__typename": "DiscountPercentage", "percentage": percentage});
    }
    if let Some(amount) = input.get("discountAmount") {
        let value = amount.get("amount").and_then(Value::as_str).unwrap_or("0");
        return json!({
            "__typename": "DiscountAmount",
            "amount": {"amount": decimal(value), "currencyCode": currency},
            "appliesOnEachItem": amount.get("appliesOnEachItem").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    if let Some(on_quantity) = input.get("discountOnQuantity") {
        let effect = on_quantity.get("effect").cloned().unwrap_or(Value::Null);
        let effect = match (
            effect.get("percentage").and_then(Value::as_f64),
            effect.get("amount").and_then(Value::as_str),
        ) {
            (Some(percentage), _) => {
                json!({"__typename": "DiscountPercentage", "percentage": percentage})
            }
            (None, Some(amount)) => json!({
                "__typename": "DiscountAmount",
                "amount": {"amount": decimal(amount), "currencyCode": currency},
                "appliesOnEachItem": false,
            }),
            _ => json!({"__typename": "DiscountPercentage", "percentage": 1.0}),
        };
        return json!({
            "__typename": "DiscountOnQuantity",
            "quantity": {"quantity": on_quantity.get("quantity").cloned().unwrap_or(json!("1"))},
            "effect": effect,
        });
    }
    json!({"__typename": "DiscountPercentage", "percentage": 0.0})
}

/// A `Decimal` input as `MoneyV2.amount` reads it: `"10.0"`.
fn decimal(text: &str) -> String {
    let parsed: f64 = text.trim().parse().unwrap_or(0.0);
    let cents = (parsed * 100.0).round() as i64;
    value::decimal(cents)
}

fn customer_gets(input: &Value, previous: Option<&Value>, currency: &str) -> Value {
    let flag = |name: &str, fallback: bool| {
        input
            .get(name)
            .and_then(Value::as_bool)
            .or_else(|| previous.and_then(|p| p.get(name)).and_then(Value::as_bool))
            .unwrap_or(fallback)
    };
    json!({
        "appliesOnOneTimePurchase": flag("appliesOnOneTimePurchase", true),
        "appliesOnSubscription": flag("appliesOnSubscription", false),
        "items": match input.get("items") {
            Some(items_input) => items(items_input, previous.and_then(|p| p.get("items"))),
            None => previous.and_then(|p| p.get("items")).cloned()
                .unwrap_or_else(|| json!({"__typename": "AllDiscountItems", "allItems": true})),
        },
        "value": match input.get("value") {
            Some(value) => gets_value(value, currency),
            None => previous.and_then(|p| p.get("value")).cloned()
                .unwrap_or_else(|| json!({"__typename": "DiscountPercentage", "percentage": 0.0})),
        },
    })
}

fn customer_buys(input: &Value, previous: Option<&Value>) -> Value {
    let value = input.get("value").cloned().unwrap_or(Value::Null);
    let value = match (value.get("quantity"), value.get("amount")) {
        (Some(quantity), _) if !quantity.is_null() => {
            json!({"__typename": "DiscountQuantity", "quantity": quantity})
        }
        (_, Some(amount)) if !amount.is_null() => {
            json!({"__typename": "DiscountPurchaseAmount", "amount": decimal(amount.as_str().unwrap_or("0"))})
        }
        _ => previous
            .and_then(|p| p.get("value"))
            .cloned()
            .unwrap_or_else(|| json!({"__typename": "DiscountQuantity", "quantity": "1"})),
    };
    json!({
        "isOneTimePurchase": input.get("isOneTimePurchase").and_then(Value::as_bool).unwrap_or(true),
        "isSubscription": input.get("isSubscription").and_then(Value::as_bool).unwrap_or(false),
        "items": match input.get("items") {
            Some(items_input) => items(items_input, previous.and_then(|p| p.get("items"))),
            None => previous.and_then(|p| p.get("items")).cloned()
                .unwrap_or_else(|| json!({"__typename": "AllDiscountItems", "allItems": true})),
        },
        "value": value,
    })
}

fn minimum_requirement(input: &Value, currency: &str) -> Value {
    if let Some(subtotal) = input
        .get("subtotal")
        .and_then(|subtotal| subtotal.get("greaterThanOrEqualToSubtotal"))
        .and_then(Value::as_str)
    {
        return json!({
            "__typename": "DiscountMinimumSubtotal",
            "greaterThanOrEqualToSubtotal": {"amount": decimal(subtotal), "currencyCode": currency},
        });
    }
    if let Some(quantity) = input
        .get("quantity")
        .and_then(|quantity| quantity.get("greaterThanOrEqualToQuantity"))
        .filter(|quantity| !quantity.is_null())
    {
        return json!({"__typename": "DiscountMinimumQuantity", "greaterThanOrEqualToQuantity": quantity});
    }
    Value::Null
}

fn destination(input: &Value, previous: Option<&Value>) -> Value {
    if input.get("all").and_then(Value::as_bool) == Some(true) {
        return json!({"__typename": "DiscountCountryAll", "allCountries": true});
    }
    if let Some(countries) = input.get("countries") {
        let mut list: Vec<Value> = previous
            .and_then(|previous| previous.get("countries"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for country in countries
            .get("add")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !list.contains(country) {
                list.push(country.clone());
            }
        }
        let remove = countries.get("remove").and_then(Value::as_array);
        list.retain(|country| !remove.into_iter().flatten().any(|gone| gone == country));
        return json!({
            "__typename": "DiscountCountries",
            "countries": list,
            "includeRestOfWorld": countries.get("includeRestOfWorld").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    previous
        .cloned()
        .unwrap_or_else(|| json!({"__typename": "DiscountCountryAll", "allCountries": true}))
}

/// A short summary of a discount of Shopify's types, in the spirit of the admin's ("10% off
/// entire order"): lsf's wording, the admin's is not published.
fn summary(discount: &Record) -> String {
    let kind = text(discount, "__typename").unwrap_or_default();
    if kind.ends_with("FreeShipping") {
        return "Free shipping on all products".to_string();
    }
    if kind.ends_with("Bxgy") {
        return "Buy X get Y".to_string();
    }
    let gets = discount.get("customerGets");
    let value = gets.and_then(|gets| gets.get("value"));
    let amount = match value
        .and_then(|value| value.get("__typename"))
        .and_then(Value::as_str)
    {
        Some("DiscountPercentage") => format!(
            "{}%",
            value
                .and_then(|value| value.get("percentage"))
                .and_then(Value::as_f64)
                .map_or(0.0, |p| p * 100.0)
                .round()
        ),
        _ => value
            .and_then(|value| value.pointer("/amount/amount"))
            .and_then(Value::as_str)
            .map_or_else(String::new, str::to_string),
    };
    let target = match gets
        .and_then(|gets| gets.pointer("/items/__typename"))
        .and_then(Value::as_str)
    {
        Some("AllDiscountItems") => "entire order",
        Some("DiscountCollections") => "collections",
        _ => "products",
    };
    format!("{amount} off {target}")
}

/// Normalizes a list of tags: trimmed, no blanks, no repeats.
fn tags_of(value: Option<&Value>) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let tag = tag.trim();
        if !tag.is_empty() && !tags.iter().any(|known| known.eq_ignore_ascii_case(tag)) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// The checks every discount input shares: title, dates, code, tags. `previous` is the
/// discount an update changes.
fn check_common(
    state: &ShopState,
    kind: &Kind,
    input: &Map<String, Value>,
    previous: Option<(&str, &Record)>,
    errors: &mut Vec<UserError>,
) {
    let at = |field: &'static str| vec![kind.input, field];
    let creating = previous.is_none();
    match input.get("title").and_then(Value::as_str) {
        Some(title) if title.trim().is_empty() => {
            errors.push(UserError::new(&at("title"), "Title can't be blank").code("BLANK"))
        }
        Some(title) if title.chars().count() > 255 => errors.push(
            UserError::new(
                &at("title"),
                "Title is too long (maximum is 255 characters)",
            )
            .code("TOO_LONG"),
        ),
        None if creating => {
            errors.push(UserError::new(&at("title"), "Title can't be blank").code("BLANK"))
        }
        _ => {}
    }
    for field in ["startsAt", "endsAt"] {
        if let Some(date) = input.get(field).and_then(Value::as_str)
            && date_time(date).is_none()
        {
            errors.push(UserError::new(&at(field), format!("{field} is invalid")).code("INVALID"));
        }
    }
    let previous_discount = previous.map(|(_, record)| record);
    let starts = input
        .get("startsAt")
        .and_then(Value::as_str)
        .and_then(date_time)
        .or_else(|| {
            previous_discount
                .and_then(|discount| text(discount, "startsAt"))
                .and_then(date_time)
        })
        .unwrap_or_else(|| state.timestamp());
    let ends = match input.get("endsAt") {
        Some(Value::String(date)) => date_time(date),
        Some(_) => None,
        None => previous_discount
            .and_then(|discount| text(discount, "endsAt"))
            .and_then(date_time),
    };
    if ends.as_deref().is_some_and(|ends| ends < starts.as_str()) {
        errors.push(
            UserError::new(&at("endsAt"), "Ends at needs to be after starts_at")
                .code("END_DATE_BEFORE_START_DATE"),
        );
    }
    if !kind.automatic {
        let code = input.get("code").and_then(Value::as_str).map(str::trim);
        match code {
            Some("") => {
                errors.push(UserError::new(&at("code"), "Code can't be blank").code("BLANK"))
            }
            None if creating => {
                errors.push(UserError::new(&at("code"), "Code can't be blank").code("BLANK"))
            }
            Some(code) if code.chars().count() > 255 => errors.push(
                UserError::new(&at("code"), "Code is too long (maximum is 255 characters)")
                    .code("TOO_LONG"),
            ),
            Some(code) if node_with_code(state, code, previous.map(|(key, _)| key)).is_some() => {
                errors.push(
                    UserError::new(
                        &at("code"),
                        "Code must be unique. Please try a different code.",
                    )
                    .code("TAKEN"),
                );
            }
            _ => {}
        }
        if let Some(limit) = input.get("usageLimit").and_then(Value::as_i64)
            && limit < 1
        {
            errors.push(
                UserError::new(&at("usageLimit"), "Usage limit must be greater than 0")
                    .code("GREATER_THAN"),
            );
        }
    }
    // The products, variants and collections the discount names must exist: a connection of
    // them would otherwise name a record that is not there.
    for part in ["customerGets", "customerBuys"] {
        let Some(items) = input.get(part).and_then(|part| part.get("items")) else {
            continue;
        };
        for (group, field, type_name) in [
            ("products", "productsToAdd", "Product"),
            ("products", "productVariantsToAdd", "ProductVariant"),
            ("collections", "add", "Collection"),
        ] {
            let ids = items
                .get(group)
                .and_then(|group| group.get(field))
                .and_then(Value::as_array);
            for id in ids.into_iter().flatten().filter_map(Value::as_str) {
                if state.type_of(id) != Some(type_name) {
                    errors.push(
                        UserError::new(
                            &[kind.input, part, "items", group, field],
                            format!("{type_name} {id} does not exist"),
                        )
                        .code("INVALID"),
                    );
                }
            }
        }
    }
    if tags_of(input.get("tags")).len() > MAX_TAGS {
        errors.push(
            UserError::new(&at("tags"), "The number of tags exceeds the maximum of 5.")
                .code("TOO_MANY_TAGS"),
        );
    }
}

/// The Function of an app discount input, with its error in the discount's words.
fn discount_function(
    state: &ShopState,
    kind: &Kind,
    input: &Map<String, Value>,
) -> Result<super::functions::FunctionRef, UserError> {
    let client_id = app_of(state).1;
    let client_id = client_id.as_str().unwrap_or_default();
    function_of(state, input, DISCOUNT_APIS).map_err(|problem| match problem {
        FunctionProblem::Missing => UserError::new(
            &[kind.input, "functionHandle"],
            "Either function ID or function handle must be provided.",
        )
        .code("MISSING_FUNCTION_IDENTIFIER"),
        FunctionProblem::Multiple => UserError::new(
            &[kind.input, "functionHandle"],
            "Only one of function ID or function handle is allowed.",
        )
        .code("MULTIPLE_FUNCTION_IDENTIFIERS"),
        FunctionProblem::NotFound { field, value } => UserError::new(
            &[kind.input, field],
            format!(
                "Function {value} not found. Ensure that it is released in the current app ({client_id}), and that the app is installed."
            ),
        )
        .code("INVALID"),
        FunctionProblem::DoesNotImplement { field, api_type } => UserError::new(
            &[kind.input, field],
            format!(
                "Function {} is not a discount function: it implements the {api_type} API.",
                input
                    .get(field)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ),
        )
        .code("INVALID"),
    })
}

/// The discount classes of an app discount: those given, else those of its legacy API
/// (`product_discounts` is `PRODUCT`), else all three (*chosen*, as the seed does for a discount
/// of the data that names none).
fn app_classes(input: &Map<String, Value>, api_type: &str, previous: Option<&Value>) -> Value {
    if let Some(classes) = input
        .get("discountClasses")
        .filter(|value| !value.is_null())
    {
        return classes.clone();
    }
    if let Some(previous) = previous {
        return previous.clone();
    }
    match api_type {
        "product_discounts" => json!(["PRODUCT"]),
        "order_discounts" => json!(["ORDER"]),
        "shipping_discounts" => json!(["SHIPPING"]),
        _ => json!(["PRODUCT", "ORDER", "SHIPPING"]),
    }
}

/// Sets on `discount` the fields of `input` that all kinds share, and those of its kind.
#[allow(clippy::too_many_arguments)]
fn apply_input(
    state: &ShopState,
    kind: &Kind,
    input: &Map<String, Value>,
    discount: &mut Record,
    function: Option<&super::functions::FunctionRef>,
    node: &str,
    now: &str,
    creating: bool,
) {
    let currency = state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();
    if let Some(title) = input.get("title").and_then(Value::as_str) {
        discount.insert("title".into(), json!(title.trim()));
    }
    if let Some(starts) = input
        .get("startsAt")
        .and_then(Value::as_str)
        .and_then(date_time)
    {
        discount.insert("startsAt".into(), json!(starts));
    } else if creating {
        discount.insert("startsAt".into(), json!(now));
    }
    match input.get("endsAt") {
        Some(Value::String(ends)) => {
            discount.insert("endsAt".into(), json!(date_time(ends)));
        }
        Some(_) => {
            discount.insert("endsAt".into(), Value::Null);
        }
        None if creating => {
            discount.insert("endsAt".into(), Value::Null);
        }
        None => {}
    }
    if let Some(combines) = input.get("combinesWith") {
        discount.insert("combinesWith".into(), combines_with(combines));
    } else if creating {
        discount.insert("combinesWith".into(), combines_with(&Value::Null));
    }
    if let Some(context_input) = input.get("context").filter(|value| !value.is_null()) {
        let merged = context(context_input, discount.get("context"));
        discount.insert("context".into(), merged);
    } else if creating {
        discount.insert("context".into(), context(&Value::Null, None));
    }
    if input.contains_key("tags") {
        discount.insert("tags".into(), json!(tags_of(input.get("tags"))));
    } else if creating {
        discount.insert("tags".into(), json!([]));
    }
    if let Some(limit) = input.get("recurringCycleLimit").and_then(Value::as_i64) {
        discount.insert("recurringCycleLimit".into(), json!(limit));
    } else if creating {
        discount.insert(
            "recurringCycleLimit".into(),
            if kind.family == Family::App {
                json!(1)
            } else {
                Value::Null
            },
        );
    }
    if !kind.automatic {
        if let Some(code) = input.get("code").and_then(Value::as_str) {
            let code = code.trim();
            let codes = discount
                .get("codes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let created = codes
                .first()
                .and_then(|first| first.get("createdAt"))
                .cloned()
                .unwrap_or_else(|| json!(now));
            let mut codes: Vec<Value> = codes.into_iter().skip(1).collect();
            codes.insert(
                0,
                redeem_code(state, node, code, created.as_str().unwrap_or(now)),
            );
            discount.insert("codes".into(), Value::Array(codes));
        }
        if input.contains_key("usageLimit") {
            discount.insert(
                "usageLimit".into(),
                input.get("usageLimit").cloned().unwrap_or(Value::Null),
            );
        } else if creating {
            discount.insert("usageLimit".into(), Value::Null);
        }
        if let Some(once) = input.get("appliesOncePerCustomer").and_then(Value::as_bool) {
            discount.insert("appliesOncePerCustomer".into(), json!(once));
        } else if creating {
            discount.insert("appliesOncePerCustomer".into(), json!(false));
        }
        if creating {
            discount.insert("shareableUrls".into(), json!([]));
            discount.insert("hasTimelineComment".into(), json!(false));
            discount.insert(
                "totalSales".into(),
                json!({"amount": "0.0", "currencyCode": currency}),
            );
        }
    }
    match kind.family {
        Family::App => {
            for field in ["appliesOnOneTimePurchase", "appliesOnSubscription"] {
                if let Some(flag) = input.get(field).and_then(Value::as_bool) {
                    discount.insert(field.into(), json!(flag));
                } else if creating {
                    discount.insert(field.into(), json!(true));
                }
            }
            if let Some(function) = function {
                let previous = discount.get("discountClasses").cloned();
                let classes = app_classes(
                    input,
                    &function.api_type,
                    if creating { None } else { previous.as_ref() },
                );
                let (app, app_key) = app_of(state);
                discount.insert(
                    "appDiscountType".into(),
                    json!({
                        "app": app,
                        "appKey": app_key,
                        "functionId": function.id,
                        "title": function.title,
                        "description": null,
                        "discountClasses": classes,
                    }),
                );
                discount.insert("discountClasses".into(), classes);
            }
        }
        Family::Basic => {
            if let Some(gets) = input.get("customerGets").filter(|value| !value.is_null()) {
                let previous = discount.get("customerGets").cloned();
                discount.insert(
                    "customerGets".into(),
                    customer_gets(gets, previous.as_ref(), &currency),
                );
            } else if creating {
                discount.insert(
                    "customerGets".into(),
                    customer_gets(&Value::Null, None, &currency),
                );
            }
            if let Some(minimum) = input.get("minimumRequirement") {
                discount.insert(
                    "minimumRequirement".into(),
                    minimum_requirement(minimum, &currency),
                );
            } else if creating {
                discount.insert("minimumRequirement".into(), Value::Null);
            }
            let all = discount
                .get("customerGets")
                .and_then(|gets| gets.pointer("/items/__typename"))
                .and_then(Value::as_str)
                == Some("AllDiscountItems");
            discount.insert(
                "discountClasses".into(),
                json!([if all { "ORDER" } else { "PRODUCT" }]),
            );
        }
        Family::Bxgy => {
            if let Some(gets) = input.get("customerGets").filter(|value| !value.is_null()) {
                let previous = discount.get("customerGets").cloned();
                discount.insert(
                    "customerGets".into(),
                    customer_gets(gets, previous.as_ref(), &currency),
                );
            }
            if let Some(buys) = input.get("customerBuys").filter(|value| !value.is_null()) {
                let previous = discount.get("customerBuys").cloned();
                discount.insert(
                    "customerBuys".into(),
                    customer_buys(buys, previous.as_ref()),
                );
            }
            if let Some(limit) = input
                .get("usesPerOrderLimit")
                .filter(|value| !value.is_null())
            {
                discount.insert("usesPerOrderLimit".into(), limit.clone());
            }
            discount.insert("discountClasses".into(), json!(["PRODUCT"]));
        }
        Family::FreeShipping => {
            if let Some(destination_input) =
                input.get("destination").filter(|value| !value.is_null())
            {
                let previous = discount.get("destinationSelection").cloned();
                discount.insert(
                    "destinationSelection".into(),
                    destination(destination_input, previous.as_ref()),
                );
            } else if creating {
                discount.insert(
                    "destinationSelection".into(),
                    destination(&Value::Null, None),
                );
            }
            if let Some(price) = input.get("maximumShippingPrice") {
                discount.insert(
                    "maximumShippingPrice".into(),
                    match price.as_str() {
                        Some(price) => json!({"amount": decimal(price), "currencyCode": currency}),
                        None => Value::Null,
                    },
                );
            }
            if let Some(minimum) = input.get("minimumRequirement") {
                discount.insert(
                    "minimumRequirement".into(),
                    minimum_requirement(minimum, &currency),
                );
            }
            for field in ["appliesOnOneTimePurchase", "appliesOnSubscription"] {
                if let Some(flag) = input.get(field).and_then(Value::as_bool) {
                    discount.insert(field.into(), json!(flag));
                } else if creating {
                    discount.insert(field.into(), json!(field == "appliesOnOneTimePurchase"));
                }
            }
            discount.insert("discountClasses".into(), json!(["SHIPPING"]));
        }
    }
    if kind.family != Family::App {
        let summary = summary(discount);
        discount.insert("summary".into(), json!(summary));
        discount.insert("shortSummary".into(), json!(summary));
    }
    discount.insert(
        "status".into(),
        json!(status_at(discount, &state.timestamp())),
    );
    discount.insert("updatedAt".into(), json!(now));
}

/// A `DiscountRedeemCode` of a code discount: its id as the seed makes it.
fn redeem_code(state: &ShopState, node: &str, code: &str, created: &str) -> Value {
    let app = app_of(state).0;
    json!({
        "id": gid(
            "DiscountRedeemCode",
            lsf_core::util::stable_id("discount_redeem_code", &format!("{node}/{code}")),
        ),
        "code": code,
        "asyncUsageCount": 0,
        "createdAt": created,
        "createdBy": app,
    })
}

// ---- the mutations ------------------------------------------------------------------------

fn input_of(ctx: &FieldContext<'_>, name: &str) -> Map<String, Value> {
    ctx.arg(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The payload of a create or an update: the discount for an app discount, the node otherwise.
fn answer(state: &ShopState, kind: &Kind, node: Option<&str>) -> Value {
    let value = match node {
        None => Value::Null,
        Some(node) if kind.family == Family::App => state
            .get(node)
            .and_then(inner)
            .cloned()
            .map_or(Value::Null, Value::Object),
        Some(node) => json!(node),
    };
    json!({ kind.payload_field: value })
}

fn create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    kind: &Kind,
) -> Result<Value, FieldError> {
    let input = input_of(ctx, kind.input);
    let state = mutation.state();
    let mut errors = Vec::new();
    check_common(state, kind, &input, None, &mut errors);
    let function = if kind.family == Family::App {
        match discount_function(state, kind, &input) {
            Ok(function) => Some(function),
            Err(error) => {
                errors.push(error);
                None
            }
        }
    } else {
        None
    };
    let inputs = metafields::inputs_of(&input);
    metafields::check(
        state,
        None,
        "DISCOUNT",
        &inputs,
        &[kind.input],
        Some("INVALID"),
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(answer(state, kind, None), &errors));
    }
    if input.get("customerSelection").is_some() {
        mutation.note(format!(
            "{}: lsf does not simulate the deprecated `customerSelection`: use `context`",
            ctx.field_name()
        ));
    }

    let state = mutation.state_mut();
    let now = state.timestamp();
    let node = state.new_gid(kind.node_type());
    let mut discount = value::object(json!({
        "__typename": kind.type_name,
        "discountId": node,
        "asyncUsageCount": 0,
        "createdAt": now,
    }));
    if !kind.automatic {
        discount.insert("codes".into(), json!([]));
    }
    apply_input(
        state,
        kind,
        &input,
        &mut discount,
        function.as_ref(),
        &node,
        &now,
        true,
    );
    state.insert(value::object(json!({
        "__typename": kind.node_type(),
        "id": node,
        kind.inner_field(): discount,
        "metafields": [],
    })));
    metafields::apply(state, &node, "DISCOUNT", &inputs);
    state.emit("discounts/create", &node);
    Ok(payload(answer(state, kind, Some(&node)), &[]))
}

fn update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    kind: &Kind,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let input = input_of(ctx, kind.input);
    let state = mutation.state();
    let node = node_of(state, id, &[kind.node_type()]).filter(|node| {
        state
            .get(node)
            .and_then(inner)
            .and_then(|d| text(d, "__typename"))
            == Some(kind.type_name)
    });
    let Some(node) = node else {
        let error = UserError::new(&["id"], "Discount does not exist").code("INVALID");
        return Ok(payload(answer(state, kind, None), &[error]));
    };
    let previous = state
        .get(&node)
        .and_then(inner)
        .cloned()
        .unwrap_or_default();
    let mut errors = Vec::new();
    check_common(state, kind, &input, Some((&node, &previous)), &mut errors);
    let names_function = ["functionHandle", "functionId"]
        .iter()
        .any(|name| input.get(*name).is_some_and(|value| !value.is_null()));
    let current_function = previous
        .get("appDiscountType")
        .and_then(|kind| kind.get("functionId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let function = if kind.family == Family::App && names_function {
        match discount_function(state, kind, &input) {
            Ok(function) if function.id == current_function => Some(function),
            Ok(_) => {
                errors.push(
                    UserError::new(
                        &[kind.input, "functionHandle"],
                        "The Function of a discount cannot be changed",
                    )
                    .code("INVALID"),
                );
                None
            }
            Err(error) => {
                errors.push(error);
                None
            }
        }
    } else if kind.family == Family::App && input.contains_key("discountClasses") {
        // New classes: the `appDiscountType` follows.
        state
            .all("ShopifyFunction")
            .find(|function| text(function, "id") == Some(current_function.as_str()))
            .map(|function| super::functions::FunctionRef {
                id: current_function.clone(),
                handle: text(function, "handle").unwrap_or_default().to_string(),
                api_type: text(function, "apiType").unwrap_or_default().to_string(),
                title: text(function, "title").unwrap_or_default().to_string(),
            })
    } else {
        None
    };
    let inputs = metafields::inputs_of(&input);
    metafields::check(
        state,
        Some(&node),
        "DISCOUNT",
        &inputs,
        &[kind.input],
        Some("INVALID"),
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(answer(state, kind, None), &errors));
    }

    let state = mutation.state_mut();
    let now = state.timestamp();
    let mut discount = previous;
    apply_input(
        state,
        kind,
        &input,
        &mut discount,
        function.as_ref(),
        &node,
        &now,
        false,
    );
    if let Some(slot) = state.get_mut(&node).and_then(inner_mut) {
        *slot = discount;
    }
    metafields::apply(state, &node, "DISCOUNT", &inputs);
    state.emit("discounts/update", &node);
    Ok(payload(answer(state, kind, Some(&node)), &[]))
}

/// Starts or ends a discount now, as `…Activate` and `…Deactivate` do. Answers whether its
/// dates changed.
fn set_active(state: &mut ShopState, node: &str, active: bool) -> bool {
    let now = state.timestamp();
    let Some(discount) = state.get_mut(node).and_then(inner_mut) else {
        return false;
    };
    let before = discount.clone();
    let status = status_at(discount, &now);
    match (active, status) {
        (true, "SCHEDULED") => {
            discount.insert("startsAt".into(), json!(now));
        }
        (true, "EXPIRED") => {
            discount.insert("endsAt".into(), Value::Null);
        }
        (false, "SCHEDULED") => {
            discount.insert("startsAt".into(), json!(now));
            discount.insert("endsAt".into(), json!(now));
        }
        (false, "ACTIVE") => {
            discount.insert("endsAt".into(), json!(now));
        }
        _ => {}
    }
    let status = status_at(discount, &now);
    discount.insert("status".into(), json!(status));
    let changed = *discount != before;
    if changed {
        discount.insert("updatedAt".into(), json!(now));
    }
    changed
}

fn activate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    automatic: bool,
    active: bool,
) -> Result<Value, FieldError> {
    let node_type = if automatic {
        "DiscountAutomaticNode"
    } else {
        "DiscountCodeNode"
    };
    let field = if automatic {
        "automaticDiscountNode"
    } else {
        "codeDiscountNode"
    };
    let id = ctx.arg_str("id").unwrap_or_default();
    let Some(node) = node_of(mutation.state(), id, &[node_type]) else {
        let error = UserError::new(&["id"], "Discount does not exist").code("INVALID");
        return Ok(payload(json!({ field: null }), &[error]));
    };
    let state = mutation.state_mut();
    if set_active(state, &node, active) {
        state.emit("discounts/update", &node);
    }
    Ok(payload(json!({ field: node }), &[]))
}

fn delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    automatic: bool,
) -> Result<Value, FieldError> {
    let (node_type, field) = if automatic {
        ("DiscountAutomaticNode", "deletedAutomaticDiscountId")
    } else {
        ("DiscountCodeNode", "deletedCodeDiscountId")
    };
    let id = ctx.arg_str("id").unwrap_or_default();
    let Some(node) = node_of(mutation.state(), id, &[node_type]) else {
        let error = UserError::new(&["id"], "Discount does not exist").code("INVALID");
        return Ok(payload(json!({ field: null }), &[error]));
    };
    let state = mutation.state_mut();
    delete_node(state, &node);
    Ok(payload(json!({ field: node }), &[]))
}

/// Deletes a discount and its metafields, which go with it.
fn delete_node(state: &mut ShopState, node: &str) {
    metafields::remove_all(state, node);
    state.delete(node, "discounts/delete");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bulk {
    Delete,
    Activate,
    Deactivate,
}

/// The nodes a bulk mutation targets: `ids`, or those `search` matches; exactly one of `ids`,
/// `search` and `savedSearchId` (shopify.dev's "Only one of …" examples).
fn targets(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    node_type: &str,
    automatic: bool,
) -> Result<Vec<String>, UserError> {
    let given = |name: &str| ctx.arg(name).is_some_and(|value| !value.is_null());
    let count = ["ids", "search", "savedSearchId"]
        .iter()
        .filter(|name| given(name))
        .count();
    if count > 1 {
        let message = if automatic {
            "Only one of IDs, search argument or saved search ID is allowed."
        } else {
            "Only one of 'ids', 'search' or 'saved_search_id' is allowed."
        };
        return Err(UserError::new(&[], message).code("TOO_MANY_ARGUMENTS"));
    }
    if count == 0 {
        let message = if automatic {
            "One of IDs, search argument or saved search ID is required."
        } else {
            "One of 'ids', 'search' or 'saved_search_id' is required."
        };
        return Err(UserError::new(&[], message).code("MISSING_ARGUMENT"));
    }
    if given("savedSearchId") {
        return Err(UserError::new(
            &["savedSearchId"],
            "lsf has no saved searches: use `ids` or `search`",
        )
        .code("INVALID"));
    }
    if let Some(ids) = ctx.arg("ids").and_then(Value::as_array) {
        return Ok(ids
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|id| node_of(state, id, &[node_type]))
            .collect());
    }
    let query = Query::parse(ctx.arg_str("search").unwrap_or_default());
    let options = crate::search::Options::utc(state.now());
    Ok(state
        .keys(node_type)
        .filter(|key| {
            let Some(record) = state.get(key) else {
                return false;
            };
            let searched = Searched {
                state,
                record,
                id: parse_gid(key).map_or(0, |(_, id)| id),
            };
            query.matches(&searched, &options)
        })
        .map(str::to_string)
        .collect())
}

/// A `Job` that is done: what lsf's bulk discount mutations answer, having done the work.
fn done_job(state: &ShopState, name: &str) -> Value {
    json!({
        "id": format!("gid://shopify/Job/{}", value::uuid(&format!("job/{}/{name}/{}", state.domain(), state.events().len()))),
        "done": true,
    })
}

fn bulk(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    automatic: bool,
    action: Bulk,
) -> Result<Value, FieldError> {
    let node_type = if automatic {
        "DiscountAutomaticNode"
    } else {
        "DiscountCodeNode"
    };
    let nodes = match targets(mutation.state(), ctx, node_type, automatic) {
        Ok(nodes) => nodes,
        Err(error) => return Ok(payload(json!({"job": null}), &[error])),
    };
    let state = mutation.state_mut();
    for node in &nodes {
        match action {
            Bulk::Delete => delete_node(state, node),
            Bulk::Activate | Bulk::Deactivate => {
                if set_active(state, node, action == Bulk::Activate) {
                    state.emit("discounts/update", node);
                }
            }
        }
    }
    let job = done_job(state, ctx.field_name());
    Ok(payload(json!({ "job": job }), &[]))
}

/// The code node a `discountId` names.
fn code_node(state: &ShopState, id: &str) -> Option<String> {
    node_of(state, id, &["DiscountCodeNode"])
}

/// `discountRedeemCodeBulkAdd`: the codes are added at once; each code that is blank or taken
/// fails alone, with its error in the bulk creation's `codes`.
fn redeem_codes_add(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("discountId").unwrap_or_default();
    let codes: Vec<String> = ctx
        .arg("codes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|code| code.get("code").and_then(Value::as_str))
        .map(|code| code.trim().to_string())
        .collect();
    let state = mutation.state();
    let Some(node) = code_node(state, id) else {
        let error = UserError::new(&["discountId"], "Discount does not exist").code("INVALID");
        return Ok(payload(json!({"bulkCreation": null}), &[error]));
    };
    if codes.len() > MAX_CODES {
        let error =
            UserError::new(&["codes"], "Codes exceeds the maximum of 250").code("EXCEEDED_MAX");
        return Ok(payload(json!({"bulkCreation": null}), &[error]));
    }

    let state = mutation.state_mut();
    let now = state.timestamp();
    let mut results = Vec::new();
    let mut imported = 0;
    for code in &codes {
        let taken = node_with_code(state, code, None).is_some();
        let error = if code.is_empty() {
            Some(UserError::new(&["code"], "Code can't be blank").code("BLANK"))
        } else if taken {
            Some(
                UserError::new(
                    &["code"],
                    "Code must be unique. Please try a different code.",
                )
                .code("TAKEN"),
            )
        } else {
            None
        };
        if let Some(error) = error {
            let mut error = error.to_json();
            error["extraInfo"] = Value::Null;
            results.push(json!({"code": code, "discountRedeemCode": null, "errors": [error]}));
            continue;
        }
        let redeem = redeem_code(state, &node, code, &now);
        if let Some(Value::Array(list)) = state
            .get_mut(&node)
            .and_then(inner_mut)
            .and_then(|discount| discount.get_mut("codes"))
        {
            list.push(redeem.clone());
        }
        imported += 1;
        results.push(json!({"code": code, "discountRedeemCode": redeem.clone(), "errors": []}));
        state.emit_with(
            "discounts/redeemcode_added",
            redeem["id"].as_str().unwrap_or_default(),
            value::object(json!({"id": node, "redeemCode": redeem, "updatedAt": now})),
        );
    }
    if let Some(discount) = state.get_mut(&node).and_then(inner_mut) {
        discount.insert("updatedAt".into(), json!(now));
    }
    let creation = state.new_gid("DiscountRedeemCodeBulkCreation");
    state.insert(value::object(json!({
        "__typename": "DiscountRedeemCodeBulkCreation",
        "id": creation,
        "createdAt": now,
        "discountCode": node,
        "done": true,
        "codesCount": codes.len(),
        "importedCount": imported,
        "failedCount": codes.len() - imported,
        "codes": results,
    })));
    Ok(payload(json!({"bulkCreation": creation}), &[]))
}

/// `discountCodeRedeemCodeBulkDelete`: removes the codes `ids` names (the GIDs of
/// `DiscountRedeemCode`s), or those `search` matches, of one discount.
fn redeem_codes_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("discountId").unwrap_or_default();
    let state = mutation.state();
    let Some(node) = code_node(state, id) else {
        let error = UserError::new(&["discountId"], "Discount does not exist").code("INVALID");
        return Ok(payload(json!({"job": null}), &[error]));
    };
    let given = |name: &str| ctx.arg(name).is_some_and(|value| !value.is_null());
    let count = ["ids", "search", "savedSearchId"]
        .iter()
        .filter(|name| given(name))
        .count();
    if count != 1 || given("savedSearchId") {
        let message = if count > 1 {
            "Only one of 'ids', 'search' or 'saved_search_id' is allowed."
        } else if count == 0 {
            "One of 'ids', 'search' or 'saved_search_id' is required."
        } else {
            "lsf has no saved searches: use `ids` or `search`"
        };
        let code = if count > 1 {
            "TOO_MANY_ARGUMENTS"
        } else {
            "MISSING_ARGUMENT"
        };
        return Ok(payload(
            json!({"job": null}),
            &[UserError::new(&[], message).code(code)],
        ));
    }
    let ids: Vec<&str> = ctx
        .arg("ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let search = ctx.arg_str("search").map(str::trim).unwrap_or_default();
    let matches = |code: &Value| -> bool {
        let text = code.get("code").and_then(Value::as_str).unwrap_or_default();
        let id = code.get("id").and_then(Value::as_str).unwrap_or_default();
        if !ids.is_empty() {
            return ids.contains(&id);
        }
        let wanted = search
            .strip_prefix("code:")
            .unwrap_or(search)
            .trim_matches('"');
        wanted.is_empty() || text.eq_ignore_ascii_case(wanted)
    };

    let state = mutation.state_mut();
    let now = state.timestamp();
    let mut removed = Vec::new();
    if let Some(Value::Array(codes)) = state
        .get_mut(&node)
        .and_then(inner_mut)
        .and_then(|discount| discount.get_mut("codes"))
    {
        let mut kept = Vec::new();
        for code in codes.drain(..) {
            if matches(&code) {
                removed.push(code);
            } else {
                kept.push(code);
            }
        }
        *codes = kept;
    }
    for code in removed {
        state.emit_with(
            "discounts/redeemcode_removed",
            code["id"].as_str().unwrap_or_default(),
            value::object(json!({"id": node, "redeemCode": code, "updatedAt": now})),
        );
    }
    let job = done_job(state, ctx.field_name());
    Ok(payload(json!({"job": job}), &[]))
}

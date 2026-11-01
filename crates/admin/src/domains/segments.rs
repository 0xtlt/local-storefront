//! Customer segments: `segmentCreate`, `segmentUpdate`, `segmentDelete`,
//! `customerSegmentMembersQueryCreate`, the queries that list a segment's members, and the
//! filters and values a segment query is written with. The query language is [`language`], the
//! filters [`filters`], what they read of a customer [`evaluate`]; saved searches (the customer
//! groups segments replaced, and the saved searches of the other resources) are
//! [`saved_searches`].
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - **`segmentCreate(name:, query:)`**: "Segment names must be unique"; the query follows the
//!   segment query language, and a bad one is a user error on `query` (`Query Line 1 Column 79:
//!   'MATCHES' is unexpected.`, a message quoted from Shopify). A blank name is Rails' "Name
//!   can't be blank", a taken one "Name has already been taken" (lsf's choice of Rails'
//!   wording). `segmentUpdate` changes the name and the query (`lastEditDate` is now);
//!   `segmentDelete` answers `deletedSegmentId`. An id that names no segment is "Segment does
//!   not exist" (lsf's words). Webhooks: `segments/create`, `segments/update`, `segments/delete`.
//! - **Members**: `customerSegmentMembers(segmentId: | query: | queryId:)` evaluates the query on
//!   the customers of the shop (a `queryId` reads the members its job found: "Async queries
//!   won't update as your customer base changes"), sorted by `sortKey` (`created_at`,
//!   `first_order_date`, `last_abandoned_order_date`, `last_order_date`, `number_of_orders`,
//!   `amount_spent`), with `totalCount` and `statistics.attributeStatistics` (the average and the
//!   sum of `amount_spent` or `number_of_orders` over every member). A member's id is
//!   `gid://shopify/CustomerSegmentMember/<customer id>`, as shopify.dev's guide shows it. Given
//!   none of the three, the field is an error (lsf's words). `customerSegmentMembership` says
//!   which of the segments a customer is in.
//! - **`customerSegmentMembersQueryCreate`**: Shopify runs the query as a job; lsf runs it at
//!   once, and the `CustomerSegmentMembersQuery` is `done`, its `currentCount` the members found
//!   (an id of a UUID, as the guide's). A bad query is a user error, code `INVALID`.
//! - **`customer.joined_segment` and `customer.left_segment`**: after each change that concerns
//!   customers (a customer's own topics, an order, a company contact, a checkout), every segment
//!   is evaluated again for the customers it changed; one that moves in or out of a segment
//!   fires the topic, its payload `{shop_id, customer_id, segment_id}`. Creating or editing a
//!   segment moves nobody: its members are computed again, without webhooks (Shopify does not
//!   say it sends them then), and a member whose dates move them out as the clock runs leaves
//!   only at their next change.
//!
//! Segments are records of their own (`Segment`, ids of the shop's counter); the members each
//! segment had when last evaluated are a memo of the shop (`segments/members/<segment GID>`),
//! which a failed mutation puts back. Segments are not store data: they do not sync.

pub mod evaluate;
pub mod filters;
pub mod language;
pub mod saved_searches;

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use self::evaluate::{Context, Facts};
use self::language::Query;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, parse_gid};

/// The memo of the members a segment had when it was last evaluated.
fn members_memo(segment: &str) -> String {
    format!("segments/members/{segment}")
}

pub fn register(registry: &mut Registry) {
    registry
        .mutation("segmentCreate", segment_create)
        .mutation("segmentUpdate", segment_update)
        .mutation("segmentDelete", segment_delete)
        .mutation("customerSegmentMembersQueryCreate", members_query_create)
        .query("customerSegmentMembers", customer_segment_members)
        .query("customerSegmentMembership", customer_segment_membership)
        .query("segmentFilters", segment_filters)
        .query("segmentFilterSuggestions", segment_filter_suggestions)
        .query("segmentValueSuggestions", segment_value_suggestions)
        .field(
            "SegmentStatistics",
            "attributeStatistics",
            attribute_statistics,
        )
        .default_text("Segment", |_r, segment| {
            segment
                .get("name")
                .and_then(Value::as_str)
                .map(|name| vec![name.to_string()])
                .unwrap_or_default()
        });
    saved_searches::register(registry);
}

/// The customers a query keeps, in creation order.
pub fn members(state: &ShopState, query: &Query, context: &Context) -> Vec<String> {
    let created = evaluate::created_by_app(state);
    state
        .all("Customer")
        .filter(|customer| {
            let facts = evaluate::facts(state, customer, &created);
            evaluate::matches(query, &facts, context)
        })
        .filter_map(|customer| customer.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// A query as a user error on `field`, or the query.
fn parsed(text: &str, field: &[&str]) -> Result<Query, UserError> {
    language::parse(text).map_err(|error| UserError::new(field, error.to_string()))
}

/// The segment a GID names.
fn segment_of<'s>(state: &'s ShopState, id: &str) -> Option<&'s Record> {
    state.get_of(id, "Segment")
}

/// The errors of a segment's name: blank, or another segment's (`except` is the one renamed).
fn check_name(state: &ShopState, name: &str, except: Option<&str>, errors: &mut Vec<UserError>) {
    let taken = || {
        state.all("Segment").any(|segment| {
            segment.get("name").and_then(Value::as_str) == Some(name)
                && segment.get("id").and_then(Value::as_str) != except
        })
    };
    if name.is_empty() {
        errors.push(UserError::new(&["name"], "Name can't be blank"));
    } else if taken() {
        errors.push(UserError::new(&["name"], "Name has already been taken"));
    }
}

/// The segment query of the `query` argument, or its error in `errors`.
fn query_arg(text: &str, errors: &mut Vec<UserError>) -> Option<Query> {
    parsed(text, &["query"])
        .map_err(|error| errors.push(error))
        .ok()
}

/// Computes the members of a segment again and keeps them, without webhooks.
fn remember_members(state: &mut ShopState, segment: &str, query: &Query) {
    let context = Context::of(state);
    let found = members(state, query, &context);
    state.set_memo(members_memo(segment), json!(found));
}

fn segment_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let name = ctx.arg_str("name").unwrap_or_default().trim().to_string();
    let text = ctx.arg_str("query").unwrap_or_default().to_string();
    let mut errors = Vec::new();
    check_name(m.state(), &name, None, &mut errors);
    let query = query_arg(&text, &mut errors);
    let (Some(query), true) = (query, errors.is_empty()) else {
        return Ok(payload(json!({"segment": null}), &errors));
    };
    let now = m.timestamp();
    let state = m.state_mut();
    let id = state.new_gid("Segment");
    state.insert_as(
        id.clone(),
        crate::state::value::object(json!({
            "__typename": "Segment",
            "id": id,
            "name": name,
            "query": text,
            "creationDate": now,
            "lastEditDate": now,
        })),
    );
    remember_members(state, &id, &query);
    state.emit("segments/create", &id);
    Ok(payload(json!({"segment": id}), &[]))
}

fn segment_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if segment_of(m.state(), &id).is_none() {
        let error = UserError::new(&["id"], "Segment does not exist");
        return Ok(payload(json!({"segment": null}), &[error]));
    }
    let name = ctx.arg_str("name").map(|name| name.trim().to_string());
    let text = ctx.arg_str("query").map(str::to_string);
    let mut errors = Vec::new();
    if let Some(name) = &name {
        check_name(m.state(), name, Some(&id), &mut errors);
    }
    let query = text
        .as_deref()
        .and_then(|text| query_arg(text, &mut errors));
    if !errors.is_empty() {
        return Ok(payload(json!({"segment": null}), &errors));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    if let Some(record) = state.get_mut(&id) {
        if let Some(name) = name {
            record.insert("name".into(), json!(name));
        }
        if let Some(text) = &text {
            record.insert("query".into(), json!(text));
        }
        record.insert("lastEditDate".into(), json!(now));
    }
    if let Some(query) = query {
        remember_members(state, &id, &query);
    }
    state.emit("segments/update", &id);
    Ok(payload(json!({"segment": id}), &[]))
}

fn segment_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if segment_of(m.state(), &id).is_none() {
        let error = UserError::new(&["id"], "Segment does not exist");
        return Ok(payload(json!({"deletedSegmentId": null}), &[error]));
    }
    let state = m.state_mut();
    state.delete(&id, "segments/delete");
    state.set_memo(members_memo(&id), Value::Null);
    Ok(payload(json!({"deletedSegmentId": id}), &[]))
}

/// The members query of `customerSegmentMembersQueryCreate`'s input: its `query`, or its
/// segment's.
fn input_query(state: &ShopState, input: &Map<String, Value>) -> Result<Query, UserError> {
    match (
        input.get("query").and_then(Value::as_str),
        input.get("segmentId").and_then(Value::as_str),
    ) {
        (Some(text), _) => parsed(text, &["input", "query"]).map_err(|error| error.code("INVALID")),
        (None, Some(segment)) => {
            let Some(record) = segment_of(state, segment) else {
                return Err(
                    UserError::new(&["input", "segmentId"], "Segment does not exist")
                        .code("INVALID"),
                );
            };
            let text = record
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            parsed(text, &["input", "segmentId"]).map_err(|error| error.code("INVALID"))
        }
        // lsf's words: Shopify does not publish this error.
        (None, None) => Err(UserError::new(
            &["input"],
            "Either a query or a segment id is required.",
        )
        .code("INVALID")),
    }
}

fn members_query_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx
        .arg("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let query = match input_query(m.state(), &input) {
        Ok(query) => query,
        Err(error) => {
            return Ok(payload(
                json!({"customerSegmentMembersQuery": null}),
                &[error],
            ));
        }
    };
    let sort_key = input
        .get("sortKey")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(key) = &sort_key
        && !SORT_KEYS.contains(&key.as_str())
    {
        let error = UserError::new(&["input", "sortKey"], format!("Invalid sort key: {key}"))
            .code("INVALID");
        return Ok(payload(
            json!({"customerSegmentMembersQuery": null}),
            &[error],
        ));
    }
    let reverse = input
        .get("reverse")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = m.state();
    let context = Context::of(state);
    let mut found = members(state, &query, &context);
    sort_members(state, &mut found, sort_key.as_deref(), reverse);
    let count = state.count("CustomerSegmentMembersQuery");
    let id = format!(
        "gid://shopify/CustomerSegmentMembersQuery/{}",
        crate::state::value::uuid(&format!(
            "customer-segment-members-query/{}/{count}",
            state.domain()
        ))
    );
    let state = m.state_mut();
    state.insert_as(
        id.clone(),
        crate::state::value::object(json!({
            "__typename": "CustomerSegmentMembersQuery",
            "id": id,
            "done": true,
            "currentCount": found.len(),
            // The members the job found, in their order: what `customerSegmentMembers(queryId:)`
            // pages over.
            "lsfMembers": found,
        })),
    );
    // Not a webhook topic: says that the shop changed.
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"customerSegmentMembersQuery": id}), &[]))
}

/// The sort keys of `customerSegmentMembers`, as its reference lists them.
const SORT_KEYS: &[&str] = &[
    "created_at",
    "first_order_date",
    "last_abandoned_order_date",
    "last_order_date",
    "number_of_orders",
    "amount_spent",
];

/// Sorts members by a sort key of `customerSegmentMembers` (ascending, then `reverse`); without
/// one they stay in creation order. Ties and missing values follow the creation order.
fn sort_members(state: &ShopState, members: &mut [String], key: Option<&str>, reverse: bool) {
    if let Some(key) = key {
        let created = evaluate::created_by_app(state);
        let value = |customer: &String| -> Option<f64> {
            let record = state.get(customer)?;
            let facts = evaluate::facts(state, record, &created);
            let date =
                |at: Option<chrono::DateTime<chrono::Utc>>| at.map(|at| at.timestamp() as f64);
            match key {
                "created_at" => date(facts.created_at),
                "first_order_date" => date(facts.first_order()),
                "last_order_date" => date(facts.last_order()),
                "last_abandoned_order_date" => date(facts.abandoned_checkout),
                "number_of_orders" => Some(facts.number_of_orders as f64),
                "amount_spent" => Some(facts.amount_spent),
                _ => None,
            }
        };
        let mut keyed: Vec<(Option<f64>, String)> = members
            .iter()
            .map(|member| (value(member), member.clone()))
            .collect();
        keyed.sort_by(|(a, _), (b, _)| match (a, b) {
            (Some(a), Some(b)) => a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        for (slot, (_, member)) in members.iter_mut().zip(keyed) {
            *slot = member;
        }
    }
    if reverse {
        members.reverse();
    }
}

/// A `CustomerSegmentMember`, built from its customer: its id is the customer's number.
fn member_of(state: &ShopState, customer: &str) -> Option<Value> {
    let record = state.get(customer)?;
    let (_, number) = parse_gid(customer)?;
    let field = |name: &str| record.get(name).cloned().unwrap_or(Value::Null);
    let mergeable = crate::domains::customer_merge::mergeable(state, customer);
    Some(json!({
        "id": format!("gid://shopify/CustomerSegmentMember/{number}"),
        "firstName": field("firstName"),
        "lastName": field("lastName"),
        "displayName": record.get("displayName").cloned().unwrap_or_else(|| json!("")),
        "amountSpent": field("amountSpent"),
        "defaultAddress": field("defaultAddress"),
        "defaultEmailAddress": field("defaultEmailAddress"),
        "defaultPhoneNumber": field("defaultPhoneNumber"),
        "lastOrderId": field("lastOrder"),
        "numberOfOrders": record.get("numberOfOrders").cloned().unwrap_or_else(|| json!("0")),
        "note": field("note"),
        "mergeable": mergeable,
        "metafields": record.get("metafields").cloned().unwrap_or_else(|| json!([])),
    }))
}

/// The statistics of members, for `attributeStatistics`: the sum and the average of each
/// numeric attribute.
fn statistics(state: &ShopState, members: &[String]) -> Value {
    let created = evaluate::created_by_app(state);
    let facts: Vec<Facts> = members
        .iter()
        .filter_map(|member| state.get(member))
        .map(|record| evaluate::facts(state, record, &created))
        .collect();
    let stat = |values: Vec<f64>| {
        let sum: f64 = values.iter().sum();
        let average = if values.is_empty() {
            0.0
        } else {
            sum / values.len() as f64
        };
        json!({"sum": sum, "average": average})
    };
    json!({
        "lsfStatistics": {
            "amount_spent": stat(facts.iter().map(|facts| facts.amount_spent).collect()),
            "number_of_orders": stat(facts.iter().map(|facts| facts.number_of_orders as f64).collect()),
        }
    })
}

/// `SegmentStatistics.attributeStatistics(attributeName:)`.
fn attribute_statistics(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let name = ctx.arg_str("attributeName").unwrap_or_default();
    let found = r
        .map_of(object)
        .and_then(|statistics| statistics.get("lsfStatistics"))
        .and_then(|all| all.get(name))
        .and_then(Value::as_object)
        .cloned();
    let value = found.unwrap_or_else(|| {
        r.note(format!(
            "attributeStatistics: lsf computes amount_spent and number_of_orders, not `{name}`"
        ));
        crate::state::value::object(json!({"sum": 0.0, "average": 0.0}))
    });
    Ok(Output::Object(Obj::built(
        "SegmentAttributeStatistics",
        value,
    )))
}

/// `customerSegmentMembers`: the members of a segment, of a query, or of a members query.
fn customer_segment_members(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let mut context = Context::of(state);
    // "The timezone that's used to interpret relative date arguments. The timezone defaults to
    // UTC if the timezone isn't provided."
    context.zone = ctx
        .arg_str("timezone")
        .and_then(|zone| zone.parse().ok())
        .unwrap_or(chrono_tz::Tz::UTC);
    let sort_key = ctx.arg_str("sortKey");
    if let Some(key) = sort_key
        && !SORT_KEYS.contains(&key)
    {
        return Err(FieldError::new(format!(
            "Invalid sort key: {key}. Valid values: {}.",
            SORT_KEYS.join(", ")
        )));
    }
    let reverse = ctx.arg_bool("reverse").unwrap_or(false);
    let mut found = if let Some(query_id) = ctx.arg_str("queryId") {
        let Some(record) = state.get_of(query_id, "CustomerSegmentMembersQuery") else {
            // lsf's words: Shopify does not publish this error.
            return Err(FieldError::new("The segment members query does not exist."));
        };
        crate::domains::catalog::gids(record.get("lsfMembers"))
    } else {
        let text = match (ctx.arg_str("segmentId"), ctx.arg_str("query")) {
            (Some(segment), _) => match segment_of(state, segment) {
                Some(record) => record
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                None => return Err(FieldError::new("Segment does not exist")),
            },
            (None, Some(text)) => text.to_string(),
            (None, None) => {
                return Err(FieldError::new(
                    "Provide one of segmentId, query or queryId.",
                ));
            }
        };
        let query = language::parse(&text).map_err(|error| FieldError::new(error.to_string()))?;
        let mut found = members(state, &query, &context);
        sort_members(state, &mut found, sort_key, false);
        found
    };
    if ctx.arg_str("queryId").is_some() {
        // The job's members are in the order it sorted them.
        sort_members(state, &mut found, sort_key, false);
    }
    if reverse {
        found.reverse();
    }
    let total = found.len();
    let statistics = statistics(state, &found);
    let items: Vec<Value> = found
        .iter()
        .filter_map(|customer| member_of(state, customer))
        .collect();
    // The connection pages without reading `query`, `sortKey` and `reverse`: they are the
    // segment's, applied above.
    let mut arguments = ctx.arguments().clone();
    for name in ["query", "sortKey", "reverse"] {
        arguments.remove(name);
    }
    let output = r.connection(&ctx.with_arguments(&arguments), items)?;
    let Output::Object(Obj::Inline(connection)) = output else {
        return Ok(output);
    };
    let mut value = connection.value.clone();
    value.insert("totalCount".into(), json!(total));
    value.insert("statistics".into(), statistics);
    Ok(Output::Object(Obj::built(&connection.type_name, value)))
}

/// `customerSegmentMembership(customerId:, segmentIds:)`.
fn customer_segment_membership(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let customer = ctx.arg_str("customerId").unwrap_or_default();
    let context = Context::of(state);
    let created = evaluate::created_by_app(state);
    let facts = state
        .get_of(customer, "Customer")
        .map(|record| evaluate::facts(state, record, &created));
    let memberships: Vec<Value> = ctx
        .arg("segmentIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|segment| {
            let query = segment_of(state, segment)
                .and_then(|record| record.get("query").and_then(Value::as_str))
                .and_then(|text| language::parse(text).ok());
            let is_member = match (&query, &facts) {
                (Some(query), Some(facts)) => evaluate::matches(query, facts, &context),
                _ => false,
            };
            json!({"segmentId": segment, "isMember": is_member})
        })
        .collect();
    let mut value = Record::new();
    value.insert("memberships".into(), json!(memberships));
    Ok(Output::Object(Obj::built(
        "SegmentMembershipResponse",
        value,
    )))
}

/// A filter as `segmentFilters` lists it.
fn filter_item(filter: &filters::Filter) -> Value {
    let mut item = json!({
        "__typename": filter.type_name(),
        "localizedName": filter.localized_name,
        "queryName": filter.query_name,
        "multiValue": filter.multi_value(),
    });
    match filter.kind {
        filters::Kind::Float | filters::Kind::Integer => {
            item["minRange"] = Value::Null;
            item["maxRange"] = Value::Null;
        }
        filters::Kind::Function => {
            item["returnValueType"] = json!("Boolean");
            item["parameters"] = json!(
                filter
                    .params
                    .iter()
                    .map(|param| json!({
                        "queryName": param.name,
                        "localizedName": param.localized_name,
                        "localizedDescription": param.description,
                        "parameterType": match param.kind {
                            filters::ParamKind::Id => "ID",
                            filters::ParamKind::Text => "String",
                            filters::ParamKind::Date => "Date",
                            filters::ParamKind::Number => "Float",
                            filters::ParamKind::Coordinates => "Coordinates",
                        },
                        "acceptsMultipleValues": param.multiple,
                        "optional": true,
                        "minRange": null,
                        "maxRange": null,
                    }))
                    .collect::<Vec<_>>()
            );
        }
        _ => {}
    }
    item
}

fn segment_filters(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = filters::FILTERS.iter().map(filter_item).collect();
    r.connection(ctx, items)
}

/// `segmentFilterSuggestions(search:)`: the filters whose name or query name holds the search,
/// case ignored.
fn segment_filter_suggestions(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let search = ctx.arg_str("search").unwrap_or_default().to_lowercase();
    let items = filters::FILTERS
        .iter()
        .filter(|filter| {
            filter.localized_name.to_lowercase().contains(&search)
                || filter.query_name.contains(&search)
        })
        .map(filter_item)
        .collect();
    r.connection(ctx, items)
}

/// The records of a type as `(the number of their GID, their field)`.
fn numbered(state: &ShopState, type_name: &str, field: &str) -> Vec<(String, String)> {
    state
        .all(type_name)
        .filter_map(|record| {
            let (_, id) = parse_gid(record.get("id")?.as_str()?)?;
            Some((id.to_string(), record.get(field)?.as_str()?.to_string()))
        })
        .collect()
}

/// The values a filter (or a parameter of a function) takes in the shop: `(query name,
/// localized value)`.
fn values_of(state: &ShopState, filter: &str, parameter: Option<&str>) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    let mut push = |query: String, localized: String| {
        if !values.iter().any(|(known, _)| *known == query) {
            values.push((query, localized));
        }
    };
    let customers: Vec<&Record> = state.all("Customer").collect();
    let created = evaluate::created_by_app(state);
    match (filter, parameter) {
        ("customer_tags", _) => {
            for customer in &customers {
                for tag in crate::domains::catalog::tags_of(customer.get("tags")) {
                    push(tag.clone(), tag);
                }
            }
        }
        ("customer_countries" | "customer_regions" | "customer_cities", _) => {
            for customer in &customers {
                let facts = evaluate::facts(state, customer, &created);
                let list = match filter {
                    "customer_countries" => facts.countries,
                    "customer_regions" => facts.regions,
                    _ => facts.cities,
                };
                for value in list {
                    push(value.clone(), value);
                }
            }
        }
        ("customer_language", _) => {
            for customer in &customers {
                if let Some(locale) = customer.get("locale").and_then(Value::as_str) {
                    push(locale.to_string(), locale.to_string());
                }
            }
        }
        ("customer_email_domain", _) => {
            for customer in &customers {
                if let Some(domain) = evaluate::facts(state, customer, &created).email_domain {
                    push(domain.clone(), domain);
                }
            }
        }
        ("companies", _) => {
            for (id, name) in numbered(state, "Company", "name") {
                push(id, name);
            }
        }
        ("products_purchased" | "storefront_event.product_viewed", Some("id")) => {
            for (id, title) in numbered(state, "Product", "title") {
                push(id, title);
            }
        }
        ("storefront_event.collection_viewed", Some("id")) => {
            for (id, title) in numbered(state, "Collection", "title") {
                push(id, title);
            }
        }
        ("products_purchased" | "storefront_event.product_viewed", Some("tag")) => {
            for product in state.all("Product") {
                for tag in crate::domains::catalog::tags_of(product.get("tags")) {
                    push(tag.clone(), tag);
                }
            }
        }
        (_, Some("activity_id")) => {
            for (id, title) in numbered(state, "MarketingActivity", "title") {
                push(id, title);
            }
        }
        (_, Some("location_id")) => {
            for (id, name) in numbered(state, "Location", "name") {
                push(id, name);
            }
        }
        (name, None) => {
            if let Some(filters::Kind::Enum(values)) = filters::find(name).map(|f| f.kind) {
                // Humanized, as the guide's example shows them (`NOT_SUBSCRIBED` is "Not
                // subscribed").
                for value in values {
                    push(
                        value.to_string(),
                        crate::state::value::humanize(&value.to_lowercase()),
                    );
                }
            }
        }
        _ => {}
    }
    values
}

/// `segmentValueSuggestions(search:, filterQueryName:, functionParameterQueryName:)`.
fn segment_value_suggestions(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let search = ctx.arg_str("search").unwrap_or_default().to_lowercase();
    let filter = ctx.arg_str("filterQueryName").unwrap_or_default();
    let parameter = ctx.arg_str("functionParameterQueryName");
    let items = values_of(r.state(), filter, parameter)
        .into_iter()
        .filter(|(query, localized)| {
            query.to_lowercase().contains(&search) || localized.to_lowercase().contains(&search)
        })
        .map(|(query, localized)| json!({"queryName": query, "localizedValue": localized}))
        .collect();
    r.connection(ctx, items)
}

/// The customers the events since `start` are about: those of the customer topics, of the
/// orders, of the company contacts and of the checkouts.
fn customers_changed(state: &ShopState, start: usize) -> Vec<String> {
    let mut customers: Vec<String> = Vec::new();
    for event in state.events().iter().skip(start) {
        let topic = event.topic.as_str();
        let record = state.get(&event.gid).or(event.snapshot.as_ref());
        let customer = if topic.starts_with("customers/")
            || topic.starts_with("customer.")
            || topic.starts_with("customers_")
        {
            (crate::state::gid_type(&event.gid) == Some("Customer")).then(|| event.gid.clone())
        } else if topic.starts_with("orders/")
            || topic.starts_with("refunds/")
            || topic.starts_with("company_contacts/")
            || topic.starts_with("checkouts/")
        {
            record
                .and_then(|record| record.get("customer"))
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        };
        if let Some(customer) = customer
            && !customers.contains(&customer)
        {
            customers.push(customer);
        }
    }
    customers
}

/// What follows a change of the shop for the segments and the customers' sales: the
/// `customer.joined_segment` and `customer.left_segment` of the customers the events since
/// `start` concern, and their `customers/purchasing_summary` when their orders changed. The
/// mutation framework runs it after each mutation that succeeded, and the server after the
/// changes it makes itself (an order of the checkout, a cart). `subscribed` says whether the
/// app subscribes to a topic: `customers/purchasing_summary`, which follows every change of an
/// order, is emitted only then.
pub fn follow(state: &mut ShopState, start: usize, subscribed: &dyn Fn(&str) -> bool) {
    if state.event_count() <= start {
        return;
    }
    let customers = customers_changed(state, start);
    if customers.is_empty() {
        return;
    }
    let report = subscribed("customers/purchasing_summary");
    crate::domains::customer_merge::follow_purchases(state, start, &customers, report);
    let segments: Vec<(String, String)> = state
        .all("Segment")
        .filter_map(|segment| {
            Some((
                segment.get("id")?.as_str()?.to_string(),
                segment.get("query")?.as_str()?.to_string(),
            ))
        })
        .collect();
    if segments.is_empty() {
        return;
    }
    let context = Context::of(state);
    let created = evaluate::created_by_app(state);
    let shop = state.shop_gid().map(str::to_string);
    for (segment, text) in segments {
        let Ok(query) = language::parse(&text) else {
            continue;
        };
        let memo = members_memo(&segment);
        let before: Vec<String> = match state.memo(&memo) {
            Some(Value::Array(members)) => members
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            // Never evaluated (a segment of a reset shop): its members as they are, moving
            // nobody.
            _ => members(state, &query, &context),
        };
        let mut after: Vec<String> = before.clone();
        let mut moves = Vec::new();
        for customer in &customers {
            let is_member = state
                .get_of(customer, "Customer")
                .map(|record| {
                    let facts = evaluate::facts(state, record, &created);
                    evaluate::matches(&query, &facts, &context)
                })
                .unwrap_or(false);
            let was_member = before.contains(customer);
            if is_member && !was_member {
                after.push(customer.clone());
                moves.push(("customer.joined_segment", customer.clone()));
            } else if !is_member && was_member {
                after.retain(|member| member != customer);
                moves.push(("customer.left_segment", customer.clone()));
            }
        }
        if moves.is_empty() && state.memo(&memo).is_some() {
            continue;
        }
        // Keep the creation order of the customers.
        let order: HashSet<String> = after.iter().cloned().collect();
        let kept: Vec<String> = state
            .keys("Customer")
            .filter(|key| order.contains(*key))
            .map(str::to_string)
            .collect();
        state.set_memo(memo, json!(kept));
        for (topic, customer) in moves {
            let key = format!("segment-membership/{customer}/{segment}");
            let snapshot = crate::state::value::object(json!({
                "__typename": "SegmentMembership",
                "shopId": shop,
                "customerId": customer,
                "segmentId": segment,
            }));
            state.emit_with(topic, &key, snapshot);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;

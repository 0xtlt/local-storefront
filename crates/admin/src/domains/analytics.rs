//! Analytics: annotations (`analyticsAnnotationCreate`, `…Update`, `…Delete`,
//! `Shop.analyticsAnnotations`), targets (`analyticsTargetCreate`, `…Update`,
//! `analyticsTargetsDelete`, `analyticsTargets`, `AnalyticsTarget.presentmentExpectedValue`), and
//! `shopifyqlQuery` ([`shopifyql`]).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - **Annotations** mark an event on the analytics charts: a `title`, a `type` among the types
//!   apps may create (the "Annotation categories and types" of ShopifyQL's `ANNOTATE`:
//!   `campaign`, `product_launch`, `store_redesign`, …, alone or with their category,
//!   `marketing_events.campaign`), `startedAt`, an optional `endedAt`, a `description`. An
//!   annotation is the app's (`source: APP`, `createdByApp`); "An app can update only
//!   annotations that it created", and an id that names none is `NOT_FOUND`. An unknown type, a
//!   blank title or an end before the start is `INVALID` (lsf's messages). Shopify limits how
//!   many an app may have (`LIMIT_REACHED`) without saying how many: lsf has no limit.
//! - **Targets**: a goal for a `metric` (one of the `sales` schema's: `total_sales`, `orders`,
//!   …; `INVALID_METRIC` otherwise) over `startDate`–`endDate`, `expectedValue` in the shop's
//!   currency, optional `filters` (a ShopifyQL condition). "A target is uniquely identified by
//!   the combination of metric, startDate, endDate, and filters" (`TAKEN`). Its
//!   `shopifyqlQuery` is the query that measures it (`FROM sales SHOW total_sales WHERE … SINCE
//!   … UNTIL …`). `analyticsTargetsDelete` deletes what it finds and reports the rest
//!   (`NOT_FOUND`), the others still deleted. `presentmentExpectedValue(currencyCode:)` is the
//!   value in the shop's currency; lsf converts to no other currency (`null`, said in a note).
//!
//! No webhook topic covers analytics: their changes emit `lsf/changed`. They are not store data:
//! they do not sync.

pub mod shopifyql;

use chrono::NaiveDate;
use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::ShopState;

/// The annotation types apps may create, by category: ShopifyQL's "Annotation categories and
/// types" whose GraphQL operations are "Return, create, update".
const ANNOTATION_TYPES: &[(&str, &[&str])] = &[
    (
        "product_events",
        &[
            "product_launch",
            "product_discontinuation",
            "collection_launch",
            "bundle_launch",
            "preorder_launch",
        ],
    ),
    (
        "marketing_events",
        &[
            "in_person_event",
            "campaign",
            "brand_change",
            "influencer_collaboration",
            "brand_collaboration",
            "channel_launch",
            "ad_spend_change",
            "media_mention",
            "attribution_change",
        ],
    ),
    (
        "discount_events",
        &[
            "checkout_offer",
            "amount_off_products_discount",
            "free_shipping_discount",
            "buy_x_get_y_discount",
            "amount_off_order_discount",
            "seasonal_promotion",
            "shipping_promotion",
            "loyalty_program_offer",
            "reward_program_offer",
            "subscription_program_offer",
            "shop_cash_offer",
            "pricing_experiment",
        ],
    ),
    (
        "inventory_events",
        &[
            "supplier_change",
            "warehouse_change",
            "fulfillment_service_change",
        ],
    ),
    (
        "online_store_events",
        &[
            "landing_page_launch",
            "store_redesign",
            "payment_method_change",
        ],
    ),
    (
        "operation_events",
        &[
            "retail_store_change",
            "popup_store",
            "revenue_milestone",
            "order_milestone",
            "customer_milestone",
            "capital_funding",
            "expansion_milestone",
            "award_recognition",
            "anniversary",
            "market_change",
            "tax_change",
            "loyalty_program_change",
            "reward_program_change",
            "subscription_program_change",
            "retention_program_change",
            "return_program_change",
            "external_event",
            "team_change",
        ],
    ),
    ("custom_events", &["other"]),
];

/// Whether apps may create annotations of a type: `campaign` or `marketing_events.campaign`.
fn creatable_type(kind: &str) -> bool {
    let (category, name) = match kind.split_once('.') {
        Some((category, name)) => (Some(category), name),
        None => (None, kind),
    };
    ANNOTATION_TYPES.iter().any(|(known, types)| {
        category.is_none_or(|category| category == *known) && types.contains(&name)
    })
}

pub fn register(registry: &mut Registry) {
    registry
        .mutation("analyticsAnnotationCreate", annotation_create)
        .mutation("analyticsAnnotationUpdate", annotation_update)
        .mutation("analyticsAnnotationDelete", annotation_delete)
        .mutation("analyticsTargetCreate", target_create)
        .mutation("analyticsTargetUpdate", target_update)
        .mutation("analyticsTargetsDelete", targets_delete)
        .field("Shop", "analyticsAnnotations", |r, ctx, _object| {
            r.connection(ctx, r.records_of("AnalyticsAnnotation"))
        })
        .field(
            "AnalyticsTarget",
            "presentmentExpectedValue",
            presentment_expected_value,
        )
        .search_field("AnalyticsAnnotation", "started_at", |r, _item, record| {
            Some(date_values(&r.search_options(), record.get("startedAt")))
        })
        .search_field("AnalyticsAnnotation", "ended_at", |r, _item, record| {
            Some(date_values(&r.search_options(), record.get("endedAt")))
        })
        .search_field("AnalyticsAnnotation", "updated_at", |r, _item, record| {
            Some(date_values(&r.search_options(), record.get("updatedAt")))
        })
        .search_field("AnalyticsTarget", "start_date", |r, _item, record| {
            Some(date_values(&r.search_options(), record.get("startDate")))
        })
        .search_field("AnalyticsTarget", "end_date", |r, _item, record| {
            Some(date_values(&r.search_options(), record.get("endDate")))
        })
        .query("shopifyqlQuery", shopifyql::resolve);
}

/// A `DateTime` or a `Date` (the start of that day of the shop, as the query's day) as search
/// values.
fn date_values(
    zone: &crate::search::Options,
    value: Option<&Value>,
) -> Vec<crate::search::SearchValue> {
    value
        .and_then(Value::as_str)
        .and_then(|text| zone.instant(text))
        .map(crate::search::SearchValue::Date)
        .into_iter()
        .collect()
}

fn input_of(ctx: &FieldContext<'_>) -> Map<String, Value> {
    ctx.arg("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// What is wrong with the fields of an annotation as they would be.
fn annotation_errors(fields: &Map<String, Value>) -> Vec<UserError> {
    let mut errors = Vec::new();
    if let Some(title) = fields.get("title")
        && title.as_str().is_none_or(|title| title.trim().is_empty())
    {
        errors.push(UserError::new(&["input", "title"], "Title can't be blank").code("INVALID"));
    }
    if let Some(kind) = fields.get("type")
        && !kind.as_str().is_some_and(creatable_type)
    {
        errors.push(
            UserError::new(
                &["input", "type"],
                format!(
                    "Type {} is not a type of annotation apps can create.",
                    kind.as_str().unwrap_or_default()
                ),
            )
            .code("INVALID"),
        );
    }
    if let (Some(start), Some(end)) = (
        crate::state::value::date_time_of(fields.get("startedAt")),
        crate::state::value::date_time_of(fields.get("endedAt")),
    ) && end < start
    {
        errors.push(
            UserError::new(&["input", "endedAt"], "Ended at must be after started at")
                .code("INVALID"),
        );
    }
    errors
}

/// Shopify writes the dates of an annotation in UTC (`2026-11-28T00:00:00Z`).
fn utc(value: &Value) -> Value {
    crate::state::value::date_time_of(Some(value))
        .map_or(Value::Null, |at| json!(crate::state::value::date_time(at)))
}

fn annotation_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input_of(ctx);
    let errors = annotation_errors(&input);
    if !errors.is_empty() {
        return Ok(payload(json!({"analyticsAnnotation": null}), &errors));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    let app = state.keys("App").next().map(str::to_string);
    let id = state.new_gid("AnalyticsAnnotation");
    state.insert_as(
        id.clone(),
        crate::state::value::object(json!({
            "__typename": "AnalyticsAnnotation",
            "id": id,
            "title": input.get("title"),
            "type": input.get("type"),
            "description": input.get("description").cloned().unwrap_or(Value::Null),
            "startedAt": utc(input.get("startedAt").unwrap_or(&Value::Null)),
            "endedAt": utc(input.get("endedAt").unwrap_or(&Value::Null)),
            "source": "APP",
            "createdByApp": app,
            "createdAt": now,
            "updatedAt": now,
        })),
    );
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"analyticsAnnotation": id}), &[]))
}

/// An annotation this app made.
fn own_annotation(state: &ShopState, id: &str) -> bool {
    let app = state.keys("App").next();
    state
        .get(id)
        .filter(|_| state.type_of(id) == Some("AnalyticsAnnotation"))
        .is_some_and(|record| record.get("createdByApp").and_then(Value::as_str) == app)
}

fn annotation_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !own_annotation(m.state(), &id) {
        let error = UserError::new(&["id"], "Analytics annotation not found").code("NOT_FOUND");
        return Ok(payload(json!({"analyticsAnnotation": null}), &[error]));
    }
    let input = input_of(ctx);
    let merged = merged(m.state(), &id, &input);
    let mut checked = Map::new();
    for field in ["title", "type"] {
        if input.contains_key(field) {
            checked.insert(
                field.into(),
                merged.get(field).cloned().unwrap_or(Value::Null),
            );
        }
    }
    for field in ["startedAt", "endedAt"] {
        checked.insert(
            field.into(),
            merged.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    let errors = annotation_errors(&checked);
    if !errors.is_empty() {
        return Ok(payload(json!({"analyticsAnnotation": null}), &errors));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    if let Some(record) = state.get_mut(&id) {
        for (field, value) in &input {
            let value = match field.as_str() {
                "startedAt" | "endedAt" => utc(value),
                _ => value.clone(),
            };
            record.insert(field.clone(), value);
        }
        record.insert("updatedAt".into(), json!(now));
    }
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"analyticsAnnotation": id}), &[]))
}

fn annotation_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !own_annotation(m.state(), &id) {
        let error = UserError::new(&["id"], "Analytics annotation not found").code("NOT_FOUND");
        return Ok(payload(json!({"deletedId": null}), &[error]));
    }
    m.state_mut().delete(&id, "analytics_annotations/delete");
    Ok(payload(json!({"deletedId": id}), &[]))
}

/// The metrics a target can be set on: those of the `sales` schema.
fn is_metric(metric: &str) -> bool {
    shopifyql::SALES_METRICS
        .iter()
        .any(|(name, _, _)| *name == metric)
}

/// The query that measures a target of these fields.
fn target_query(fields: &Map<String, Value>) -> String {
    let text = |field: &str| {
        fields
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let filter = fields
        .get("filters")
        .and_then(Value::as_str)
        .filter(|filters| !filters.trim().is_empty())
        .map(|filters| format!(" WHERE {filters}"))
        .unwrap_or_default();
    format!(
        "FROM sales SHOW {}{filter} SINCE {} UNTIL {}",
        text("metric"),
        text("startDate"),
        text("endDate")
    )
}

/// A record with the fields of an update's input over its own.
fn merged(state: &ShopState, id: &str, input: &Map<String, Value>) -> Map<String, Value> {
    let mut merged = state.get(id).cloned().unwrap_or_default();
    for (field, value) in input {
        merged.insert(field.clone(), value.clone());
    }
    merged
}

/// What is wrong with a target as it would be.
fn target_errors(
    state: &ShopState,
    fields: &Map<String, Value>,
    except: Option<&str>,
) -> Vec<UserError> {
    let mut errors = Vec::new();
    let text = |field: &str| {
        fields
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    if text("name").trim().is_empty() {
        errors.push(UserError::new(&["input", "name"], "Name can't be blank").code("INVALID"));
    }
    if !is_metric(text("metric")) {
        errors.push(
            UserError::new(
                &["input", "metric"],
                format!("{} is not a metric of the sales schema", text("metric")),
            )
            .code("INVALID_METRIC"),
        );
    }
    let start = NaiveDate::parse_from_str(text("startDate"), "%Y-%m-%d").ok();
    let end = NaiveDate::parse_from_str(text("endDate"), "%Y-%m-%d").ok();
    if let (Some(start), Some(end)) = (start, end)
        && end < start
    {
        errors.push(
            UserError::new(&["input", "endDate"], "End date must be after start date")
                .code("INVALID"),
        );
    }
    if let Some(filters) = fields.get("filters").and_then(Value::as_str)
        && let Err(problem) = shopifyql::check_condition(filters)
    {
        errors.push(UserError::new(&["input", "filters"], problem).code("INVALID"));
    }
    let same = state.all("AnalyticsTarget").any(|target| {
        target.get("id").and_then(Value::as_str) != except
            && ["metric", "startDate", "endDate"]
                .iter()
                .all(|field| target.get(*field) == fields.get(*field))
            && target.get("filters").filter(|f| !f.is_null())
                == fields.get("filters").filter(|f| !f.is_null())
    });
    if same {
        errors.push(
            UserError::new(
                &["input"],
                "A target with this metric, dates and filters already exists",
            )
            .code("TAKEN"),
        );
    }
    errors
}

/// A decimal input as Shopify writes `Decimal`s.
fn decimal(value: &Value) -> Value {
    json!(crate::state::value::decimal(
        crate::domains::catalog::cents_of(Some(value)).unwrap_or(0)
    ))
}

fn target_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input_of(ctx);
    let errors = target_errors(m.state(), &input, None);
    if !errors.is_empty() {
        return Ok(payload(json!({"analyticsTarget": null}), &errors));
    }
    let now = m.timestamp();
    let currency = crate::domains::orders::model::currency(m.state());
    let query = target_query(&input);
    let state = m.state_mut();
    let id = state.new_gid("AnalyticsTarget");
    state.insert_as(
        id.clone(),
        crate::state::value::object(json!({
            "__typename": "AnalyticsTarget",
            "id": id,
            "name": input.get("name"),
            "metric": input.get("metric"),
            "expectedValue": decimal(input.get("expectedValue").unwrap_or(&Value::Null)),
            "currencyCode": currency,
            "startDate": input.get("startDate"),
            "endDate": input.get("endDate"),
            "filters": input.get("filters").cloned().unwrap_or(Value::Null),
            "shopifyqlQuery": query,
            "createdAt": now,
            "updatedAt": now,
        })),
    );
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"analyticsTarget": id}), &[]))
}

fn is_target(state: &ShopState, id: &str) -> bool {
    state.type_of(id) == Some("AnalyticsTarget")
}

fn target_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if !is_target(m.state(), &id) {
        let error = UserError::new(&["id"], "Analytics target not found").code("NOT_FOUND");
        return Ok(payload(json!({"analyticsTarget": null}), &[error]));
    }
    let input = input_of(ctx);
    let merged = merged(m.state(), &id, &input);
    let errors = target_errors(m.state(), &merged, Some(&id));
    if !errors.is_empty() {
        return Ok(payload(json!({"analyticsTarget": null}), &errors));
    }
    let now = m.timestamp();
    let query = target_query(&merged);
    let state = m.state_mut();
    if let Some(record) = state.get_mut(&id) {
        for (field, value) in &input {
            let value = match field.as_str() {
                "expectedValue" => decimal(value),
                _ => value.clone(),
            };
            record.insert(field.clone(), value);
        }
        record.insert("shopifyqlQuery".into(), json!(query));
        record.insert("updatedAt".into(), json!(now));
    }
    state.emit(crate::domains::app::CHANGED, &id);
    Ok(payload(json!({"analyticsTarget": id}), &[]))
}

fn targets_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let ids = crate::domains::catalog::gids(ctx.arg("ids"));
    let mut errors = Vec::new();
    let mut deleted = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        if is_target(m.state(), id) {
            m.state_mut().delete(id, "analytics_targets/delete");
            deleted.push(id.clone());
        } else {
            errors.push(
                UserError::new(&["ids", &index.to_string()], "Analytics target not found")
                    .code("NOT_FOUND"),
            );
        }
    }
    // "If any ID is not found, a user error is returned for that ID while the remaining valid
    // targets are still deleted."
    m.keep_changes();
    Ok(payload(json!({"deletedIds": deleted}), &errors))
}

/// `AnalyticsTarget.presentmentExpectedValue(currencyCode:)`.
fn presentment_expected_value(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(target) = r.map_of(object) else {
        return Ok(Output::Null);
    };
    let wanted = ctx.arg_str("currencyCode").unwrap_or_default();
    let currency = target
        .get("currencyCode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if wanted != currency {
        r.note(format!(
            "presentmentExpectedValue: lsf converts no currency, {wanted} is not the shop's ({currency})"
        ));
        return Ok(Output::Null);
    }
    let mut value = crate::state::Record::new();
    value.insert(
        "amount".into(),
        target.get("expectedValue").cloned().unwrap_or(json!("0.0")),
    );
    value.insert("currencyCode".into(), json!(currency));
    Ok(Output::Object(Obj::built("MoneyV2", value)))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::domains::segments::tests::{data, demo, run};

    #[test]
    fn annotations_are_the_apps_and_typed() {
        let mut shop = demo();
        let create = "mutation($input: AnalyticsAnnotationCreateInput!) { analyticsAnnotationCreate(input: $input) {
            analyticsAnnotation { id type title startedAt endedAt description source } userErrors { field message code } } }";
        let created = run(
            &mut shop,
            create,
            json!({"input": {"type": "campaign", "title": "BFCM cadence", "startedAt": "2026-11-28T00:00:00-05:00",
                "endedAt": "2026-12-01T00:00:00Z", "description": "12 emails"}}),
        );
        let annotation = &data(&created, "analyticsAnnotationCreate")["analyticsAnnotation"];
        assert_eq!(annotation["startedAt"], "2026-11-28T05:00:00Z");
        assert_eq!(annotation["source"], "APP");
        let id = annotation["id"].clone();
        let listed = run(
            &mut shop,
            "{ shop { analyticsAnnotations(first: 5, query: \"type:campaign\") { nodes { title } } } }",
            json!({}),
        );
        assert_eq!(
            listed["data"]["shop"]["analyticsAnnotations"]["nodes"],
            json!([{"title": "BFCM cadence"}])
        );
        let updated = run(
            &mut shop,
            "mutation($id: ID!) { analyticsAnnotationUpdate(id: $id, input: {title: \"Renamed\", endedAt: null}) { analyticsAnnotation { title endedAt } userErrors { code } } }",
            json!({"id": id}),
        );
        assert_eq!(
            data(&updated, "analyticsAnnotationUpdate")["analyticsAnnotation"],
            json!({"title": "Renamed", "endedAt": null})
        );
        let before = shop.picture();
        let refused = run(
            &mut shop,
            create,
            json!({"input": {"type": "app_installed", "title": "x", "startedAt": "2026-11-28T00:00:00Z"}}),
        );
        assert_eq!(
            data(&refused, "analyticsAnnotationCreate")["userErrors"][0]["code"],
            "INVALID"
        );
        let missing = run(
            &mut shop,
            "mutation { analyticsAnnotationDelete(id: \"gid://shopify/AnalyticsAnnotation/9\") { deletedId userErrors { code } } }",
            json!({}),
        );
        assert_eq!(
            data(&missing, "analyticsAnnotationDelete")["userErrors"][0]["code"],
            "NOT_FOUND"
        );
        assert_eq!(shop.picture(), before);
        let deleted = run(
            &mut shop,
            "mutation($id: ID!) { analyticsAnnotationDelete(id: $id) { deletedId } }",
            json!({"id": id}),
        );
        assert_eq!(data(&deleted, "analyticsAnnotationDelete")["deletedId"], id);
    }

    #[test]
    fn targets_are_unique_and_measured_by_their_query() {
        let mut shop = demo();
        let create = "mutation($input: AnalyticsTargetCreateInput!) { analyticsTargetCreate(input: $input) {
            analyticsTarget { id name metric expectedValue currencyCode startDate endDate shopifyqlQuery
                presentmentExpectedValue(currencyCode: USD) { amount currencyCode } } userErrors { field message code } } }";
        let target = json!({"input": {"name": "Q4", "metric": "total_sales", "expectedValue": "50000",
            "startDate": "2026-10-01", "endDate": "2026-12-31"}});
        let created = run(&mut shop, create, target.clone());
        let made = &data(&created, "analyticsTargetCreate")["analyticsTarget"];
        assert_eq!(made["expectedValue"], "50000.0");
        assert_eq!(made["currencyCode"], "USD");
        assert_eq!(
            made["shopifyqlQuery"],
            "FROM sales SHOW total_sales SINCE 2026-10-01 UNTIL 2026-12-31"
        );
        assert_eq!(made["presentmentExpectedValue"]["amount"], "50000.0");
        let query = made["shopifyqlQuery"].as_str().unwrap().to_string();
        let measured = run(
            &mut shop,
            "query($q: String!) { shopifyqlQuery(query: $q) { analyticsTargets { name } parseErrors tableData { rows } } }",
            json!({"q": query}),
        );
        assert_eq!(
            data(&measured, "shopifyqlQuery")["analyticsTargets"],
            json!([{"name": "Q4"}])
        );
        let before = shop.picture();
        let taken = run(&mut shop, create, target);
        assert_eq!(
            data(&taken, "analyticsTargetCreate")["userErrors"][0]["code"],
            "TAKEN"
        );
        let metric = run(
            &mut shop,
            create,
            json!({"input": {"name": "x", "metric": "sessions", "expectedValue": "1", "startDate": "2026-10-01", "endDate": "2026-10-31"}}),
        );
        assert_eq!(
            data(&metric, "analyticsTargetCreate")["userErrors"][0]["code"],
            "INVALID_METRIC"
        );
        assert_eq!(shop.picture(), before);
        let id = made["id"].clone();
        let updated = run(
            &mut shop,
            "mutation($id: ID!) { analyticsTargetUpdate(id: $id, input: {expectedValue: \"60000\"}) { analyticsTarget { expectedValue } userErrors { code } } }",
            json!({"id": id}),
        );
        assert_eq!(
            data(&updated, "analyticsTargetUpdate")["analyticsTarget"]["expectedValue"],
            "60000.0"
        );
        // One found, one not: the found one is deleted all the same.
        let deleted = run(
            &mut shop,
            "mutation($ids: [ID!]!) { analyticsTargetsDelete(ids: $ids) { deletedIds userErrors { field code } } }",
            json!({"ids": [id, "gid://shopify/AnalyticsTarget/99"]}),
        );
        assert_eq!(
            data(&deleted, "analyticsTargetsDelete"),
            &json!({"deletedIds": [id], "userErrors": [{"field": ["ids", "1"], "code": "NOT_FOUND"}]})
        );
        assert_eq!(shop.count("AnalyticsTarget"), 0);
    }

    #[test]
    fn shopifyql_answers_the_sales_of_the_orders() {
        let mut shop = demo();
        let ql = |shop: &mut crate::state::ShopState, query: &str| {
            let answer = run(
                shop,
                "query($q: String!) { shopifyqlQuery(query: $q) { parseErrors parseWarnings
                    tableData { columns { name dataType displayName } rows } } }",
                json!({"q": query}),
            );
            data(&answer, "shopifyqlQuery").clone()
        };
        // Every order of the demo store, at once.
        let all = ql(
            &mut shop,
            "FROM sales SHOW orders, gross_sales, total_sales WITH TOTALS",
        );
        assert_eq!(all["parseErrors"], json!([]), "{all}");
        let orders = shop.count("Order") as i64;
        assert_eq!(all["tableData"]["rows"][0]["orders"], orders);
        assert_eq!(
            all["tableData"]["columns"][0],
            json!({"name": "orders", "dataType": "INTEGER", "displayName": "Orders"})
        );
        // The sales of each order add up to the orders' totals.
        let total: f64 = shop
            .all("Order")
            .map(|order| {
                order["totalPriceSet"]["shopMoney"]["amount"]
                    .as_str()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
            })
            .sum();
        let reported: f64 = all["tableData"]["rows"][0]["total_sales"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let refunded: f64 = shop.all("Refund").count() as f64;
        assert!(
            refunded > 0.0 || (reported - total).abs() < 0.01,
            "{reported} {total}"
        );
        // By month over the year of the demo orders, every month filled in.
        let monthly = ql(
            &mut shop,
            "FROM sales SHOW total_sales TIMESERIES month SINCE 2024-01-01 UNTIL 2024-12-31 ORDER BY month",
        );
        let rows = monthly["tableData"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 12, "{monthly}");
        assert_eq!(rows[0]["month"], "2024-01-01");
        assert_eq!(
            monthly["tableData"]["columns"][0],
            json!({"name": "month", "dataType": "MONTH_TIMESTAMP", "displayName": "Month"})
        );
        // By product, ranked.
        let products = ql(
            &mut shop,
            "FROM sales SHOW net_sales GROUP BY product_title SINCE 2020-01-01 ORDER BY net_sales DESC LIMIT 2",
        );
        assert_eq!(products["tableData"]["rows"].as_array().unwrap().len(), 2);
        // A date past the calendar is not a date, whatever writes it.
        for (query, error) in [
            (
                "FROM sales SHOW orders SINCE -99999999999d",
                "Syntax Error: '-99999999999d' is not a date",
            ),
            (
                "FROM sales SHOW orders UNTIL +99999999999h",
                "Syntax Error: '+99999999999h' is not a date",
            ),
            (
                "FROM sales SHOW orders SINCE startOfMonth(-9999999999m)",
                "Syntax Error: 'startOfMonth(-9999999999m)' is not a date",
            ),
            (
                "FROM sales SHOW orders DURING bfcm999999",
                "Syntax Error: 'bfcm999999' is not a period",
            ),
        ] {
            let answer = ql(&mut shop, query);
            assert_eq!(answer["parseErrors"], json!([error]), "{query}");
        }
        let far = ql(&mut shop, "FROM sales SHOW orders SINCE -10000y");
        assert_eq!(far["parseErrors"], json!([]), "{far}");
        // The reference's error.
        let error = ql(&mut shop, "FROM sales SHOW total_sale");
        assert_eq!(error["tableData"], Value::Null);
        assert_eq!(
            error["parseErrors"],
            json!(["Column Not Found: Column 'total_sale' not found"])
        );
    }
}

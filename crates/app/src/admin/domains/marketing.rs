//! Marketing activities, the marketing events behind them, and their engagements:
//! `marketingActivityUpsertExternal`, `marketingActivityUpdateExternal`,
//! `marketingActivityDeleteExternal`, `marketingActivitiesDeleteAllExternal`,
//! `marketingActivityCreate` and `marketingActivityUpdate` (the deprecated activities of a
//! marketing activity extension), `marketingEngagementCreate`, `marketingEngagementsDelete`; the
//! `marketingActivities`, `marketingActivity`, `marketingEvent` and `marketingEvents` queries.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - An **external activity** is the app's campaign on a channel lsf never talks to: its title,
//!   `remoteId` (the app's own id of it, which `…UpdateExternal`, `…DeleteExternal` and
//!   `marketingEngagementCreate` take instead of the GID), `remoteUrl`, `tactic`,
//!   `marketingChannelType`, `status`, dates, budget and ad spend, UTM parameters or a URL
//!   parameter, and a place in a hierarchy (`CAMPAIGN`, `AD_GROUP`, `AD`, with its parent). Each
//!   has a `MarketingEvent` (`type` the tactic, `remoteId`, `manageUrl` the remote URL,
//!   `previewUrl` the preview image, the UTM parameters, `startedAt`, `endedAt`,
//!   `scheduledToEndAt`) that Shopify attributes sessions and orders to.
//! - `…UpsertExternal` creates an activity, or updates the one of its `remoteId` ("When optional
//!   fields are absent or null, associated information will be removed from an existing
//!   marketing activity"); `…UpdateExternal` updates the one its `marketingActivityId`,
//!   `remoteId` or `utm` names (exactly one); `…DeleteExternal` deletes one by id or remote id;
//!   `…DeleteAllExternal` deletes every external activity of the app as a `Job` (done at once in
//!   lsf: nothing waits for it).
//! - The errors are `MarketingActivityUserErrorCode`'s: the remote id or UTM campaign another
//!   activity has (`MARKETING_ACTIVITY_WITH_REMOTE_ID_ALREADY_EXISTS`,
//!   `…_WITH_UTM_CAMPAIGN_ALREADY_EXISTS`, `…_WITH_URL_PARAMETER_VALUE_ALREADY_EXISTS`), an
//!   activity outside a hierarchy without UTM or URL parameter
//!   (`NON_HIERARCHIAL_REQUIRES_UTM_URL_PARAMETER`), what cannot change once set
//!   (`IMMUTABLE_UTM_PARAMETERS`, `IMMUTABLE_URL_PARAMETER`, `IMMUTABLE_HIERARCHY_LEVEL`,
//!   `IMMUTABLE_PARENT_ID`, `IMMUTABLE_CHANNEL_HANDLE`), an activity that does not exist
//!   (`MARKETING_ACTIVITY_DOES_NOT_EXIST`), one that is not external (`ACTIVITY_NOT_EXTERNAL`), a
//!   parent with children (`CANNOT_DELETE_ACTIVITY_WITH_CHILD_EVENTS`), amounts in two
//!   currencies (`CURRENCY_CODE_MISMATCH_INPUT`), the wrong mix of arguments (`INVALID_…_ARGUMENTS`).
//!   The messages are the codes' descriptions in the schema, as the catalog family does for
//!   errors documented by their code only.
//! - `marketingEngagementCreate` records a day of metrics for the activity its
//!   `marketingActivityId` or `remoteId` names (or, deprecated, a channel by `channelHandle`);
//!   the activity's `adSpend` is the sum of its engagements' (or what its input set last).
//!   `marketingEngagementsDelete` marks a channel's (or every channel's) engagements deleted,
//!   with the reference's result strings.
//! - The deprecated `marketingActivityCreate` takes the GID of a marketing activity extension
//!   (`gid://shopify/MarketingActivityExtension/<id>`) and makes a non-external activity in the
//!   status given; `marketingActivityUpdate` changes one.
//!
//! No webhook topic covers marketing in 2026-10: their changes emit `lsf/changed`. lsf sends
//! nothing to the channels: a marketing activity is the state Shopify would show. They are not
//! store data: they do not sync.

use serde_json::{Map, Value, json};

use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::{Record, ShopState};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("marketingActivityCreateExternal", create_external_mutation)
        .mutation("marketingActivityUpsertExternal", upsert_external)
        .mutation("marketingActivityUpdateExternal", update_external)
        .mutation("marketingActivityDeleteExternal", delete_external)
        .mutation("marketingActivitiesDeleteAllExternal", delete_all_external)
        .mutation("marketingActivityCreate", activity_create)
        .mutation("marketingActivityUpdate", activity_update)
        .mutation("marketingEngagementCreate", engagement_create)
        .mutation("marketingEngagementsDelete", engagements_delete)
        .query("marketingActivities", marketing_activities)
        .search_field("MarketingActivity", "app_name", |r, _item, _activity| {
            let app = r
                .state()
                .all("App")
                .next()
                .and_then(|app| app.get("title").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();
            Some(vec![crate::admin::search::SearchValue::Keyword(app)])
        })
        .search_field(
            "MarketingActivity",
            "scheduled_to_start_at",
            |r, _item, activity| Some(event_date(r, activity, "lsfScheduledStart")),
        )
        .search_field(
            "MarketingActivity",
            "scheduled_to_end_at",
            |r, _item, activity| Some(event_date(r, activity, "scheduledToEndAt")),
        )
        .search_field(
            "MarketingActivity",
            "marketing_campaign_id",
            |_r, _item, activity| {
                Some(
                    activity
                        .get("parentActivityId")
                        .and_then(Value::as_str)
                        .and_then(crate::admin::search::SearchValue::id)
                        .into_iter()
                        .collect(),
                )
            },
        )
        .search_field("MarketingEvent", "type", |_r, _item, event| {
            Some(keyword(event, "type"))
        })
        .search_field("MarketingActivity", "tactic", |_r, _item, activity| {
            Some(keyword(activity, "tactic"))
        })
        .default_text("MarketingActivity", |_r, activity| {
            activity
                .get("title")
                .and_then(Value::as_str)
                .map(|title| vec![title.to_string()])
                .unwrap_or_default()
        })
        .default_text("MarketingEvent", |_r, event| {
            event
                .get("description")
                .and_then(Value::as_str)
                .map(|text| vec![text.to_string()])
                .unwrap_or_default()
        });
}

/// A field of a record as a search keyword.
fn keyword(record: &Record, field: &str) -> Vec<crate::admin::search::SearchValue> {
    record
        .get(field)
        .and_then(Value::as_str)
        .map(|value| crate::admin::search::SearchValue::Keyword(value.to_string()))
        .into_iter()
        .collect()
}

/// A date of an activity's marketing event, for a search filter.
fn event_date(
    r: &ShopResolver<'_>,
    activity: &Record,
    field: &str,
) -> Vec<crate::admin::search::SearchValue> {
    activity
        .get("marketingEvent")
        .and_then(Value::as_str)
        .and_then(|event| r.state().get(event))
        .and_then(|event| event.get(field))
        .and_then(Value::as_str)
        .and_then(crate::admin::search::SearchValue::date)
        .into_iter()
        .collect()
}

/// The description of a code in the schema: the message of an error Shopify documents by its
/// code only.
fn message(m: &Mutation<'_>, code: &str) -> String {
    m.schema()
        .enum_values("MarketingActivityUserErrorCode")
        .into_iter()
        .flatten()
        .find(|value| value.name == code)
        .and_then(|value| value.description.clone())
        .unwrap_or_else(|| code.to_string())
}

fn error(m: &Mutation<'_>, field: &[&str], code: &str) -> UserError {
    UserError::new(field, message(m, code)).code(code)
}

/// A humanized enum value: `DELETED_EXTERNALLY` is "Deleted externally".
fn humanized(value: &str) -> String {
    let lower = value.replace('_', " ").to_lowercase();
    let mut chars = lower.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `sourceAndMedium`: the UTM source (else the channel) and the tactic, as the reference's
/// examples write them ("Facebook ad", "Display newsletter").
fn source_and_medium(utm_source: Option<&str>, channel: &str, tactic: &str) -> String {
    let source = utm_source
        .filter(|source| !source.is_empty())
        .map_or_else(|| humanized(channel), humanized);
    format!("{source} {}", tactic.replace('_', " ").to_lowercase())
}

/// The badge of a status (`BadgeType`) *(chosen: Shopify does not document the mapping)*.
fn badge(status: &str) -> &'static str {
    match status {
        "ACTIVE" => "SUCCESS",
        "SCHEDULED" | "PENDING" => "INFO",
        "PAUSED" | "INACTIVE" | "DRAFT" => "DEFAULT",
        "FAILED" | "DISCONNECTED" => "CRITICAL",
        "DELETED" | "DELETED_EXTERNALLY" => "WARNING",
        _ => "DEFAULT",
    }
}

/// The money of a `MoneyInput`, as a `MoneyV2`.
fn money(input: Option<&Value>) -> Value {
    match input.and_then(Value::as_object) {
        Some(money) => json!({
            "amount": crate::admin::state::value::decimal(crate::admin::state::value::exact_cents(money.get("amount")).unwrap_or(0)),
            "currencyCode": money.get("currencyCode"),
        }),
        None => Value::Null,
    }
}

fn currency_of(money: &Value) -> Option<&str> {
    money.get("currencyCode").and_then(Value::as_str)
}

/// The activity a remote id names.
fn by_remote_id(state: &ShopState, remote: &str) -> Option<String> {
    state
        .all("MarketingActivity")
        .find(|activity| activity.get("lsfRemoteId").and_then(Value::as_str) == Some(remote))
        .and_then(|activity| activity.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

/// The activity whose UTM campaign, medium and source are `utm`'s.
fn by_utm(state: &ShopState, utm: &Map<String, Value>) -> Option<String> {
    state
        .all("MarketingActivity")
        .find(|activity| {
            let params = activity.get("utmParameters");
            ["campaign", "medium", "source"]
                .iter()
                .all(|field| params.and_then(|params| params.get(*field)) == utm.get(*field))
        })
        .and_then(|activity| activity.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

fn is_activity(state: &ShopState, id: &str) -> bool {
    state.type_of(id) == Some("MarketingActivity")
}

/// Checks the amounts of an input: its budget and its ad spend are in one currency.
fn check_currencies(m: &Mutation<'_>, input: &Map<String, Value>, errors: &mut Vec<UserError>) {
    let budget = input
        .get("budget")
        .and_then(|budget| budget.get("total"))
        .and_then(|total| total.get("currencyCode"))
        .and_then(Value::as_str);
    let spend = input
        .get("adSpend")
        .and_then(|spend| spend.get("currencyCode"))
        .and_then(Value::as_str);
    if let (Some(budget), Some(spend)) = (budget, spend)
        && budget != spend
    {
        errors.push(error(m, &["input"], "CURRENCY_CODE_MISMATCH_INPUT"));
    }
}

/// Writes an external input on an activity and its event. `replace`: absent optional fields
/// clear what the activity had (an upsert).
fn write_external(
    state: &mut ShopState,
    activity: &str,
    input: &Map<String, Value>,
    replace: bool,
    now: &str,
) {
    let get = |field: &str| input.get(field).filter(|value| !value.is_null());
    let Some(record) = state.get(activity).cloned() else {
        return;
    };
    let event = record
        .get("marketingEvent")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut activity_record = record;
    let set = |record: &mut Record, field: &str, value: Option<Value>| match value {
        Some(value) => {
            record.insert(field.into(), value);
        }
        None if replace => {
            record.insert(field.into(), Value::Null);
        }
        None => {}
    };
    let tactic = get("tactic").cloned();
    let channel = get("marketingChannelType")
        .or_else(|| get("channel"))
        .cloned();
    let status = get("status").cloned();
    set(&mut activity_record, "title", get("title").cloned());
    if let Some(tactic) = tactic.clone() {
        activity_record.insert("tactic".into(), tactic);
    }
    if let Some(channel) = channel.clone() {
        activity_record.insert("marketingChannelType".into(), channel.clone());
        activity_record.insert("marketingChannel".into(), channel);
    }
    if let Some(status) = status {
        let before = activity_record.get("status").cloned();
        if before.as_ref() != Some(&status) {
            activity_record.insert("statusTransitionedAt".into(), json!(now));
        }
        activity_record.insert("status".into(), status);
    }
    let status = activity_record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("UNDEFINED")
        .to_string();
    activity_record.insert("statusLabel".into(), json!(humanized(&status)));
    activity_record.insert("statusBadgeTypeV2".into(), json!(badge(&status)));
    set(
        &mut activity_record,
        "budget",
        get("budget").map(|budget| {
            json!({
                "budgetType": budget.get("budgetType").cloned().unwrap_or(json!("DAILY")),
                "total": money(budget.get("total")),
            })
        }),
    );
    if let Some(spend) = get("adSpend") {
        activity_record.insert("adSpend".into(), money(Some(spend)));
    } else if replace {
        activity_record.insert("adSpend".into(), Value::Null);
    }
    if let Some(utm) = get("utm") {
        activity_record.insert(
            "utmParameters".into(),
            json!({"campaign": utm.get("campaign"), "medium": utm.get("medium"), "source": utm.get("source"), "content": null, "term": null}),
        );
    }
    if let Some(value) = get("urlParameterValue") {
        activity_record.insert("urlParameterValue".into(), value.clone());
    }
    if let Some(level) = get("hierarchyLevel") {
        activity_record.insert("hierarchyLevel".into(), level.clone());
    }
    let utm_source = activity_record
        .get("utmParameters")
        .and_then(|utm| utm.get("source"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let channel_name = activity_record
        .get("marketingChannelType")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tactic_name = activity_record
        .get("tactic")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let source = source_and_medium(utm_source.as_deref(), &channel_name, &tactic_name);
    activity_record.insert("sourceAndMedium".into(), json!(source));
    activity_record.insert("updatedAt".into(), json!(now));
    let utm = activity_record.get("utmParameters").cloned();
    let title = activity_record.get("title").cloned();
    if let Some(slot) = state.get_mut(activity) {
        *slot = activity_record;
    }
    let Some(event) = event else {
        return;
    };
    if let Some(record) = state.get_mut(&event) {
        if let Some(tactic) = tactic {
            record.insert("type".into(), tactic);
        }
        if let Some(channel) = channel {
            record.insert("marketingChannelType".into(), channel.clone());
            record.insert("channel".into(), channel);
        }
        record.insert("description".into(), title.unwrap_or(Value::Null));
        for (field, from) in [
            ("manageUrl", "remoteUrl"),
            ("previewUrl", "remotePreviewImageUrl"),
            ("scheduledToEndAt", "scheduledEnd"),
            ("lsfScheduledStart", "scheduledStart"),
            ("endedAt", "end"),
        ] {
            set(record, field, get(from).cloned());
        }
        if let Some(start) = get("start").or_else(|| get("scheduledStart")) {
            record.insert("startedAt".into(), start.clone());
        }
        if let Some(utm) = utm.filter(|utm| !utm.is_null()) {
            record.insert("utmCampaign".into(), utm["campaign"].clone());
            record.insert("utmMedium".into(), utm["medium"].clone());
            record.insert("utmSource".into(), utm["source"].clone());
        }
        record.insert("sourceAndMedium".into(), json!(source.clone()));
        record.insert("targetTypeDisplayText".into(), json!(source));
    }
}

/// Creates an external activity and its event from an input of `…UpsertExternal`.
fn create_external(state: &mut ShopState, input: &Map<String, Value>, now: &str) -> String {
    let app = state.app_gid().map(str::to_string);
    let activity = state.new_gid("MarketingActivity");
    let event = state.new_gid("MarketingEvent");
    let parent = input
        .get("parentRemoteId")
        .and_then(Value::as_str)
        .and_then(|remote| by_remote_id(state, remote))
        .or_else(|| {
            input
                .get("parentActivityId")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    state.insert_as(
        event.clone(),
        crate::admin::state::value::object(json!({
            "__typename": "MarketingEvent",
            "id": event,
            "app": app,
            "remoteId": input.get("remoteId"),
            "channelHandle": input.get("channelHandle"),
            "startedAt": input.get("start").or_else(|| input.get("scheduledStart")).cloned().unwrap_or_else(|| json!(now)),
            "endedAt": null,
            "scheduledToEndAt": null,
            "manageUrl": null,
            "previewUrl": null,
            "utmCampaign": null,
            "utmMedium": null,
            "utmSource": null,
            "description": null,
            "createdAt": now,
        })),
    );
    state.insert_as(
        activity.clone(),
        crate::admin::state::value::object(json!({
            "__typename": "MarketingActivity",
            "id": activity,
            "app": app,
            "isExternal": true,
            "inMainWorkflowVersion": false,
            "marketingEvent": event,
            "status": "UNDEFINED",
            "formData": null,
            "activityListUrl": null,
            "appErrors": null,
            "targetStatus": null,
            "statusTransitionedAt": null,
            "hierarchyLevel": null,
            "parentActivityId": parent,
            "parentRemoteId": input.get("parentRemoteId"),
            "urlParameterValue": null,
            "utmParameters": null,
            "budget": null,
            "adSpend": null,
            "createdAt": now,
            "updatedAt": now,
            // The app's id of the activity, which GraphQL's `MarketingActivity` does not show.
            "lsfRemoteId": input.get("remoteId"),
            "lsfChannelHandle": input.get("channelHandle"),
            "lsfEngagements": [],
        })),
    );
    write_external(state, &activity, input, true, now);
    activity
}

/// What an upsert or an update may not change of an existing activity.
fn immutable(
    m: &Mutation<'_>,
    existing: &Record,
    input: &Map<String, Value>,
    errors: &mut Vec<UserError>,
) {
    let had = |field: &str| existing.get(field).filter(|value| !value.is_null());
    let given = |field: &str| input.get(field).filter(|value| !value.is_null());
    if let (Some(before), Some(utm)) = (had("utmParameters"), given("utm")) {
        let same = ["campaign", "medium", "source"]
            .iter()
            .all(|field| before.get(*field) == utm.get(*field));
        if !same {
            errors.push(error(m, &["input", "utm"], "IMMUTABLE_UTM_PARAMETERS"));
        }
    }
    for (record_field, input_field, code) in [
        (
            "urlParameterValue",
            "urlParameterValue",
            "IMMUTABLE_URL_PARAMETER",
        ),
        (
            "hierarchyLevel",
            "hierarchyLevel",
            "IMMUTABLE_HIERARCHY_LEVEL",
        ),
        ("parentRemoteId", "parentRemoteId", "IMMUTABLE_PARENT_ID"),
        (
            "lsfChannelHandle",
            "channelHandle",
            "IMMUTABLE_CHANNEL_HANDLE",
        ),
    ] {
        if let (Some(before), Some(after)) = (had(record_field), given(input_field))
            && before != after
        {
            errors.push(error(m, &["input", input_field], code));
        }
    }
}

/// The UTM campaign or URL parameter another activity already has.
fn taken(
    m: &Mutation<'_>,
    input: &Map<String, Value>,
    except: Option<&str>,
    errors: &mut Vec<UserError>,
) {
    let others = || {
        m.state()
            .all("MarketingActivity")
            .filter(move |activity| activity.get("id").and_then(Value::as_str) != except)
    };
    if let Some(campaign) = input
        .get("utm")
        .and_then(|utm| utm.get("campaign"))
        .filter(|value| !value.is_null())
        && others().any(|activity| {
            activity
                .get("utmParameters")
                .and_then(|utm| utm.get("campaign"))
                == Some(campaign)
        })
    {
        errors.push(error(
            m,
            &["input", "utm", "campaign"],
            "MARKETING_ACTIVITY_WITH_UTM_CAMPAIGN_ALREADY_EXISTS",
        ));
    }
    if let Some(value) = input
        .get("urlParameterValue")
        .filter(|value| !value.is_null())
        && others().any(|activity| activity.get("urlParameterValue") == Some(value))
    {
        errors.push(error(
            m,
            &["input", "urlParameterValue"],
            "MARKETING_ACTIVITY_WITH_URL_PARAMETER_VALUE_ALREADY_EXISTS",
        ));
    }
}

/// What an external activity's input must hold, created or upserted: a hierarchy, or UTM
/// parameters or a URL parameter; one currency.
fn check_external(m: &Mutation<'_>, input: &Map<String, Value>, errors: &mut Vec<UserError>) {
    let hierarchical = input
        .get("hierarchyLevel")
        .is_some_and(|level| !level.is_null());
    let has_utm = input.get("utm").is_some_and(|utm| !utm.is_null());
    let has_parameter = input
        .get("urlParameterValue")
        .is_some_and(|value| !value.is_null());
    if !hierarchical && !has_utm && !has_parameter {
        errors.push(error(
            m,
            &["input"],
            "NON_HIERARCHIAL_REQUIRES_UTM_URL_PARAMETER",
        ));
    }
    check_currencies(m, input, errors);
}

/// `marketingActivityCreateExternal`, deprecated for `marketingActivityUpsertExternal`: the
/// same activity, created only (a remote id another activity has is
/// `MARKETING_ACTIVITY_WITH_REMOTE_ID_ALREADY_EXISTS`); its input's deprecated `channel` is read
/// when `marketingChannelType` is not given.
fn create_external_mutation(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let mut input = ctx.arg_object_or_default("input");
    if input.get("marketingChannelType").is_none_or(Value::is_null)
        && let Some(channel) = input.get("channel").cloned()
    {
        input.insert("marketingChannelType".into(), channel);
    }
    let mut errors = Vec::new();
    if let Some(remote) = input.get("remoteId").and_then(Value::as_str)
        && by_remote_id(m.state(), remote).is_some()
    {
        errors.push(error(
            m,
            &["input", "remoteId"],
            "MARKETING_ACTIVITY_WITH_REMOTE_ID_ALREADY_EXISTS",
        ));
    }
    check_external(m, &input, &mut errors);
    taken(m, &input, None, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"marketingActivity": null}), &errors));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    let activity = create_external(state, &input, &now);
    state.emit(crate::admin::domains::app::CHANGED, &activity);
    Ok(payload(json!({"marketingActivity": activity}), &[]))
}

fn upsert_external(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let remote = input
        .get("remoteId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let existing = by_remote_id(m.state(), &remote);
    let mut errors = Vec::new();
    if remote.trim().is_empty() {
        errors.push(error(m, &["input", "remoteId"], "INVALID_REMOTE_ID"));
    }
    check_external(m, &input, &mut errors);
    if let Some(existing) = &existing
        && let Some(record) = m.state().get(existing)
    {
        immutable(m, record, &input, &mut errors);
        if let (Some(before), Some(spend)) = (
            record.get("adSpend").and_then(currency_of),
            input.get("adSpend").and_then(currency_of),
        ) && before != spend
        {
            errors.push(error(
                m,
                &["input", "adSpend"],
                "MARKETING_ACTIVITY_CURRENCY_CODE_MISMATCH",
            ));
        }
    }
    taken(m, &input, existing.as_deref(), &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"marketingActivity": null}), &errors));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    let activity = match existing {
        Some(activity) => {
            write_external(state, &activity, &input, true, &now);
            activity
        }
        None => create_external(state, &input, &now),
    };
    state.emit(crate::admin::domains::app::CHANGED, &activity);
    Ok(payload(json!({"marketingActivity": activity}), &[]))
}

/// The activity `…UpdateExternal` names: exactly one of its id, remote id and UTM.
fn named_activity(m: &Mutation<'_>, ctx: &FieldContext<'_>) -> Result<String, UserError> {
    let id = ctx.arg_str("marketingActivityId");
    let remote = ctx.arg_str("remoteId");
    let utm = ctx.arg_object("utm");
    let given = [id.is_some(), remote.is_some(), utm.is_some()]
        .iter()
        .filter(|given| **given)
        .count();
    if given != 1 {
        return Err(error(
            m,
            &[],
            "INVALID_MARKETING_ACTIVITY_EXTERNAL_ARGUMENTS",
        ));
    }
    let found = match (id, remote, utm) {
        (Some(id), _, _) => is_activity(m.state(), id).then(|| id.to_string()),
        (_, Some(remote), _) => by_remote_id(m.state(), remote),
        (_, _, Some(utm)) => by_utm(m.state(), utm),
        _ => None,
    };
    found.ok_or_else(|| error(m, &[], "MARKETING_ACTIVITY_DOES_NOT_EXIST"))
}

fn update_external(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let activity = match named_activity(m, ctx) {
        Ok(activity) => activity,
        Err(error) => return Ok(payload(json!({"marketingActivity": null}), &[error])),
    };
    let mut errors = Vec::new();
    let external = m
        .state()
        .get(&activity)
        .and_then(|record| record.get("isExternal"))
        .and_then(Value::as_bool)
        == Some(true);
    if !external {
        errors.push(error(m, &[], "ACTIVITY_NOT_EXTERNAL"));
    }
    check_currencies(m, &input, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"marketingActivity": null}), &errors));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    write_external(state, &activity, &input, false, &now);
    state.emit(crate::admin::domains::app::CHANGED, &activity);
    Ok(payload(json!({"marketingActivity": activity}), &[]))
}

/// Deletes an activity and its event.
fn remove_activity(state: &mut ShopState, activity: &str) {
    let event = state
        .get(activity)
        .and_then(|record| record.get("marketingEvent"))
        .and_then(Value::as_str)
        .map(str::to_string);
    state.delete(activity, "marketing_activities/delete");
    if let Some(event) = event {
        state.delete(&event, "marketing_events/delete");
    }
}

fn delete_external(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("marketingActivityId");
    let remote = ctx.arg_str("remoteId");
    let found = match (id, remote) {
        (Some(_), Some(_)) | (None, None) => {
            let error = error(m, &[], "INVALID_DELETE_ACTIVITY_EXTERNAL_ARGUMENTS");
            return Ok(payload(
                json!({"deletedMarketingActivityId": null}),
                &[error],
            ));
        }
        (Some(id), None) => is_activity(m.state(), id).then(|| id.to_string()),
        (None, Some(remote)) => by_remote_id(m.state(), remote),
    };
    let Some(activity) = found else {
        let error = error(m, &[], "MARKETING_ACTIVITY_DOES_NOT_EXIST");
        return Ok(payload(
            json!({"deletedMarketingActivityId": null}),
            &[error],
        ));
    };
    let has_children = m.state().all("MarketingActivity").any(|record| {
        record.get("parentActivityId").and_then(Value::as_str) == Some(activity.as_str())
    });
    if has_children {
        let error = error(m, &[], "CANNOT_DELETE_ACTIVITY_WITH_CHILD_EVENTS");
        return Ok(payload(
            json!({"deletedMarketingActivityId": null}),
            &[error],
        ));
    }
    remove_activity(m.state_mut(), &activity);
    Ok(payload(
        json!({"deletedMarketingActivityId": activity}),
        &[],
    ))
}

fn delete_all_external(m: &mut Mutation<'_>, _ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let external: Vec<String> = m
        .state()
        .all("MarketingActivity")
        .filter(|record| record.get("isExternal").and_then(Value::as_bool) == Some(true))
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let state = m.state_mut();
    for activity in &external {
        remove_activity(state, activity);
    }
    let job = crate::admin::resolve::done_job(state, "marketing_activities_delete_all");
    state.emit(crate::admin::domains::app::CHANGED, &job);
    Ok(payload(json!({"job": job}), &[]))
}

fn activity_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let extension = input
        .get("marketingActivityExtensionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if crate::admin::state::gid_type(&extension) != Some("MarketingActivityExtension") {
        // lsf's words: Shopify does not publish this error.
        let error = UserError::new(
            &["input", "marketingActivityExtensionId"],
            "Marketing activity extension does not exist",
        );
        return Ok(payload(
            json!({"marketingActivity": null, "redirectPath": null}),
            &[error],
        ));
    }
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("DRAFT")
        .to_string();
    let now = m.timestamp();
    let state = m.state_mut();
    let app = state.app_gid().map(str::to_string);
    let activity = state.new_gid("MarketingActivity");
    let title = input
        .get("marketingActivityTitle")
        .and_then(Value::as_str)
        .unwrap_or("Marketing activity")
        .to_string();
    state.insert_as(
        activity.clone(),
        crate::admin::state::value::object(json!({
            "__typename": "MarketingActivity",
            "id": activity,
            "app": app,
            "title": title,
            "isExternal": false,
            "inMainWorkflowVersion": false,
            "marketingEvent": null,
            "status": status,
            "statusLabel": humanized(&status),
            "statusBadgeTypeV2": badge(&status),
            "statusTransitionedAt": now,
            "targetStatus": null,
            "tactic": "NEWSLETTER",
            "marketingChannelType": "EMAIL",
            "marketingChannel": "EMAIL",
            "sourceAndMedium": source_and_medium(None, "EMAIL", "NEWSLETTER"),
            "formData": input.get("formData"),
            "urlParameterValue": input.get("urlParameterValue"),
            "utmParameters": input.get("utm").filter(|utm| !utm.is_null()).map(|utm| json!({"campaign": utm.get("campaign"), "medium": utm.get("medium"), "source": utm.get("source"), "content": null, "term": null})),
            "budget": input.get("budget").filter(|b| !b.is_null()).map(|budget| json!({"budgetType": budget.get("budgetType").cloned().unwrap_or(json!("DAILY")), "total": money(budget.get("total"))})),
            "adSpend": null,
            "hierarchyLevel": null,
            "parentActivityId": null,
            "parentRemoteId": null,
            "activityListUrl": null,
            "appErrors": null,
            "createdAt": now,
            "updatedAt": now,
            "lsfExtension": extension,
            "lsfEngagements": [],
        })),
    );
    state.emit(crate::admin::domains::app::CHANGED, &activity);
    let redirect = format!(
        "/marketing/activities/{}",
        crate::admin::state::gid_number(&activity)
    );
    Ok(payload(
        json!({"marketingActivity": activity, "redirectPath": redirect}),
        &[],
    ))
}

fn activity_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is_activity(m.state(), &id) {
        let error = UserError::new(&["input", "id"], "Marketing activity does not exist");
        return Ok(payload(
            json!({"marketingActivity": null, "redirectPath": null}),
            &[error],
        ));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    if let Some(record) = state.get_mut(&id) {
        if let Some(title) = input.get("title").filter(|t| !t.is_null()) {
            record.insert("title".into(), title.clone());
        }
        if let Some(status) = input.get("status").and_then(Value::as_str) {
            record.insert("status".into(), json!(status));
            record.insert("statusLabel".into(), json!(humanized(status)));
            record.insert("statusBadgeTypeV2".into(), json!(badge(status)));
            record.insert("statusTransitionedAt".into(), json!(now));
        }
        if let Some(status) = input.get("targetStatus").filter(|t| !t.is_null()) {
            record.insert("targetStatus".into(), status.clone());
        }
        if let Some(budget) = input.get("budget").filter(|b| !b.is_null()) {
            record.insert(
                "budget".into(),
                json!({"budgetType": budget.get("budgetType").cloned().unwrap_or(json!("DAILY")), "total": money(budget.get("total"))}),
            );
        }
        if let Some(spend) = input.get("adSpend").filter(|s| !s.is_null()) {
            record.insert("adSpend".into(), money(Some(spend)));
        }
        for field in ["formData", "urlParameterValue"] {
            if let Some(value) = input.get(field).filter(|v| !v.is_null()) {
                record.insert(field.into(), value.clone());
            }
        }
        if let Some(utm) = input.get("utm").filter(|utm| !utm.is_null()) {
            record.insert(
                "utmParameters".into(),
                json!({"campaign": utm.get("campaign"), "medium": utm.get("medium"), "source": utm.get("source"), "content": null, "term": null}),
            );
        }
        if let Some(errors) = input.get("errors").filter(|e| !e.is_null()) {
            record.insert(
                "appErrors".into(),
                json!({"code": "VALIDATION_ERROR", "userErrors": errors.as_array().cloned().unwrap_or_else(|| vec![json!({"field": null, "message": errors.to_string()})])}),
            );
        }
        record.insert("updatedAt".into(), json!(now));
    }
    state.emit(crate::admin::domains::app::CHANGED, &id);
    Ok(payload(
        json!({"marketingActivity": id, "redirectPath": null}),
        &[],
    ))
}

fn engagement_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let engagement = ctx.arg_object_or_default("marketingEngagement");
    let id = ctx.arg_str("marketingActivityId");
    let remote = ctx.arg_str("remoteId");
    let channel = ctx.arg_str("channelHandle");
    let given = [id.is_some(), remote.is_some(), channel.is_some()]
        .iter()
        .filter(|given| **given)
        .count();
    if given == 0 {
        let error = error(m, &[], "INVALID_MARKETING_ENGAGEMENT_ARGUMENT_MISSING");
        return Ok(payload(json!({"marketingEngagement": null}), &[error]));
    }
    if given > 1 {
        let error = error(m, &[], "INVALID_MARKETING_ENGAGEMENT_ARGUMENTS");
        return Ok(payload(json!({"marketingEngagement": null}), &[error]));
    }
    let activity = match (id, remote) {
        (Some(id), _) => Some(is_activity(m.state(), id).then(|| id.to_string())),
        (_, Some(remote)) => Some(by_remote_id(m.state(), remote)),
        _ => None,
    };
    if let Some(None) = activity {
        let error = error(m, &[], "MARKETING_ACTIVITY_DOES_NOT_EXIST");
        return Ok(payload(json!({"marketingEngagement": null}), &[error]));
    }
    if channel.is_some_and(|handle| handle.trim().is_empty()) {
        let error = error(m, &["channelHandle"], "INVALID_CHANNEL_HANDLE");
        return Ok(payload(json!({"marketingEngagement": null}), &[error]));
    }
    let activity = activity.flatten();
    let decimal = |field: &str| {
        engagement
            .get(field)
            .filter(|value| !value.is_null())
            .map(|value| {
                json!(crate::admin::state::value::decimal(
                    crate::admin::state::value::exact_cents(Some(value)).unwrap_or(0)
                ))
            })
            .unwrap_or(Value::Null)
    };
    let int = |field: &str| engagement.get(field).cloned().unwrap_or(Value::Null);
    let record = json!({
        "occurredOn": engagement.get("occurredOn"),
        "utcOffset": engagement.get("utcOffset"),
        "isCumulative": engagement.get("isCumulative").and_then(Value::as_bool).unwrap_or(false),
        "adSpend": money(engagement.get("adSpend")),
        "sales": money(engagement.get("sales")),
        "clicksCount": int("clicksCount"),
        "impressionsCount": int("impressionsCount"),
        "commentsCount": int("commentsCount"),
        "favoritesCount": int("favoritesCount"),
        "unsubscribesCount": int("unsubscribesCount"),
        "complaintsCount": int("complaintsCount"),
        "failsCount": int("failsCount"),
        "sendsCount": int("sendsCount"),
        "uniqueViewsCount": int("uniqueViewsCount"),
        "uniqueClicksCount": int("uniqueClicksCount"),
        "sharesCount": int("sharesCount"),
        "viewsCount": int("viewsCount"),
        "sessionsCount": int("sessionsCount"),
        "orders": decimal("orders"),
        "firstTimeCustomers": decimal("firstTimeCustomers"),
        "returningCustomers": decimal("returningCustomers"),
        "primaryConversions": decimal("primaryConversions"),
        "allConversions": decimal("allConversions"),
        "marketingActivity": activity,
        "channelHandle": channel,
    });
    let state = m.state_mut();
    match &activity {
        Some(activity) => {
            if let Some(stored) = state.get_mut(activity) {
                let mut engagements = stored
                    .get("lsfEngagements")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                engagements.push(record.clone());
                // The activity's ad spend: what its engagements spent.
                let mut currency = None;
                let spent: i64 = engagements
                    .iter()
                    .filter_map(|engagement| {
                        let spend = engagement.get("adSpend").filter(|s| !s.is_null())?;
                        currency = spend.get("currencyCode").cloned();
                        crate::admin::state::value::exact_cents(spend.get("amount"))
                    })
                    .sum();
                if let Some(currency) = currency {
                    stored.insert(
                        "adSpend".into(),
                        json!({"amount": crate::admin::state::value::decimal(spent), "currencyCode": currency}),
                    );
                }
                stored.insert("lsfEngagements".into(), json!(engagements));
            }
            state.emit(crate::admin::domains::app::CHANGED, activity);
        }
        None => {
            let key = format!("marketing/engagements/{}", channel.unwrap_or_default());
            let mut kept = state
                .memo(&key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            kept.push(record.clone());
            state.set_memo(key, json!(kept));
            let shop = state.shop_gid().unwrap_or_default().to_string();
            state.emit(crate::admin::domains::app::CHANGED, &shop);
        }
    }
    let value = record.as_object().cloned().unwrap_or_default();
    Ok(payload(
        json!({"marketingEngagement": Value::Object(value)}),
        &[],
    ))
}

fn engagements_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let channel = ctx.arg_str("channelHandle").map(str::to_string);
    let all = ctx.arg_bool("deleteEngagementsForAllChannels") == Some(true);
    if channel.is_some() == all {
        let error = error(m, &[], "INVALID_DELETE_ENGAGEMENTS_ARGUMENTS");
        return Ok(payload(json!({"result": null}), &[error]));
    }
    let state = m.state_mut();
    let channels: Vec<String> = match &channel {
        Some(channel) => vec![channel.clone()],
        None => {
            let mut handles: Vec<String> = state
                .all("MarketingActivity")
                .filter_map(|activity| activity.get("lsfChannelHandle").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            handles.sort();
            handles.dedup();
            handles
        }
    };
    for handle in &channels {
        state.set_memo(format!("marketing/engagements/{handle}"), Value::Null);
    }
    let shop = state.shop_gid().unwrap_or_default().to_string();
    state.emit(crate::admin::domains::app::CHANGED, &shop);
    // The result strings of the reference's examples.
    let result = match channel {
        Some(handle) => {
            format!("Engagement data associated to channel handle '{handle}' marked for deletion")
        }
        None => format!(
            "Engagement data marked for deletion for {} channel(s)",
            channels.len()
        ),
    };
    Ok(payload(json!({"result": result}), &[]))
}

/// `marketingActivities(marketingActivityIds:, remoteIds:, utm:, …)`: the activities the lists
/// and the UTM name, then the generic connection (`query`, `sortKey`, paging).
fn marketing_activities(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let ids: Vec<&str> = ctx
        .arg("marketingActivityIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let remotes: Vec<&str> = ctx
        .arg("remoteIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let utm = ctx.arg_object("utm");
    let items = r
        .state()
        .all("MarketingActivity")
        .filter(|activity| {
            let id = activity
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let remote = activity.get("lsfRemoteId").and_then(Value::as_str);
            (ids.is_empty() || ids.contains(&id))
                && (remotes.is_empty() || remote.is_some_and(|remote| remotes.contains(&remote)))
                && utm.is_none_or(|utm| {
                    ["campaign", "medium", "source"].iter().all(|field| {
                        activity
                            .get("utmParameters")
                            .and_then(|params| params.get(*field))
                            == utm.get(*field)
                    })
                })
        })
        .filter_map(|activity| activity.get("id").cloned())
        .collect();
    r.connection(ctx, items)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::admin::domains::segments::tests::{data, demo, run};

    const UPSERT: &str = "mutation($input: MarketingActivityUpsertExternalInput!) {
        marketingActivityUpsertExternal(input: $input) {
            marketingActivity { id title status statusLabel isExternal tactic marketingChannelType sourceAndMedium
                utmParameters { campaign medium source } adSpend { amount currencyCode } budget { budgetType total { amount } }
                marketingEvent { id type remoteId manageUrl previewUrl utmCampaign utmSource sourceAndMedium startedAt } }
            userErrors { field message code } } }";

    fn input(remote: &str) -> Value {
        json!({
            "remoteId": remote,
            "title": "Summer ad",
            "status": "ACTIVE",
            "remoteUrl": "https://ads.example.com/campaigns/1",
            "remotePreviewImageUrl": "https://ads.example.com/preview.png",
            "tactic": "AD",
            "marketingChannelType": "SOCIAL",
            "utm": {"campaign": "summer", "medium": "cpc", "source": "facebook"},
            "budget": {"budgetType": "DAILY", "total": {"amount": "10.00", "currencyCode": "USD"}},
            "start": "2026-10-01T00:00:00Z",
        })
    }

    #[test]
    fn an_external_activity_is_upserted_updated_and_deleted() {
        let mut shop = demo();
        let created = run(&mut shop, UPSERT, json!({"input": input("ad-1")}));
        let payload = data(&created, "marketingActivityUpsertExternal");
        assert_eq!(payload["userErrors"], json!([]), "{created}");
        let activity = &payload["marketingActivity"];
        assert_eq!(activity["title"], "Summer ad");
        assert_eq!(activity["statusLabel"], "Active");
        assert_eq!(activity["isExternal"], true);
        assert_eq!(activity["sourceAndMedium"], "Facebook ad");
        assert_eq!(activity["marketingEvent"]["remoteId"], "ad-1");
        assert_eq!(
            activity["marketingEvent"]["manageUrl"],
            "https://ads.example.com/campaigns/1"
        );
        assert_eq!(activity["marketingEvent"]["utmCampaign"], "summer");
        let id = activity["id"].as_str().unwrap().to_string();

        // The same remote id updates it; what the input leaves out is removed.
        let mut again = input("ad-1");
        again["title"] = json!("Summer ad, round 2");
        again.as_object_mut().unwrap().remove("budget");
        let upserted = run(&mut shop, UPSERT, json!({"input": again}));
        let activity = &data(&upserted, "marketingActivityUpsertExternal")["marketingActivity"];
        assert_eq!(activity["id"], json!(id));
        assert_eq!(activity["title"], "Summer ad, round 2");
        assert_eq!(activity["budget"], Value::Null);

        let updated = run(
            &mut shop,
            "mutation { marketingActivityUpdateExternal(remoteId: \"ad-1\", input: {status: PAUSED}) {
                marketingActivity { status statusLabel } userErrors { code } } }",
            json!({}),
        );
        assert_eq!(
            data(&updated, "marketingActivityUpdateExternal")["marketingActivity"],
            json!({"status": "PAUSED", "statusLabel": "Paused"})
        );
        let listed = run(
            &mut shop,
            "{ marketingActivities(first: 5, remoteIds: [\"ad-1\"]) { nodes { id } }
               byUtm: marketingActivities(first: 5, utm: {campaign: \"summer\", medium: \"cpc\", source: \"facebook\"}) { nodes { id } }
               marketingEvents(first: 5, query: \"type:AD\") { nodes { remoteId type } } }",
            json!({}),
        );
        assert_eq!(
            data(&listed, "marketingActivities")["nodes"],
            json!([{"id": id}])
        );
        assert_eq!(listed["data"]["byUtm"]["nodes"], json!([{"id": id}]));
        assert_eq!(
            data(&listed, "marketingEvents")["nodes"],
            json!([{"remoteId": "ad-1", "type": "AD"}])
        );

        let deleted = run(
            &mut shop,
            "mutation { marketingActivityDeleteExternal(remoteId: \"ad-1\") { deletedMarketingActivityId userErrors { code } } }",
            json!({}),
        );
        assert_eq!(
            data(&deleted, "marketingActivityDeleteExternal")["deletedMarketingActivityId"],
            json!(id)
        );
        assert_eq!(shop.count("MarketingActivity"), 0);
        assert_eq!(shop.count("MarketingEvent"), 0);
    }

    #[test]
    fn a_bad_external_activity_changes_nothing() {
        let mut shop = demo();
        run(&mut shop, UPSERT, json!({"input": input("ad-1")}));
        let before = shop.picture();
        // Another remote id with the same UTM campaign.
        let taken = run(&mut shop, UPSERT, json!({"input": input("ad-2")}));
        assert_eq!(
            data(&taken, "marketingActivityUpsertExternal")["userErrors"][0]["code"],
            "MARKETING_ACTIVITY_WITH_UTM_CAMPAIGN_ALREADY_EXISTS"
        );
        // Neither a hierarchy nor a UTM or URL parameter.
        let mut bare = input("ad-3");
        bare.as_object_mut().unwrap().remove("utm");
        let refused = run(&mut shop, UPSERT, json!({"input": bare}));
        assert_eq!(
            data(&refused, "marketingActivityUpsertExternal")["userErrors"][0]["code"],
            "NON_HIERARCHIAL_REQUIRES_UTM_URL_PARAMETER"
        );
        // The UTM of an activity does not change.
        let mut moved = input("ad-1");
        moved["utm"]["campaign"] = json!("winter");
        let immutable = run(&mut shop, UPSERT, json!({"input": moved}));
        assert_eq!(
            data(&immutable, "marketingActivityUpsertExternal")["userErrors"][0]["code"],
            "IMMUTABLE_UTM_PARAMETERS"
        );
        let missing = run(
            &mut shop,
            "mutation { marketingActivityUpdateExternal(remoteId: \"nope\", input: {title: \"x\"}) { userErrors { message code } } }",
            json!({}),
        );
        let error = &data(&missing, "marketingActivityUpdateExternal")["userErrors"][0];
        assert_eq!(error["code"], "MARKETING_ACTIVITY_DOES_NOT_EXIST");
        assert!(!error["message"].as_str().unwrap().is_empty());
        let both = run(
            &mut shop,
            "mutation { marketingActivityDeleteExternal { userErrors { code } } }",
            json!({}),
        );
        assert_eq!(
            data(&both, "marketingActivityDeleteExternal")["userErrors"][0]["code"],
            "INVALID_DELETE_ACTIVITY_EXTERNAL_ARGUMENTS"
        );
        assert_eq!(shop.picture(), before);
    }

    #[test]
    fn engagements_add_up_and_activities_go_all_at_once() {
        let mut shop = demo();
        run(&mut shop, UPSERT, json!({"input": input("ad-1")}));
        let engage = "mutation($engagement: MarketingEngagementInput!) {
            marketingEngagementCreate(remoteId: \"ad-1\", marketingEngagement: $engagement) {
                marketingEngagement { occurredOn clicksCount adSpend { amount currencyCode } orders marketingActivity { title } }
                userErrors { code } } }";
        let engagement = json!({"occurredOn": "2026-10-30", "utcOffset": "+00:00", "clicksCount": 40,
            "adSpend": {"amount": "19.90", "currencyCode": "USD"}, "orders": "8"});
        let answer = run(&mut shop, engage, json!({"engagement": engagement}));
        assert_eq!(
            data(&answer, "marketingEngagementCreate")["marketingEngagement"],
            json!({"occurredOn": "2026-10-30", "clicksCount": 40, "adSpend": {"amount": "19.9", "currencyCode": "USD"}, "orders": "8.0", "marketingActivity": {"title": "Summer ad"}})
        );
        run(&mut shop, engage, json!({"engagement": engagement}));
        let spent = run(
            &mut shop,
            "{ marketingActivities(first: 1) { nodes { adSpend { amount } } } }",
            json!({}),
        );
        assert_eq!(
            data(&spent, "marketingActivities")["nodes"][0]["adSpend"]["amount"],
            "39.8"
        );
        let none = run(
            &mut shop,
            "mutation($engagement: MarketingEngagementInput!) { marketingEngagementCreate(marketingEngagement: $engagement) { userErrors { code } } }",
            json!({"engagement": engagement}),
        );
        assert_eq!(
            data(&none, "marketingEngagementCreate")["userErrors"][0]["code"],
            "INVALID_MARKETING_ENGAGEMENT_ARGUMENT_MISSING"
        );
        let channel = run(
            &mut shop,
            "mutation { marketingEngagementsDelete(channelHandle: \"my-channel-handle\") { result userErrors { code } } }",
            json!({}),
        );
        assert_eq!(
            data(&channel, "marketingEngagementsDelete")["result"],
            "Engagement data associated to channel handle 'my-channel-handle' marked for deletion"
        );
        let all = run(
            &mut shop,
            "mutation { marketingActivitiesDeleteAllExternal { job { id done } userErrors { code } } }",
            json!({}),
        );
        assert_eq!(
            data(&all, "marketingActivitiesDeleteAllExternal")["job"]["done"],
            true
        );
        assert_eq!(shop.count("MarketingActivity"), 0);
    }

    #[test]
    fn an_extension_activity_is_created_and_updated() {
        let mut shop = demo();
        let created = run(
            &mut shop,
            "mutation { marketingActivityCreate(input: {marketingActivityExtensionId: \"gid://shopify/MarketingActivityExtension/1\", marketingActivityTitle: \"Draft Marketing Activity\", status: DRAFT}) {
                marketingActivity { id title status isExternal } redirectPath userErrors { message } } }",
            json!({}),
        );
        let activity = &data(&created, "marketingActivityCreate")["marketingActivity"];
        assert_eq!(activity["status"], "DRAFT");
        assert_eq!(activity["isExternal"], false);
        let id = activity["id"].clone();
        let updated = run(
            &mut shop,
            "mutation($id: ID!) { marketingActivityUpdate(input: {id: $id, status: ACTIVE, title: \"Live\"}) { marketingActivity { title status } userErrors { message } } }",
            json!({"id": id}),
        );
        assert_eq!(
            data(&updated, "marketingActivityUpdate")["marketingActivity"],
            json!({"title": "Live", "status": "ACTIVE"})
        );
        let before = shop.picture();
        let refused = run(
            &mut shop,
            "mutation { marketingActivityCreate(input: {marketingActivityExtensionId: \"gid://shopify/Product/1\", status: DRAFT}) { userErrors { field message } } }",
            json!({}),
        );
        assert_eq!(
            data(&refused, "marketingActivityCreate")["userErrors"][0]["field"],
            json!(["input", "marketingActivityExtensionId"])
        );
        assert_eq!(shop.picture(), before);
    }

    #[test]
    fn the_deprecated_create_external_makes_a_new_activity_only() {
        let mut shop = demo();
        let create = "mutation($input: MarketingActivityCreateExternalInput!) { marketingActivityCreateExternal(input: $input) {
            marketingActivity { title marketingChannelType tactic marketingEvent { remoteId } } userErrors { field code } } }";
        let mut legacy = input("ad-1");
        legacy["marketingChannelType"] = json!("SEARCH");
        let created = run(&mut shop, create, json!({"input": legacy}));
        assert_eq!(
            data(&created, "marketingActivityCreateExternal")["marketingActivity"],
            json!({"title": "Summer ad", "marketingChannelType": "SEARCH", "tactic": "AD", "marketingEvent": {"remoteId": "ad-1"}})
        );
        let before = shop.picture();
        let mut again = input("ad-1");
        again["utm"]["campaign"] = json!("other");
        let refused = run(&mut shop, create, json!({"input": again}));
        assert_eq!(
            data(&refused, "marketingActivityCreateExternal")["userErrors"],
            json!([{"field": ["input", "remoteId"], "code": "MARKETING_ACTIVITY_WITH_REMOTE_ID_ALREADY_EXISTS"}])
        );
        assert_eq!(shop.picture(), before);
    }
}

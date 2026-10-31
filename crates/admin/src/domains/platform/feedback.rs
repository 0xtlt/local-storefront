//! Resource feedback: `shopResourceFeedbackCreate` (the app's `App.feedback`) and
//! `bulkProductResourceFeedbackCreate` (each product's).
//!
//! As the reference says: "Sending feedback replaces previously sent feedback for the shop";
//! "Sending `state: ACCEPTED` removes the active feedback entry … reading `app.feedback` … may
//! return `null`"; feedback generated before the last one kept is refused with "Feedback for a
//! later version of this resource was already accepted." (field `["input"]` for the shop,
//! `["feedbackInput", "<i>", "feedbackGeneratedAt"]` for a product: the reference's examples);
//! a shop's message is shown after the app's title ("Generic Channel is not connected …" for
//! `is not connected …`). Feedback in the future is refused (`FEEDBACK_DATE_IN_FUTURE`); a
//! bulk takes 50 at most. The merchant would see the feedback in the admin, which lsf does not
//! have: it is kept, read back, and that is all.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::value;

/// How many products one bulk gives feedback on.
pub const MAX_BULK: usize = 50;

const OUTDATED: &str = "Feedback for a later version of this resource was already accepted.";

pub fn register(registry: &mut Registry) {
    registry
        .mutation("shopResourceFeedbackCreate", shop_feedback)
        .mutation("bulkProductResourceFeedbackCreate", product_feedback);
}

fn date(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.with_timezone(&Utc))
}

fn messages(input: &Value) -> Vec<String> {
    input
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// `shopResourceFeedbackCreate`.
fn shop_feedback(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg("input").cloned().unwrap_or(Value::Null);
    let Some(app) = m.state().keys("App").next().map(str::to_string) else {
        let error = UserError::new(&[], "App not found").code("NOT_FOUND");
        return Ok(payload(json!({"feedback": null}), &[error]));
    };
    let Some(generated) = date(input.get("feedbackGeneratedAt")) else {
        let error = UserError::new(
            &["input", "feedbackGeneratedAt"],
            "Feedback generated at can't be blank",
        )
        .code("BLANK");
        return Ok(payload(json!({"feedback": null}), &[error]));
    };
    if generated > m.now() {
        let error = UserError::new(
            &["input", "feedbackGeneratedAt"],
            "Feedback generated at can't be in the future",
        )
        .code("FEEDBACK_DATE_IN_FUTURE");
        return Ok(payload(json!({"feedback": null}), &[error]));
    }
    let record = m.state().get(&app).cloned().unwrap_or_default();
    let last = record
        .get("lsfFeedbackGeneratedAt")
        .and_then(|at| date(Some(at)));
    if last.is_some_and(|last| generated < last) {
        let error = UserError::new(&["input"], OUTDATED).code("OUTDATED_FEEDBACK");
        return Ok(payload(json!({"feedback": null}), &[error]));
    }
    let state_name = input
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("REQUIRES_ACTION")
        .to_string();
    let title = record
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let feedback = (state_name == "REQUIRES_ACTION").then(|| {
        json!({
            "app": app,
            "channel": null,
            "feedbackGeneratedAt": value::date_time(generated),
            "link": null,
            "messages": messages(&input)
                .iter()
                .map(|message| json!({"field": null, "message": format!("{title} {message}")}))
                .collect::<Vec<_>>(),
            "state": state_name,
        })
    });
    let state = m.state_mut();
    if let Some(record) = state.get_mut(&app) {
        record.insert("feedback".into(), feedback.clone().unwrap_or(Value::Null));
        record.insert(
            "lsfFeedbackGeneratedAt".into(),
            json!(value::date_time(generated)),
        );
    }
    state.emit(super::super::app::CHANGED, &app);
    Ok(payload(
        json!({"feedback": feedback.unwrap_or(Value::Null)}),
        &[],
    ))
}

/// `bulkProductResourceFeedbackCreate`: each product's feedback, all or nothing.
fn product_feedback(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let inputs: Vec<Value> = ctx
        .arg("feedbackInput")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut errors = Vec::new();
    if inputs.len() > MAX_BULK {
        errors.push(
            UserError::new(
                &["feedbackInput"],
                format!("Feedback input must be less than or equal to {MAX_BULK}"),
            )
            .code("MAXIMUM_FEEDBACK_LIMIT_EXCEEDED"),
        );
        return Ok(payload(json!({"feedback": []}), &errors));
    }
    let now = m.now();
    let mut kept = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let position = index.to_string();
        let product = input
            .get("productId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if m.state().type_of(&product) != Some("Product") {
            errors.push(
                UserError::new(
                    &["feedbackInput", &position, "productId"],
                    "Product does not exist",
                )
                .code("PRODUCT_NOT_FOUND"),
            );
            continue;
        }
        let (Some(generated), Some(updated)) = (
            date(input.get("feedbackGeneratedAt")),
            date(input.get("productUpdatedAt")),
        ) else {
            errors.push(
                UserError::new(&["feedbackInput", &position], "The dates can't be blank")
                    .code("BLANK"),
            );
            continue;
        };
        let key = format!("product-resource-feedback/{product}");
        let last = m
            .state()
            .get(&key)
            .and_then(|record| date(record.get("feedbackGeneratedAt")));
        if generated > now {
            errors.push(
                UserError::new(
                    &["feedbackInput", &position, "feedbackGeneratedAt"],
                    "Feedback generated at can't be in the future",
                )
                .code("INVALID"),
            );
            continue;
        }
        if last.is_some_and(|last| generated < last) {
            errors.push(
                UserError::new(
                    &["feedbackInput", &position, "feedbackGeneratedAt"],
                    OUTDATED,
                )
                .code("OUTDATED_FEEDBACK"),
            );
            continue;
        }
        kept.push((
            key,
            json!({
                "__typename": "ProductResourceFeedback",
                "productId": product,
                "state": input.get("state").and_then(Value::as_str).unwrap_or("ACCEPTED"),
                "feedbackGeneratedAt": value::date_time(generated),
                "productUpdatedAt": value::date_time(updated),
                "messages": messages(input),
                "channel": null,
            }),
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"feedback": []}), &errors));
    }
    let state = m.state_mut();
    let mut feedback = Vec::new();
    for (key, record) in kept {
        state.insert_as(key.clone(), value::object(record.clone()));
        state.emit(super::super::app::CHANGED, &key);
        feedback.push(record);
    }
    Ok(payload(json!({"feedback": feedback}), &[]))
}

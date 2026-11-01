//! The deprecated subscriptions to Amazon EventBridge and Google Cloud Pub/Sub:
//! `eventBridgeWebhookSubscriptionCreate`, `eventBridgeWebhookSubscriptionUpdate`,
//! `pubSubWebhookSubscriptionCreate`, `pubSubWebhookSubscriptionUpdate` ("Use
//! `webhookSubscriptionCreate` instead": still in 2026-10).
//!
//! Each is `webhookSubscriptionCreate` (or `Update`) with the endpoint given its own way: an
//! EventBridge event source's `arn`, a Pub/Sub `pubSubProject` and `pubSubTopic`, which are
//! the `arn:aws:events:…` and `pubsub://<project>:<topic>` addresses the registry of
//! subscriptions keeps (`WebhookEventBridgeEndpoint`, `WebhookPubSubEndpoint`, as the
//! reference's examples answer). The same code runs them (the arguments rewritten for it), so
//! the checks, the scopes a topic needs and the records are the same; the payloads keep the
//! deprecated shapes: `userErrors` of `UserError`, or of the Pub/Sub error types, whose `code`
//! is `TAKEN` for an address taken and `INVALID_PARAMETERS` otherwise.
//!
//! lsf delivers nothing to EventBridge or Pub/Sub: each delivery of such a subscription is in
//! the webhooks' log (`GET /__lsf/webhooks`), `not_sent`, with its payload and why.

use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("eventBridgeWebhookSubscriptionCreate", event_bridge_create)
        .mutation("eventBridgeWebhookSubscriptionUpdate", event_bridge_update)
        .mutation("pubSubWebhookSubscriptionCreate", pub_sub_create)
        .mutation("pubSubWebhookSubscriptionUpdate", pub_sub_update);
}

/// Which endpoint the input names, and where its errors point.
#[derive(Clone, Copy, PartialEq)]
enum Endpoint {
    EventBridge,
    PubSub,
}

/// The arguments of `webhookSubscriptionCreate`/`Update` for the deprecated ones: the input's
/// endpoint as the `uri` the registry reads.
fn rewritten(ctx: &FieldContext<'_>, endpoint: Endpoint) -> Map<String, Value> {
    let mut arguments = ctx.arguments().clone();
    let mut input = ctx.arg_object_or_default("webhookSubscription");
    let uri = match endpoint {
        Endpoint::EventBridge => input
            .remove("arn")
            .and_then(|arn| arn.as_str().map(str::to_string)),
        Endpoint::PubSub => {
            let project = input
                .remove("pubSubProject")
                .and_then(|value| value.as_str().map(str::to_string));
            let topic = input
                .remove("pubSubTopic")
                .and_then(|value| value.as_str().map(str::to_string));
            match (project, topic) {
                (Some(project), Some(topic))
                    if !project.trim().is_empty() && !topic.trim().is_empty() =>
                {
                    Some(format!("pubsub://{}:{}", project.trim(), topic.trim()))
                }
                // An address the registry refuses: the error says which.
                _ => Some(String::new()),
            }
        }
    };
    if let Some(uri) = uri {
        input.insert("uri".into(), json!(uri));
    }
    arguments.insert("webhookSubscription".into(), Value::Object(input));
    arguments
}

/// The payload of `webhookSubscription…`, in the deprecated shape: errors on `uri` are on the
/// endpoint's own fields, and the Pub/Sub ones have a code.
fn reshaped(mut answer: Value, endpoint: Endpoint) -> Value {
    let Some(errors) = answer.get_mut("userErrors").and_then(Value::as_array_mut) else {
        return answer;
    };
    for error in errors {
        let on_uri = error["field"] == json!(["webhookSubscription", "uri"]);
        if on_uri {
            error["field"] = match endpoint {
                Endpoint::EventBridge => json!(["webhookSubscription", "arn"]),
                Endpoint::PubSub => json!(["webhookSubscription", "pubSubProject"]),
            };
        }
        if endpoint == Endpoint::PubSub {
            let message = error["message"].as_str().unwrap_or_default();
            let code = if message.contains("taken") {
                "TAKEN"
            } else {
                "INVALID_PARAMETERS"
            };
            error["code"] = json!(code);
        }
    }
    answer
}

fn run(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    endpoint: Endpoint,
    mutation: &str,
) -> Result<Value, FieldError> {
    let arguments = rewritten(ctx, endpoint);
    let inner = ctx.with_arguments(&arguments);
    let Some(run) = m.registry().mutation_fn(mutation) else {
        return Err(crate::resolve::not_simulated(mutation));
    };
    let answer = run(m, &inner)?;
    Ok(reshaped(answer, endpoint))
}

fn event_bridge_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    run(m, ctx, Endpoint::EventBridge, "webhookSubscriptionCreate")
}

fn event_bridge_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    run(m, ctx, Endpoint::EventBridge, "webhookSubscriptionUpdate")
}

fn pub_sub_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    run(m, ctx, Endpoint::PubSub, "webhookSubscriptionCreate")
}

fn pub_sub_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    run(m, ctx, Endpoint::PubSub, "webhookSubscriptionUpdate")
}

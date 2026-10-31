//! The webhook subscriptions the backend makes through the Admin API: the `webhookSubscription`,
//! `webhookSubscriptions` and `webhookSubscriptionsCount` queries, and
//! `webhookSubscriptionCreate`, `webhookSubscriptionUpdate`, `webhookSubscriptionDelete`, over
//! the registry of the app's [`Webhooks`](crate::webhooks::Webhooks).
//!
//! As on Shopify, the queries list the subscriptions created through the Admin API, per shop;
//! those of `shopify.app.toml` are the app's and are not listed ("app-specific subscriptions").
//! A subscription is not a record of the shop: the store data reloading keeps it (it is the
//! app's setup of the shop), `DELETE /__lsf/admin` forgets it. A subscription requires the
//! access scope its topic
//! names (the description of each `WebhookSubscriptionTopic` value, read into
//! [`crate::webhooks::topics`]); without it, the mutation answers a user error on `topic`, worded
//! by lsf.

use serde_json::{Value, json};

use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::Filter;
use crate::webhooks::subscriptions::{
    Format, MetafieldIdentifier, SubscriptionInput, UserError as RegistryError,
};
use crate::webhooks::{Webhooks, topics};

pub fn register(registry: &mut Registry) {
    registry
        .query("webhookSubscription", webhook_subscription)
        .query("webhookSubscriptions", webhook_subscriptions)
        .query("webhookSubscriptionsCount", webhook_subscriptions_count)
        .mutation("webhookSubscriptionCreate", create)
        .mutation("webhookSubscriptionUpdate", update)
        .mutation("webhookSubscriptionDelete", delete);
}

/// The subscriptions of the shop, as `WebhookSubscription` objects, with the filters of the
/// `webhookSubscriptions` arguments (`topics`, `format`, `uri`, `callbackUrl`) applied.
fn subscriptions(r: &ShopResolver<'_>, ctx: &FieldContext<'_>) -> Vec<Value> {
    let Some(webhooks) = r.webhooks() else {
        return Vec::new();
    };
    let topics: Option<Vec<&str>> = ctx
        .arg("topics")
        .and_then(Value::as_array)
        .map(|topics| topics.iter().filter_map(Value::as_str).collect());
    let format = ctx.arg_str("format");
    let uri = ctx.arg_str("uri").or_else(|| ctx.arg_str("callbackUrl"));
    webhooks
        .registry()
        .list(r.state().domain())
        .iter()
        .map(|subscription| subscription.to_record())
        .filter(|record| {
            let field = |name: &str| record.get(name).and_then(Value::as_str);
            topics
                .as_ref()
                .is_none_or(|topics| field("topic").is_some_and(|topic| topics.contains(&topic)))
                && format.is_none_or(|format| field("format") == Some(format))
                && uri.is_none_or(|uri| field("uri") == Some(uri))
        })
        .map(Value::Object)
        .collect()
}

fn webhook_subscription(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let found = r.webhooks().and_then(|webhooks| {
        webhooks
            .registry()
            .get(r.state().domain(), id)
            .map(|subscription| subscription.to_record())
    });
    Ok(Output::object_or_null(
        found.map(|record| Obj::inline("WebhookSubscription", record)),
    ))
}

fn webhook_subscriptions(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, subscriptions(r, ctx))
}

fn webhook_subscriptions_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = r.search(ctx, "WebhookSubscription", subscriptions(r, ctx));
    Ok(r.count(ctx, items.len()))
}

/// `WebhookSubscriptionInput`, with the checks the registry does not make (the filter's
/// syntax).
fn input_of(ctx: &FieldContext<'_>, errors: &mut Vec<UserError>) -> SubscriptionInput {
    let input = ctx.arg("webhookSubscription").cloned().unwrap_or_default();
    let text = |name: &str| input.get(name).and_then(Value::as_str).map(str::to_string);
    let strings = |name: &str| {
        input.get(name).and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
    };
    let filter = text("filter");
    if let Some(filter) = filter.as_deref().filter(|filter| !filter.trim().is_empty())
        && let Err(error) = Filter::parse(filter)
    {
        errors.push(UserError::new(
            &["webhookSubscription", "filter"],
            format!("Filter is invalid: {error}"),
        ));
    }
    SubscriptionInput {
        uri: text("uri").or_else(|| text("callbackUrl")),
        name: text("name"),
        filter,
        include_fields: strings("includeFields"),
        metafield_namespaces: strings("metafieldNamespaces"),
        metafields: input
            .get("metafields")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| MetafieldIdentifier {
                        namespace: item
                            .get("namespace")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        key: item
                            .get("key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect()
            }),
        format: text("format").and_then(|format| Format::parse(&format)),
    }
}

fn converted(errors: Vec<RegistryError>) -> Vec<UserError> {
    errors
        .into_iter()
        .map(|error| UserError {
            field: Some(error.field),
            message: error.message,
            code: None,
        })
        .collect()
}

/// The webhooks of the server, which a resolver run alone may not have.
fn webhooks_of<'m>(mutation: &'m Mutation<'_>) -> Result<&'m Webhooks, FieldError> {
    mutation
        .webhooks()
        .ok_or_else(|| FieldError::new("lsf: this shop has no webhooks to subscribe to"))
}

/// Whether the token may subscribe to `topic`: it holds one of the scopes the topic names.
fn topic_allowed(mutation: &Mutation<'_>, topic: &str) -> Result<(), UserError> {
    let Some(topic) = topics::by_name(topic).or_else(|| topics::by_written(topic)) else {
        return Ok(());
    };
    let Some(grant) = mutation.grant() else {
        return Ok(());
    };
    if topic.scopes.is_empty() || topic.scopes.iter().any(|scope| grant.covers(scope)) {
        return Ok(());
    }
    let scopes: Vec<String> = topic
        .scopes
        .iter()
        .map(|scope| format!("`{scope}`"))
        .collect();
    Err(UserError::new(
        &["topic"],
        format!(
            "The app needs the {} access scope to subscribe to {}",
            scopes.join(" or "),
            topic.name
        ),
    ))
}

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let topic = ctx.arg_str("topic").unwrap_or_default().to_string();
    let mut errors = Vec::new();
    let input = input_of(ctx, &mut errors);
    if let Err(error) = topic_allowed(mutation, &topic) {
        errors.push(error);
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"webhookSubscription": null}), &errors));
    }
    let (shop, version, now) = (
        mutation.shop().to_string(),
        mutation.api_version().to_string(),
        chrono::Utc::now(),
    );
    let created = webhooks_of(mutation)?
        .registry()
        .create(&shop, &topic, input, &version, now);
    Ok(match created {
        Ok(subscription) => payload(
            json!({"webhookSubscription": subscription.to_record()}),
            &[],
        ),
        Err(errors) => payload(json!({"webhookSubscription": null}), &converted(errors)),
    })
}

fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let mut errors = Vec::new();
    let input = input_of(ctx, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"webhookSubscription": null}), &errors));
    }
    let shop = mutation.shop().to_string();
    let updated = webhooks_of(mutation)?
        .registry()
        .update(&shop, &id, input, chrono::Utc::now());
    Ok(match updated {
        Ok(subscription) => payload(
            json!({"webhookSubscription": subscription.to_record()}),
            &[],
        ),
        Err(errors) => payload(json!({"webhookSubscription": null}), &converted(errors)),
    })
}

fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let shop = mutation.shop().to_string();
    let deleted = webhooks_of(mutation)?.registry().delete(&shop, &id);
    Ok(match deleted {
        Ok(subscription) => payload(
            json!({"deletedWebhookSubscriptionId": subscription.id}),
            &[],
        ),
        Err(errors) => payload(
            json!({"deletedWebhookSubscriptionId": null}),
            &converted(errors),
        ),
    })
}

//! The app: the `App` and its installation on the shop, its Shopify Functions, and the objects
//! that run them at the checkout: the app discounts of the store data's `checkout.discounts`,
//! and the cart transforms, delivery and payment customizations and validations the store data
//! declares (`checkout.functions.<handle>.enabled`). A shop the app was just installed on has
//! none of those: the app creates them.
//!
//! The records and their ids: the "Records" section of `crates/app/docs/admin/design.md`. The ids of
//! the discounts and of the other owners of Functions are those the checkout gives them in the
//! Function input, so that a backend reads the same GIDs as its Functions.

pub mod billing;
pub mod bulk;
mod discount_tags;
pub mod discounts;
pub mod functions;
pub mod metafields;
pub mod pricing;
mod staged_targets;
pub mod uploads;

use lsf_core::store::{CheckoutDiscount, model::CheckoutDiscountClass};
use lsf_core::util::stable_id;
use serde_json::{Value, json};

use crate::admin::state::gid;
use crate::admin::state::seed::Seed;
use crate::admin::state::value;
use crate::config::{App, FunctionExtension, TargetKind};

/// The client id of the app when there is none (no `--app`, or no `client_id` in
/// `shopify.app.toml`).
pub const LOCAL_CLIENT_ID: &str = "lsf-local-client-id";

/// The title of the app when there is none.
pub const LOCAL_APP_TITLE: &str = "lsf";

/// The client id of the app: `client_id` of `shopify.app.toml`, else [`LOCAL_CLIENT_ID`].
pub fn client_id(app: Option<&App>) -> String {
    app.and_then(|app| app.client_id.clone())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| LOCAL_CLIENT_ID.to_string())
}

/// The numeric id of the app (`gid://shopify/App/<id>`): stable for its client id.
pub fn app_id(app: Option<&App>) -> u64 {
    stable_id("app", &client_id(app))
}

pub fn app_gid(app: Option<&App>) -> String {
    gid("App", app_id(app))
}

pub fn installation_gid(app: Option<&App>) -> String {
    gid(
        "AppInstallation",
        stable_id("app_installation", &client_id(app)),
    )
}

/// The id of a Function: a UUID, as Shopify's are, stable for the app and the handle. It is
/// also the key of its `ShopifyFunction` record, which has no GID.
pub fn function_id(app: Option<&App>, handle: &str) -> String {
    value::uuid(&format!("function/{}/{handle}", client_id(app)))
}

/// The GID of the owner of a discount Function: `DiscountCodeNode/<n>` for a discount with a
/// code, `DiscountAutomaticNode/<n>` otherwise, `n` the discount's `id` in `checkout.discounts`
/// (its position from 1 by default), as the checkout's `discount.id` has it.
/// (`DiscountNode/<n>`, which the legacy discount APIs read, names the same discount.)
pub fn discount_gid(discount: &CheckoutDiscount) -> String {
    let node = if discount.code.is_some() {
        "DiscountCodeNode"
    } else {
        "DiscountAutomaticNode"
    };
    gid(node, discount.id)
}

/// What a Function owner is in the Admin API, by the target kind the checkout runs it for:
/// its type and the `MetafieldOwnerType` of its metafields.
fn owner_of(kind: TargetKind) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        TargetKind::CartTransform => ("CartTransform", "CARTTRANSFORM"),
        TargetKind::DeliveryCustomization => ("DeliveryCustomization", "DELIVERY_CUSTOMIZATION"),
        TargetKind::PaymentCustomization => ("PaymentCustomization", "PAYMENT_CUSTOMIZATION"),
        TargetKind::Validation => ("Validation", "VALIDATION"),
        _ => return None,
    })
}

/// The GID of the cart transform, customization or validation that runs a Function: the
/// Function's position in the app from 1, as the checkout's owner ids have it.
pub fn owner_gid(type_name: &str, function_index: usize) -> String {
    gid(type_name, function_index as u64 + 1)
}

/// The `apiType` of a Function, from its targets.
pub fn api_type(function: &FunctionExtension) -> &'static str {
    let mut kinds = function.targets.iter().map(|target| target.kind);
    let first = kinds.next();
    match first {
        Some(TargetKind::LinesDiscounts | TargetKind::DeliveryDiscounts)
        | Some(TargetKind::LinesDiscountsFetch | TargetKind::DeliveryDiscountsFetch) => "discounts",
        Some(TargetKind::ProductDiscount) => "product_discounts",
        Some(TargetKind::OrderDiscount) => "order_discounts",
        Some(TargetKind::ShippingDiscount) => "shipping_discounts",
        Some(TargetKind::CartTransform) => "cart_transform",
        Some(TargetKind::DeliveryCustomization) => "delivery_customization",
        Some(TargetKind::PaymentCustomization) => "payment_customization",
        Some(TargetKind::Validation | TargetKind::ValidationFetch) => "cart_checkout_validation",
        _ => {
            let target = function
                .targets
                .first()
                .map_or("", |target| target.target.as_str());
            if target.contains("fulfillment-constraint") {
                "fulfillment_constraints"
            } else if target.contains("order-routing") {
                "order_routing_location_rule"
            } else if target.contains("pickup-point") {
                "pickup_point_delivery_option_generator"
            } else if target.contains("local-pickup") {
                "local_pickup_delivery_option_generator"
            } else {
                "function"
            }
        }
    }
}

fn function_title(function: &FunctionExtension) -> String {
    function
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| function.handle.clone())
}

pub fn seed(seed: &mut Seed) {
    let app = seed.app;
    let created = Seed::epoch_string();
    let app_gid = app_gid(app);
    let installation = installation_gid(app);
    let client_id = client_id(app);
    let scopes: Vec<Value> = match app {
        Some(app) => app
            .config
            .scopes
            .iter()
            .map(|scope| json!({"handle": scope}))
            .collect(),
        None => ALL_SCOPES
            .iter()
            .map(|scope| json!({"handle": scope}))
            .collect(),
    };
    let optional: Vec<Value> = app
        .map(|app| {
            app.config
                .optional_scopes
                .iter()
                .map(|scope| json!({"handle": scope}))
                .collect()
        })
        .unwrap_or_default();
    let handle = app.and_then(|app| app.handle.clone());
    let domain = seed.state.domain().to_string();
    seed.insert(json!({
        "__typename": "App",
        "id": app_gid,
        "title": app.and_then(|app| app.name.clone()).unwrap_or_else(|| LOCAL_APP_TITLE.to_string()),
        "handle": handle,
        "apiKey": client_id,
        "embedded": app.and_then(|app| app.config.embedded).unwrap_or(true),
        "developerType": "PARTNER",
        "requestedAccessScopes": scopes,
        "availableAccessScopes": scopes,
        "optionalAccessScopes": optional,
        "installation": installation,
        "webhookApiVersion": app
            .and_then(|app| app.config.webhooks_api_version.clone())
            .unwrap_or_else(|| crate_version().to_string()),
        "published": false,
        "shopifyDeveloped": false,
        "previouslyInstalled": false,
        "isPostPurchaseAppInUse": false,
        "failedRequirements": [],
        "features": [],
    }));
    seed.insert(json!({
        "__typename": "AppInstallation",
        "id": installation,
        "app": app_gid,
        "accessScopes": scopes,
        "launchUrl": format!(
            "https://{domain}/admin/apps/{}",
            handle.as_deref().unwrap_or(&client_id)
        ),
        "uninstallUrl": null,
        "activeSubscriptions": [],
        "metafields": [],
    }));

    let functions: Vec<&FunctionExtension> = app
        .map(|app| app.functions.iter().collect())
        .unwrap_or_default();
    for function in &functions {
        let id = function_id(app, &function.handle);
        seed.state.insert_as(
            id.clone(),
            value::object(json!({
                "__typename": "ShopifyFunction",
                "id": id,
                "title": function_title(function),
                "handle": function.handle,
                "apiType": api_type(function),
                "apiVersion": function.api_version,
                "app": app_gid,
                "appKey": client_id,
                "description": null,
                "inputQuery": function
                    .targets
                    .iter()
                    .map(|target| target.input_query.as_str())
                    .find(|query| !query.trim().is_empty()),
                "useCreationUi": true,
            })),
        );
    }

    // The discounts of the store data, each backed by a Function of the app.
    let store = seed.store;
    for discount in &store.checkout.discounts {
        seed_discount(seed, discount, &created);
    }

    // The cart transforms, customizations and validations the store data declares
    // (`checkout.functions.<handle>.enabled`): none otherwise, as on a shop the app was just
    // installed on, whose app creates them. One per kind of target of the Function, enabled or
    // not as the data says, with the metafields `checkout.functions` gives it.
    for (index, function) in functions.iter().enumerate() {
        let Some(&enabled) = store.checkout.owners.get(&function.handle) else {
            continue;
        };
        let mut kinds: Vec<TargetKind> = Vec::new();
        for target in &function.targets {
            if owner_of(target.kind).is_some() && !kinds.contains(&target.kind) {
                kinds.push(target.kind);
            }
        }
        let empty = Vec::new();
        let config = store
            .checkout
            .functions
            .get(&function.handle)
            .unwrap_or(&empty);
        for kind in kinds {
            let Some((type_name, owner_type)) = owner_of(kind) else {
                continue;
            };
            // A cart transform cannot be turned off: one that is not enabled is not there.
            if kind == TargetKind::CartTransform && !enabled {
                continue;
            }
            let id = owner_gid(type_name, index);
            let metafields = seed.checkout_metafields(&id, owner_type, &created, config);
            let function_id = function_id(app, &function.handle);
            let record = match kind {
                TargetKind::CartTransform => json!({
                    "__typename": type_name,
                    "id": id,
                    "functionId": function_id,
                    "blockOnFailure": false,
                    "metafields": metafields,
                }),
                TargetKind::Validation => json!({
                    "__typename": type_name,
                    "id": id,
                    "title": function_title(function),
                    "enabled": enabled,
                    "blockOnFailure": false,
                    "shopifyFunction": function_id,
                    "metafields": metafields,
                }),
                _ => json!({
                    "__typename": type_name,
                    "id": id,
                    "title": function_title(function),
                    "enabled": enabled,
                    "functionId": function_id,
                    "shopifyFunction": function_id,
                    "metafields": metafields,
                }),
            };
            seed.insert(record);
        }
    }
}

fn seed_discount(seed: &mut Seed, discount: &CheckoutDiscount, created: &str) {
    let app = seed.app;
    let id = discount_gid(discount);
    let function = app.and_then(|app| app.function(&discount.function));
    let classes: Vec<&str> = if discount.classes.is_empty() {
        vec!["PRODUCT", "ORDER", "SHIPPING"]
    } else {
        discount
            .classes
            .iter()
            .map(|class| match class {
                CheckoutDiscountClass::Product => "PRODUCT",
                CheckoutDiscountClass::Order => "ORDER",
                CheckoutDiscountClass::Shipping => "SHIPPING",
            })
            .collect()
    };
    let owner_type = "DISCOUNT";
    let metafields = seed.checkout_metafields(&id, owner_type, created, &discount.metafields);
    let mut details = json!({
        "title": discount.title,
        "discountId": id,
        "appDiscountType": {
            "app": app_gid(app),
            "appKey": client_id(app),
            "functionId": function_id(app, &discount.function),
            "title": function.map_or_else(|| discount.function.clone(), function_title),
            "description": null,
            "discountClasses": classes,
        },
        "discountClasses": classes,
        "combinesWith": {
            "orderDiscounts": false,
            "productDiscounts": false,
            "shippingDiscounts": false,
        },
        "context": {"__typename": "DiscountBuyerSelectionAll", "all": "ALL"},
        "status": "ACTIVE",
        "startsAt": created,
        "endsAt": null,
        "asyncUsageCount": 0,
        "appliesOnOneTimePurchase": true,
        "appliesOnSubscription": false,
        "tags": [],
        "createdAt": created,
        "updatedAt": created,
    });
    let record = match &discount.code {
        Some(code) => {
            details["__typename"] = json!("DiscountCodeApp");
            details["codes"] = json!([{
                "id": gid("DiscountRedeemCode", stable_id("discount_redeem_code", &format!("{id}/{code}"))),
                "code": code,
                "asyncUsageCount": 0,
                "createdAt": created,
            }]);
            details["appliesOncePerCustomer"] = json!(false);
            details["usageLimit"] = Value::Null;
            details["shareableUrls"] = json!([]);
            json!({
                "__typename": "DiscountCodeNode",
                "id": id,
                "codeDiscount": details,
                "metafields": metafields,
            })
        }
        None => {
            details["__typename"] = json!("DiscountAutomaticApp");
            json!({
                "__typename": "DiscountAutomaticNode",
                "id": id,
                "automaticDiscount": details,
                "metafields": metafields,
            })
        }
    };
    seed.insert(record);
}

/// The API version the webhooks of an app without `[webhooks] api_version` use:
/// [`crate::admin::version::DEFAULT`].
fn crate_version() -> &'static str {
    crate::admin::version::DEFAULT
}

/// Every access scope of the versions of the Admin API lsf knows: what the installation has when
/// there is no app. The list is [`crate::admin::auth::scopes::ALL_SCOPES`], which the access checks share.
pub use crate::admin::auth::scopes::ALL_SCOPES;
/// The topic of the event a change emits when Shopify has no webhook topic for it (a
/// customization, a validation, a cart transform, a usage record): no subscription receives it
/// (`webhookSubscriptionCreate` takes only Shopify's topics), but it tells the server that the
/// shop changed, as every change must ("Writing a resolver or a mutation" in
/// `crates/app/docs/admin/design.md`).
pub const CHANGED: &str = "lsf/changed";

/// The custom resolvers and the mutations of this family: see "Writing a resolver or a
/// mutation" in `crates/app/docs/admin/design.md`.
pub fn register(registry: &mut crate::admin::resolve::Registry) {
    billing::register(registry);
    bulk::register(registry);
    discount_tags::register(registry);
    discounts::register(registry);
    functions::register(registry);
    staged_targets::register(registry);
    uploads::register(registry);
}

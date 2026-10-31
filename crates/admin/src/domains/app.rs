//! The app: the `App` and its installation on the shop, its Shopify Functions, and the objects
//! that run them at the checkout: the app discounts of the store data's `checkout.discounts`,
//! and the cart transforms, delivery and payment customizations and validations of its
//! Functions.
//!
//! The records and their ids: the "Records" section of `crates/admin/docs/design.md`. The ids of
//! the discounts and of the other owners of Functions are those the checkout gives them in the
//! Function input, so that a backend reads the same GIDs as its Functions.

use lsf_checkout::app::{App, FunctionExtension, TargetKind};
use lsf_core::store::{CheckoutDiscount, model::CheckoutDiscountClass};
use lsf_core::util::stable_id;
use serde_json::{Value, json};

use crate::state::gid;
use crate::state::seed::Seed;
use crate::state::value;

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
/// code, `DiscountAutomaticNode/<n>` otherwise, `n` its position in `checkout.discounts` from 1,
/// as the checkout's `discount.id` has it. (`DiscountNode/<n>`, which the legacy discount APIs
/// read, names the same discount.)
pub fn discount_gid(index: usize, discount: &CheckoutDiscount) -> String {
    let node = if discount.code.is_some() {
        "DiscountCodeNode"
    } else {
        "DiscountAutomaticNode"
    };
    gid(node, index as u64 + 1)
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
    for (index, discount) in store.checkout.discounts.iter().enumerate() {
        seed_discount(seed, index, discount, &created);
    }

    // The cart transforms, customizations and validations: one per Function that has such a
    // target, with the metafields `checkout.functions` gives it.
    for (index, function) in functions.iter().enumerate() {
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
                    "enabled": true,
                    "blockOnFailure": false,
                    "shopifyFunction": function_id,
                    "metafields": metafields,
                }),
                _ => json!({
                    "__typename": type_name,
                    "id": id,
                    "title": function_title(function),
                    "enabled": true,
                    "functionId": function_id,
                    "shopifyFunction": function_id,
                    "metafields": metafields,
                }),
            };
            seed.insert(record);
        }
    }
}

fn seed_discount(seed: &mut Seed, index: usize, discount: &CheckoutDiscount, created: &str) {
    let app = seed.app;
    let id = discount_gid(index, discount);
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

/// The API version lsf serves, which the webhooks of an app without `[webhooks] api_version`
/// use.
fn crate_version() -> &'static str {
    "2026-10"
}

/// Every access scope the 2026-10 Admin API names (in the descriptions of its fields and
/// mutations): what the installation has when there is no app.
pub const ALL_SCOPES: [&str; 154] = [
    "read_all_orders",
    "read_analytics_annotations",
    "read_apps",
    "read_assigned_fulfillment_orders",
    "read_audit_events",
    "read_buyer_membership_orders",
    "read_capital",
    "read_cart_transforms",
    "read_cash_tracking",
    "read_checkout_and_accounts_configurations",
    "read_checkout_settings",
    "read_companies",
    "read_content",
    "read_customer_data_erasure",
    "read_customer_merge",
    "read_customer_payment_methods",
    "read_customers",
    "read_delivery_customizations",
    "read_delivery_promises",
    "read_discounts",
    "read_draft_orders",
    "read_files",
    "read_financial_kyc_information",
    "read_fulfillment_constraint_rules",
    "read_fulfillments",
    "read_gift_card_transactions",
    "read_gift_cards",
    "read_images",
    "read_inventory",
    "read_inventory_purchase_orders",
    "read_inventory_shipments",
    "read_inventory_shipments_received_items",
    "read_inventory_transfers",
    "read_legal_policies",
    "read_locales",
    "read_locations",
    "read_marketing_events",
    "read_marketplace_fulfillment_orders",
    "read_marketplace_orders",
    "read_marketplace_returns",
    "read_markets",
    "read_markets_home",
    "read_merchant_approval_signals",
    "read_merchant_managed_fulfillment_orders",
    "read_metaobject_definitions",
    "read_metaobjects",
    "read_mobile_platform_applications",
    "read_online_store_navigation",
    "read_online_store_pages",
    "read_order_edits",
    "read_orders",
    "read_own_subscription_contracts",
    "read_payment_customizations",
    "read_payment_mandate",
    "read_payment_terms",
    "read_pixels",
    "read_point_of_sale_devices",
    "read_privacy_settings",
    "read_product_listings",
    "read_products",
    "read_publications",
    "read_quick_sale",
    "read_reports",
    "read_resource_feedbacks",
    "read_returns",
    "read_rollouts",
    "read_script_tags",
    "read_shipping",
    "read_shop_promise_program",
    "read_shopify_payments",
    "read_shopify_payments_accounts",
    "read_shopify_payments_accounts_sensitive",
    "read_shopify_payments_bank_accounts",
    "read_shopify_payments_dispute_evidences",
    "read_shopify_payments_disputes",
    "read_shopify_payments_legal_entities",
    "read_shopify_payments_payouts",
    "read_shopify_payments_tooling",
    "read_store_credit_account_transactions",
    "read_store_credit_accounts",
    "read_taxes",
    "read_themes",
    "read_third_party_fulfillment_orders",
    "read_translations",
    "read_users",
    "read_validations",
    "write_analytics_annotations",
    "write_assigned_fulfillment_orders",
    "write_buyer_membership_orders",
    "write_cart_transforms",
    "write_cash_tracking",
    "write_checkout_and_accounts_configurations",
    "write_checkout_settings",
    "write_companies",
    "write_content",
    "write_customer_data_erasure",
    "write_customer_merge",
    "write_customer_payment_methods",
    "write_customers",
    "write_delivery_customizations",
    "write_delivery_promises",
    "write_discounts",
    "write_draft_orders",
    "write_files",
    "write_fulfillment_constraint_rules",
    "write_fulfillments",
    "write_gift_card_transactions",
    "write_gift_cards",
    "write_images",
    "write_inventory",
    "write_inventory_purchase_orders",
    "write_inventory_shipments",
    "write_inventory_shipments_received_items",
    "write_inventory_transfers",
    "write_legal_policies",
    "write_locales",
    "write_locations",
    "write_marketing_events",
    "write_marketplace_fulfillment_orders",
    "write_marketplace_orders",
    "write_marketplace_returns",
    "write_markets",
    "write_merchant_managed_fulfillment_orders",
    "write_metaobject_definitions",
    "write_metaobjects",
    "write_mobile_platform_applications",
    "write_online_store_navigation",
    "write_online_store_pages",
    "write_order_edits",
    "write_orders",
    "write_own_subscription_contracts",
    "write_payment_customizations",
    "write_payment_mandate",
    "write_payment_terms",
    "write_pixels",
    "write_pos_staff_member_event_attribution_overrides",
    "write_privacy_settings",
    "write_products",
    "write_publications",
    "write_purchase_options",
    "write_quick_sale",
    "write_reports",
    "write_resource_feedbacks",
    "write_returns",
    "write_script_tags",
    "write_shipping",
    "write_shopify_payments_dispute_evidences",
    "write_shopify_payments_tooling",
    "write_store_credit_account_transactions",
    "write_taxes",
    "write_themes",
    "write_third_party_fulfillment_orders",
    "write_translations",
    "write_validations",
];
/// The custom resolvers and the mutations of this family (none yet): see "Writing a resolver or
/// a mutation" in `crates/admin/docs/design.md`.
pub fn register(_registry: &mut crate::resolve::Registry) {}

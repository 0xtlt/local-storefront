//! `metafieldsSet` and `metafieldsDelete`, on any owner the schema allows, and the fields of a
//! `Metafield` that are computed (`compareDigest`, `translatable`).
//!
//! As shopify.dev's reference of 2026-10 describes `metafieldsSet`:
//!
//! - "Allows a maximum of 25 metafields to be set at a time", and "This operation is atomic,
//!   meaning no changes are persisted if an error is encountered": lsf checks every input first,
//!   answers every error, and changes nothing when there is one.
//! - `namespace` omitted is the app's reserved namespace (`$app`, `app--<app id>`); `$app:x` is
//!   `app--<app id>--x`. Another app's reserved namespace is refused with `APP_NOT_AUTHORIZED`
//!   ("Access to this namespace and key on Metafields for this resource type is not allowed.",
//!   Shopify's message, quoted on community.shopify.dev). A namespace is 3 to 255 characters, a
//!   key 2 to 64, of letters, digits, `-` and `_` (the input's description).
//! - `type` "Required when there's no corresponding definition": without one and without a
//!   definition, "Type can't be blank" (Shopify's, quoted in Shopify/shopify-api-js#990). With a
//!   definition, the type must be the definition's, and the value is checked against its
//!   validations ([`super::types`]).
//! - `compareDigest`: given, the metafield must exist with that digest; `null`, it must not
//!   exist yet; left out, nothing is compared. A mismatch is `STALE_OBJECT`.
//! - An owner of a type that has no metafields is a GraphQL error, "invalid id" (as Shopify
//!   answers a `MediaImage` owner in a community report); an owner of a right type that does not
//!   exist is "Owner does not exist." (Shopify's, community.shopify.dev).
//! - The token needs "the same access level needed to mutate the owner resource" (the access
//!   text of the mutation): the access of the owner's update mutation, checked here.
//!
//! A metafield is kept as the seed keeps it (`content::seed_metafield`), with the GID the seed
//! would give it (`metafield_gid(owner, namespace, key)`), so that store sync writes it back to
//! the store data. Setting one does not create a definition (Shopify keeps metafields without one,
//! "unstructured"); it is linked to the definition of its owner type, namespace and key when there
//! is one.
//!
//! Events: a metafield of a product (or of one of its variants) changes the product:
//! `products/update` (community reports since 2023: product metafields fire it; variant
//! metafields do since 2025). Shopify has no webhook topic for the metafields of other owners:
//! their change is the event `metafields/update` (or `metafields/delete`), which no subscription
//! can name, so that the server knows the shop changed.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::types::{self, Context};
use super::{admin_namespace, metafield_gid};
use crate::admin::auth::scopes::{self, Root};
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::access::denied;
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
pub(super) use crate::admin::state::value::str_field_or_empty as text;
use crate::admin::state::{Record, ShopState, gid_number, gid_type, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("metafieldsSet", metafields_set)
        .mutation("metafieldsDelete", metafields_delete)
        .field("Metafield", "compareDigest", compare_digest_field)
        .field("Metafield", "translatable", translatable_field);
}

/// The most metafields one `metafieldsSet` sets.
const MAX_INPUTS: usize = 25;

/// The `MetafieldOwnerType` of a record type, `None` for a type without metafields.
pub fn owner_type(type_name: &str) -> Option<&'static str> {
    Some(match type_name {
        "Product" => "PRODUCT",
        "ProductVariant" => "PRODUCTVARIANT",
        "Collection" => "COLLECTION",
        "Customer" => "CUSTOMER",
        "Company" => "COMPANY",
        "CompanyLocation" => "COMPANY_LOCATION",
        "Location" => "LOCATION",
        "Market" => "MARKET",
        "Page" => "PAGE",
        "Blog" => "BLOG",
        "Article" => "ARTICLE",
        "Order" => "ORDER",
        "DraftOrder" => "DRAFTORDER",
        "Shop" => "SHOP",
        "DiscountAutomaticNode" | "DiscountCodeNode" | "DiscountNode" => "DISCOUNT",
        "CartTransform" => "CARTTRANSFORM",
        "DeliveryCustomization" => "DELIVERY_CUSTOMIZATION",
        "PaymentCustomization" => "PAYMENT_CUSTOMIZATION",
        "Validation" => "VALIDATION",
        "SellingPlan" => "SELLING_PLAN",
        "AppInstallation" => "API_PERMISSION",
        "FulfillmentConstraintRule" => "FULFILLMENT_CONSTRAINT_RULE",
        "GiftCardCreditTransaction" | "GiftCardDebitTransaction" | "GiftCardCashOutTransaction" => {
            "GIFT_CARD_TRANSACTION"
        }
        "MediaImage" => "MEDIA_IMAGE",
        "InventoryTransfer" => "TRANSFER",
        _ => return None,
    })
}

/// The record types of a `MetafieldOwnerType`: what `HasMetafieldDefinitions` types answer the
/// definitions of.
pub fn owner_types_of(owner_type: &str) -> &'static [&'static str] {
    match owner_type {
        "PRODUCT" => &["Product"],
        "PRODUCTVARIANT" => &["ProductVariant"],
        "COLLECTION" => &["Collection"],
        "CUSTOMER" => &["Customer"],
        "COMPANY" => &["Company"],
        "COMPANY_LOCATION" => &["CompanyLocation"],
        "LOCATION" => &["Location"],
        "MARKET" => &["Market"],
        "PAGE" => &["Page"],
        "BLOG" => &["Blog"],
        "ARTICLE" => &["Article"],
        "ORDER" => &["Order"],
        "DRAFTORDER" => &["DraftOrder"],
        "SHOP" => &["Shop"],
        "DISCOUNT" => &["DiscountAutomaticNode", "DiscountCodeNode", "DiscountNode"],
        "CARTTRANSFORM" => &["CartTransform"],
        "DELIVERY_CUSTOMIZATION" => &["DeliveryCustomization"],
        "PAYMENT_CUSTOMIZATION" => &["PaymentCustomization"],
        "VALIDATION" => &["Validation"],
        "SELLING_PLAN" => &["SellingPlan"],
        "API_PERMISSION" => &["AppInstallation"],
        "FULFILLMENT_CONSTRAINT_RULE" => &["FulfillmentConstraintRule"],
        "GIFT_CARD_TRANSACTION" => &[
            "GiftCardCreditTransaction",
            "GiftCardDebitTransaction",
            "GiftCardCashOutTransaction",
        ],
        "MEDIA_IMAGE" => &["MediaImage"],
        "TRANSFER" => &["InventoryTransfer"],
        _ => &[],
    }
}

/// The mutation whose access setting a metafield of a record type needs ("the same access level
/// needed to mutate the owner resource"). `None`: nothing to check (the shop, the app's own
/// installation).
fn owner_mutation(type_name: &str) -> Option<&'static str> {
    Some(match type_name {
        "Product" => "productUpdate",
        "ProductVariant" => "productVariantsBulkUpdate",
        "Collection" => "collectionUpdate",
        "Customer" => "customerUpdate",
        "Company" => "companyUpdate",
        "CompanyLocation" => "companyLocationUpdate",
        "Location" => "locationEdit",
        "Market" => "marketUpdate",
        "Page" => "pageUpdate",
        "Blog" => "blogUpdate",
        "Article" => "articleUpdate",
        "Order" => "orderUpdate",
        "DraftOrder" => "draftOrderUpdate",
        "DiscountAutomaticNode" | "DiscountCodeNode" | "DiscountNode" => {
            "discountAutomaticAppUpdate"
        }
        "CartTransform" => "cartTransformCreate",
        "DeliveryCustomization" => "deliveryCustomizationUpdate",
        "PaymentCustomization" => "paymentCustomizationUpdate",
        "Validation" => "validationUpdate",
        "SellingPlan" => "sellingPlanGroupUpdate",
        "FulfillmentConstraintRule" => "fulfillmentConstraintRuleUpdate",
        "MediaImage" => "fileUpdate",
        "InventoryTransfer" => "inventoryTransferEdit",
        _ => return None,
    })
}

/// Checks that the token may change the metafields of an owner of `type_name`.
fn check_owner_access(
    mutation: &Mutation<'_>,
    type_name: &str,
    field: &str,
) -> Result<(), FieldError> {
    let (Some(grant), Some(name)) = (mutation.grant(), owner_mutation(type_name)) else {
        return Ok(());
    };
    match scopes::required_access(Root::Mutation, name) {
        Some(access) if !grant.allows(&access) => Err(denied(&access, field)),
        _ => Ok(()),
    }
}

/// The namespace of an input as the Admin API stores it: omitted, the app's (`app--<id>`);
/// `$app` and `$app:x`, the app's reserved ones.
pub fn stored_namespace(state: &ShopState, namespace: Option<&str>) -> String {
    admin_namespace(state.app_id(), namespace.unwrap_or("$app"))
}

/// Whether a stored namespace is the reserved namespace of another app.
pub fn other_apps(state: &ShopState, namespace: &str) -> bool {
    let own = format!("app--{}", state.app_id());
    namespace.starts_with("app--")
        && namespace != own
        && !namespace.starts_with(&format!("{own}--"))
}

/// The checks of a namespace and a key as an input gives them (`field` the path of the input),
/// Shopify's rules: a namespace 3 to 255 characters, a key 2 to 64, of letters, digits, `-` and
/// `_` (`$app` and `$app:x` are the app's namespaces). The messages are Rails' validations.
pub fn check_namespace_and_key(
    namespace: Option<&str>,
    key: &str,
    field: &[&str],
    errors: &mut Vec<UserError>,
    invalid_code: &str,
) {
    let allowed = |text: &str| {
        text.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    let path = |name: &str| {
        let mut path: Vec<&str> = field.to_vec();
        path.push(name);
        path.iter().map(|part| part.to_string()).collect::<Vec<_>>()
    };
    let error = |name: &str, message: String, code: &str| UserError {
        field: Some(path(name)),
        message,
        code: Some(code.to_string()),
    };
    if let Some(namespace) = namespace {
        let own = namespace
            .strip_prefix("$app")
            .is_some_and(|rest| rest.is_empty() || rest.strip_prefix(':').is_some_and(allowed));
        let length = namespace.chars().count();
        if !own && length < 3 {
            errors.push(error(
                "namespace",
                "Namespace is too short (minimum is 3 characters)".into(),
                "TOO_SHORT",
            ));
        } else if length > 255 {
            errors.push(error(
                "namespace",
                "Namespace is too long (maximum is 255 characters)".into(),
                "TOO_LONG",
            ));
        } else if !own && !allowed(namespace) {
            errors.push(error(
                "namespace",
                "Namespace contains one or more invalid characters.".into(),
                invalid_code,
            ));
        }
    }
    let length = key.chars().count();
    if length < 2 {
        errors.push(error(
            "key",
            "Key is too short (minimum is 2 characters)".into(),
            "TOO_SHORT",
        ));
    } else if length > 64 {
        errors.push(error(
            "key",
            "Key is too long (maximum is 64 characters)".into(),
            "TOO_LONG",
        ));
    } else if !allowed(key) {
        errors.push(error(
            "key",
            "Key contains one or more invalid characters.".into(),
            invalid_code,
        ));
    }
}

/// The metafield of an owner with a namespace (as stored) and a key.
pub fn find(state: &ShopState, owner: &str, namespace: &str, key: &str) -> Option<String> {
    let record = state.get(owner)?;
    record
        .get("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|metafield| {
            state.get(metafield).is_some_and(|metafield| {
                text(metafield, "namespace") == namespace && text(metafield, "key") == key
            })
        })
        .map(str::to_string)
}

/// The metafields of an owner as the store data writes them: namespace (as stored, `app--<id>`
/// for the app's) → key → type and value, the value being the text the Admin API returns. What
/// `appMetafields` reads of a resource the store data does not have metafields of (an order).
pub fn of_owner(state: &ShopState, owner: &str) -> lsf_core::store::Metafields {
    let mut metafields = lsf_core::store::Metafields::new();
    let ids = state
        .get(owner)
        .and_then(|record| record.get("metafields"))
        .and_then(Value::as_array);
    for record in ids
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|id| state.get(id))
    {
        metafields
            .entry(text(record, "namespace").to_string())
            .or_default()
            .insert(
                text(record, "key").to_string(),
                lsf_core::store::Metafield {
                    kind: text(record, "type").to_string(),
                    value: json!(text(record, "value")),
                },
            );
    }
    metafields
}

/// The definition of an owner type, namespace (as stored) and key.
pub fn find_definition(
    state: &ShopState,
    owner_type: &str,
    namespace: &str,
    key: &str,
) -> Option<String> {
    state
        .keys("MetafieldDefinition")
        .find(|id| {
            state.get(id).is_some_and(|definition| {
                text(definition, "ownerType") == owner_type
                    && text(definition, "namespace") == namespace
                    && text(definition, "key") == key
            })
        })
        .map(str::to_string)
}

/// The digest of a metafield's value: what `compareDigest` answers and `metafieldsSet` compares.
/// Shopify does not say how it is made; lsf's is the SHA-256 of the value, in hex, as Shopify's
/// digests of translatable content are.
pub fn compare_digest(record: &Record) -> String {
    let digest = Sha256::digest(text(record, "value").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn compare_digest_field(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    match r.map_of(object) {
        Some(record) if record.contains_key("value") => Ok(Output::value(compare_digest(record))),
        _ => r.generic(ctx, object),
    }
}

/// Whether a type is translatable ("List of data types": the text types, `json`, `link`, `url`
/// and `file_reference`).
pub fn is_translatable(kind: &str) -> bool {
    matches!(
        kind,
        "single_line_text_field"
            | "multi_line_text_field"
            | "rich_text_field"
            | "json"
            | "link"
            | "url"
            | "file_reference"
    )
}

fn translatable_field(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    match r.map_of(object) {
        Some(record) => Ok(Output::value(is_translatable(text(record, "type")))),
        None => r.generic(ctx, object),
    }
}

/// The currency of the shop, which `money` values must be in.
pub fn shop_currency(state: &ShopState) -> String {
    state
        .shop()
        .map(|shop| text(shop, "currencyCode").to_string())
        .unwrap_or_default()
}

/// The validations of a definition, `[]` without one.
pub fn validations_of(state: &ShopState, definition: Option<&str>) -> Vec<Value> {
    definition
        .and_then(|id| state.get(id))
        .and_then(|definition| definition.get("validations"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The type of a definition.
pub fn definition_kind(state: &ShopState, definition: &str) -> Option<String> {
    state
        .get(definition)?
        .get("type")?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// Writes a checked value into a metafield record: `type`, `value`, `jsonValue`, `reference` or
/// `references` (only for the reference types, as the seed), `sizeInBytes`.
pub fn write_value(record: &mut Record, kind: &str, checked: &types::Checked) {
    record.insert("type".into(), json!(kind));
    record.insert("value".into(), json!(checked.value));
    record.insert("jsonValue".into(), checked.json.clone());
    record.insert("sizeInBytes".into(), json!(checked.value.len()));
    record.remove("reference");
    record.remove("references");
    if types::is_reference(kind) {
        if types::is_list(kind) {
            record.insert(
                "references".into(),
                json!(checked.references.clone().unwrap_or_default()),
            );
        } else {
            record.insert("reference".into(), json!(checked.reference));
        }
    }
}

/// Sets a metafield of an owner: updates the one it has, or creates it (with the GID the seed
/// gives, else a new one) and adds it to the owner's `metafields`. Links it to its definition
/// when there is one. Returns its GID and whether it was created.
pub fn set(
    state: &mut ShopState,
    owner: &str,
    namespace: &str,
    key: &str,
    kind: &str,
    checked: &types::Checked,
) -> (String, bool) {
    let now = state.timestamp();
    if let Some(id) = find(state, owner, namespace, key) {
        if let Some(record) = state.get_mut(&id) {
            write_value(record, kind, checked);
            record.insert("updatedAt".into(), json!(now));
        }
        link_definition(state, &id);
        return (id, false);
    }
    let owner_type = state
        .type_of(owner)
        .and_then(owner_type)
        .unwrap_or_default();
    let stable = metafield_gid(owner, namespace, key);
    let id = state.created_gid("Metafield", stable);
    let mut record = value::object(json!({
        "__typename": "Metafield",
        "id": id,
        "namespace": namespace,
        "key": key,
        "owner": owner,
        "ownerType": owner_type,
        "definition": null,
        "createdAt": now,
        "updatedAt": now,
    }));
    write_value(&mut record, kind, checked);
    state.insert(record);
    if let Some(owner) = state.get_mut(owner) {
        match owner.get_mut("metafields") {
            Some(Value::Array(metafields)) => metafields.push(json!(id)),
            _ => {
                owner.insert("metafields".into(), json!([id]));
            }
        }
    }
    link_definition(state, &id);
    (id, true)
}

/// Links a metafield to the definition of its owner type, namespace and key, when there is one
/// (unlike [`super::attach_definition`], which the seed runs, it creates none), and keeps the
/// definition's `metafields`, `metafieldsCount` and `validationStatus` right.
pub fn link_definition(state: &mut ShopState, metafield: &str) {
    let Some(record) = state.get(metafield) else {
        return;
    };
    let (owner_type, namespace, key) = (
        text(record, "ownerType").to_string(),
        text(record, "namespace").to_string(),
        text(record, "key").to_string(),
    );
    let Some(definition) = find_definition(state, &owner_type, &namespace, &key) else {
        return;
    };
    if let Some(record) = state.get_mut(metafield) {
        record.insert("definition".into(), json!(definition));
    }
    if let Some(Value::Array(metafields)) = state
        .get_mut(&definition)
        .and_then(|record| record.get_mut("metafields"))
        && !metafields
            .iter()
            .any(|known| known.as_str() == Some(metafield))
    {
        metafields.push(json!(metafield));
    }
    refresh_definition(state, &definition);
}

/// Recomputes what a definition derives from its metafields: `metafieldsCount` and
/// `validationStatus` (`SOME_INVALID` when a metafield's type is not the definition's, or its
/// value fails the definition's validations).
pub fn refresh_definition(state: &mut ShopState, definition: &str) {
    let Some(record) = state.get(definition) else {
        return;
    };
    let kind = definition_kind(state, definition).unwrap_or_default();
    let validations: Vec<Value> = record
        .get("validations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let metafields: Vec<String> = record
        .get("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| state.contains(id))
        .map(str::to_string)
        .collect();
    let currency = shop_currency(state);
    let cx = Context {
        state,
        currency: &currency,
        validations: &validations,
    };
    let all_valid = metafields.iter().all(|id| {
        state.get(id).is_some_and(|metafield| {
            text(metafield, "type") == kind
                && types::check(&kind, text(metafield, "value"), &cx).is_ok()
        })
    });
    if let Some(record) = state.get_mut(definition) {
        record.insert("metafields".into(), json!(metafields));
        record.insert("metafieldsCount".into(), json!(metafields.len()));
        record.insert(
            "validationStatus".into(),
            json!(if all_valid {
                "ALL_VALID"
            } else {
                "SOME_INVALID"
            }),
        );
    }
}

/// Removes a metafield: from its owner's `metafields`, from its definition's, and from the
/// shop. Emits nothing.
pub fn remove(state: &mut ShopState, metafield: &str) -> Option<Record> {
    let record = state.get(metafield)?.clone();
    state.pull_key(text(&record, "owner"), "metafields", metafield);
    let definition = record
        .get("definition")
        .and_then(Value::as_str)
        .map(str::to_string);
    state.remove(metafield);
    if let Some(definition) = definition {
        state.pull_key(&definition, "metafields", metafield);
        refresh_definition(state, &definition);
    }
    Some(record)
}

/// The event of a change to the metafields of an owner: `products/update` for a product or a
/// variant (its product, touched), else `metafields/<action>` on the metafield, which no
/// webhook topic names.
pub fn emit_change(state: &mut ShopState, owner: &str, metafield: &str, action: &str) {
    let product = match state.type_of(owner) {
        Some("Product") => Some(owner.to_string()),
        Some("ProductVariant") => state
            .get(owner)
            .and_then(|variant| variant.get("product"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    };
    match product {
        Some(product) => {
            state.touch(owner);
            state.touch(&product);
            state.emit("products/update", &product);
        }
        None => {
            state.emit(&format!("metafields/{action}"), metafield);
        }
    }
}

/// An input of `metafieldsSet`, checked.
struct Planned {
    owner: String,
    namespace: String,
    key: String,
    kind: String,
    checked: types::Checked,
}

/// The owner of an input, by its GID: `Err` a GraphQL error for an id of a type that has no
/// metafields, `Ok(None)` for an owner that does not exist.
fn owner_of(state: &ShopState, id: &str) -> Result<Option<(String, &'static str)>, FieldError> {
    let Some(type_name) = gid_type(id).filter(|_| parse_gid(id).is_some()) else {
        return Err(FieldError::new("invalid id"));
    };
    if owner_type(type_name).is_none() {
        return Err(FieldError::new("invalid id"));
    }
    let key = if state.contains(id) {
        Some(id.to_string())
    } else if type_name == "DiscountNode" {
        let number = gid_number(id);
        ["DiscountAutomaticNode", "DiscountCodeNode"]
            .into_iter()
            .map(|alias| crate::admin::state::gid(alias, number))
            .find(|alias| state.contains(alias))
    } else {
        None
    };
    Ok(key.and_then(|key| {
        let owner_type = owner_type(state.type_of(&key)?)?;
        Some((key, owner_type))
    }))
}

/// An error about the field `field` of the input at `path` (`["metafields", "0"]`).
fn input_error(path: &[&str], field: &str, message: impl Into<String>, code: &str) -> UserError {
    let mut path = path.to_vec();
    if !field.is_empty() {
        path.push(field);
    }
    UserError::new(&path, message).code(code)
}

/// What an input checked sets: the namespace as stored, the type, the value.
pub struct Checked {
    pub namespace: String,
    pub key: String,
    pub kind: String,
    pub checked: types::Checked,
}

/// Checks an input that sets a metafield of an owner of `owner_type` (`owner` when it exists
/// already): `namespace` (omitted, the app's), `key`, `type` (else the definition's), `value`
/// against the type and the definition's validations, `compareDigest` when the input has the
/// field. The errors are about the fields of the input at `path`.
pub fn check_input(
    state: &ShopState,
    owner: Option<&str>,
    owner_type: &str,
    input: &Map<String, Value>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Option<Checked> {
    let get = |name: &str| input.get(name).and_then(Value::as_str);
    let key = get("key").unwrap_or_default();
    let given_namespace = get("namespace");
    let before = errors.len();
    check_namespace_and_key(given_namespace, key, path, errors, "INVALID");
    if errors.len() > before {
        return None;
    }
    let namespace = stored_namespace(state, given_namespace);
    if other_apps(state, &namespace) {
        errors.push(input_error(
            path,
            "namespace",
            "Access to this namespace and key on Metafields for this resource type is not allowed.",
            "APP_NOT_AUTHORIZED",
        ));
        return None;
    }
    let definition = find_definition(state, owner_type, &namespace, key);
    let defined_kind = definition
        .as_deref()
        .and_then(|definition| definition_kind(state, definition));
    let kind = match (get("type"), &defined_kind) {
        (Some(kind), Some(defined)) if kind != defined => {
            // lsf's wording: Shopify's message is not published.
            errors.push(input_error(
                path,
                "type",
                format!(
                    "Type '{kind}' must be consistent with the definition's type: '{defined}'."
                ),
                "INVALID_TYPE",
            ));
            return None;
        }
        (Some(kind), _) => kind.to_string(),
        (None, Some(defined)) => defined.clone(),
        (None, None) => {
            errors.push(input_error(path, "type", "Type can't be blank", "BLANK"));
            return None;
        }
    };
    if !types::is_type(&kind) {
        errors.push(input_error(
            path,
            "type",
            format!("Type '{kind}' is not a valid type."),
            "INVALID_TYPE",
        ));
        return None;
    }
    let existing = owner.and_then(|owner| find(state, owner, &namespace, key));
    if let Some(digest) = input.get("compareDigest") {
        let current = existing
            .as_deref()
            .and_then(|id| state.get(id))
            .map(compare_digest);
        let malformed = digest
            .as_str()
            .is_some_and(|digest| !digest.chars().all(|c| c.is_ascii_hexdigit()));
        if malformed {
            errors.push(input_error(
                path,
                "compareDigest",
                "The compareDigest is invalid.",
                "INVALID_COMPARE_DIGEST",
            ));
            return None;
        }
        let stale = match (digest, &current) {
            (Value::Null, None) => false,
            (Value::String(digest), Some(current)) => digest != current,
            _ => true,
        };
        if stale {
            // lsf's wording: Shopify's message is not published.
            errors.push(input_error(
                path,
                "compareDigest",
                "The metafield has been modified since it was loaded: query its compareDigest \
                 again.",
                "STALE_OBJECT",
            ));
            return None;
        }
    }
    let validations = validations_of(state, definition.as_deref());
    let currency = shop_currency(state);
    let cx = Context {
        state,
        currency: &currency,
        validations: &validations,
    };
    match types::check(&kind, get("value").unwrap_or_default(), &cx) {
        Ok(checked) => Some(Checked {
            namespace,
            key: key.to_string(),
            kind,
            checked,
        }),
        Err(invalid) => {
            errors.push(input_error(path, "value", invalid.message, invalid.code));
            None
        }
    }
}

/// The `metafields` (`[MetafieldInput]`) of a page, a blog or an article, checked: an input with
/// an `id` names a metafield of the owner (its namespace and key), the others a namespace and
/// a key. `path` is the input's path to `metafields`.
pub fn check_owned_inputs(
    state: &ShopState,
    owner: Option<&str>,
    owner_type: &str,
    inputs: Option<&Value>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<Checked> {
    let mut checked = Vec::new();
    for (index, input) in inputs
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(mut input) = input.as_object().cloned() else {
            continue;
        };
        let index = index.to_string();
        let mut at = path.to_vec();
        at.push(&index);
        if let Some(id) = input.get("id").and_then(Value::as_str) {
            let found = state
                .get(id)
                .filter(|metafield| owner.is_some_and(|owner| text(metafield, "owner") == owner));
            let Some(metafield) = found else {
                errors.push(input_error(
                    &at,
                    "id",
                    "Metafield does not exist.",
                    "INVALID",
                ));
                continue;
            };
            input.insert("namespace".into(), json!(text(metafield, "namespace")));
            input.insert("key".into(), json!(text(metafield, "key")));
        }
        if input.get("value").is_none_or(Value::is_null) {
            errors.push(input_error(&at, "value", "Value can't be blank", "BLANK"));
            continue;
        }
        if let Some(found) = check_input(state, owner, owner_type, &input, &at, errors) {
            checked.push(found);
        }
    }
    checked
}

/// Sets the metafields an owner's own mutation checked: the owner's event says the change.
pub fn set_owned(state: &mut ShopState, owner: &str, checked: &[Checked]) {
    for metafield in checked {
        set(
            state,
            owner,
            &metafield.namespace,
            &metafield.key,
            &metafield.kind,
            &metafield.checked,
        );
    }
}

fn metafields_set(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let inputs: Vec<Map<String, Value>> = ctx
        .arg("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|input| input.as_object().cloned())
        .collect();
    if inputs.len() > MAX_INPUTS {
        // lsf's wording: Shopify's message is not published.
        let error = UserError::new(
            &["metafields"],
            format!("Exceeded the maximum metafields input limit of {MAX_INPUTS}."),
        )
        .code("LESS_THAN_OR_EQUAL_TO");
        return Ok(payload(json!({"metafields": null}), &[error]));
    }
    let state = mutation.state();
    let mut errors = Vec::new();
    let mut planned = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let owner_id = input
            .get("ownerId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let owner = owner_of(state, owner_id)?;
        if let Some(type_name) = gid_type(owner_id) {
            check_owner_access(mutation, type_name, ctx.field_name())?;
        }
        let index = index.to_string();
        let path = ["metafields", index.as_str()];
        let Some((owner, owner_type)) = owner else {
            // The namespace and the key are checked first.
            let key = input.get("key").and_then(Value::as_str).unwrap_or_default();
            let namespace = input.get("namespace").and_then(Value::as_str);
            check_namespace_and_key(namespace, key, &path, &mut errors, "INVALID");
            errors.push(input_error(
                &path,
                "ownerId",
                "Owner does not exist.",
                "INVALID",
            ));
            continue;
        };
        if let Some(checked) =
            check_input(state, Some(&owner), owner_type, input, &path, &mut errors)
        {
            planned.push(Planned {
                owner,
                namespace: checked.namespace,
                key: checked.key,
                kind: checked.kind,
                checked: checked.checked,
            });
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"metafields": null}), &errors));
    }

    let state = mutation.state_mut();
    let mut ids = Vec::new();
    let mut changed_owners: Vec<(String, String, &str)> = Vec::new();
    for plan in &planned {
        let (id, created) = set(
            state,
            &plan.owner,
            &plan.namespace,
            &plan.key,
            &plan.kind,
            &plan.checked,
        );
        if !changed_owners
            .iter()
            .any(|(owner, _, _)| *owner == plan.owner)
        {
            changed_owners.push((
                plan.owner.clone(),
                id.clone(),
                if created { "create" } else { "update" },
            ));
        }
        ids.push(id);
    }
    for (owner, metafield, action) in changed_owners {
        emit_change(state, &owner, &metafield, action);
    }
    Ok(payload(json!({"metafields": ids}), &[]))
}

/// `metafieldsDelete`: `deletedMetafields` has an identifier per input, `null` for one that names
/// no metafield ("null if the corresponding metafield isn't found").
fn metafields_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let inputs: Vec<Map<String, Value>> = ctx
        .arg("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|input| input.as_object().cloned())
        .collect();
    let state = mutation.state();
    let mut found = Vec::new();
    for input in &inputs {
        let get = |name: &str| input.get(name).and_then(Value::as_str);
        let owner_id = get("ownerId").unwrap_or_default();
        let owner = owner_of(state, owner_id)?;
        if let Some(type_name) = gid_type(owner_id) {
            check_owner_access(mutation, type_name, ctx.field_name())?;
        }
        let namespace = stored_namespace(state, get("namespace"));
        let key = get("key").unwrap_or_default();
        found.push(owner.and_then(|(owner, _)| {
            let id = find(state, &owner, &namespace, key)?;
            Some((owner, id, namespace, key.to_string()))
        }));
    }
    if found.iter().all(Option::is_none) {
        return Ok(payload(
            json!({"deletedMetafields": found.iter().map(|_| Value::Null).collect::<Vec<_>>()}),
            &[],
        ));
    }
    let state = mutation.state_mut();
    let mut deleted = Vec::new();
    for entry in &found {
        let Some((owner, id, namespace, key)) = entry else {
            deleted.push(Value::Null);
            continue;
        };
        if state.contains(id) {
            let is_product = matches!(state.type_of(owner), Some("Product" | "ProductVariant"));
            if !is_product {
                state.emit("metafields/delete", id);
            }
            remove(state, id);
            if is_product {
                emit_change(state, owner, id, "delete");
            }
        }
        deleted.push(json!({"ownerId": owner, "namespace": namespace, "key": key}));
    }
    Ok(payload(json!({"deletedMetafields": deleted}), &[]))
}

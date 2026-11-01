//! Standard definitions: `standardMetafieldDefinitionEnable`, `standardMetaobjectDefinitionEnable`;
//! `standardMetafieldDefinitionTemplates`, and the `standardTemplate` of a definition enabled
//! from one.
//!
//! The templates are shopify.dev's ("List of standard metafield definitions" and "Standard
//! metaobject definitions", read on 2026-10-31): their ids, names, types, owner types, reserved
//! namespaces and keys, and validations (a maximum length; the shape of an ISBN, a UPC, an EAN;
//! the scale of a rating), in [`METAFIELD_TEMPLATES`] and [`METAOBJECT_TEMPLATES`]. Each shop
//! has the metafield templates as `StandardMetafieldDefinitionTemplate` records (they are
//! `Node`s); the Admin API has no query of the metaobject templates ("The Admin API has no query
//! that lists all of the templates").
//!
//! Enabling one creates the definition through the content family's `metafieldDefinitionCreate`
//! or `metaobjectDefinitionCreate`, run in the same mutation (`Mutation::resolver`): the same
//! checks (`TAKEN` for a definition that exists), the same records, the same
//! `metafield_definitions/create` webhook, and its `standardTemplate` set. A template is found by
//! `id`, or by `namespace` and `key` (`TEMPLATE_NOT_FOUND` otherwise); the owner type must be one
//! of the template's (`INVALID`, lsf's wording). The product review metaobject is "only
//! available to approved partners" on Shopify: lsf enables it for any app.

use serde_json::{Map, Value, json};

use crate::domains::content::{fit_code, types};
use crate::graphql::{self, FieldContext, FieldError, Output, Request};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::seed::Seed;
use crate::state::{Record, ShopState, gid};

/// A standard metafield definition template: `(id, name, description, namespace, key, type,
/// owner types, validations)`.
struct MetafieldTemplate {
    id: u64,
    name: &'static str,
    description: &'static str,
    namespace: &'static str,
    key: &'static str,
    kind: &'static str,
    owner_types: &'static [&'static str],
    validations: &'static [(&'static str, &'static str)],
}

const PRODUCT_AND_VARIANT: &[&str] = &["PRODUCT", "PRODUCTVARIANT"];

/// shopify.dev's standard metafield definitions, in its order.
const METAFIELD_TEMPLATES: &[MetafieldTemplate] = &[
    MetafieldTemplate {
        id: 1,
        name: "Product subtitle",
        description: "Stores a concise description of a product.",
        namespace: "descriptors",
        key: "subtitle",
        kind: "single_line_text_field",
        owner_types: PRODUCT_AND_VARIANT,
        validations: &[("max", "70")],
    },
    MetafieldTemplate {
        id: 2,
        name: "Care guide",
        // The reference's example of `standardMetafieldDefinitionEnable`.
        description: "Instructions for taking care of a product or apparel",
        namespace: "descriptors",
        key: "care_guide",
        kind: "multi_line_text_field",
        owner_types: PRODUCT_AND_VARIANT,
        validations: &[("max", "500")],
    },
    MetafieldTemplate {
        id: 3,
        name: "ISBN",
        description: "Stores an ISBN book identifier.",
        namespace: "facts",
        key: "isbn",
        kind: "single_line_text_field",
        owner_types: PRODUCT_AND_VARIANT,
        // "Must be a 10-digit or 13-digit number, with optional hyphens."
        validations: &[("regex", "^(?:[0-9]-?){9}[0-9]$|^(?:[0-9]-?){12}[0-9]$")],
    },
    MetafieldTemplate {
        id: 4,
        name: "UPC",
        description: "Stores a Universal Product Code (UPC).",
        namespace: "facts",
        key: "upc",
        kind: "single_line_text_field",
        owner_types: PRODUCT_AND_VARIANT,
        // "Must be a 12-digit number with at least one zero."
        validations: &[("regex", "^[0-9]*0[0-9]*$"), ("min", "12"), ("max", "12")],
    },
    MetafieldTemplate {
        id: 5,
        name: "EAN",
        description: "Stores a European Article Number (EAN).",
        namespace: "facts",
        key: "ean",
        kind: "single_line_text_field",
        owner_types: PRODUCT_AND_VARIANT,
        validations: &[("regex", "^[0-9]{13}$")],
    },
    MetafieldTemplate {
        id: 6,
        name: "Product rating",
        description: "Stores the average rating for a product or variant.",
        namespace: "reviews",
        key: "rating",
        kind: "rating",
        owner_types: PRODUCT_AND_VARIANT,
        validations: &[("scale_min", "1.0"), ("scale_max", "5.0")],
    },
    MetafieldTemplate {
        id: 7,
        name: "Product rating count",
        description: "Stores the total number of ratings for a product or variant.",
        namespace: "reviews",
        key: "rating_count",
        kind: "number_integer",
        owner_types: PRODUCT_AND_VARIANT,
        validations: &[("min", "0")],
    },
    MetafieldTemplate {
        id: 14,
        name: "Related products",
        description: "Stores products that are similar to a selected product.",
        namespace: "shopify--discovery--product_recommendation",
        key: "related_products",
        kind: "list.product_reference",
        owner_types: &["PRODUCT"],
        validations: &[],
    },
    MetafieldTemplate {
        id: 15,
        name: "Related products setting",
        description: "Stores a setting value that controls how manual product recommendations are displayed.",
        namespace: "shopify--discovery--product_recommendation",
        key: "related_products_display",
        kind: "single_line_text_field",
        owner_types: &["PRODUCT"],
        validations: &[("choices", "[\"only manual\",\"ahead\"]")],
    },
    MetafieldTemplate {
        id: 16,
        name: "Search product boosts",
        description: "Stores search terms that are associated to a product.",
        namespace: "shopify--discovery--product_search_boost",
        key: "queries",
        kind: "list.single_line_text_field",
        owner_types: &["PRODUCT"],
        validations: &[],
    },
    MetafieldTemplate {
        id: 17,
        name: "Complementary products",
        description: "Stores products that are often bought in addition to a selected product.",
        namespace: "shopify--discovery--product_recommendation",
        key: "complementary_products",
        kind: "list.product_reference",
        owner_types: &["PRODUCT"],
        validations: &[],
    },
    MetafieldTemplate {
        id: 19,
        name: "Birth date",
        description: "Stores the customer's date of birth in ISO 8601 format.",
        namespace: "facts",
        key: "birth_date",
        kind: "date",
        owner_types: &["CUSTOMER"],
        validations: &[],
    },
    MetafieldTemplate {
        id: 20,
        name: "Trade item description",
        description: "Stores a precise product description that describes specific attributes of the product for customs and import.",
        namespace: "import_information",
        key: "trade_item_description",
        kind: "multi_line_text_field",
        owner_types: PRODUCT_AND_VARIANT,
        validations: &[],
    },
    MetafieldTemplate {
        id: 21,
        name: "Product transport declaration",
        description: "Stores public transport declarations, such as batteries, hazardous materials, creams, liquids, or powders.",
        namespace: "import_information",
        key: "product_transport_declaration",
        kind: "list.single_line_text_field",
        owner_types: PRODUCT_AND_VARIANT,
        validations: &[],
    },
    MetafieldTemplate {
        id: 27,
        name: "External URL",
        description: "Stores a product URL at the product variant level for merchants that host their storefronts outside of Shopify.",
        namespace: "shopify",
        key: "external_url",
        kind: "url",
        owner_types: &["PRODUCTVARIANT"],
        validations: &[],
    },
    MetafieldTemplate {
        id: 28,
        name: "Unavailable reason",
        description: "Stores the reason a product is currently unavailable to be sold.",
        namespace: "shopify",
        key: "unavailable_reason",
        kind: "single_line_text_field",
        owner_types: &["PRODUCT"],
        validations: &[],
    },
];

/// A standard metaobject definition template: its type, name, description, display name key,
/// and fields (`(key, name, type, required)`).
struct MetaobjectTemplate {
    kind: &'static str,
    name: &'static str,
    description: &'static str,
    display_name_key: &'static str,
    fields: &'static [(&'static str, &'static str, &'static str, bool)],
}

/// shopify.dev's standard metaobject definitions.
const METAOBJECT_TEMPLATES: &[MetaobjectTemplate] = &[MetaobjectTemplate {
    kind: "product_review",
    name: "Product Review",
    description: "Stores and manages product reviews, including ratings, review content, and associated metadata.",
    display_name_key: "title",
    fields: &[
        ("rating", "Rating", "rating", true),
        ("title", "Title", "single_line_text_field", false),
        ("body", "Body", "multi_line_text_field", false),
        ("submitted_at", "Submitted At", "date_time", true),
        ("edited_at", "Edited At", "date_time", false),
        ("published_at", "Published At", "date_time", false),
        ("source", "Source", "single_line_text_field", true),
        ("author", "Author", "customer_reference", false),
        (
            "author_display_name",
            "Author display name",
            "single_line_text_field",
            false,
        ),
        ("order", "Order", "order_reference", false),
        ("product", "Product", "product_reference", false),
        (
            "product_variant",
            "Product Variant",
            "variant_reference",
            false,
        ),
        (
            "merchant_reply",
            "Merchant Reply",
            "multi_line_text_field",
            false,
        ),
        (
            "merchant_replied_at",
            "Merchant Replied At",
            "date_time",
            false,
        ),
        ("media_urls", "Media URLs", "list.url", false),
        ("language", "Language", "language", false),
        (
            "app_verification_status",
            "App Verification Status",
            "single_line_text_field",
            false,
        ),
    ],
}];

fn template_gid(id: u64) -> String {
    gid("StandardMetafieldDefinitionTemplate", id)
}

/// The validations of a template as `MetafieldDefinitionValidation`s: `{name, type, value}`,
/// the type the definition type gives the validation.
fn validations_of(template: &MetafieldTemplate) -> Vec<Value> {
    let supported = types::supported_validations(template.kind);
    template
        .validations
        .iter()
        .map(|(name, value)| {
            let kind = supported
                .iter()
                .find(|(supported, _)| supported == name)
                .map_or("single_line_text_field".to_string(), |(_, kind)| {
                    kind.clone()
                });
            json!({"name": name, "type": kind, "value": value})
        })
        .collect()
}

/// The template records of a shop.
pub fn seed(seed: &mut Seed) {
    for template in METAFIELD_TEMPLATES {
        seed.insert(json!({
            "__typename": "StandardMetafieldDefinitionTemplate",
            "id": template_gid(template.id),
            "name": template.name,
            "description": template.description,
            "namespace": template.namespace,
            "key": template.key,
            "ownerTypes": template.owner_types,
            "type": types::definition_type(template.kind),
            "validations": validations_of(template),
            "visibleToStorefrontApi": true,
        }));
    }
}

pub fn register(registry: &mut Registry) {
    registry
        .mutation("standardMetafieldDefinitionEnable", metafield_enable)
        .mutation("standardMetaobjectDefinitionEnable", metaobject_enable)
        .query("standardMetafieldDefinitionTemplates", metafield_templates);
}

fn text<'r>(record: &'r Record, field: &str) -> &'r str {
    record
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Whether the shop has a definition of a template's namespace and key, for any owner type.
fn activated(state: &ShopState, template: &MetafieldTemplate) -> bool {
    state.all("MetafieldDefinition").any(|definition| {
        text(definition, "namespace") == template.namespace
            && text(definition, "key") == template.key
    })
}

/// `standardMetafieldDefinitionTemplates(excludeActivated:, constraintStatus:)`.
fn metafield_templates(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let exclude = ctx.arg_bool("excludeActivated") == Some(true);
    // The templates have no constraints: none is constrained.
    let constrained_only = ctx.arg_str("constraintStatus") == Some("CONSTRAINED_ONLY");
    let items: Vec<Value> = METAFIELD_TEMPLATES
        .iter()
        .filter(|template| !constrained_only && !(exclude && activated(r.state(), template)))
        .map(|template| json!(template_gid(template.id)))
        .collect();
    r.connection(ctx, items)
}

/// The user errors of an internal mutation's payload, fitted to `code_enum`, their `field`
/// without the input's name (`["definition", "key"]` is `["key"]`).
fn errors_of(payload: &Value, code_enum: &str) -> Vec<UserError> {
    payload["userErrors"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|error| {
            let field: Vec<String> = error["field"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .skip(1)
                .map(str::to_string)
                .collect();
            let refs: Vec<&str> = field.iter().map(String::as_str).collect();
            let mut user_error =
                UserError::new(&refs, error["message"].as_str().unwrap_or_default());
            if let Some(code) = fit_code(error["code"].as_str(), code_enum) {
                user_error = user_error.code(&code);
            }
            user_error
        })
        .collect()
}

/// The errors of an inner mutation that created nothing: its own, or lsf's when it gave none.
fn or_invalid(errors: Vec<UserError>) -> Vec<UserError> {
    if errors.is_empty() {
        vec![UserError::new(&[], "The definition could not be created").code("INVALID")]
    } else {
        errors
    }
}

/// Runs a mutation of the registry within this one: what it changes is this mutation's.
fn run_inner(m: &mut Mutation<'_>, query: &str, variables: Map<String, Value>) -> Value {
    let mut request = Request::new(query);
    request.variables = variables;
    let schema = m.schema();
    let mut resolver = m.resolver();
    graphql::run(schema, &mut resolver, &request, None).to_json()
}

fn metafield_enable(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let refuse = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"createdDefinition": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let template = match (
        ctx.arg_str("id"),
        ctx.arg_str("namespace"),
        ctx.arg_str("key"),
    ) {
        (Some(id), _, _) => METAFIELD_TEMPLATES
            .iter()
            .find(|template| template_gid(template.id) == id),
        (None, Some(namespace), Some(key)) => METAFIELD_TEMPLATES
            .iter()
            .find(|template| template.namespace == namespace && template.key == key),
        _ => None,
    };
    let Some(template) = template else {
        return refuse(
            &[],
            "A standard definition wasn't found.",
            "TEMPLATE_NOT_FOUND",
        );
    };
    let owner_type = ctx.arg_str("ownerType").unwrap_or_default().to_string();
    if !template.owner_types.contains(&owner_type.as_str()) {
        return refuse(
            &["ownerType"],
            &format!(
                "The standard definition {} is for {}, not {owner_type}",
                template.name,
                template.owner_types.join(", ")
            ),
            "INVALID",
        );
    }
    let mut definition = json!({
        "name": template.name,
        "description": template.description,
        "namespace": template.namespace,
        "key": template.key,
        "type": template.kind,
        "ownerType": owner_type,
        "validations": template
            .validations
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect::<Vec<_>>(),
    });
    if let Some(pin) = ctx.arg_bool("pin") {
        definition["pin"] = json!(pin);
    }
    if let Some(capabilities) = ctx.arg("capabilities") {
        definition["capabilities"] = capabilities.clone();
    }
    let mut access = ctx.arg("access").cloned().unwrap_or_else(|| json!({}));
    if ctx.arg_bool("visibleToStorefrontApi") == Some(false) && access.get("storefront").is_none() {
        access["storefront"] = json!("NONE");
    }
    if access.as_object().is_some_and(|access| !access.is_empty()) {
        definition["access"] = access;
    }
    let mut variables = Map::new();
    variables.insert("definition".into(), definition);
    let answer = run_inner(
        m,
        "mutation($definition: MetafieldDefinitionInput!) { metafieldDefinitionCreate(definition: $definition) { createdDefinition { id } userErrors { field message code } } }",
        variables,
    );
    let created = &answer["data"]["metafieldDefinitionCreate"];
    let errors = errors_of(created, "StandardMetafieldDefinitionEnableUserErrorCode");
    let Some(id) = created["createdDefinition"]["id"]
        .as_str()
        .filter(|_| errors.is_empty())
        .map(str::to_string)
    else {
        return Ok(payload(
            json!({"createdDefinition": null}),
            &or_invalid(errors),
        ));
    };
    if let Some(record) = m.state_mut().get_mut(&id) {
        record.insert("standardTemplate".into(), json!(template_gid(template.id)));
    }
    Ok(payload(json!({"createdDefinition": id}), &[]))
}

/// The validations a field of a standard metaobject needs: a rating's scale, 1 to 5 as the
/// product rating metafield's.
fn field_validations(kind: &str) -> Value {
    if kind == "rating" {
        json!([{"name": "scale_min", "value": "1.0"}, {"name": "scale_max", "value": "5.0"}])
    } else {
        json!([])
    }
}

/// The `StandardMetaobjectDefinitionTemplate` of a template, inline.
fn metaobject_template_value(template: &MetaobjectTemplate) -> Value {
    json!({
        "type": template.kind,
        "name": template.name,
        "description": template.description,
        "displayNameKey": template.display_name_key,
        "access": {"admin": "MERCHANT_READ_WRITE", "storefront": "PUBLIC_READ"},
        "enabledCapabilities": [],
        "fieldDefinitions": template.fields.iter().map(|(key, name, kind, required)| json!({
            "key": key,
            "name": name,
            "description": null,
            "required": required,
            "type": types::definition_type(kind),
            "validations": field_validations(kind)
                .as_array()
                .into_iter()
                .flatten()
                .map(|validation| json!({"name": validation["name"], "type": "number_decimal", "value": validation["value"]}))
                .collect::<Vec<_>>(),
            "visibleToStorefrontApi": true,
        })).collect::<Vec<_>>(),
    })
}

/// `standardMetaobjectDefinitionEnable(type:)`.
fn metaobject_enable(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let kind = ctx.arg_str("type").unwrap_or_default().trim().to_string();
    let Some(template) = METAOBJECT_TEMPLATES
        .iter()
        .find(|template| template.kind == kind)
    else {
        let error = UserError::new(
            &["type"],
            format!(
                "No standard metaobject definition of type `{kind}`: the standard types are {}",
                METAOBJECT_TEMPLATES
                    .iter()
                    .map(|template| template.kind)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .code("RECORD_NOT_FOUND");
        return Ok(payload(json!({"metaobjectDefinition": null}), &[error]));
    };
    let mut variables = Map::new();
    variables.insert(
        "definition".into(),
        json!({
            "type": template.kind,
            "name": template.name,
            "description": template.description,
            "displayNameKey": template.display_name_key,
            "access": {"storefront": "PUBLIC_READ"},
            "fieldDefinitions": template.fields.iter().map(|(key, name, kind, required)| json!({
                "key": key,
                "name": name,
                "type": kind,
                "required": required,
                "validations": field_validations(kind),
            })).collect::<Vec<_>>(),
        }),
    );
    let answer = run_inner(
        m,
        "mutation($definition: MetaobjectDefinitionCreateInput!) { metaobjectDefinitionCreate(definition: $definition) { metaobjectDefinition { id } userErrors { field message code } } }",
        variables,
    );
    let created = &answer["data"]["metaobjectDefinitionCreate"];
    let errors = errors_of(created, "MetaobjectUserErrorCode");
    let Some(id) = created["metaobjectDefinition"]["id"]
        .as_str()
        .filter(|_| errors.is_empty())
        .map(str::to_string)
    else {
        return Ok(payload(
            json!({"metaobjectDefinition": null}),
            &or_invalid(errors),
        ));
    };
    if let Some(record) = m.state_mut().get_mut(&id) {
        record.insert(
            "standardTemplate".into(),
            metaobject_template_value(template),
        );
    }
    Ok(payload(json!({"metaobjectDefinition": id}), &[]))
}

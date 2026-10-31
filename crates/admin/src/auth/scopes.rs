//! Access scopes: what a token's scopes cover, and what each field of the Admin API 2026-10
//! requires.
//!
//! **Implication.** A `write_x` scope covers `read_x` (shopify.dev: "A `write_*` scope includes
//! its matching `read_*` scope"), and `unauthenticated_write_x` covers `unauthenticated_read_x`,
//! as `AuthScopes` of `@shopify/shopify-api` expands them. No other implication is documented:
//! `read_all_orders` extends `read_orders` to older orders but grants nothing alone, and the
//! alternatives a field accepts (`read_companies` for `read_customers`, …) are part of what the
//! field requires, below.
//!
//! **The table.** Shopify's introspection of 2026-10 (the pinned `@shopify/dev-mcp`) carries,
//! for each type and field, the text shopify.dev prints as "Requires … access scope" and an
//! `ACCESS_DENIED` error repeats. `tools/admin/access_scopes.mjs` writes those texts into
//! `schemas/2026-10/access.json`; nothing in it is guessed. A root field requires its own text,
//! or, for a query field without one, the text of the type it answers (`products` answers
//! `Product`: "`read_products` access scope."). [`RequiredAccess::parse`] reads the scopes out of
//! a text; how sure each reading is:
//!
//! - Certain: "`read_x` access scope.", "`a` access scope or `b` access scope.", "Any of `a`,
//!   `b` access scopes.", "Requires `x` access scope.", "Apps must have `x` access scope." (any
//!   one of the scopes named), "`a` access scope as well as any of `b`, `c` access scopes." (`a`
//!   and one of the others), and the markets' "`read_markets` for queries and both `read_markets`
//!   as well as `write_markets` for mutations.".
//! - Read by rule: a bare resource name ("Any of `orders` or `shipping` access scopes") is its
//!   `read_` scope in a query and its `write_` scope in a mutation; a scope written without
//!   backticks ("The user needs write_themes and an exemption …") is required.
//! - Not checked: what lsf cannot know or does not simulate. Staff permissions ("`manage_…` user
//!   permission", "The user must have …"), plans, exemptions, private programs, and requirements
//!   that depend on the arguments (`metafieldsSet` needs the scope of each owner it writes to,
//!   `tagsAdd` that of the tagged resource): a field whose text names no scope requires none
//!   here. The extra scope an "Also:" clause asks only in some cases (`orderUpdate` assigning
//!   staff, `shippingLabelPurchase` by location) is not required; one it always asks ("Also:
//!   Requires `write_customer_payment_methods` scope.") is.
//!
//! Fields without a text need no scope: `shop`, `app`, `currentAppInstallation`, `node`,
//! `webhookSubscriptions`, `webhookSubscriptionCreate`, … (`node` and `nodes` answer records
//! whose own type may require one: see [`type_access`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Value, json};

/// The page `extensions.documentation` of an `ACCESS_DENIED` error links to.
pub const ACCESS_SCOPES_DOCUMENTATION: &str = "https://shopify.dev/api/usage/access-scopes";

/// The `extensions.code` of an error for a field the token's scopes do not cover.
pub const ACCESS_DENIED: &str = "ACCESS_DENIED";

/// The scopes of a comma-separated list (`scope` of OAuth, `[access_scopes] scopes`), trimmed,
/// without empty or repeated ones, in their order.
pub fn parse_scopes(list: &str) -> Vec<String> {
    let mut scopes: Vec<String> = Vec::new();
    for scope in list.split(',').map(str::trim) {
        if !scope.is_empty() && !scopes.iter().any(|known| known == scope) {
            scopes.push(scope.to_string());
        }
    }
    scopes
}

/// Whether `scope` has the form of an access scope: `read_products`, `write_orders`,
/// `unauthenticated_read_product_listings`, `customer_read_orders`.
pub fn is_scope_name(scope: &str) -> bool {
    let rest = scope
        .strip_prefix("unauthenticated_")
        .or_else(|| scope.strip_prefix("customer_"))
        .unwrap_or(scope);
    let Some(resource) = rest
        .strip_prefix("read_")
        .or_else(|| rest.strip_prefix("write_"))
    else {
        return false;
    };
    !resource.is_empty()
        && resource
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The scope `scope` implies, if any: `read_x` for `write_x`, `unauthenticated_read_x` for
/// `unauthenticated_write_x`.
pub fn implied(scope: &str) -> Option<String> {
    if let Some(resource) = scope.strip_prefix("write_") {
        return Some(format!("read_{resource}"));
    }
    scope
        .strip_prefix("unauthenticated_write_")
        .map(|resource| format!("unauthenticated_read_{resource}"))
}

/// The scopes `granted` covers: themselves and those they imply.
pub fn expand<S: AsRef<str>>(granted: &[S]) -> BTreeSet<String> {
    let mut expanded = BTreeSet::new();
    for scope in granted {
        let scope = scope.as_ref().trim();
        expanded.insert(scope.to_string());
        if let Some(implied) = implied(scope) {
            expanded.insert(implied);
        }
    }
    expanded
}

/// Whether the scopes `granted` cover `required`: `["write_products"]` covers
/// `read_products`.
pub fn covers<S: AsRef<str>>(granted: &[S], required: &str) -> bool {
    granted.iter().any(|scope| {
        let scope = scope.as_ref().trim();
        scope == required || implied(scope).as_deref() == Some(required)
    })
}

/// `scopes` without those another of them implies (`read_products` next to
/// `write_products`), as `AuthScopes#toString` of `@shopify/shopify-api` writes them.
pub fn compress<S: AsRef<str>>(scopes: &[S]) -> Vec<String> {
    let implied: BTreeSet<String> = scopes
        .iter()
        .filter_map(|scope| implied(scope.as_ref()))
        .collect();
    parse_scopes(
        &scopes
            .iter()
            .map(AsRef::as_ref)
            .filter(|scope| !implied.contains(*scope))
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// The root type a field belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Root {
    /// `QueryRoot`.
    Query,
    /// `Mutation`.
    Mutation,
}

/// What a field requires: Shopify's text, and the scopes it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiredAccess {
    /// Shopify's words, as `extensions.requiredAccess` and the message of an `ACCESS_DENIED`
    /// error repeat them: "`read_products` access scope.".
    pub text: String,
    /// The sets of scopes that grant access: the token needs every scope of at least one set.
    /// Empty when the text names no scope lsf can check (a staff permission, a plan, …).
    pub alternatives: Vec<Vec<String>>,
}

impl RequiredAccess {
    /// Reads the scopes out of a `requiredAccess` text, for a field of a query or of a mutation
    /// (some texts say what each needs, and a bare resource name means its `read_` scope in one
    /// and its `write_` scope in the other). The rules are in the module's documentation.
    pub fn parse(text: &str, root: Root) -> RequiredAccess {
        let (main, also) = match text.split_once(" Also:") {
            Some((main, also)) => (main, Some(also)),
            None => (text, None),
        };
        let mut alternatives: Vec<Vec<String>> =
            if let Some((queries, mutations)) = main.split_once(" for queries and both ") {
                match root {
                    Root::Query => scopes_named(queries, root)
                        .into_iter()
                        .map(|scope| vec![scope])
                        .collect(),
                    Root::Mutation => vec![scopes_named(mutations, root)],
                }
            } else if let Some((all, any)) = main.split_once(" as well as any of ") {
                let all = scopes_named(all, root);
                scopes_named(any, root)
                    .into_iter()
                    .map(|scope| {
                        let mut set = all.clone();
                        set.push(scope);
                        set
                    })
                    .collect()
            } else {
                scopes_named(main, root)
                    .into_iter()
                    .map(|scope| vec![scope])
                    .collect()
            };
        if let Some(also) = also.map(str::trim_start)
            && also.starts_with("Requires `")
        {
            let sentence = also.split(". ").next().unwrap_or(also);
            let extra = scopes_named(sentence, root);
            for set in &mut alternatives {
                set.extend(extra.iter().cloned());
            }
        }
        alternatives.retain(|set| !set.is_empty());
        RequiredAccess {
            text: text.to_string(),
            alternatives,
        }
    }

    /// Whether the scopes `granted` grant access: they cover every scope of one alternative.
    /// `true` when there is none to check.
    pub fn allows<S: AsRef<str>>(&self, granted: &[S]) -> bool {
        self.alternatives.is_empty()
            || self
                .alternatives
                .iter()
                .any(|set| set.iter().all(|scope| covers(granted, scope)))
    }

    /// The message of the `ACCESS_DENIED` error for `field`, as Shopify writes it: "Access denied
    /// for products field. Required access: `read_products` access scope.".
    pub fn message(&self, field: &str) -> String {
        format!(
            "Access denied for {field} field. Required access: {}",
            self.text
        )
    }

    /// The `extensions` of the `ACCESS_DENIED` error.
    pub fn extensions(&self) -> Value {
        json!({
            "code": ACCESS_DENIED,
            "documentation": ACCESS_SCOPES_DOCUMENTATION,
            "requiredAccess": self.text,
        })
    }

    /// The `ACCESS_DENIED` error for `field`, in the order graphql-ruby writes an error:
    /// `message`, `locations` (when given), `path`, `extensions`.
    pub fn error(&self, field: &str, locations: Option<Value>, path: Vec<Value>) -> Value {
        let mut error = serde_json::Map::new();
        error.insert("message".to_string(), Value::String(self.message(field)));
        if let Some(locations) = locations {
            error.insert("locations".to_string(), locations);
        }
        error.insert("path".to_string(), Value::Array(path));
        error.insert("extensions".to_string(), self.extensions());
        Value::Object(error)
    }
}

/// The scopes a piece of text names, in order: `read_x`-like words, backticked or not, and
/// backticked resource names followed by "access scope(s)" or "scope" (or listed after "one of
/// the following:"), which are `read_x` in a query and `write_x` in a mutation.
fn scopes_named(text: &str, root: Root) -> Vec<String> {
    let mut scopes: Vec<String> = Vec::new();
    fn push(scopes: &mut Vec<String>, scope: String) {
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            // A run of backticked names: `a`, `b` or `c`.
            let mut run: Vec<&str> = Vec::new();
            let mut j = i;
            while let Some(close) = text[j + 1..].find('`').map(|at| j + 1 + at) {
                run.push(&text[j + 1..close]);
                j = close + 1;
                let rest = &text[j..];
                let separator = [", or ", ", ", " or ", " and "]
                    .into_iter()
                    .find(|separator| {
                        rest.starts_with(separator) && rest[separator.len()..].starts_with('`')
                    });
                match separator {
                    Some(separator) => j += separator.len(),
                    None => break,
                }
            }
            if run.is_empty() {
                break;
            }
            let after = text[j..].trim_start();
            let before = text[..i].trim_end();
            let resources = after.starts_with("access scope")
                || after.starts_with("scope")
                || before.ends_with("one of the following:");
            for name in run {
                if is_scope_name(name) {
                    push(&mut scopes, name.to_string());
                } else if resources && is_resource_name(name) {
                    push(
                        &mut scopes,
                        match root {
                            Root::Query => format!("read_{name}"),
                            Root::Mutation => format!("write_{name}"),
                        },
                    );
                }
            }
            i = j.max(i + 1);
        } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            let end = text[i..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(text.len(), |at| i + at);
            let word = &text[i..end];
            if is_scope_name(word) {
                push(&mut scopes, word.to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if scopes.is_empty()
        && let Some(before) = text
            .split(" read access scope")
            .next()
            .filter(|_| text.contains(" read access scope"))
    {
        // "products or product_listings read access scope."
        for name in before.split(" or ") {
            let name = name.trim();
            if is_resource_name(name) {
                push(&mut scopes, format!("read_{name}"));
            }
        }
    }
    scopes
}

/// Whether a name is that of a resource: `orders`, `shipping`, `custom_fulfillment_services`.
fn is_resource_name(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// `schemas/2026-10/access.json`.
#[derive(Deserialize)]
struct Table {
    query: String,
    mutation: String,
    types: HashMap<String, String>,
    fields: HashMap<String, HashMap<String, String>>,
    roots: HashMap<String, String>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../schemas/2026-10/access.json"))
        .expect("schemas/2026-10/access.json is the table tools/admin/access_scopes.mjs writes")
});

/// What the root field `field` requires, when it requires a scope: its own text, or for a query
/// field without one, that of the type it answers. `None` for a field that needs none (`shop`,
/// `app`, `currentAppInstallation`, `webhookSubscriptionCreate`, …), for an unknown field, and
/// for one whose requirement names no scope lsf checks.
pub fn required_access(root: Root, field: &str) -> Option<RequiredAccess> {
    let table = &*TABLE;
    let type_name = match root {
        Root::Query => &table.query,
        Root::Mutation => &table.mutation,
    };
    let text = table
        .fields
        .get(type_name)
        .and_then(|fields| fields.get(field))
        .or_else(|| match root {
            Root::Query => table
                .roots
                .get(field)
                .and_then(|node| table.types.get(node)),
            Root::Mutation => None,
        })?;
    checked(RequiredAccess::parse(text, root))
}

/// What a field of any type requires by its own text (`Shop.productTags`:
/// "`read_products` access scope."), read for an operation of kind `root`. The type a
/// field answers may require more: [`type_access`].
pub fn field_access(type_name: &str, field: &str, root: Root) -> Option<RequiredAccess> {
    let text = TABLE.fields.get(type_name)?.get(field)?;
    checked(RequiredAccess::parse(text, root))
}

/// What reading a record of a type requires (`Customer`: "`read_customers` access scope."),
/// for an operation of kind `root`. Shopify denies a field that answers such a record, nested
/// or not, with the field's name: "Access denied for customer field.".
pub fn type_access(type_name: &str, root: Root) -> Option<RequiredAccess> {
    let text = TABLE.types.get(type_name)?;
    checked(RequiredAccess::parse(text, root))
}

/// `access`, unless it names no scope to check.
fn checked(access: RequiredAccess) -> Option<RequiredAccess> {
    (!access.alternatives.is_empty()).then_some(access)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(list: &[&str]) -> Vec<String> {
        list.iter().map(|scope| scope.to_string()).collect()
    }

    #[test]
    fn write_implies_read() {
        assert!(covers(&["write_products"], "read_products"));
        assert!(covers(&["write_products"], "write_products"));
        assert!(!covers(&["read_products"], "write_products"));
        assert!(!covers(&["write_products"], "read_orders"));
        assert!(covers(
            &["unauthenticated_write_checkouts"],
            "unauthenticated_read_checkouts"
        ));
        assert!(!covers(
            &["unauthenticated_write_checkouts"],
            "read_checkouts"
        ));
        assert!(!covers(&["read_all_orders"], "read_orders"));
        assert_eq!(
            expand(&["write_orders", "read_products"]),
            ["read_orders", "read_products", "write_orders"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert_eq!(
            compress(&["read_products", "write_products", "read_orders"]),
            ["write_products", "read_orders"]
        );
    }

    #[test]
    fn parses_scope_lists() {
        assert_eq!(
            parse_scopes(" read_products, write_orders,,read_products "),
            ["read_products", "write_orders"]
        );
        assert!(is_scope_name("read_products"));
        assert!(is_scope_name("unauthenticated_read_product_listings"));
        assert!(is_scope_name("customer_read_orders"));
        assert!(!is_scope_name("manage_delivery_settings"));
        assert!(!is_scope_name("read_"));
        assert!(!is_scope_name("Read_products"));
    }

    #[test]
    fn maps_the_documented_samples() {
        // shopify.dev's page of each query and mutation: "Requires … access scope".
        let expect = |root, field: &str, alternatives: &[&[&str]]| {
            let access =
                required_access(root, field).unwrap_or_else(|| panic!("{field} requires a scope"));
            let wanted: Vec<Vec<String>> = alternatives.iter().map(|set| scopes(set)).collect();
            assert_eq!(access.alternatives, wanted, "{field}: {}", access.text);
        };
        expect(Root::Query, "products", &[&["read_products"]]);
        expect(Root::Query, "product", &[&["read_products"]]);
        expect(Root::Query, "productsCount", &[&["read_products"]]);
        expect(Root::Query, "collections", &[&["read_products"]]);
        expect(
            Root::Query,
            "orders",
            &[
                &["read_orders"],
                &["read_marketplace_orders"],
                &["read_buyer_membership_orders"],
                &["read_quick_sale"],
            ],
        );
        expect(Root::Query, "customers", &[&["read_customers"]]);
        expect(Root::Query, "metaobjects", &[&["read_metaobjects"]]);
        expect(Root::Query, "discountNodes", &[&["read_discounts"]]);
        expect(Root::Mutation, "productCreate", &[&["write_products"]]);
        expect(
            Root::Mutation,
            "discountAutomaticAppCreate",
            &[&["write_discounts"]],
        );
        expect(
            Root::Mutation,
            "inventoryAdjustQuantities",
            &[&["write_inventory"]],
        );
        expect(
            Root::Mutation,
            "draftOrderCreate",
            &[&["write_draft_orders"], &["write_quick_sale"]],
        );
        // The extra scope of the "Also:" clause is only needed to assign staff.
        expect(
            Root::Mutation,
            "orderUpdate",
            &[
                &["write_orders"],
                &["write_marketplace_orders"],
                &["write_quick_sale"],
            ],
        );
        expect(Root::Mutation, "orderEditBegin", &[&["write_order_edits"]]);
        // "for queries and both … for mutations".
        expect(Root::Query, "backupRegion", &[&["read_markets"]]);
        expect(
            Root::Mutation,
            "backupRegionUpdate",
            &[&["read_markets", "write_markets"]],
        );
        // A bare resource name.
        expect(
            Root::Mutation,
            "deliveryProfileCreate",
            &[&["write_shipping"]],
        );
        // Unbackticked.
        expect(Root::Mutation, "themeCreate", &[&["write_themes"]]);
    }

    #[test]
    fn needs_no_scope_for_what_shopify_gives_every_app() {
        for field in [
            "shop",
            "app",
            "currentAppInstallation",
            "node",
            "nodes",
            "webhookSubscriptions",
            "publicApiVersions",
        ] {
            assert_eq!(required_access(Root::Query, field), None, "{field}");
        }
        for field in [
            "webhookSubscriptionCreate",
            "appSubscriptionCreate",
            "stagedUploadsCreate",
            "appRevokeAccessScopes",
            // Its scope depends on the owners it writes to.
            "metafieldsSet",
        ] {
            assert_eq!(required_access(Root::Mutation, field), None, "{field}");
        }
        assert_eq!(required_access(Root::Query, "noSuchField"), None);
    }

    #[test]
    fn reads_every_text_of_the_table() {
        // Every text names a scope, unless it is about something lsf does not check.
        let table = &*TABLE;
        let mut texts: Vec<&String> = table.types.values().collect();
        texts.extend(table.fields.values().flat_map(HashMap::values));
        let mut unchecked = BTreeSet::new();
        for text in texts {
            for root in [Root::Query, Root::Mutation] {
                let access = RequiredAccess::parse(text, root);
                for set in &access.alternatives {
                    for scope in set {
                        assert!(is_scope_name(scope), "{scope} from {text}");
                    }
                }
                if access.alternatives.is_empty() {
                    unchecked.insert(text.as_str());
                }
            }
        }
        for text in &unchecked {
            assert!(
                !text.contains("`read_") && !text.contains("`write_"),
                "a text naming a scope is not checked: {text}"
            );
        }
        assert!(unchecked.len() < 30, "{unchecked:#?}");
    }

    #[test]
    fn reads_compound_texts() {
        let access = RequiredAccess::parse(
            "`write_products` access scope as well as any of `write_purchase_options`, `write_own_subscription_contracts` access scopes. Also: The user must have a permission to manage products.",
            Root::Mutation,
        );
        assert_eq!(
            access.alternatives,
            vec![
                scopes(&["write_products", "write_purchase_options"]),
                scopes(&["write_products", "write_own_subscription_contracts"]),
            ]
        );
        assert!(access.allows(&["write_products", "write_purchase_options"]));
        assert!(!access.allows(&["write_products"]));
        let access = RequiredAccess::parse(
            "`write_customers` access scope. Also: Requires `write_customer_payment_methods` scope.",
            Root::Mutation,
        );
        assert_eq!(
            access.alternatives,
            vec![scopes(&[
                "write_customers",
                "write_customer_payment_methods"
            ])]
        );
        let access = RequiredAccess::parse(
            "Any of `orders` or `shipping` access scopes, `read_markets` access scope or `manage_delivery_settings` user permission.",
            Root::Query,
        );
        assert_eq!(
            access.alternatives,
            vec![
                scopes(&["read_orders"]),
                scopes(&["read_shipping"]),
                scopes(&["read_markets"])
            ]
        );
        let access = RequiredAccess::parse(
            "products or product_listings read access scope. The user must have permission to view markets or customers or companies.",
            Root::Query,
        );
        assert_eq!(
            access.alternatives,
            vec![
                scopes(&["read_products"]),
                scopes(&["read_product_listings"])
            ]
        );
        let access = RequiredAccess::parse(
            "This mutation can only be run on the current app.",
            Root::Mutation,
        );
        assert!(access.alternatives.is_empty());
        assert!(access.allows::<&str>(&[]));
    }

    #[test]
    fn checks_nested_fields_and_types() {
        let product_tags = field_access("Shop", "productTags", Root::Query).unwrap();
        assert_eq!(product_tags.alternatives, vec![scopes(&["read_products"])]);
        // `shop { shopPolicies }` answers `ShopPolicy` records.
        let policy = type_access("ShopPolicy", Root::Query).unwrap();
        assert_eq!(policy.alternatives, vec![scopes(&["read_legal_policies"])]);
        let customer = type_access("Customer", Root::Query).unwrap();
        assert!(customer.allows(&["write_customers"]));
        assert!(!customer.allows(&["read_orders"]));
        assert_eq!(type_access("Shop", Root::Query), None);
    }

    #[test]
    fn writes_the_access_denied_error_as_shopify() {
        let access = required_access(Root::Query, "products").unwrap();
        assert!(!access.allows(&["read_orders"]));
        assert!(access.allows(&["write_products"]));
        assert_eq!(
            access.error(
                "products",
                Some(json!([{"line": 2, "column": 3}])),
                vec![json!("products")]
            ),
            json!({
                "message": "Access denied for products field. Required access: `read_products` access scope.",
                "locations": [{"line": 2, "column": 3}],
                "path": ["products"],
                "extensions": {
                    "code": "ACCESS_DENIED",
                    "documentation": "https://shopify.dev/api/usage/access-scopes",
                    "requiredAccess": "`read_products` access scope.",
                },
            })
        );
        let keys: Vec<String> = access
            .error("products", None, vec![json!("products")])
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys, ["message", "path", "extensions"]);
    }
}

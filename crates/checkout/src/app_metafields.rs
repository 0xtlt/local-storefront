//! What `appMetafields` of checkout and customer account UI extensions reads besides the cart:
//! the metafields the extensions declare (`[[extensions.metafields]]`), of the shop, of the
//! products and variants bought, of the customer and of the company and location they buy for,
//! read from the store data. The page keeps, for each extension, the entries it declares for its
//! target, and adds the ones of the cart, which change as extensions write them
//! (`assets/sandbox-api.js`).

use lsf_core::Store;
use lsf_core::store::{Customer, Metafields};
use serde::Serialize;

use crate::Metafield;
use crate::app::MetafieldDeclaration;
use crate::input::{canonical_namespace, store_metafield};

/// An entry of `appMetafields`: a metafield and the resource it belongs to.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppMetafieldEntry {
    pub target: AppMetafieldTarget,
    /// The namespace is written `$app` (`$app:config`) for the app's own, as Shopify returns it.
    pub metafield: Metafield,
}

/// The resource a metafield belongs to.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppMetafieldTarget {
    /// `shop`, `product`, `variant`, `customer`, `company` or `companyLocation`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The GID of the resource: `gid://shopify/Product/42`.
    pub id: String,
}

/// The `target.type` of the entries a declaration's `owner_type` names.
fn target_type(owner_type: &str) -> Option<&'static str> {
    Some(match owner_type {
        "CART" => "cart",
        "COMPANY" => "company",
        "COMPANY_LOCATION" => "companyLocation",
        "CUSTOMER" => "customer",
        "PRODUCT" => "product",
        "PRODUCTVARIANT" => "variant",
        "SHOP" => "shop",
        _ => return None,
    })
}

/// The entries of the store's metafields that one of `declared` names: the same namespace (the
/// app's own in any of its spellings) and key, of the owner its `owner_type` names, or of any
/// owner without one. The owners are the shop, the product and the variant of each of
/// `variants`, the customer, and the company they buy for with its location
/// (`company_location`, else their first one), in that order. A metafield comes once, however
/// many declarations name it.
pub fn app_metafields(
    store: &Store,
    declared: &[&MetafieldDeclaration],
    customer: Option<&Customer>,
    company_location: Option<u64>,
    variants: &[u64],
) -> Vec<AppMetafieldEntry> {
    let gid = |kind: &str, id: u64| format!("gid://shopify/{kind}/{id}");
    let mut owners: Vec<(&'static str, String, &Metafields)> =
        vec![("shop", gid("Shop", store.shop.id), &store.shop.metafields)];
    for (product, variant) in variants.iter().filter_map(|id| store.variant(*id)) {
        owners.push(("product", gid("Product", product.id), &product.metafields));
        owners.push((
            "variant",
            gid("ProductVariant", variant.id),
            &variant.metafields,
        ));
    }
    if let Some(customer) = customer {
        owners.push((
            "customer",
            gid("Customer", customer.id),
            &customer.metafields,
        ));
        if let Some(company) = customer.company.map(|index| &store.companies[index]) {
            owners.push(("company", gid("Company", company.id), &company.metafields));
            if let Some(index) = customer.current_location(company_location, store) {
                let location = &company.locations[index];
                owners.push((
                    "companyLocation",
                    gid("CompanyLocation", location.id),
                    &location.metafields,
                ));
            }
        }
    }

    let mut entries: Vec<AppMetafieldEntry> = Vec::new();
    for (kind, id, metafields) in owners {
        for declaration in declared {
            let owner = declaration.owner_type.as_deref();
            if owner.is_some_and(|owner| target_type(owner) != Some(kind)) {
                continue;
            }
            let Some((type_name, value)) =
                store_metafield(metafields, &declaration.namespace, &declaration.key)
            else {
                continue;
            };
            let entry = AppMetafieldEntry {
                target: AppMetafieldTarget {
                    kind,
                    id: id.clone(),
                },
                metafield: Metafield {
                    namespace: canonical_namespace(&declaration.namespace),
                    key: declaration.key.clone(),
                    kind: type_name,
                    value,
                },
            };
            if !entries.contains(&entry) {
                entries.push(entry);
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use lsf_core::store::Metafield as StoreMetafield;
    use lsf_core::store::load::{DataSource, LoadOptions, load};
    use serde_json::{Value, json};

    use super::*;

    fn declaration(namespace: &str, key: &str, owner_type: Option<&str>) -> MetafieldDeclaration {
        MetafieldDeclaration {
            namespace: namespace.to_string(),
            key: key.to_string(),
            owner_type: owner_type.map(str::to_string),
            target: None,
        }
    }

    fn set(metafields: &mut Metafields, namespace: &str, key: &str, kind: &str, value: Value) {
        metafields.entry(namespace.to_string()).or_default().insert(
            key.to_string(),
            StoreMetafield {
                kind: kind.to_string(),
                value,
            },
        );
    }

    /// `(type, id, namespace.key, value)` of each entry.
    fn summary(entries: &[AppMetafieldEntry]) -> Vec<(&str, &str, String, &str)> {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.target.kind,
                    entry.target.id.as_str(),
                    format!("{}.{}", entry.metafield.namespace, entry.metafield.key),
                    entry.metafield.value.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn the_declared_metafields_of_the_shop_are_read_with_the_app_namespace() {
        let (mut store, _) = load(&DataSource::Demo, &LoadOptions::default());
        set(
            &mut store.shop.metafields,
            "$app",
            "gift-codes",
            "json",
            json!(["GIFT-1", "GIFT-2"]),
        );
        // Shopify's spelling of the app's namespace is the same namespace.
        set(
            &mut store.shop.metafields,
            "app--4242--config",
            "mode",
            "single_line_text_field",
            json!("strict"),
        );
        let gift = declaration("$app", "gift-codes", None);
        let mode = declaration("$app:config", "mode", Some("SHOP"));
        let undeclared = declaration("$app", "nothing", None);
        let entries = app_metafields(&store, &[&gift, &mode, &undeclared], None, None, &[]);
        let shop = format!("gid://shopify/Shop/{}", store.shop.id);
        assert_eq!(
            summary(&entries),
            [
                (
                    "shop",
                    shop.as_str(),
                    "$app.gift-codes".to_string(),
                    r#"["GIFT-1","GIFT-2"]"#
                ),
                (
                    "shop",
                    shop.as_str(),
                    "$app:config.mode".to_string(),
                    "strict"
                ),
            ]
        );
        assert_eq!(entries[0].metafield.kind, "json");
        // As the page reads it.
        assert_eq!(
            serde_json::to_value(&entries[1]).unwrap(),
            json!({
                "target": {"type": "shop", "id": shop},
                "metafield": {
                    "namespace": "$app:config",
                    "key": "mode",
                    "type": "single_line_text_field",
                    "value": "strict"
                }
            })
        );
        // The shop's other metafields (the demo's `custom.announcement`) are not declared.
        assert!(app_metafields(&store, &[], None, None, &[]).is_empty());
    }

    #[test]
    fn every_owner_is_read_and_owner_type_keeps_one() {
        let (mut store, _) = load(&DataSource::Demo, &LoadOptions::default());
        let shirt = &mut store.products[0];
        let (product, variant) = (shirt.id, shirt.variants[0].id);
        set(
            &mut shirt.metafields,
            "$app",
            "tier",
            "single_line_text_field",
            json!("product tier"),
        );
        set(
            &mut shirt.variants[0].metafields,
            "$app",
            "tier",
            "number_integer",
            json!(2),
        );
        set(
            &mut store.shop.metafields,
            "$app",
            "tier",
            "single_line_text_field",
            json!("shop tier"),
        );
        // Alex buys for Northwind Hotels, whose metafields the demo store has.
        let alex = store
            .customers
            .iter()
            .find(|customer| customer.email == "alex.morgan@example.com")
            .unwrap()
            .clone();
        let company = &store.companies[alex.company.unwrap()];
        let seattle = company.locations[1].id;
        let declared = [
            declaration("$app", "tier", None),
            declaration("custom", "job_title", Some("CUSTOMER")),
            declaration("custom", "payment_terms", Some("COMPANY")),
            declaration("custom", "delivery_notes", Some("COMPANY_LOCATION")),
            // Declared for the cart only: the product's is not read.
            declaration("custom", "care", Some("CART")),
        ];
        let declared: Vec<&MetafieldDeclaration> = declared.iter().collect();
        // The same variant on two lines is read once.
        let entries = app_metafields(&store, &declared, Some(&alex), None, &[variant, variant]);
        let gid = |kind: &str, id: u64| format!("gid://shopify/{kind}/{id}");
        let portland = gid("CompanyLocation", company.locations[0].id);
        let expected = [
            ("shop", gid("Shop", store.shop.id), "$app.tier", "shop tier"),
            (
                "product",
                gid("Product", product),
                "$app.tier",
                "product tier",
            ),
            ("variant", gid("ProductVariant", variant), "$app.tier", "2"),
            (
                "customer",
                gid("Customer", alex.id),
                "custom.job_title",
                "Purchasing manager",
            ),
            (
                "company",
                gid("Company", company.id),
                "custom.payment_terms",
                "Net 30",
            ),
            (
                "companyLocation",
                portland,
                "custom.delivery_notes",
                "Loading dock B, weekdays 8am to 4pm",
            ),
        ];
        let expected: Vec<(&str, &str, String, &str)> = expected
            .iter()
            .map(|(kind, id, name, value)| (*kind, id.as_str(), name.to_string(), *value))
            .collect();
        assert_eq!(summary(&entries), expected);

        // The location the customer chose; Seattle has no delivery notes.
        let entries = app_metafields(&store, &declared, Some(&alex), Some(seattle), &[]);
        assert!(
            entries
                .iter()
                .all(|entry| entry.target.kind != "companyLocation")
        );
        // Without a customer, no customer, company or location.
        let entries = app_metafields(&store, &declared, None, None, &[variant]);
        let kinds: Vec<&str> = entries.iter().map(|entry| entry.target.kind).collect();
        assert_eq!(kinds, ["shop", "product", "variant"]);
        // `owner_type = "PRODUCTVARIANT"` reads the variant only.
        let variant_only = declaration("$app", "tier", Some("PRODUCTVARIANT"));
        let entries = app_metafields(&store, &[&variant_only], None, None, &[variant]);
        let kinds: Vec<&str> = entries.iter().map(|entry| entry.target.kind).collect();
        assert_eq!(kinds, ["variant"]);
    }
}

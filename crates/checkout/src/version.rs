//! The Shopify API releases the checkout supports, and the Function API schemas of each.
//!
//! A Function declares its release with `api_version` in `shopify.extension.toml`. The input
//! query of a Function is checked against the schema of that release, and its output has to fit
//! the result type of the target in that schema (see [`crate::schema`]).
//!
//! Adding a release takes one line in [`VERSIONS`] and the schemas in `crates/checkout/schemas`
//! (`mise run checkout:schemas`, which reads the same list of versions).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::schema::Schema;

/// A Shopify API release: `2026-10`, `2027-01`.
#[derive(Clone, Copy)]
pub struct ApiVersion(&'static VersionSpec);

struct VersionSpec {
    id: &'static str,
    /// The schema of each Function API, in [`FunctionApi::ALL`] order.
    schemas: [&'static str; FunctionApi::ALL.len()],
}

macro_rules! version {
    ($id:literal) => {
        VersionSpec {
            id: $id,
            schemas: [
                include_str!(concat!(
                    "../schemas/",
                    $id,
                    "/cart_checkout_validation.graphql"
                )),
                include_str!(concat!("../schemas/", $id, "/cart_transform.graphql")),
                include_str!(concat!(
                    "../schemas/",
                    $id,
                    "/delivery_customization.graphql"
                )),
                include_str!(concat!("../schemas/", $id, "/discount.graphql")),
                include_str!(concat!("../schemas/", $id, "/order_discount.graphql")),
                include_str!(concat!(
                    "../schemas/",
                    $id,
                    "/payment_customization.graphql"
                )),
                include_str!(concat!("../schemas/", $id, "/product_discount.graphql")),
                include_str!(concat!("../schemas/", $id, "/shipping_discount.graphql")),
            ],
        }
    };
}

/// The supported releases, oldest first. The only place a release is listed.
static VERSIONS: &[VersionSpec] = &[version!("2026-10"), version!("2027-01")];

impl ApiVersion {
    /// Every supported release, oldest first.
    pub fn all() -> impl Iterator<Item = ApiVersion> {
        VERSIONS.iter().map(ApiVersion)
    }

    /// The release written as `api_version` (`2026-10`), or the error to show: it names the
    /// supported ones.
    pub fn parse(text: &str) -> Result<ApiVersion, String> {
        ApiVersion::all()
            .find(|version| version.as_str() == text.trim())
            .ok_or_else(|| {
                format!(
                    "api_version \"{text}\" is not supported (supported: {})",
                    ApiVersion::supported()
                )
            })
    }

    /// The supported releases, for messages: `2026-10, 2027-01`.
    pub fn supported() -> String {
        ApiVersion::all()
            .map(|version| version.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn as_str(self) -> &'static str {
        self.0.id
    }

    /// The most recent release.
    pub fn latest() -> ApiVersion {
        ApiVersion(VERSIONS.last().expect("at least one release"))
    }

    /// Whether a checkout UI extension has to give the `owner_type` of each of its metafield
    /// declarations: from 2027-01 on. `refusalOf` in `assets/api-versions.js` has the same rule.
    pub fn requires_metafield_owner_type(self) -> bool {
        self.as_str() >= "2027-01"
    }

    /// The schema of a Function API in this release.
    pub fn schema(self, api: FunctionApi) -> &'static Schema {
        type Cache = Mutex<HashMap<(&'static str, FunctionApi), &'static Schema>>;
        static CACHE: OnceLock<Cache> = OnceLock::new();
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.entry((self.0.id, api)).or_insert_with(|| {
            let source = self.0.schemas[api as usize];
            let schema = Schema::parse(source).unwrap_or_else(|error| {
                panic!("the {api} schema of {} is invalid: {error}", self.0.id)
            });
            Box::leak(Box::new(schema))
        })
    }
}

impl PartialEq for ApiVersion {
    fn eq(&self, other: &ApiVersion) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for ApiVersion {}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.id)
    }
}

impl fmt::Debug for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ApiVersion({})", self.0.id)
    }
}

/// A Function API: the schema its input query and its output follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FunctionApi {
    CartCheckoutValidation,
    CartTransform,
    DeliveryCustomization,
    Discount,
    /// Deprecated.
    OrderDiscount,
    PaymentCustomization,
    /// Deprecated.
    ProductDiscount,
    /// Deprecated.
    ShippingDiscount,
}

impl FunctionApi {
    /// In the order of [`VersionSpec::schemas`].
    pub const ALL: [FunctionApi; 8] = [
        FunctionApi::CartCheckoutValidation,
        FunctionApi::CartTransform,
        FunctionApi::DeliveryCustomization,
        FunctionApi::Discount,
        FunctionApi::OrderDiscount,
        FunctionApi::PaymentCustomization,
        FunctionApi::ProductDiscount,
        FunctionApi::ShippingDiscount,
    ];

    /// The name of its schema file in `crates/checkout/schemas/<version>`.
    pub fn name(self) -> &'static str {
        match self {
            FunctionApi::CartCheckoutValidation => "cart_checkout_validation",
            FunctionApi::CartTransform => "cart_transform",
            FunctionApi::DeliveryCustomization => "delivery_customization",
            FunctionApi::Discount => "discount",
            FunctionApi::OrderDiscount => "order_discount",
            FunctionApi::PaymentCustomization => "payment_customization",
            FunctionApi::ProductDiscount => "product_discount",
            FunctionApi::ShippingDiscount => "shipping_discount",
        }
    }
}

impl fmt::Display for FunctionApi {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// What a target is in the schemas: the Function API it belongs to and the type of its output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetSpec {
    pub api: FunctionApi,
    /// The input object the output of the Function has to fit.
    pub result: &'static str,
    /// A `purchase.*` target of the first generation of the APIs: its result is the
    /// `FunctionRunResult` of the schema, which the new operations are not part of.
    pub legacy: bool,
}

/// The targets the checkout runs. `None` for the others (fulfillment constraints, order
/// routing, pickup generators, the `.fetch` targets).
pub fn target_spec(target: &str) -> Option<TargetSpec> {
    use FunctionApi::*;
    let (api, result, legacy) = match target {
        "cart.transform.run" => (CartTransform, "CartTransformRunResult", false),
        "purchase.cart-transform.run" => (CartTransform, "FunctionRunResult", true),
        "cart.lines.discounts.generate.run" => {
            (Discount, "CartLinesDiscountsGenerateRunResult", false)
        }
        "cart.delivery-options.discounts.generate.run" => (
            Discount,
            "CartDeliveryOptionsDiscountsGenerateRunResult",
            false,
        ),
        "purchase.product-discount.run" => (ProductDiscount, "FunctionRunResult", true),
        "purchase.order-discount.run" => (OrderDiscount, "FunctionRunResult", true),
        "purchase.shipping-discount.run" => (ShippingDiscount, "FunctionRunResult", true),
        "cart.delivery-options.transform.run" => (
            DeliveryCustomization,
            "CartDeliveryOptionsTransformRunResult",
            false,
        ),
        "purchase.delivery-customization.run" => (DeliveryCustomization, "FunctionRunResult", true),
        "cart.payment-methods.transform.run" => (
            PaymentCustomization,
            "CartPaymentMethodsTransformRunResult",
            false,
        ),
        "purchase.payment-customization.run" => (PaymentCustomization, "FunctionRunResult", true),
        "cart.validations.generate.run" => (
            CartCheckoutValidation,
            "CartValidationsGenerateRunResult",
            false,
        ),
        "purchase.validation.run" => (CartCheckoutValidation, "FunctionRunResult", true),
        _ => return None,
    };
    Some(TargetSpec {
        api,
        result,
        legacy,
    })
}

/// The target that replaces a legacy `purchase.*` one.
pub fn successor(legacy_target: &str) -> Option<&'static str> {
    Some(match legacy_target {
        "purchase.cart-transform.run" => "cart.transform.run",
        "purchase.product-discount.run" | "purchase.order-discount.run" => {
            "cart.lines.discounts.generate.run"
        }
        "purchase.shipping-discount.run" => "cart.delivery-options.discounts.generate.run",
        "purchase.delivery-customization.run" => "cart.delivery-options.transform.run",
        "purchase.payment-customization.run" => "cart.payment-methods.transform.run",
        "purchase.validation.run" => "cart.validations.generate.run",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supported_versions_are_the_two_releases() {
        assert_eq!(ApiVersion::supported(), "2026-10, 2027-01");
        assert_eq!(ApiVersion::latest().as_str(), "2027-01");
        assert_eq!(ApiVersion::parse("2026-10").unwrap().as_str(), "2026-10");
        assert_eq!(
            ApiVersion::parse(" 2027-01 ").unwrap().to_string(),
            "2027-01"
        );
        assert_eq!(
            ApiVersion::parse("2025-10").unwrap_err(),
            "api_version \"2025-10\" is not supported (supported: 2026-10, 2027-01)"
        );
        for other in ["", "unstable", "latest", "2026-1", "2026-10-01", "2027-04"] {
            assert!(ApiVersion::parse(other).is_err(), "{other}");
        }
    }

    #[test]
    fn owner_type_is_required_from_2027_01() {
        assert!(
            !ApiVersion::parse("2026-10")
                .unwrap()
                .requires_metafield_owner_type()
        );
        assert!(
            ApiVersion::parse("2027-01")
                .unwrap()
                .requires_metafield_owner_type()
        );
    }

    #[test]
    fn a_legacy_target_has_a_successor() {
        for target in ["purchase.validation.run", "purchase.product-discount.run"] {
            assert!(target_spec(target).unwrap().legacy);
            assert!(successor(target).is_some());
        }
        assert_eq!(successor("cart.validations.generate.run"), None);
    }
}
